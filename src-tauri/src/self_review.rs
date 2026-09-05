//! 变更-34 · A3：让 Helm 自评审当前会话变更（BackgroundOperation）。
//!
//! 用户在「变更」交付物区点击「让 Helm 自评审」后，用当前 Engine Binding 的 fast model
//! （缺失回落 primary）经 ModelOnlyOperationPolicy + 真实 CLI Adapter 审阅这批 diff，
//! 产出行级意见（只报编译/逻辑/安全问题与明显 bug，不报风格），返回给前端以 `.is-ai` 渲染。
//!
//! 内容只发给当前 Engine Binding 的服务商；与起标题共用同一条无工具隔离路径。

use crate::adapter::agent_environment_from_settings;
use crate::budget::TurnBudgetSnapshot;
use crate::capability_registry::EngineCapabilityRegistry;
use crate::commands::{
    ensure_binding_runtime_ready, resolve_engine_capability_snapshot, resolve_routed_effort,
    subscription_profile_for_binding,
};
use crate::operations::{
    BackgroundOperation, ModelOnlyOperationPolicy, NewBackgroundOperation, OperationExecutionSpec,
};
use crate::protocol::{DiffKind, EngineId};
use crate::providers::{BindingConfig, KeyringSecretStore, ProviderStore};
use crate::reasoning::ReasoningEffort;
use crate::runtime_registry::RuntimeRegistry;
use crate::sessions::{SessionDetail, SessionHistoryStore};
use crate::settings::load_app_settings_from_store;
use crate::subscription_profiles::SubscriptionProfileStore;
use crate::turn_start::{build_runtime_route, digest_json};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

const MAX_DIFF_CHARS: usize = 9000;
const MAX_NOTES: usize = 24;

/// 回给前端的行级意见（camelCase 与前端 ReviewNote 对齐）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReviewNoteDto {
    pub file: String,
    pub line: u32,
    pub text: String,
    #[serde(default)]
    pub from_ai: bool,
}

/// 「让 Helm 自评审」：跑一次真实 fast model 无工具调用，返回该会话当前 diff 的行级意见。
/// 幂等：相同 diff（aggregate 后 digest 一致）复用已成功的结果，不重复消耗模型调用。
pub async fn review_changes(
    app: &AppHandle,
    history_session_id: &str,
) -> Result<Vec<ReviewNoteDto>, String> {
    let history_store = app
        .try_state::<SessionHistoryStore>()
        .ok_or("历史存储未初始化")?;
    let detail = history_store.get_session(history_session_id)?;
    let files = aggregate_change_files(&detail);
    if files.is_empty() {
        return Ok(Vec::new());
    }
    let prompt = build_review_prompt(&files);
    let profiles = app
        .try_state::<SubscriptionProfileStore>()
        .ok_or("订阅 Profile 存储未初始化")?;
    let capabilities = app
        .try_state::<EngineCapabilityRegistry>()
        .ok_or("Engine Capability Registry 未初始化")?;
    let runtime_registry = app
        .try_state::<RuntimeRegistry>()
        .ok_or("RuntimeRegistry 未初始化")?;
    let provider_store = app
        .try_state::<ProviderStore<KeyringSecretStore>>()
        .ok_or("服务商存储未初始化")?;
    let settings = load_app_settings_from_store(&history_store)?;
    let engine_id = match detail.summary.engine {
        EngineId::ClaudeCode => "claude-code",
        EngineId::Codex => "codex",
    };
    let input_digest = digest_json(&files)?;
    let idempotency_key = format!("self_review:{history_session_id}:{input_digest}");

    let mut committed = None;
    for _ in 0..3 {
        let candidate = provider_store.route_candidate()?;
        let binding = candidate
            .config
            .bindings
            .iter()
            .find(|binding| binding.engine_id == engine_id)
            .cloned()
            .ok_or_else(|| format!("引擎还没有配置生效绑定：{engine_id}"))?;
        let model = binding
            .fast_model
            .as_deref()
            .filter(|model| !model.trim().is_empty())
            .unwrap_or(&binding.primary_model)
            .to_string();
        let launch_binding = BindingConfig {
            primary_model: model.clone(),
            assistant_model_id: None,
            ..binding.clone()
        };
        ensure_binding_runtime_ready(&profiles, &candidate.config, &launch_binding).await?;
        let mut env = provider_store.launch_env_for_config(&candidate.config, &launch_binding)?;
        let subscription_home =
            subscription_profile_for_binding(&profiles, &candidate.config, &launch_binding)?;
        if subscription_home.is_some() {
            profiles.append_launch_env(&mut env, engine_id)?;
        }
        env.extend(agent_environment_from_settings(&settings));
        let bin = candidate
            .config
            .engine_bin(engine_id)
            .filter(|bin| !bin.is_empty())
            .unwrap_or(if engine_id == "codex" {
                "codex"
            } else {
                "claude"
            })
            .to_string();
        let pricing_profile = candidate
            .config
            .models
            .iter()
            .find(|item| item.provider_id == binding.provider_id && item.id == model)
            .map(|item| provider_store.model_pricing_profile(&candidate.config, item))
            .transpose()?
            .flatten();
        let requested_effort = binding.reasoning_effort.unwrap_or(ReasoningEffort::Auto);
        let route = build_runtime_route(
            &candidate.config,
            &launch_binding,
            &model,
            &bin,
            &env,
            requested_effort,
            pricing_profile,
        )?;
        let capability = resolve_engine_capability_snapshot(
            &capabilities,
            &route,
            &bin,
            &env,
            subscription_home,
        )
        .await?;
        let routed_effort = resolve_routed_effort(&capability, requested_effort);
        let created_at = crate::util::now_millis();
        let operation_id = format!("operation-{:032x}", rand::random::<u128>());
        let spec = OperationExecutionSpec::from_binding_route(
            operation_id.clone(),
            "self_review",
            format!("binding:{}", binding.engine_id),
            binding.revision,
            &route,
            &capability,
            requested_effort,
            routed_effort,
            created_at,
        )?;
        let policy = ModelOnlyOperationPolicy::freeze_from_capability(&capability, created_at);
        let new_operation = NewBackgroundOperation {
            operation: BackgroundOperation {
                id: operation_id,
                kind: "self_review".to_string(),
                source_session_id: Some(history_session_id.to_string()),
                input_digest: input_digest.clone(),
                input: None,
                idempotency_key: idempotency_key.clone(),
                status: "committed".to_string(),
                result: None,
                error_code: None,
                created_at,
                started_at: None,
                cancel_requested_at: None,
                ended_at: None,
            },
            spec,
            policy,
            budget: TurnBudgetSnapshot::standard(created_at),
        };
        match provider_store.commit_route_if_unchanged(&candidate.config_digest, |_| {
            history_store.create_background_operation(&new_operation)
        })? {
            Some((operation, false)) => return existing_review_notes(&operation),
            Some((_operation, true)) => {
                committed = Some((new_operation, capability, bin, env));
                break;
            }
            None => continue,
        }
    }
    let (operation, capability, bin, env) = committed.ok_or_else(|| {
        "Provider 配置连续变化，OperationStart 有界重算未能收敛，请重试".to_string()
    })?;
    if let Err(error) =
        ModelOnlyOperationPolicy::from_capability(&capability, operation.operation.created_at)
    {
        history_store.fail_committed_background_operation(&operation.operation.id, &error)?;
        return Err(error);
    }
    let (attempt_no, output) = runtime_registry
        .run_model_only_operation(
            &operation.spec,
            &operation.policy,
            &operation.budget,
            &bin,
            &env,
            &prompt,
        )
        .await?;
    let notes = parse_review_notes(&output.text);
    let result = serde_json::json!({ "notes": notes });
    history_store.complete_model_only_operation(
        &operation.operation.id,
        attempt_no,
        &output,
        &result,
    )?;
    Ok(notes)
}

fn existing_review_notes(operation: &BackgroundOperation) -> Result<Vec<ReviewNoteDto>, String> {
    match operation.status.as_str() {
        "succeeded" => {
            let raw = operation
                .result
                .clone()
                .unwrap_or(serde_json::json!({ "notes": [] }));
            let notes: Vec<ReviewNoteDto> = raw
                .get("notes")
                .cloned()
                .and_then(|value| serde_json::from_value(value).ok())
                .unwrap_or_default();
            Ok(notes)
        }
        _ => Err(operation
            .error_code
            .clone()
            .unwrap_or_else(|| format!("自评审任务状态：{}", operation.status))),
    }
}

/// 从会话历史聚合 diff 文件（与前端 changeReviewFiles 同构，line 数与前端渲染对齐）。
fn aggregate_change_files(detail: &SessionDetail) -> Vec<ChangeFile> {
    use std::collections::HashMap;
    let mut by_path: HashMap<String, ChangeFile> = HashMap::new();
    for call in &detail.tool_calls {
        let Some(diff) = &call.diff else { continue };
        if diff.hunks.is_empty() {
            continue;
        }
        let entry = by_path
            .entry(diff.path.clone())
            .or_insert_with(|| ChangeFile {
                path: diff.path.clone(),
                hunks: Vec::new(),
            });
        let mut last_new = 0usize;
        for (hunk_index, hunk) in diff.hunks.iter().enumerate() {
            let mut old_no: u32 = hunk.old_start;
            let mut new_no: u32 = hunk.new_start;
            let mut lines: Vec<ChangeLine> = Vec::new();
            for line in &hunk.lines {
                match line.kind {
                    DiffKind::Add => {
                        lines.push(ChangeLine {
                            kind: "add".to_string(),
                            old_no: None,
                            new_no: Some(new_no),
                            text: line.text.clone(),
                        });
                        new_no += 1;
                    }
                    DiffKind::Del => {
                        lines.push(ChangeLine {
                            kind: "del".to_string(),
                            old_no: Some(old_no),
                            new_no: None,
                            text: line.text.clone(),
                        });
                        old_no += 1;
                    }
                    DiffKind::Ctx => {
                        lines.push(ChangeLine {
                            kind: "ctx".to_string(),
                            old_no: Some(old_no),
                            new_no: Some(new_no),
                            text: line.text.clone(),
                        });
                        old_no += 1;
                        new_no += 1;
                    }
                }
            }
            let skip = if hunk_index == 0 {
                (hunk.new_start as usize).saturating_sub(1)
            } else {
                (hunk.new_start as usize).saturating_sub(last_new.saturating_add(1))
            };
            let new_line_count = lines.iter().filter(|line| line.kind != "del").count();
            entry.hunks.push(ChangeHunk { skip, lines });
            last_new = hunk.new_start as usize + new_line_count - 1;
        }
    }
    let mut files: Vec<ChangeFile> = by_path.into_values().collect();
    files.sort_by(|a, b| a.path.cmp(&b.path));
    files
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct ChangeLine {
    kind: String,
    old_no: Option<u32>,
    new_no: Option<u32>,
    text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct ChangeHunk {
    skip: usize,
    lines: Vec<ChangeLine>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct ChangeFile {
    path: String,
    hunks: Vec<ChangeHunk>,
}

/// 构造自评审 prompt：只报编译/逻辑/安全与明显 bug，明确不报风格。
fn build_review_prompt(files: &[ChangeFile]) -> String {
    let mut body = String::new();
    for file in files {
        body.push_str(&format!("\n## {}\n", file.path));
        let mut emitted = 0usize;
        for hunk in &file.hunks {
            for line in &hunk.lines {
                if emitted >= MAX_DIFF_CHARS {
                    body.push_str("\n…（diff 过长已截断）\n");
                    break;
                }
                let no = line.new_no.or(line.old_no).unwrap_or(0);
                let sig = match line.kind.as_str() {
                    "add" => "+",
                    "del" => "-",
                    _ => " ",
                };
                let text = truncate_chars(&line.text, 120);
                let mut chunk = format!("{no:>4} {sig} {text}\n");
                if emitted + chunk.len() > MAX_DIFF_CHARS {
                    chunk = format!("{chunk}\n…（diff 过长已截断）\n");
                }
                body.push_str(&chunk);
                emitted += chunk.len();
            }
        }
    }
    format!(
        "你是资深代码审查者，请审阅下面这批代码变更。\n\
         只报告：编译错误、逻辑错误、安全问题（如注入/越权/敏感信息泄露）和明显 bug；不报风格。\n\
         绝对不要报告：代码风格、命名、格式、可读性等主观偏好。\n\
         没有确定的问题就不要编造。\n\
         以严格 JSON 数组输出，不要包裹 Markdown：\n\
         [{{\"file\":\"文件路径\", \"line\":行号, \"text\":\"一句话意见\"}}]\n\
         行号要与上方 diff 行号对齐；无问题时输出 [].\n\n{}",
        body
    )
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let truncated: String = text.chars().take(max_chars).collect();
    format!("{truncated}…")
}

/// 解析模型输出：去 Markdown 围栏后按严格 JSON 数组解析；不合规时回退 [ ]（不编造）。
fn parse_review_notes(raw: &str) -> Vec<ReviewNoteDto> {
    let cleaned = raw
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();
    let start = cleaned.find('[');
    let end = cleaned.rfind(']');
    let slice = match (start, end) {
        (Some(s), Some(e)) if e > s => &cleaned[s..=e],
        _ => cleaned,
    };
    let mut notes: Vec<ReviewNoteDto> = serde_json::from_str(slice).unwrap_or_default();
    notes.truncate(MAX_NOTES);
    for note in &mut notes {
        note.text = truncate_chars(note.text.trim(), 200).into();
        note.from_ai = true;
    }
    notes
}

#[cfg(test)]
mod tests {
    use super::{build_review_prompt, parse_review_notes, ChangeFile, ChangeHunk, ChangeLine};

    #[test]
    fn parse_review_notes_handles_fenced_json() {
        let raw = "```json\n[{\"file\":\"src/a.ts\", \"line\": 12, \"text\":\"空指针风险\"}]\n```";
        let notes = parse_review_notes(raw);
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].file, "src/a.ts");
        assert_eq!(notes[0].line, 12);
        assert!(notes[0].from_ai);
    }

    #[test]
    fn parse_review_notes_returns_empty_on_garbage() {
        let notes = parse_review_notes("这个 diff 没什么问题");
        assert!(notes.is_empty());
    }

    #[test]
    fn parse_review_notes_caps_count_and_length() {
        let raw = (0..40)
            .map(|i| {
                format!(
                    "{{\"file\":\"f{}\", \"line\":1, \"text\":\"{}#\"}}",
                    i,
                    "长".repeat(500)
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        let notes = parse_review_notes(&format!("[{raw}]"));
        assert_eq!(notes.len(), 24);
        assert!(notes.iter().all(|n| n.text.chars().count() <= 201));
    }

    #[test]
    fn build_review_prompt_mentions_no_style_and_bounds_diff() {
        let files = vec![ChangeFile {
            path: "src/a.ts".into(),
            hunks: vec![ChangeHunk {
                skip: 0,
                lines: vec![
                    ChangeLine {
                        kind: "del".into(),
                        old_no: Some(1),
                        new_no: None,
                        text: "const bad = 1;".into(),
                    },
                    ChangeLine {
                        kind: "add".into(),
                        old_no: None,
                        new_no: Some(1),
                        text: "const ok = 2;".into(),
                    },
                ],
            }],
        }];
        let prompt = build_review_prompt(&files);
        assert!(prompt.contains("不报"));
        assert!(prompt.contains("不报风格") || prompt.contains("绝对不要报告"));
        assert!(prompt.contains("src/a.ts"));
    }
}
