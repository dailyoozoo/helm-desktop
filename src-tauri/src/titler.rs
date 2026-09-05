//! fast model 自动起标题 + 会话摘要（BackgroundOperation）。
//!
//! 首轮 TurnComplete 后，用当前 Engine Binding 的 fast model（缺失回落 primary）经
//! ModelOnlyOperationPolicy + 真实 CLI Adapter 生成标题与摘要。
//!
//! 披露与开关（隐私要求：默认外发必须显式披露 + 可关）：
//! - 设置 `general.autoTitleSessions` 可关闭（默认开）；
//! - 内容只发给当前 Engine Binding 的服务商；
//! - API/订阅共用真实 CLI 路径，不保留 direct HTTP 旁路。

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
use crate::providers::{BindingConfig, KeyringSecretStore, ProviderStore};
use crate::reasoning::ReasoningEffort;
use crate::runtime_registry::RuntimeRegistry;
use crate::sessions::SessionHistoryStore;
use crate::settings::{load_app_settings_from_store, AppSettings};
use crate::subscription_profiles::SubscriptionProfileStore;
use crate::turn_start::{build_runtime_route, digest_json};
use tauri::{AppHandle, Emitter, Manager};

const MAX_SNIPPET_CHARS: usize = 600;
const MAX_TITLE_CHARS: usize = 24;

/// TurnComplete 后调用：条件满足则后台生成标题（不阻塞事件流）
pub fn maybe_generate_title(app: &AppHandle, history_session_id: &str) {
    let app = app.clone();
    let history_session_id = history_session_id.to_string();
    tauri::async_runtime::spawn(async move {
        if let Err(err) = generate_title(&app, &history_session_id).await {
            // 起标题是锦上添花：失败只留诊断，不打扰用户；摘要仍为空，下一轮会再试
            eprintln!("[titler] 会话 {history_session_id} 自动起标题跳过/失败：{err}");
        }
    });
}

async fn generate_title(app: &AppHandle, history_session_id: &str) -> Result<(), String> {
    let history_store = app
        .try_state::<SessionHistoryStore>()
        .ok_or("历史存储未初始化")?;
    let settings = load_app_settings_from_store(&history_store)?;
    if !settings.general.auto_title_sessions {
        return Ok(());
    }
    if !history_store.session_needs_auto_title(history_session_id)? {
        return Ok(());
    }

    let detail = history_store.get_session(history_session_id)?;
    let user_text = detail
        .messages
        .iter()
        .find(|message| matches!(message.role, crate::protocol::Role::User))
        .map(|message| message.text.clone())
        .ok_or("没有用户消息")?;
    let assistant_text = detail
        .messages
        .iter()
        .find(|message| matches!(message.role, crate::protocol::Role::Assistant))
        .map(|message| message.text.clone())
        .ok_or("没有助手回复")?;

    let provider_store = app
        .try_state::<ProviderStore<KeyringSecretStore>>()
        .ok_or("服务商存储未初始化")?;
    let prompt = title_prompt(&user_text, &assistant_text);
    let profiles = app
        .try_state::<SubscriptionProfileStore>()
        .ok_or("订阅 Profile 存储未初始化")?;
    let capabilities = app
        .try_state::<EngineCapabilityRegistry>()
        .ok_or("Engine Capability Registry 未初始化")?;
    let runtime_registry = app
        .try_state::<RuntimeRegistry>()
        .ok_or("RuntimeRegistry 未初始化")?;
    let engine_id = match detail.summary.engine {
        crate::protocol::EngineId::ClaudeCode => "claude-code",
        crate::protocol::EngineId::Codex => "codex",
    };
    // Codex 当前没有可证明的原生 no-tools 合同（变更-27I：投递前阻断），自动标题
    // 无法经真实 CLI 运行；提前跳过，不再跑完整路由/能力探测后才知道不可用。
    if engine_id == "codex" {
        eprintln!("[titler] 会话 {history_session_id} 跳过自动起标题：Codex 无 no-tools 合同");
        return Ok(());
    }
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
        append_operation_environment(&mut env, &settings);
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
            "auto_title",
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
                kind: "auto_title".to_string(),
                source_session_id: Some(history_session_id.to_string()),
                input_digest: digest_json(&prompt)?,
                input: None,
                idempotency_key: format!("auto_title:{history_session_id}:v1"),
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
            Some((operation, false)) => return existing_operation_result(&operation),
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
    let (title, summary) = parse_title_summary(&output.text, &user_text);
    let result = serde_json::json!({"title": title, "summary": summary});
    history_store.complete_model_only_operation(
        &operation.operation.id,
        attempt_no,
        &output,
        &result,
    )?;
    // 通知前端刷新会话列表（侧栏标题即时更新）
    let _ = app.emit("helm-sessions-changed", history_session_id);
    Ok(())
}

pub async fn retry_background_operation(app: &AppHandle, operation_id: &str) -> Result<(), String> {
    let history_store = app
        .try_state::<SessionHistoryStore>()
        .ok_or("历史存储未初始化")?;
    let execution = history_store
        .load_background_operation_execution(operation_id)?
        .ok_or_else(|| format!("找不到 BackgroundOperation：{operation_id}"))?;
    if execution.operation.kind != "auto_title" || execution.spec.purpose != "auto_title" {
        return Err("当前 BackgroundOperation 类型不支持手工重试".to_string());
    }
    if !matches!(
        execution.operation.status.as_str(),
        "failed" | "cancelled" | "delivery_unknown"
    ) {
        return Err("BackgroundOperation 当前状态不允许手工重试".to_string());
    }
    let history_session_id = execution
        .operation
        .source_session_id
        .as_deref()
        .ok_or("auto_title Operation 缺少源 Session")?;
    let detail = history_store.get_session(history_session_id)?;
    let user_text = detail
        .messages
        .iter()
        .find(|message| matches!(message.role, crate::protocol::Role::User))
        .map(|message| message.text.clone())
        .ok_or("源 Session 没有用户消息")?;
    let assistant_text = detail
        .messages
        .iter()
        .find(|message| matches!(message.role, crate::protocol::Role::Assistant))
        .map(|message| message.text.clone())
        .ok_or("源 Session 没有助手回复")?;
    let prompt = title_prompt(&user_text, &assistant_text);
    if digest_json(&prompt)? != execution.operation.input_digest {
        return Err("[operation_input_digest_mismatch] 无法从源 Session 重建冻结输入".into());
    }

    let settings = load_app_settings_from_store(&history_store)?;
    let provider_store = app
        .try_state::<ProviderStore<KeyringSecretStore>>()
        .ok_or("服务商存储未初始化")?;
    let profiles = app
        .try_state::<SubscriptionProfileStore>()
        .ok_or("订阅 Profile 存储未初始化")?;
    let capabilities = app
        .try_state::<EngineCapabilityRegistry>()
        .ok_or("Engine Capability Registry 未初始化")?;
    let runtime_registry = app
        .try_state::<RuntimeRegistry>()
        .ok_or("RuntimeRegistry 未初始化")?;
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
    append_operation_environment(&mut env, &settings);
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
            "[operation_frozen_launch_unavailable] 当前配置无法复现冻结的 Operation spec".into(),
        );
    }
    let capability =
        resolve_engine_capability_snapshot(&capabilities, &route, &bin, &env, subscription_home)
            .await?;
    ModelOnlyOperationPolicy::from_capability(&capability, crate::util::now_millis())?;
    let transitioned = provider_store
        .commit_route_if_unchanged(&candidate.config_digest, |_| {
            history_store.prepare_background_operation_retry(operation_id)
        })?;
    if transitioned.is_none() {
        return Err("Provider 配置在重试复核期间发生变化，请重新重试".to_string());
    }
    let (attempt_no, output) = runtime_registry
        .run_model_only_operation(
            &execution.spec,
            &execution.policy,
            &execution.budget,
            &bin,
            &env,
            &prompt,
        )
        .await?;
    let (title, summary) = parse_title_summary(&output.text, &user_text);
    let result = serde_json::json!({"title": title, "summary": summary});
    history_store.complete_model_only_operation(operation_id, attempt_no, &output, &result)?;
    let _ = app.emit("helm-sessions-changed", history_session_id);
    Ok(())
}

fn append_operation_environment(env: &mut Vec<(String, String)>, settings: &AppSettings) {
    env.extend(agent_environment_from_settings(settings));
}

fn existing_operation_result(operation: &BackgroundOperation) -> Result<(), String> {
    match operation.status.as_str() {
        "succeeded" | "running" | "committed" => Ok(()),
        _ => Err(operation
            .error_code
            .clone()
            .unwrap_or_else(|| format!("自动标题任务状态：{}", operation.status))),
    }
}

/// 构造起标题的 prompt（截断首轮内容，避免长对话浪费 token）
pub fn title_prompt(user_text: &str, assistant_text: &str) -> String {
    format!(
        "根据下面这轮对话，输出两行中文：\n第一行：不超过 16 个字的会话标题（不要引号、句号）。\n第二行：一句话摘要（不超过 40 字）。\n只输出这两行，不要任何其他内容。\n\n用户：{}\n\n助手：{}",
        truncate_chars(user_text, MAX_SNIPPET_CHARS),
        truncate_chars(assistant_text, MAX_SNIPPET_CHARS),
    )
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let truncated: String = text.chars().take(max_chars).collect();
    format!("{truncated}…")
}

/// 解析模型输出：第一行标题、第二行摘要；输出不合规时回退到首条消息截断
pub fn parse_title_summary(raw: &str, fallback_user_text: &str) -> (String, String) {
    let mut lines = raw
        .lines()
        .map(|line| {
            line.trim()
                .trim_start_matches(['#', '-', '*', ' '])
                .trim_matches(['"', '“', '”', '「', '」'])
                .trim()
        })
        .filter(|line| !line.is_empty());
    let title_line = lines.next().unwrap_or("");
    let summary_line = lines.next().unwrap_or("");

    let title = if title_line.is_empty() {
        truncate_chars(fallback_user_text.trim(), 20)
    } else {
        truncate_chars(title_line, MAX_TITLE_CHARS)
    };
    let summary = if summary_line.is_empty() {
        title.clone()
    } else {
        truncate_chars(summary_line, 80)
    };
    (title, summary)
}

#[cfg(test)]
mod tests {
    use super::{parse_title_summary, title_prompt};

    #[test]
    fn title_prompt_truncates_long_first_turn() {
        let long_user = "长".repeat(2000);
        let prompt = title_prompt(&long_user, "回复");
        assert!(prompt.chars().count() < 800, "prompt 必须截断长对话");
        assert!(prompt.contains('…'));
        assert!(prompt.contains("16 个字"));
    }

    #[test]
    fn parse_title_summary_takes_two_lines_and_strips_quotes() {
        let (title, summary) = parse_title_summary(
            "「修复登录超时」\n排查并修复了登录接口 30s 超时的问题。\n多余的第三行",
            "fallback",
        );
        assert_eq!(title, "修复登录超时");
        assert_eq!(summary, "排查并修复了登录接口 30s 超时的问题。");
    }

    #[test]
    fn parse_title_summary_falls_back_to_user_text_when_output_is_garbage() {
        let (title, summary) = parse_title_summary("   \n\n", "帮我看看这个报错是怎么回事");
        assert_eq!(title, "帮我看看这个报错是怎么回事");
        assert_eq!(summary, title);
    }

    #[test]
    fn parse_title_summary_caps_overlong_title() {
        let raw = format!("{}\n摘要", "标".repeat(60));
        let (title, _) = parse_title_summary(&raw, "fallback");
        assert!(title.chars().count() <= 25, "超长标题必须截断：{title}");
    }
}
