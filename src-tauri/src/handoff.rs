//! 跨 Engine 摘要派生。
//!
//! ForkJob 冻结源 Session 的已完成 Turn 边界，经目标 Engine 的 ModelOnlyOperation
//! 生成结构化 Handoff。目标 Session 只在 Handoff 成功持久化时原子创建。

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
use crate::providers::{BindingConfig, KeyringSecretStore, ModelConfig, ProviderStore};
use crate::reasoning::ReasoningEffort;
use crate::runtime_registry::RuntimeRegistry;
use crate::sessions::{SessionDetail, SessionHistoryStore, TurnLedgerRecord};
use crate::settings::load_app_settings_from_store;
use crate::subscription_profiles::SubscriptionProfileStore;
use crate::turn_start::{build_runtime_route, digest_json};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};

pub const HANDOFF_CONTRACT_VERSION: u32 = 1;
const MAX_LEDGER_TOOL_OUTPUT_BYTES: usize = 64 * 1024;
const MAX_HANDOFF_PROMPT_BYTES: usize = 512 * 1024;
const MAX_RECURSIVE_SUMMARY_DEPTH: usize = 8;

/// 分叉摘要模型目录预检（九次反馈）：路由模型必须是该绑定服务商当前启用目录中的
/// 精确 ID；否则在 spawn 前给出带 tag 的可操作错误，而不是等引擎链路以含糊的
/// unrecognized_model 非零退出。服务商目录为空（旧配置未同步过目录）时不判定，
/// 放行走引擎侧兜底映射，避免误拦合法旧配置。
fn ensure_fork_model_in_catalog(
    models: &[ModelConfig],
    provider_id: &str,
    model_id: &str,
) -> Result<(), String> {
    let provider_models: Vec<&ModelConfig> = models
        .iter()
        .filter(|model| model.provider_id == provider_id)
        .collect();
    if provider_models.is_empty() {
        return Ok(());
    }
    let known = provider_models
        .iter()
        .any(|model| model.id == model_id && model.enabled);
    if known {
        return Ok(());
    }
    Err(format!(
        "[operation_model_unavailable] 分叉摘要路由的模型 {model_id} 不在服务商当前启用的模型目录中（目录可能已漂移，或来自旧配置迁移）；请到「AI 配置」为对应引擎绑定更换快速模型或主模型后重试分叉"
    ))
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HandoffContent {
    pub contract_version: u32,
    pub goal: String,
    pub completed: Vec<String>,
    pub current_state: String,
    pub decisions_and_files: Vec<String>,
    pub remaining: Vec<String>,
    pub constraints: Vec<String>,
}

impl HandoffContent {
    pub fn validate(&self) -> Result<(), String> {
        if self.contract_version != HANDOFF_CONTRACT_VERSION {
            return Err("[handoff_invalid] 不支持的 Handoff 合同版本".to_string());
        }
        if self.goal.trim().is_empty() || self.current_state.trim().is_empty() {
            return Err("[handoff_invalid] Handoff 缺少目标或当前状态".to_string());
        }
        Ok(())
    }

    pub fn as_context(&self) -> String {
        fn lines(values: &[String]) -> String {
            if values.is_empty() {
                "- 暂无".to_string()
            } else {
                values
                    .iter()
                    .map(|value| format!("- {value}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            }
        }
        format!(
            "[跨引擎交接摘要：细节可能有损]\n\n目标\n{}\n\n已完成\n{}\n\n当前状态\n{}\n\n关键决定与文件\n{}\n\n未尽事项\n{}\n\n限制\n{}",
            self.goal,
            lines(&self.completed),
            self.current_state,
            lines(&self.decisions_and_files),
            lines(&self.remaining),
            lines(&self.constraints),
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FrozenForkInput {
    pub source_session_id: String,
    pub source_title: String,
    pub source_engine: String,
    pub source_cwd: String,
    pub source_folder_id: String,
    pub target_engine: String,
    pub boundary_turn_id: String,
    pub boundary_turn_epoch: u64,
    pub ledger_json: String,
}

pub fn freeze_fork_input(
    detail: &SessionDetail,
    ledger: &[TurnLedgerRecord],
    target_engine: &str,
    boundary_turn_id: Option<&str>,
) -> Result<FrozenForkInput, String> {
    let target_engine = normalize_engine(target_engine)?;
    let source_engine = match detail.summary.engine {
        crate::protocol::EngineId::ClaudeCode => "claude-code",
        crate::protocol::EngineId::Codex => "codex",
    };
    if detail.turns.iter().any(|turn| {
        matches!(
            turn.status.as_str(),
            "committed" | "running" | "waiting_approval" | "stalled"
        )
    }) {
        return Err("[fork_source_busy] 当前轮次尚未完成，不能冻结派生边界".to_string());
    }
    // 切点分叉：调用方给出被点回答所属的 Helm turn id 时，冻结边界取该轮（须已完成），
    // 摘要只带「这段回答及之前」；无切点（左栏整段分叉）沿用最近一个成功轮。
    let boundary = match boundary_turn_id {
        Some(turn_id) => ledger
            .iter()
            .find(|record| record.turn.id == turn_id && record.turn.status == "succeeded")
            .ok_or_else(|| {
                "[fork_boundary_unavailable] 切点 Turn 不存在或尚未完成，无法按切点派生".to_string()
            })?,
        None => ledger
            .iter()
            .filter(|record| record.turn.status == "succeeded")
            .max_by_key(|record| record.turn.epoch)
            .ok_or_else(|| {
                "[fork_no_completed_turn] 源 Session 没有可冻结的成功 Turn".to_string()
            })?,
    };
    let visible = ledger
        .iter()
        .filter(|record| record.turn.epoch <= boundary.turn.epoch)
        .map(redacted_ledger_record)
        .collect::<Vec<_>>();
    let ledger_json = serde_json::to_string(&visible).map_err(|error| error.to_string())?;
    Ok(FrozenForkInput {
        source_session_id: detail.summary.id.clone(),
        source_title: detail.summary.title.clone(),
        source_engine: source_engine.to_string(),
        source_cwd: detail.summary.cwd.clone(),
        source_folder_id: detail.summary.folder_id.clone(),
        target_engine: target_engine.to_string(),
        boundary_turn_id: boundary.turn.id.clone(),
        boundary_turn_epoch: boundary.turn.epoch,
        ledger_json,
    })
}

fn normalize_engine(engine: &str) -> Result<&str, String> {
    match engine {
        "claude-code" | "codex" => Ok(engine),
        _ => Err(format!(
            "[fork_invalid_engine] 不支持的目标 Engine：{engine}"
        )),
    }
}

fn redacted_ledger_record(record: &TurnLedgerRecord) -> serde_json::Value {
    let messages = record
        .messages
        .iter()
        .filter(|message| !message.reverted)
        .map(|message| {
            serde_json::json!({
                "role": message.role,
                "text": crate::redaction::redact_text(&message.text),
                "attachments": message.attachments.iter().map(|path| digest_label("attachment", path)).collect::<Vec<_>>(),
            })
        })
        .collect::<Vec<_>>();
    let tools = record
        .tool_calls
        .iter()
        .map(|tool| {
            let output = tool.output.as_deref().unwrap_or_default();
            let bounded = bounded_utf8(output, MAX_LEDGER_TOOL_OUTPUT_BYTES);
            serde_json::json!({
                "name": tool.name,
                "status": tool.status,
                "input": crate::redaction::redact_text(&tool.input.to_string()),
                "output": crate::redaction::redact_text(&bounded),
            })
        })
        .collect::<Vec<_>>();
    serde_json::json!({
        "turnId": record.turn.id,
        "turnEpoch": record.turn.epoch,
        "status": record.turn.status,
        "messages": messages,
        "tools": tools,
        "approvals": record.approvals.iter().map(|approval| serde_json::json!({
            "action": approval.action,
            "status": approval.status,
            "decision": approval.decision,
        })).collect::<Vec<_>>(),
        "attachments": record.attachments.iter().map(|item| serde_json::json!({
            "pathDigest": item.path_digest,
            "ordinal": item.ordinal,
        })).collect::<Vec<_>>(),
        "context": record.session_context,
    })
}

fn digest_label(kind: &str, value: &str) -> String {
    use sha2::{Digest, Sha256};
    format!("[{kind}:sha256:{:x}]", Sha256::digest(value.as_bytes()))
}

fn bounded_utf8(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }
    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n[truncated:{} bytes]", &value[..end], value.len() - end)
}

pub fn handoff_prompt(input: &FrozenForkInput) -> String {
    format!(
        "你正在为一个 CLI Agent Engine 生成交接摘要（目标可能与源同引擎，用于从摘要派生新会话）。只输出一个 JSON 对象，不要 Markdown 代码围栏或额外文字。\nJSON 必须包含：contractVersion=1、goal 字符串、completed 字符串数组、currentState 字符串、decisionsAndFiles 字符串数组、remaining 字符串数组、constraints 字符串数组。\n摘要必须基于给定事实，明确未知，不得虚构；不要输出密钥、token 或认证内容。\n源 Engine：{}\n目标 Engine：{}\n源标题：{}\n冻结 Turn：{} (epoch {})\n\nTurnLedger JSON：\n{}",
        input.source_engine,
        input.target_engine,
        input.source_title,
        input.boundary_turn_id,
        input.boundary_turn_epoch,
        input.ledger_json,
    )
}

pub fn parse_handoff(raw: &str) -> Result<HandoffContent, String> {
    let trimmed = raw.trim();
    let json = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
        .and_then(|value| value.strip_suffix("```"))
        .map(str::trim)
        .unwrap_or(trimmed);
    let content: HandoffContent = serde_json::from_str(json)
        .map_err(|error| format!("[handoff_invalid] 模型输出不是有效 Handoff JSON：{error}"))?;
    content.validate()?;
    Ok(content)
}

pub async fn start_session_fork(
    app: &AppHandle,
    source_session_id: &str,
    target_engine: &str,
    boundary_turn_id: Option<&str>,
) -> Result<BackgroundOperation, String> {
    let history = app
        .try_state::<SessionHistoryStore>()
        .ok_or("历史存储未初始化")?;
    let detail = history.get_session(source_session_id)?;
    let ledger = history.get_turn_ledger(source_session_id)?;
    let frozen = freeze_fork_input(&detail, &ledger, target_engine, boundary_turn_id)?;
    let frozen_value = serde_json::to_value(&frozen).map_err(|error| error.to_string())?;
    let provider_store = app
        .try_state::<ProviderStore<KeyringSecretStore>>()
        .ok_or("服务商存储未初始化")?;
    let profiles = app
        .try_state::<SubscriptionProfileStore>()
        .ok_or("订阅 Profile 存储未初始化")?;
    let capabilities = app
        .try_state::<EngineCapabilityRegistry>()
        .ok_or("Engine Capability Registry 未初始化")?;
    let settings = load_app_settings_from_store(&history)?;
    let mut committed = None;
    for _ in 0..3 {
        let candidate = provider_store.route_candidate()?;
        let binding = candidate
            .config
            .bindings
            .iter()
            .find(|binding| binding.engine_id == target_engine)
            .cloned()
            .ok_or_else(|| format!("引擎还没有配置生效绑定：{target_engine}"))?;
        let model = binding
            .fast_model
            .as_deref()
            .filter(|model| !model.trim().is_empty())
            .unwrap_or(&binding.primary_model)
            .to_string();
        // 模型目录预检（九次反馈）：分叉摘要路由取 绑定快速模型→主模型；目录漂移或旧
        // 配置迁移可能留下引擎链路已不认识的 ID，spawn 只会得到引擎侧含糊的
        // unrecognized_model。先在 Helm 侧给出可定位、可操作的错误（保存时已有同源
        // 校验，这里是运行时兜底）。
        ensure_fork_model_in_catalog(&candidate.config.models, &binding.provider_id, &model)?;
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
            profiles.append_launch_env(&mut env, target_engine)?;
        }
        env.extend(agent_environment_from_settings(&settings));
        let bin = candidate
            .config
            .engine_bin(target_engine)
            .filter(|bin| !bin.is_empty())
            .unwrap_or(if target_engine == "codex" {
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
            "fork_job",
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
                kind: "fork_job".to_string(),
                source_session_id: Some(source_session_id.to_string()),
                input_digest: digest_json(&frozen)?,
                input: Some(frozen_value.clone()),
                idempotency_key: format!(
                    "fork_job:{source_session_id}:{}:{target_engine}:binding:{}:v1",
                    frozen.boundary_turn_id, binding.revision
                ),
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
            history.create_background_operation(&new_operation)
        })? {
            Some((existing, false)) => {
                // 幂等命中（十次反馈修复）：此前不分状态原样返回旧记录——终态失败的
                // 分叉会被永久缓存，用户每次点分叉看到的都是同一句旧 os error 206，
                // 真实 spawn 从未再次发生，任何二进制修复都「看起来无效」。终态失败
                // 改为与手工重试同源语义：复核冻结 spec → 重置状态 → 真实重跑；
                // 非终态（进行中/已完成）维持幂等返回。
                if !matches!(
                    existing.status.as_str(),
                    "failed" | "cancelled" | "delivery_unknown"
                ) {
                    return Ok(existing);
                }
                let (execution, retry_bin, retry_env) =
                    prepare_fork_job_retry(app, &existing.id).await?;
                let rerun_id = existing.id.clone();
                let rerun_app = app.clone();
                tauri::async_runtime::spawn(async move {
                    if let Err(error) =
                        run_fork_job(&rerun_app, execution, &retry_bin, &retry_env).await
                    {
                        eprintln!("[handoff] ForkJob 自动重跑失败：{error}");
                    }
                });
                return history
                    .load_background_operation(&rerun_id)?
                    .ok_or_else(|| format!("ForkJob 自动重跑后状态丢失：{rerun_id}"));
            }
            Some((operation, true)) => {
                committed = Some((new_operation, operation, capability, bin, env));
                break;
            }
            None => continue,
        }
    }
    let (execution, operation, capability, bin, env) = committed.ok_or_else(|| {
        "Provider 配置连续变化，ForkJob OperationStart 有界重算未能收敛，请重试".to_string()
    })?;
    if let Err(error) = ModelOnlyOperationPolicy::from_capability(&capability, operation.created_at)
    {
        history.fail_committed_background_operation(&operation.id, &error)?;
        return history
            .load_background_operation(&operation.id)?
            .ok_or_else(|| "ForkJob 失败状态丢失".to_string());
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(error) = run_fork_job(&app, execution, &bin, &env).await {
            eprintln!("[handoff] ForkJob 失败：{error}");
        }
    });
    Ok(operation)
}

async fn run_fork_job(
    app: &AppHandle,
    execution: NewBackgroundOperation,
    bin: &str,
    env: &[(String, String)],
) -> Result<(), String> {
    let history = app
        .try_state::<SessionHistoryStore>()
        .ok_or("历史存储未初始化")?;
    let runtime = app
        .try_state::<RuntimeRegistry>()
        .ok_or("RuntimeRegistry 未初始化")?;
    let frozen_value = match execution.operation.input.clone() {
        Some(input) => input,
        None => {
            let error = "ForkJob 缺少冻结输入";
            fail_fork_before_dispatch(&history, &execution.operation.id, error);
            return Err(error.to_string());
        }
    };
    let frozen: FrozenForkInput = match serde_json::from_value(frozen_value) {
        Ok(frozen) => frozen,
        Err(error) => {
            let error = format!("ForkJob 冻结输入无效：{error}");
            fail_fork_before_dispatch(&history, &execution.operation.id, &error);
            return Err(error);
        }
    };
    let frozen_digest = match digest_json(&frozen) {
        Ok(digest) => digest,
        Err(error) => {
            fail_fork_before_dispatch(&history, &execution.operation.id, &error);
            return Err(error);
        }
    };
    if frozen_digest != execution.operation.input_digest {
        let error = "[operation_input_digest_mismatch] ForkJob 冻结输入摘要不匹配";
        fail_fork_before_dispatch(&history, &execution.operation.id, error);
        return Err(error.to_string());
    }
    let mut facts: Vec<serde_json::Value> = match serde_json::from_str(&frozen.ledger_json) {
        Ok(facts) => facts,
        Err(error) => {
            let error = format!("ForkJob Ledger JSON 无效：{error}");
            fail_fork_before_dispatch(&history, &execution.operation.id, &error);
            return Err(error);
        }
    };
    let mut final_output = None;
    for depth in 0..=MAX_RECURSIVE_SUMMARY_DEPTH {
        let ledger_json = serde_json::to_string(&facts).map_err(|error| error.to_string())?;
        let mut level_input = frozen.clone();
        level_input.ledger_json = ledger_json;
        let final_prompt = handoff_prompt(&level_input);
        if final_prompt.as_bytes().len() <= MAX_HANDOFF_PROMPT_BYTES {
            let (attempt_no, output) = runtime
                .run_model_only_operation(
                    &execution.spec,
                    &execution.policy,
                    &execution.budget,
                    bin,
                    env,
                    &final_prompt,
                )
                .await?;
            final_output = Some((attempt_no, output));
            break;
        }
        if depth == MAX_RECURSIVE_SUMMARY_DEPTH {
            let error = "[fork_recursive_summary_limit] 递归摘要未能在安全深度内收敛";
            history.fail_committed_background_operation(&execution.operation.id, error)?;
            return Err(error.to_string());
        }
        let chunks = match chunk_turn_facts(&facts, MAX_HANDOFF_PROMPT_BYTES / 2) {
            Ok(chunks) => chunks,
            Err(error) => {
                fail_fork_before_dispatch(&history, &execution.operation.id, &error);
                return Err(error);
            }
        };
        if chunks.len() <= 1 {
            let error = "[fork_turn_too_large] 单个 Turn 超过摘要模型输入上限，不能静默截断";
            history.fail_committed_background_operation(&execution.operation.id, error)?;
            return Err(error.to_string());
        }
        let mut summaries = Vec::with_capacity(chunks.len());
        for (chunk_index, chunk) in chunks.into_iter().enumerate() {
            let mut chunk_input = frozen.clone();
            chunk_input.ledger_json =
                serde_json::to_string(&chunk).map_err(|error| error.to_string())?;
            let chunk_prompt = handoff_prompt(&chunk_input);
            let (attempt_no, output) = runtime
                .run_model_only_operation(
                    &execution.spec,
                    &execution.policy,
                    &execution.budget,
                    bin,
                    env,
                    &chunk_prompt,
                )
                .await?;
            let summary = match parse_handoff(&output.text) {
                Ok(summary) => summary,
                Err(error) => {
                    history.finish_background_operation(
                        &execution.operation.id,
                        attempt_no,
                        "failed",
                        None,
                        Some(&error),
                    )?;
                    return Err(error);
                }
            };
            if let Err(error) = history.complete_model_only_operation_stage(
                &execution.operation.id,
                attempt_no,
                &output,
                &format!("depth:{depth}:chunk:{chunk_index}"),
            ) {
                let _ = history.finish_background_operation(
                    &execution.operation.id,
                    attempt_no,
                    "failed",
                    None,
                    Some(&error),
                );
                return Err(error);
            }
            summaries.push(serde_json::to_value(summary).map_err(|error| error.to_string())?);
        }
        facts = summaries;
    }
    let (attempt_no, output) = match final_output {
        Some(output) => output,
        None => {
            let error = "[fork_recursive_summary_limit] 缺少最终摘要 Attempt";
            fail_fork_before_dispatch(&history, &execution.operation.id, error);
            return Err(error.to_string());
        }
    };
    let handoff = match parse_handoff(&output.text) {
        Ok(handoff) => handoff,
        Err(error) => {
            history.finish_background_operation(
                &execution.operation.id,
                attempt_no,
                "failed",
                None,
                Some(&error),
            )?;
            return Err(error);
        }
    };
    let target_session_id = format!("session-{:032x}", rand::random::<u128>());
    let result = serde_json::json!({
        "handoff": handoff,
        "frozenInput": frozen,
        "targetSessionId": target_session_id,
    });
    if let Err(error) =
        history.complete_model_only_operation(&execution.operation.id, attempt_no, &output, &result)
    {
        let _ = history.finish_background_operation(
            &execution.operation.id,
            attempt_no,
            "failed",
            None,
            Some(&error),
        );
        return Err(error);
    }
    let _ = app.emit("helm-sessions-changed", &target_session_id);
    let _ = app.emit("helm-background-operation-changed", &execution.operation.id);
    Ok(())
}

fn fail_fork_before_dispatch(history: &SessionHistoryStore, operation_id: &str, error: &str) {
    let _ = history.fail_committed_background_operation(operation_id, error);
}

fn chunk_turn_facts(
    facts: &[serde_json::Value],
    max_bytes: usize,
) -> Result<Vec<Vec<serde_json::Value>>, String> {
    let mut chunks = Vec::<Vec<serde_json::Value>>::new();
    let mut current = Vec::new();
    let mut current_bytes = 2usize;
    for fact in facts {
        let fact_bytes = serde_json::to_vec(fact)
            .map_err(|error| error.to_string())?
            .len();
        if fact_bytes.saturating_add(2) > max_bytes {
            return Err(
                "[fork_turn_too_large] 单个 Turn 超过摘要模型输入上限，不能静默截断".to_string(),
            );
        }
        if !current.is_empty() && current_bytes.saturating_add(fact_bytes + 1) > max_bytes {
            chunks.push(std::mem::take(&mut current));
            current_bytes = 2;
        }
        current.push(fact.clone());
        current_bytes = current_bytes.saturating_add(fact_bytes + 1);
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    Ok(chunks)
}

/// 重试准备（十次反馈拆分）：校验 ForkJob 处于可重试终态（failed/cancelled/
/// delivery_unknown）、冻结输入摘要一致、当前配置仍能逐字段复现冻结 spec，然后把
/// 操作状态重置回 `committed` 并清空旧错误；返回真实重跑所需的 execution/bin/env。
/// 拆成独立步骤供两条入口复用：设置任务 Tab 的手工重试（`retry_fork_job`），以及
/// 侧栏再次分叉时对终态失败旧记录的自动重跑（`start_session_fork` 幂等命中分支）。
async fn prepare_fork_job_retry(
    app: &AppHandle,
    operation_id: &str,
) -> Result<(NewBackgroundOperation, String, Vec<(String, String)>), String> {
    let history = app
        .try_state::<SessionHistoryStore>()
        .ok_or("历史存储未初始化")?;
    let execution = history
        .load_background_operation_execution(operation_id)?
        .ok_or_else(|| format!("找不到 BackgroundOperation：{operation_id}"))?;
    if execution.operation.kind != "fork_job" || execution.spec.purpose != "fork_job" {
        return Err("当前 BackgroundOperation 不是 ForkJob".to_string());
    }
    if !matches!(
        execution.operation.status.as_str(),
        "failed" | "cancelled" | "delivery_unknown"
    ) {
        return Err("ForkJob 当前状态不允许手工重试".to_string());
    }
    let frozen: FrozenForkInput = serde_json::from_value(
        execution
            .operation
            .input
            .clone()
            .ok_or("ForkJob 缺少冻结输入")?,
    )
    .map_err(|error| format!("ForkJob 冻结输入无效：{error}"))?;
    if digest_json(&frozen)? != execution.operation.input_digest {
        return Err("[operation_input_digest_mismatch] ForkJob 冻结输入摘要不匹配".to_string());
    }
    let provider_store = app
        .try_state::<ProviderStore<KeyringSecretStore>>()
        .ok_or("服务商存储未初始化")?;
    let profiles = app
        .try_state::<SubscriptionProfileStore>()
        .ok_or("订阅 Profile 存储未初始化")?;
    let settings = load_app_settings_from_store(&history)?;
    let candidate = provider_store.route_candidate()?;
    let current_binding = candidate
        .config
        .bindings
        .iter()
        .find(|binding| binding.engine_id == execution.spec.engine_id)
        .cloned()
        .ok_or_else(|| format!("引擎还没有配置生效绑定：{}", execution.spec.engine_id))?;
    let launch_binding = BindingConfig {
        provider_id: execution.spec.provider_id.clone(),
        primary_model: execution.spec.routed_model_id.clone(),
        fast_model: Some(execution.spec.routed_model_id.clone()),
        assistant_model_id: None,
        reasoning_effort: Some(execution.spec.routed_reasoning_effort),
        revision: execution.spec.binding_revision,
        ..current_binding
    };
    ensure_binding_runtime_ready(&profiles, &candidate.config, &launch_binding).await?;
    let mut env = provider_store.launch_env_for_config(&candidate.config, &launch_binding)?;
    let subscription_home =
        subscription_profile_for_binding(&profiles, &candidate.config, &launch_binding)?;
    if subscription_home.is_some() {
        profiles.append_launch_env(&mut env, &execution.spec.engine_id)?;
    }
    env.extend(agent_environment_from_settings(&settings));
    let bin = candidate
        .config
        .engine_bin(&execution.spec.engine_id)
        .filter(|bin| !bin.is_empty())
        .unwrap_or(if execution.spec.engine_id == "codex" {
            "codex"
        } else {
            "claude"
        })
        .to_string();
    let route = build_runtime_route(
        &candidate.config,
        &launch_binding,
        &execution.spec.routed_model_id,
        &bin,
        &env,
        execution.spec.routed_reasoning_effort,
        execution.spec.pricing_basis_snapshot.profile.clone(),
    )?;
    if route.engine_id != execution.spec.engine_id
        || route.provider_id != execution.spec.provider_id
        || route.model_id != execution.spec.routed_model_id
        || route.engine_profile_digest != execution.spec.engine_profile_digest
        || route.provider_launch_profile_ref != execution.spec.provider_launch_profile_ref
        || route.provider_launch_profile_digest != execution.spec.provider_launch_profile_digest
        || route.launch_config_digest != execution.spec.launch_config_digest
    {
        return Err(
            "[operation_frozen_launch_unavailable] 当前配置无法复现冻结的 ForkJob spec".to_string(),
        );
    }
    let capabilities = app
        .try_state::<EngineCapabilityRegistry>()
        .ok_or("Engine Capability Registry 未初始化")?;
    let capability =
        resolve_engine_capability_snapshot(&capabilities, &route, &bin, &env, subscription_home)
            .await?;
    ModelOnlyOperationPolicy::from_capability(&capability, crate::util::now_millis())?;
    let transitioned = provider_store
        .commit_route_if_unchanged(&candidate.config_digest, |_| {
            history.prepare_background_operation_retry(operation_id)
        })?;
    if transitioned.is_none() {
        return Err("Provider 配置在 ForkJob 重试复核期间发生变化，请重新重试".to_string());
    }
    Ok((execution, bin, env))
}

/// 手工重试入口（设置任务 Tab）：语义不变，准备与重跑拆分后仅组合两步。
pub async fn retry_fork_job(app: &AppHandle, operation_id: &str) -> Result<(), String> {
    let (execution, bin, env) = prepare_fork_job_retry(app, operation_id).await?;
    run_fork_job(app, execution, &bin, &env).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::EngineId;

    fn catalog_model(id: &str, provider_id: &str, enabled: bool) -> ModelConfig {
        ModelConfig {
            id: id.to_string(),
            provider_id: provider_id.to_string(),
            display_name: id.to_string(),
            input_price_per_mtok: 0.0,
            output_price_per_mtok: 0.0,
            cached_input_price_per_mtok: None,
            price_source: None,
            enabled,
            context_window: None,
            capabilities: None,
        }
    }

    #[test]
    fn fork_model_must_be_in_provider_enabled_catalog() {
        let models = vec![
            catalog_model("deepseek-chat", "p-ds", true),
            catalog_model("stale-id", "p-ds", false),
        ];
        assert!(
            super::ensure_fork_model_in_catalog(&models, "p-ds", "deepseek-chat").is_ok(),
            "启用目录内的模型应放行"
        );
        for bad in ["stale-id", "never-existed"] {
            let error = super::ensure_fork_model_in_catalog(&models, "p-ds", bad)
                .expect_err("目录外模型必须在 spawn 前拦截");
            assert!(
                error.starts_with("[operation_model_unavailable]"),
                "错误必须带 tag：{error}"
            );
            assert!(error.contains(bad), "错误必须指认模型：{error}");
        }
        // 其他服务商的条目不能顶替本服务商的缺失（本服务商有目录但没有该模型）。
        let cross = vec![
            catalog_model("deepseek-chat", "p-ds", true),
            catalog_model("shared-id", "p-other", true),
        ];
        assert!(
            super::ensure_fork_model_in_catalog(&cross, "p-ds", "shared-id").is_err(),
            "跨服务商目录不得视为已知"
        );
        // 空目录（旧配置未同步过模型目录）不判定，放行交由引擎侧兜底。
        assert!(
            super::ensure_fork_model_in_catalog(&[], "p-ds", "anything").is_ok(),
            "空目录不应误拦旧配置"
        );
    }

    #[test]
    fn parses_structured_handoff_and_builds_lossy_context() {
        let parsed = parse_handoff(
            r#"{"contractVersion":1,"goal":"完成登录修复","completed":["定位超时"],"currentState":"测试通过","decisionsAndFiles":["src/login.rs"],"remaining":["人工验收"],"constraints":["不得记录 token"]}"#,
        )
        .unwrap();
        assert_eq!(parsed.goal, "完成登录修复");
        assert!(parsed.as_context().contains("细节可能有损"));
    }

    #[test]
    fn rejects_incomplete_handoff() {
        let error = parse_handoff(
            r#"{"contractVersion":1,"goal":"","completed":[],"currentState":"","decisionsAndFiles":[],"remaining":[],"constraints":[]}"#,
        )
        .unwrap_err();
        assert!(error.contains("handoff_invalid"));
    }

    #[test]
    fn utf8_bound_does_not_split_codepoint() {
        let value = "中".repeat(30);
        let bounded = bounded_utf8(&value, 10);
        assert!(bounded.starts_with("中中中"));
        assert!(bounded.contains("truncated"));
    }

    #[test]
    fn recursive_chunks_keep_stable_turn_boundaries() {
        let facts = vec![
            serde_json::json!({"turnId":"turn-1","text":"a".repeat(40)}),
            serde_json::json!({"turnId":"turn-2","text":"b".repeat(40)}),
            serde_json::json!({"turnId":"turn-3","text":"c".repeat(40)}),
        ];
        let chunks = chunk_turn_facts(&facts, 100).unwrap();
        assert!(chunks.len() > 1);
        assert_eq!(chunks.iter().map(Vec::len).sum::<usize>(), facts.len());
        assert_eq!(chunks[0][0]["turnId"], "turn-1");
    }

    #[test]
    fn recursive_chunks_reject_an_oversized_single_turn() {
        let facts = vec![serde_json::json!({
            "turnId": "turn-too-large",
            "text": "x".repeat(200),
        })];
        let error = chunk_turn_facts(&facts, 100).unwrap_err();
        assert!(error.contains("fork_turn_too_large"));
    }

    #[test]
    fn freeze_allows_same_engine_fork() {
        let detail = session_detail_with_turn(EngineId::Codex, "s-source", "succeeded", 7);
        let ledger = ledger_with_turn("turn-7", "succeeded", 7);
        let frozen =
            freeze_fork_input(&detail, &ledger, "codex", None).expect("同引擎派生应被允许");
        assert_eq!(frozen.source_engine, "codex");
        assert_eq!(frozen.target_engine, "codex");
        assert_eq!(frozen.boundary_turn_id, "turn-7");
        assert_eq!(frozen.source_session_id, "s-source");
    }

    #[test]
    fn freeze_allows_cross_engine_fork() {
        let detail = session_detail_with_turn(EngineId::ClaudeCode, "s-source", "succeeded", 7);
        let ledger = ledger_with_turn("turn-7", "succeeded", 7);
        let frozen =
            freeze_fork_input(&detail, &ledger, "codex", None).expect("跨引擎派生应被允许");
        assert_eq!(frozen.source_engine, "claude-code");
        assert_eq!(frozen.target_engine, "codex");
    }

    #[test]
    fn freeze_rejects_busy_source_turn() {
        let detail = session_detail_with_turn(EngineId::ClaudeCode, "s-source", "running", 7);
        let ledger = ledger_with_turn("turn-7", "succeeded", 7);
        let error = freeze_fork_input(&detail, &ledger, "codex", None).unwrap_err();
        assert!(error.contains("fork_source_busy"));
    }

    fn session_detail_with_turn(
        engine: EngineId,
        session_id: &str,
        turn_status: &str,
        epoch: u64,
    ) -> crate::sessions::SessionDetail {
        use crate::sessions::{SessionStatus, SessionSummary};
        crate::sessions::SessionDetail {
            summary: SessionSummary {
                id: session_id.to_string(),
                cli_session_id: None,
                title: "测试会话".to_string(),
                engine,
                model: "model-test".to_string(),
                cwd: "D:/work".to_string(),
                status: SessionStatus::Idle,
                message_count: 0,
                input_tokens: 0,
                output_tokens: 0,
                cost_usd: 0.0,
                created_at: 0,
                updated_at: 0,
                summary: None,
                pinned: false,
                runtime_capabilities: None,
                safe_permission_profile: "standard".to_string(),
                folder_id: "folder-default".to_string(),
                cached_input_tokens: 0,
                cache_write_input_tokens: 0,
                last_context_tokens: None,
                last_context_window: None,
                preferred_model: None,
                preferred_reasoning_effort: None,
                archived: false,
                current_tool: None,
                current_target: None,
                change_additions: 0,
                change_deletions: 0,
                pending_approval: false,
                last_turn_failed: false,
                forked_from: None,
                last_turn_status: None,
            },
            messages: Vec::new(),
            tool_calls: Vec::new(),
            checkpoints: Vec::new(),
            approvals: Vec::new(),
            turns: vec![crate::sessions::SessionTurn {
                id: format!("turn-{epoch}"),
                epoch,
                mode: "default".to_string(),
                permission_profile: "standard".to_string(),
                status: turn_status.to_string(),
                started_at: 1,
                ended_at: Some(2),
                terminal_reason: None,
                provider_display_name: None,
                requested_model_id: None,
                routed_model_id: None,
                requested_reasoning_effort: None,
                routed_reasoning_effort: None,
                resolution_source: None,
            }],
            session_context: Vec::new(),
            fork: None,
        }
    }

    fn ledger_with_turn(
        turn_id: &str,
        status: &str,
        epoch: u64,
    ) -> Vec<crate::sessions::TurnLedgerRecord> {
        use crate::sessions::{SessionMessage, SessionToolCall, SessionTurn, TurnLedgerRecord};
        vec![TurnLedgerRecord {
            turn: SessionTurn {
                id: turn_id.to_string(),
                epoch,
                mode: "default".to_string(),
                permission_profile: "standard".to_string(),
                status: status.to_string(),
                started_at: 1,
                ended_at: Some(2),
                terminal_reason: None,
                provider_display_name: None,
                requested_model_id: None,
                routed_model_id: None,
                requested_reasoning_effort: None,
                routed_reasoning_effort: None,
                resolution_source: None,
            },
            route: crate::sessions::TurnLedgerRoute {
                engine_id: "codex".to_string(),
                provider_id: "provider-test".to_string(),
                requested_model_id: "model-test".to_string(),
                routed_model_id: "model-test".to_string(),
                requested_reasoning_effort: "auto".to_string(),
                routed_reasoning_effort: "auto".to_string(),
                resolution_source: "compatible".to_string(),
                launch_config_digest: "d".to_string(),
                pricing_basis_snapshot: serde_json::Value::Null,
            },
            messages: Vec::<SessionMessage>::new(),
            tool_calls: Vec::<SessionToolCall>::new(),
            approvals: Vec::new(),
            checkpoints: Vec::new(),
            usage: Vec::new(),
            attachments: Vec::new(),
            session_context: Vec::new(),
        }]
    }
}
