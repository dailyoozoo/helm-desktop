//! Tauri 命令 + 会话存储。
//!
//! 前端 `src/engine/transport.ts` 通过 `invoke` 调这些命令；归一化事件经
//! `app.emit("agent-event", ...)` 推回前端（见 adapter.rs）。
//! 注意：JS 侧用 camelCase 参数（`handleId`），Tauri 会自动映射到 Rust 的 snake_case（`handle_id`）。

use crate::adapter::{
    agent_environment_from_settings, apply_codex_native_search, apply_codex_search_catalog,
    apply_inherited_agent_environment, build_codex_command, build_command,
    codex_native_search_enabled, codex_provider_config_args, create_codex_auth_home,
    log_runtime_line, start_claude_with_resume_and_reasoning, start_codex_with_reasoning,
    ApprovalDecision, TurnMode,
};
use crate::budget::{BudgetDimension, TurnBudgetSnapshot};
use crate::capability_registry::{
    binary_identity, bounded_probe_output, claude_capabilities_from_help,
    claude_model_only_contract_from_help, codex_capabilities_from_handshake,
    launch_profile_identity, resume_strategy, CapabilityIdentity, EngineCapabilityRegistry,
    EngineCapabilitySnapshot, ResumeStrategy, CAPABILITY_PROBE_OUTPUT_LIMIT,
};
use crate::codex_app_server::spawn_codex_app_server;
use crate::operations::ModelOnlyOperationPolicy;
use crate::protocol::EngineId;
use crate::providers::{
    classify_failure, list_provider_models,
    read_engine_config_file as read_engine_config_file_from_disk, sync_provider_models,
    test_engine_connection, test_provider_connection, test_provider_draft,
    write_engine_config_file as write_engine_config_file_to_disk, AppConfig, BindingConfig,
    ConnectionResult, EngineConfig, EngineConfigFile, KeyringSecretStore, ModelConfig, PriceSource,
    Protocol, ProviderConfig, ProviderKind, ProviderModelListing, ProviderStore, ProviderTest,
    TestOutcome,
};
use crate::reasoning::{
    claude_reasoning_capability, codex_reasoning_capability, ReasoningEffort,
    ReasoningEffortCapability, ReasoningEffortSource, ReasoningEffortSupport,
};
use crate::runtime_registry::{RuntimeOwnerRef, RuntimeRegistry};
use crate::session_actor::SessionActorHandle;
use crate::sessions::{
    NewSessionRecord, SessionDetail, SessionFolder, SessionHistoryStore, SessionSummary,
};
use crate::settings::load_app_settings_from_store;
use crate::subscription_profiles::SubscriptionProfileStore;
use crate::turn_start::{
    build_runtime_route, BindingLiveRouteResolver, PricingBasisSnapshot, RuntimeRoute,
    TurnStartCommand,
};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::io::AsyncReadExt;

const CODEX_MODEL_CATALOG_OUTPUT_LIMIT: usize = 2 * 1024 * 1024;
const CODEX_SEARCH_CATALOG_JSON_ENV: &str = "HELM_CODEX_MODEL_CATALOG_JSON";
const CODEX_SEARCH_CATALOG_DIGEST_ENV: &str = "HELM_CODEX_MODEL_CATALOG_DIGEST";
const CODEX_SEARCH_TRANSPORT_ENV: &str = "HELM_CODEX_SEARCH_TRANSPORT";
const CODEX_BINARY_IDENTITY_ENV: &str = "HELM_CODEX_BINARY_IDENTITY";

fn codex_bundled_catalog_cache() -> &'static Mutex<HashMap<String, Vec<u8>>> {
    static CACHE: std::sync::OnceLock<Mutex<HashMap<String, Vec<u8>>>> = std::sync::OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

#[derive(Debug, PartialEq, Eq)]
enum CodexSearchCatalogPlan {
    Unavailable,
    /// 模型不在 bundled 目录（自定义服务商）：搜索能力未知。按红线不硬禁、不阻断模型输入，
    /// 允许运行时尝试原生 WebSearch，由真实 Runtime 观察确认可用与否（对齐「Codex 能联网搜索」）。
    Unknown,
    HostedResponses,
    HostedResponsesCompatibility(Vec<u8>),
}

fn codex_search_catalog_plan(
    raw_catalog: &[u8],
    model: &str,
) -> Result<CodexSearchCatalogPlan, String> {
    let mut catalog: serde_json::Value = serde_json::from_slice(raw_catalog)
        .map_err(|error| format!("[codex_search_catalog_invalid] 模型目录 JSON 无效：{error}"))?;
    let models = catalog
        .get_mut("models")
        .and_then(serde_json::Value::as_array_mut)
        .ok_or_else(|| {
            "[codex_search_catalog_schema] 模型目录缺少 models 数组，已阻止启动".to_string()
        })?;
    let entry = models
        .iter_mut()
        .find(|entry| entry.get("slug").and_then(serde_json::Value::as_str) == Some(model));
    let Some(entry) = entry else {
        // 2026-08-27 用户实测：自定义服务商（OPENAI_BASE_URL）的模型不在 bundled 目录，
        // 只说明搜索能力未知，不代表模型不能跑、也不代表网关不支持原生 WebSearch。
        // 不能硬禁（否则「查天气」类联网请求永远失败）；改为允许运行时尝试原生 WebSearch，
        // 由真实 Runtime 观察确认可用与否，观察不可用才回落 [runtime_web_search_unavailable]。
        return Ok(CodexSearchCatalogPlan::Unknown);
    };
    let supports_search = entry
        .get("supports_search_tool")
        .and_then(serde_json::Value::as_bool)
        .ok_or_else(|| {
            "[codex_search_catalog_schema] 模型缺少 supports_search_tool 布尔字段".to_string()
        })?;
    let use_responses_lite = entry
        .get("use_responses_lite")
        .and_then(serde_json::Value::as_bool)
        .ok_or_else(|| {
            "[codex_search_catalog_schema] 模型缺少 use_responses_lite 布尔字段".to_string()
        })?;
    if !supports_search {
        return Ok(CodexSearchCatalogPlan::Unavailable);
    }
    if entry
        .get("web_search_tool_type")
        .and_then(serde_json::Value::as_str)
        .is_none()
    {
        return Err("[codex_search_catalog_schema] 搜索模型缺少 web_search_tool_type".to_string());
    }
    if !use_responses_lite {
        return Ok(CodexSearchCatalogPlan::HostedResponses);
    }
    entry["use_responses_lite"] = serde_json::Value::Bool(false);
    let encoded = serde_json::to_vec(&catalog)
        .map_err(|error| format!("序列化 Codex 搜索兼容目录失败：{error}"))?;
    if encoded.len() > CODEX_MODEL_CATALOG_OUTPUT_LIMIT {
        return Err(
            "[codex_search_catalog_output_limit] Codex 搜索兼容目录超过写入上限".to_string(),
        );
    }
    Ok(CodexSearchCatalogPlan::HostedResponsesCompatibility(
        encoded,
    ))
}

/// 进程会话表：handleId → 会话句柄。
/// handleId 是 Helm 内部句柄，区别于 claude 自己的 sessionId（后者来自 session_started 事件）。
#[derive(Default)]
pub struct SessionStore {
    sessions: Mutex<HashMap<String, SessionActorHandle>>,
    history_session_ids: Mutex<HashMap<String, String>>,
    counter: AtomicU64,
}

fn ensure_budget_allows_turn(budget: &crate::sessions::Budget) -> Result<(), String> {
    if budget.stop_at_100
        && budget.monthly_limit > 0.0
        && budget.current_month_cost >= budget.monthly_limit
    {
        return Err(format!(
            "已超出本月预算（${:.2} / ${:.2}），无法发起新任务。请前往「用量与成本」调整预算。",
            budget.current_month_cost, budget.monthly_limit
        ));
    }
    Ok(())
}

fn ensure_pricing_allows_turn(
    budget: &crate::sessions::Budget,
    settings: &crate::settings::AppSettings,
    provider_store: &ProviderStore<KeyringSecretStore>,
    config: &AppConfig,
    provider_id: &str,
    model_id: &str,
) -> Result<(), String> {
    if settings.general.pricing_unknown_policy != "block" || budget.monthly_limit <= 0.0 {
        return Ok(());
    }
    let model = config
        .models
        .iter()
        .find(|model| model.provider_id == provider_id && model.id == model_id)
        .ok_or_else(|| format!("当前服务商没有模型目录项：{model_id}"))?;
    if provider_store
        .model_pricing_profile(config, model)?
        .is_none()
    {
        return Err(format!(
            "模型 {model_id} 没有可用价格，严格预算模式已阻止发送。请在「服务商 → 模型目录」配置价格，或把缺价策略改为提醒。"
        ));
    }
    Ok(())
}

pub(crate) fn resolve_turn_pricing_basis(
    provider_store: &ProviderStore<KeyringSecretStore>,
    config: &AppConfig,
    provider_id: &str,
    model_id: &str,
) -> Result<PricingBasisSnapshot, String> {
    let profile = config
        .models
        .iter()
        .find(|model| model.provider_id == provider_id && model.id == model_id)
        .map(|model| provider_store.model_pricing_profile(config, model))
        .transpose()?
        .flatten();
    Ok(PricingBasisSnapshot { profile })
}

fn resolve_binding_model(
    config: &AppConfig,
    binding: &BindingConfig,
    requested_model: &str,
) -> String {
    config
        .models
        .iter()
        .any(|entry| {
            entry.provider_id == binding.provider_id && entry.id == requested_model && entry.enabled
        })
        .then(|| requested_model.to_string())
        .unwrap_or_else(|| binding.primary_model.clone())
}

fn requested_model_for_binding(
    explicit_model: Option<&str>,
    preferred_model: Option<&str>,
    binding: &BindingConfig,
) -> String {
    explicit_model
        .filter(|value| !value.trim().is_empty())
        .or_else(|| preferred_model.filter(|value| !value.trim().is_empty()))
        .unwrap_or(&binding.primary_model)
        .to_string()
}

pub(crate) fn resolve_routed_effort(
    snapshot: &EngineCapabilitySnapshot,
    requested: ReasoningEffort,
) -> ReasoningEffort {
    if requested.is_auto() || snapshot.capabilities.reasoning_efforts.contains(&requested) {
        requested
    } else {
        ReasoningEffort::Auto
    }
}

pub(crate) async fn ensure_binding_runtime_ready(
    profiles: &SubscriptionProfileStore,
    config: &AppConfig,
    binding: &BindingConfig,
) -> Result<(), String> {
    let provider = config
        .providers
        .iter()
        .find(|provider| provider.id == binding.provider_id)
        .ok_or_else(|| format!("找不到服务商：{}", binding.provider_id))?;
    if matches!(provider.kind, ProviderKind::Subscription) {
        crate::settings::ensure_subscription_login(profiles, &binding.engine_id).await?;
        // 订阅会话启动前镜像用户自定义技能到隔离目录（变更-36，只操作 skills 子树、
        // 不触碰 auth.json）；失败降级为仅日志，不阻断会话启动。
        match profiles.sync_user_skills(&binding.engine_id) {
            Ok(result) if result.copied > 0 || result.updated > 0 || result.deleted > 0 => {
                eprintln!(
                    "订阅技能同步完成：复制 {}、更新 {}、删除 {}",
                    result.copied, result.updated, result.deleted
                );
            }
            Ok(_) => {}
            Err(error) => eprintln!("订阅技能同步失败（忽略，不阻断会话）：{error}"),
        }
        if binding.engine_id == "codex" {
            let codex_home = profiles.profile_dir("codex")?;
            let models =
                discover_codex_subscription_models(config, &binding.provider_id, &codex_home)
                    .await?;
            ensure_discovered_binding_model(&models, &binding.primary_model, "主模型")?;
            if let Some(fast_model) = binding
                .fast_model
                .as_deref()
                .filter(|model| !model.trim().is_empty())
            {
                ensure_discovered_binding_model(&models, fast_model, "快速模型")?;
            }
        }
    }
    Ok(())
}

pub(crate) fn subscription_profile_for_binding(
    profiles: &SubscriptionProfileStore,
    config: &AppConfig,
    binding: &BindingConfig,
) -> Result<Option<std::path::PathBuf>, String> {
    let provider = config
        .providers
        .iter()
        .find(|provider| provider.id == binding.provider_id)
        .ok_or_else(|| format!("找不到服务商：{}", binding.provider_id))?;
    if matches!(provider.kind, ProviderKind::Subscription) {
        profiles.profile_dir(&binding.engine_id).map(Some)
    } else {
        Ok(None)
    }
}

pub(crate) async fn resolve_engine_capability_snapshot(
    registry: &EngineCapabilityRegistry,
    route: &RuntimeRoute,
    bin: &str,
    env: &[(String, String)],
    subscription_home: Option<std::path::PathBuf>,
) -> Result<EngineCapabilitySnapshot, String> {
    let profile_identity = launch_profile_identity(route, subscription_home.as_deref())?;
    let runtime_binary_identity = env
        .iter()
        .find(|(key, value)| key == CODEX_BINARY_IDENTITY_ENV && !value.trim().is_empty())
        .map(|(_, value)| value.clone())
        .map(Ok)
        .unwrap_or_else(|| binary_identity(bin))?;
    let identity = CapabilityIdentity::from_route(route, runtime_binary_identity, profile_identity);
    let engine = route.engine_id.clone();
    let model = route.model_id.clone();
    let bin = bin.to_string();
    let env = env.to_vec();
    registry
        .resolve(identity, move || async move {
            match engine.as_str() {
                "claude-code" => {
                    let mut command = build_command(&bin);
                    apply_inherited_agent_environment(&mut command);
                    for (key, value) in &env {
                        if !key.starts_with("HELM_") {
                            command.env(key, value);
                        }
                    }
                    command.arg("--help");
                    let help = run_bounded_probe_command(command, "Claude Code").await?;
                    let reasoning = claude_reasoning_capability(&model, &help);
                    Ok((
                        claude_capabilities_from_help(
                            &help,
                            &reasoning,
                            claude_model_only_contract_from_help(&help),
                        ),
                        "claude_help_contract".to_string(),
                    ))
                }
                "codex" => {
                    let auth_home = create_codex_auth_home(&env, &[])?;
                    let codex_home = auth_home
                        .as_ref()
                        .map(|home| home.path.clone())
                        .or(subscription_home);
                    let mut command = build_codex_command(&bin);
                    apply_inherited_agent_environment(&mut command);
                    for value in codex_provider_config_args(&env) {
                        command.arg("-c").arg(value);
                    }
                    apply_codex_native_search(&mut command, &env);
                    if let Some(path) = codex_home.as_ref() {
                        command.env("CODEX_HOME", path);
                    }
                    apply_codex_search_catalog(&mut command, &env, codex_home.as_deref())?;
                    for (key, value) in &env {
                        if !key.starts_with("HELM_") {
                            command.env(key, value);
                        }
                    }
                    command.current_dir(std::env::temp_dir()).kill_on_drop(true);
                    let process = spawn_codex_app_server(command).await?;
                    let provider_capabilities = tokio::time::timeout(
                        Duration::from_secs(10),
                        process.rpc.model_provider_capabilities(),
                    )
                    .await
                    .ok()
                    .and_then(Result::ok);
                    let model_list =
                        tokio::time::timeout(Duration::from_secs(10), process.rpc.model_list(None))
                            .await
                            .map_err(|_| {
                                "[capability_probe_timeout] Codex model/list 超时".to_string()
                            })??;
                    let encoded = serde_json::to_vec(&model_list)
                        .map_err(|error| format!("序列化 Codex capability 响应失败：{error}"))?;
                    let result = if encoded.len() > CAPABILITY_PROBE_OUTPUT_LIMIT {
                        Err("[capability_probe_output_limit] Codex model/list 超过上限".to_string())
                    } else {
                        let reasoning = codex_reasoning_capability(&model, &model_list);
                        let provider_search_capability =
                            provider_capabilities.as_ref().and_then(|value| {
                                value
                                    .get("webSearch")
                                    .or_else(|| value.get("web_search"))
                                    .and_then(serde_json::Value::as_bool)
                            });
                        log_runtime_line(
                            "codex-search-probe",
                            &format!(
                                "model={model} native_search_enabled={} provider_web_search={:?}",
                                codex_native_search_enabled(&env),
                                provider_search_capability
                            ),
                        );
                        Ok((
                            codex_capabilities_from_handshake(
                                &model,
                                &model_list,
                                &reasoning,
                                codex_native_search_enabled(&env),
                                provider_search_capability,
                            ),
                            "codex_initialize_model_list".to_string(),
                        ))
                    };
                    process.shutdown().await;
                    drop(auth_home);
                    result
                }
                _ => Err(format!("暂不支持的引擎：{engine}")),
            }
        })
        .await
}

/// 自定义服务商的模型按「服务商声明」放行按 Turn 指定模型（仅交互发送路径调用）。
/// 实证（.agent/evidence/ws/ws-handshake.mjs，2026-08-27）：codex app-server 的
/// model/list 对自定义 provider 也只返回 bundled OpenAI 目录（dummy key 即可复现），
/// 永远不会枚举自定义 /models 的模型——按上游该行为，用户显式绑定的自定义模型
/// 永远拿不到 codex_model_list 证据，整类配置被阻断。这里在「绑定的 primary_model
/// 就是本次路由模型」时补一条服务商/用户声明证据放行；搜索能力由 catalog/transport/
/// provider 能力动态裁决：目录命中或未显式禁用即允许原生 WebSearch，交给真实 Runtime 观察
/// （网关确实缺 WebSearch 工具时运行期再 fail-closed 为 [runtime_web_search_unavailable]，不冒充联网）。
/// 后台路径（titler/self_review/handoff）不经过本函数，红线阻断保持不变。
fn apply_provider_declared_model_override(
    model_override: &mut crate::capability_registry::CapabilityEvidence,
    engine: &str,
    env: &[(String, String)],
    binding_primary_model: &str,
    routed_model: &str,
) {
    use crate::capability_registry::CapabilitySupport;
    if engine != "codex"
        || model_override.support != CapabilitySupport::Unsupported
        || model_override.diagnostic != "codex_model_not_listed"
    {
        return;
    }
    let custom_provider = env
        .iter()
        .any(|(key, value)| key == "OPENAI_BASE_URL" && !value.trim().is_empty());
    if !custom_provider || routed_model != binding_primary_model {
        return;
    }
    *model_override = crate::capability_registry::CapabilityEvidence::new(
        CapabilitySupport::Supported,
        "helm_binding_declared",
        "custom_provider_binding_model",
    );
}

fn ensure_requested_runtime_capabilities(
    snapshot: &EngineCapabilitySnapshot,
    reasoning_effort: ReasoningEffort,
) -> Result<(), String> {
    use crate::capability_registry::CapabilitySupport;
    if snapshot.capabilities.model_override.support != CapabilitySupport::Supported {
        return Err(format!(
            "[capability_model_override_unavailable] 当前 Engine/Provider/Model 未证明可按 Turn 指定模型：{}",
            snapshot.capabilities.model_override.diagnostic
        ));
    }
    if !reasoning_effort.is_auto()
        && (snapshot.capabilities.reasoning_effort.support != CapabilitySupport::Supported
            || !snapshot
                .capabilities
                .reasoning_efforts
                .contains(&reasoning_effort))
    {
        return Err(format!(
            "[capability_reasoning_effort_unavailable] 当前模型未证明支持推理强度 {}",
            reasoning_effort.as_str()
        ));
    }
    Ok(())
}

async fn run_bounded_probe_command(
    mut command: tokio::process::Command,
    engine_label: &str,
) -> Result<String, String> {
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let mut child = command
        .spawn()
        .map_err(|error| format!("启动 {engine_label} capability 握手失败：{error}"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| format!("{engine_label} capability stdout 不可用"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| format!("{engine_label} capability stderr 不可用"))?;
    let stdout_task = tokio::spawn(async move {
        let mut bytes = Vec::new();
        stdout
            .take((CAPABILITY_PROBE_OUTPUT_LIMIT + 1) as u64)
            .read_to_end(&mut bytes)
            .await
            .map(|_| bytes)
    });
    let stderr_task = tokio::spawn(async move {
        let mut bytes = Vec::new();
        stderr
            .take((CAPABILITY_PROBE_OUTPUT_LIMIT + 1) as u64)
            .read_to_end(&mut bytes)
            .await
            .map(|_| bytes)
    });
    let status = match tokio::time::timeout(Duration::from_secs(10), child.wait()).await {
        Ok(result) => {
            result.map_err(|error| format!("等待 {engine_label} capability 握手失败：{error}"))?
        }
        Err(_) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            return Err(format!(
                "[capability_probe_timeout] {engine_label} capability 握手超时"
            ));
        }
    };
    let stdout = stdout_task
        .await
        .map_err(|error| format!("读取 {engine_label} capability stdout 失败：{error}"))?
        .map_err(|error| format!("读取 {engine_label} capability stdout 失败：{error}"))?;
    let stderr = stderr_task
        .await
        .map_err(|error| format!("读取 {engine_label} capability stderr 失败：{error}"))?
        .map_err(|error| format!("读取 {engine_label} capability stderr 失败：{error}"))?;
    let output = bounded_probe_output(&stdout, &stderr)?;
    if !status.success() {
        return Err(format!("{engine_label} capability 握手失败：{output}"));
    }
    Ok(output)
}

async fn prepare_codex_search_launch(
    engine: &str,
    bin: &str,
    model: &str,
    env: &mut Vec<(String, String)>,
) -> Result<(), String> {
    if engine != "codex" || !codex_native_search_enabled(env) {
        return Ok(());
    }
    env.retain(|(key, _)| {
        key != CODEX_SEARCH_CATALOG_JSON_ENV
            && key != CODEX_SEARCH_CATALOG_DIGEST_ENV
            && key != CODEX_SEARCH_TRANSPORT_ENV
            && key != CODEX_BINARY_IDENTITY_ENV
    });
    let catalog_binary_identity = binary_identity(bin)?;
    env.push((
        CODEX_BINARY_IDENTITY_ENV.to_string(),
        catalog_binary_identity.clone(),
    ));
    // 官方 subscription Provider 自己拥有 hosted/standalone 路由；兼容目录只用于
    // Helm custom Responses Provider，避免改变官方账号目录的模型合同。
    if !env
        .iter()
        .any(|(key, value)| key == "OPENAI_BASE_URL" && !value.trim().is_empty())
    {
        env.push((
            CODEX_SEARCH_TRANSPORT_ENV.to_string(),
            "runtime_managed".to_string(),
        ));
        return Ok(());
    }

    let cached_catalog = codex_bundled_catalog_cache()
        .lock()
        .map_err(|_| "Codex bundled model catalog 缓存锁中毒".to_string())?
        .get(&catalog_binary_identity)
        .cloned();
    let stdout = if let Some(catalog) = cached_catalog {
        catalog
    } else {
        let mut command = build_codex_command(bin);
        apply_inherited_agent_environment(&mut command);
        command
            .arg("debug")
            .arg("models")
            .arg("--bundled")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        let mut child = command
            .spawn()
            .map_err(|error| format!("启动 Codex bundled model catalog 读取失败：{error}"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "Codex bundled model catalog stdout 不可用".to_string())?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| "Codex bundled model catalog stderr 不可用".to_string())?;
        let stdout_task = tokio::spawn(async move {
            let mut bytes = Vec::new();
            stdout
                .take((CODEX_MODEL_CATALOG_OUTPUT_LIMIT + 1) as u64)
                .read_to_end(&mut bytes)
                .await
                .map(|_| bytes)
        });
        let stderr_task = tokio::spawn(async move {
            let mut bytes = Vec::new();
            stderr
                .take((CAPABILITY_PROBE_OUTPUT_LIMIT + 1) as u64)
                .read_to_end(&mut bytes)
                .await
                .map(|_| bytes)
        });
        let status = match tokio::time::timeout(Duration::from_secs(15), child.wait()).await {
            Ok(result) => {
                result.map_err(|error| format!("等待 Codex bundled model catalog 失败：{error}"))?
            }
            Err(_) => {
                let _ = child.kill().await;
                let _ = child.wait().await;
                return Err(
                    "[codex_search_catalog_timeout] Codex bundled model catalog 读取超时"
                        .to_string(),
                );
            }
        };
        let stdout = stdout_task
            .await
            .map_err(|error| format!("读取 Codex bundled model catalog stdout 失败：{error}"))?
            .map_err(|error| format!("读取 Codex bundled model catalog stdout 失败：{error}"))?;
        let stderr = stderr_task
            .await
            .map_err(|error| format!("读取 Codex bundled model catalog stderr 失败：{error}"))?
            .map_err(|error| format!("读取 Codex bundled model catalog stderr 失败：{error}"))?;
        if stdout.len() > CODEX_MODEL_CATALOG_OUTPUT_LIMIT
            || stderr.len() > CAPABILITY_PROBE_OUTPUT_LIMIT
        {
            return Err(
                "[codex_search_catalog_output_limit] Codex bundled model catalog 超过读取上限"
                    .to_string(),
            );
        }
        if !status.success() {
            return Err(format!(
                "[codex_search_catalog_failed] Codex bundled model catalog 读取失败：{}",
                String::from_utf8_lossy(&stderr)
            ));
        }
        codex_bundled_catalog_cache()
            .lock()
            .map_err(|_| "Codex bundled model catalog 缓存锁中毒".to_string())?
            .insert(catalog_binary_identity, stdout.clone());
        stdout
    };

    let catalog_plan = codex_search_catalog_plan(&stdout, model)?;
    log_runtime_line(
        "codex-search-catalog-plan",
        &format!("model={model} plan={catalog_plan:?}"),
    );
    match catalog_plan {
        CodexSearchCatalogPlan::Unavailable => env.push((
            CODEX_SEARCH_TRANSPORT_ENV.to_string(),
            "unavailable".to_string(),
        )),
        // 未知模型：不写入 unavailable 传输标记 → 原生搜索保持开启（web_search="live" + --search），
        // 让真实 Runtime 自行决定是否提供 WebSearch 工具（网关支持即可联网搜索）。
        CodexSearchCatalogPlan::Unknown => {}
        CodexSearchCatalogPlan::HostedResponses => env.push((
            CODEX_SEARCH_TRANSPORT_ENV.to_string(),
            "hosted_responses".to_string(),
        )),
        CodexSearchCatalogPlan::HostedResponsesCompatibility(encoded) => {
            let digest = format!("sha256:{}", crate::util::sha256_hex(&encoded));
            let encoded = String::from_utf8(encoded)
                .map_err(|error| format!("Codex 搜索兼容目录不是 UTF-8：{error}"))?;
            env.push((
                CODEX_SEARCH_TRANSPORT_ENV.to_string(),
                "hosted_responses".to_string(),
            ));
            env.push((CODEX_SEARCH_CATALOG_JSON_ENV.to_string(), encoded));
            env.push((CODEX_SEARCH_CATALOG_DIGEST_ENV.to_string(), digest));
        }
    }
    Ok(())
}

fn ensure_discovered_binding_model(
    models: &[ModelConfig],
    model_id: &str,
    label: &str,
) -> Result<(), String> {
    if models.iter().any(|model| model.id == model_id) {
        return Ok(());
    }
    Err(format!(
        "[model_unavailable] 当前 ChatGPT 订阅账号不可用{label} {model_id}，请在「服务商 → 模型目录」同步账号模型后重新绑定"
    ))
}

fn codex_subscription_models_from_response(
    provider_id: &str,
    response: &serde_json::Value,
) -> Result<Vec<(bool, ModelConfig)>, String> {
    let rows = response
        .get("data")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "Codex model/list 响应缺少 data".to_string())?;
    let mut models = Vec::new();
    for row in rows {
        if row
            .get("hidden")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
        {
            continue;
        }
        let id = row
            .get("model")
            .or_else(|| row.get("id"))
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "Codex model/list 返回了缺少模型 ID 的条目".to_string())?;
        let is_default = row
            .get("isDefault")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        let display_name = row
            .get("displayName")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(id);
        let display_name = if is_default {
            format!("{display_name}（账号默认）")
        } else {
            display_name.to_string()
        };
        let capabilities = row
            .get("supportedReasoningEfforts")
            .and_then(serde_json::Value::as_array)
            .map(|efforts| {
                efforts
                    .iter()
                    .filter_map(|effort| {
                        effort
                            .get("reasoningEffort")
                            .and_then(serde_json::Value::as_str)
                    })
                    .map(|effort| format!("reasoning:{effort}"))
                    .collect::<Vec<_>>()
            })
            .filter(|capabilities| !capabilities.is_empty());
        models.push((
            is_default,
            ModelConfig {
                id: id.to_string(),
                provider_id: provider_id.to_string(),
                display_name,
                input_price_per_mtok: 0.0,
                output_price_per_mtok: 0.0,
                cached_input_price_per_mtok: None,
                price_source: Some(PriceSource::Subscription),
                enabled: true,
                context_window: row.get("contextWindow").and_then(serde_json::Value::as_u64),
                capabilities,
            },
        ));
    }
    Ok(models)
}

async fn discover_codex_subscription_models(
    config: &AppConfig,
    provider_id: &str,
    codex_home: &std::path::Path,
) -> Result<Vec<ModelConfig>, String> {
    let bin = config
        .engine_bin("codex")
        .filter(|bin| !bin.trim().is_empty())
        .unwrap_or("codex");
    let mut command = build_codex_command(bin);
    apply_inherited_agent_environment(&mut command);
    command.env("CODEX_HOME", codex_home);
    for value in codex_provider_config_args(&[]) {
        command.arg("-c").arg(value);
    }
    command.current_dir(std::env::temp_dir()).kill_on_drop(true);
    let process = tokio::time::timeout(Duration::from_secs(15), spawn_codex_app_server(command))
        .await
        .map_err(|_| "读取 Codex 账号模型超时".to_string())??;
    let result = async {
        let mut cursor: Option<String> = None;
        let mut discovered = Vec::new();
        let mut seen = HashSet::new();
        for _ in 0..10 {
            let response = tokio::time::timeout(
                Duration::from_secs(10),
                process.rpc.visible_model_list(cursor.as_deref()),
            )
            .await
            .map_err(|_| "读取 Codex 账号模型超时".to_string())??;
            for (is_default, model) in
                codex_subscription_models_from_response(provider_id, &response)?
            {
                if seen.insert(model.id.clone()) {
                    discovered.push((is_default, model));
                }
            }
            cursor = response
                .get("nextCursor")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
                .filter(|value| !value.is_empty());
            if cursor.is_none() {
                break;
            }
        }
        if cursor.is_some() {
            return Err("Codex 账号模型分页超过安全上限".to_string());
        }
        discovered.sort_by_key(|(is_default, _)| !*is_default);
        let models = discovered
            .into_iter()
            .map(|(_, model)| model)
            .collect::<Vec<_>>();
        if models.is_empty() {
            return Err("Codex 当前登录账号没有返回可用模型，请重新登录后重试".to_string());
        }
        Ok(models)
    }
    .await;
    process.shutdown().await;
    result
}

impl SessionStore {
    fn next_handle(&self) -> String {
        let counter = self.counter.fetch_add(1, Ordering::Relaxed);
        let now_nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        format!("s-{now_nanos}-{counter}")
    }

    fn bind_history_session(
        &self,
        handle_id: &str,
        history_session_id: &str,
    ) -> Result<(), String> {
        self.history_session_ids
            .lock()
            .map_err(|_| "会话历史映射锁中毒".to_string())?
            .insert(handle_id.to_string(), history_session_id.to_string());
        Ok(())
    }

    fn history_session_id_for_handle(&self, handle_id: &str) -> Result<String, String> {
        Ok(self
            .history_session_ids
            .lock()
            .map_err(|_| "会话历史映射锁中毒".to_string())?
            .get(handle_id)
            .cloned()
            .unwrap_or_else(|| handle_id.to_string()))
    }

    fn actor_for_history_session(
        &self,
        history_session_id: &str,
    ) -> Result<Option<SessionActorHandle>, String> {
        let handles = self
            .history_session_ids
            .lock()
            .map_err(|_| "会话历史映射锁中毒".to_string())?;
        let sessions = self
            .sessions
            .lock()
            .map_err(|_| "会话表锁中毒".to_string())?;
        Ok(handles.iter().find_map(|(handle, history_id)| {
            (history_id == history_session_id)
                .then(|| Some(sessions.get(handle)?.clone()))
                .flatten()
        }))
    }
}

/// 创建会话运行时：返回内部句柄 id；真实 `claude` 进程在 send / approve 时启动。
#[tauri::command]
pub fn append_runtime_log(app: AppHandle, line: String) {
    crate::adapter::log_runtime_event(&app, "frontend", &line);
}

#[tauri::command]
pub async fn create_session(
    app: AppHandle,
    store: State<'_, SessionStore>,
    config_store: State<'_, ProviderStore<KeyringSecretStore>>,
    history_store: State<'_, SessionHistoryStore>,
    profiles: State<'_, SubscriptionProfileStore>,
    capability_registry: State<'_, EngineCapabilityRegistry>,
    runtime_registry: State<'_, RuntimeRegistry>,
    engine: String,
    model: String,
    cwd: String,
    reasoning_effort: Option<String>,
    mode: Option<String>,
    permission_profile: Option<String>,
    full_access_confirmed: Option<bool>,
) -> Result<String, String> {
    let _initial_mode = TurnMode::parse(mode.as_deref());
    // 单独保留一个 AppHandle 克隆用于事件广播；下方启动 runtime 会按值移动 app。
    let app_for_events = app.clone();
    // 预算护栏：检查是否超预算
    let budget = history_store.get_budget()?;
    ensure_budget_allows_turn(&budget)?;

    let config = config_store.load()?;
    let app_settings = load_app_settings_from_store(&history_store)?;
    sync_history_model_prices(&history_store, &config_store, &config);
    let engine = if engine.is_empty() {
        config.default_engine.clone()
    } else {
        engine
    };
    let binding = config
        .bindings
        .iter()
        .find(|binding| binding.engine_id == engine)
        .cloned()
        .ok_or_else(|| format!("引擎还没有配置生效绑定：{engine}"))?;
    let model = if model.is_empty() {
        binding.primary_model.clone()
    } else {
        model
    };
    let reasoning_effort = ReasoningEffort::parse(reasoning_effort.as_deref())?
        .or(binding.reasoning_effort)
        .unwrap_or_default();
    if engine == "claude-code" && !reasoning_effort.is_claude_level() {
        return Err("Claude Code 不支持该推理强度".to_string());
    }
    // 用量归属（P3-6）：记录本会话实际使用的服务商
    let provider_id = binding.provider_id.clone();
    ensure_pricing_allows_turn(
        &budget,
        &app_settings,
        &config_store,
        &config,
        &provider_id,
        &model,
    )?;
    let launch_binding = BindingConfig {
        primary_model: model.clone(),
        ..binding
    };
    ensure_binding_runtime_ready(&profiles, &config, &launch_binding).await?;
    let mut env = config_store.launch_env_for_config(&config, &launch_binding)?;
    let subscription_home = subscription_profile_for_binding(&profiles, &config, &launch_binding)?;
    if subscription_home.is_some() {
        profiles.append_launch_env(&mut env, &engine)?;
    }
    env.extend(agent_environment_from_settings(&app_settings));
    let bin = config
        .engine_bin(&engine)
        .filter(|bin| !bin.is_empty())
        .unwrap_or(if engine == "codex" { "codex" } else { "claude" })
        .to_string();
    prepare_codex_search_launch(&engine, &bin, &model, &mut env).await?;
    let pricing_profile = config
        .models
        .iter()
        .find(|candidate| candidate.provider_id == provider_id && candidate.id == model)
        .map(|candidate| config_store.model_pricing_profile(&config, candidate))
        .transpose()?
        .flatten();
    let runtime_route = build_runtime_route(
        &config,
        &launch_binding,
        &model,
        &bin,
        &env,
        reasoning_effort,
        pricing_profile,
    )?;
    let mut capability_snapshot = resolve_engine_capability_snapshot(
        &capability_registry,
        &runtime_route,
        &bin,
        &env,
        subscription_home.clone(),
    )
    .await?;
    apply_provider_declared_model_override(
        &mut capability_snapshot.capabilities.model_override,
        &engine,
        &env,
        launch_binding.primary_model.as_str(),
        &model,
    );
    ensure_requested_runtime_capabilities(&capability_snapshot, reasoning_effort)?;
    let handle = store.next_handle();
    let permission_profile = crate::adapter::PermissionProfile::parse(
        permission_profile.as_deref().unwrap_or("standard"),
    )?;
    if permission_profile == crate::adapter::PermissionProfile::FullAccess {
        require_full_access_confirmed(full_access_confirmed)?;
    }
    // Runtime 初始化会读取该 Session 的持久 Turn epoch。必须先落本地 Session，
    // 否则首条消息创建 Runtime 时会在 latest_turn_epoch 中得到 QueryReturnedNoRows。
    let previous_active_session_id = history_store
        .active_session()?
        .map(|detail| detail.summary.id);
    let (_, created_folder_id) = history_store.create_session_for_cwd_tracked(
        NewSessionRecord {
            id: handle.clone(),
            engine: engine_id_from_str(&engine)?,
            model: model.clone(),
            cwd: cwd.clone(),
            created_at: unix_timestamp_seconds()?,
        },
        None,
    )?;
    if let Err(error) = history_store.set_safe_permission_profile(
        &handle,
        match permission_profile {
            crate::adapter::PermissionProfile::Auto => "auto",
            crate::adapter::PermissionProfile::Standard
            | crate::adapter::PermissionProfile::FullAccess => "standard",
        },
    ) {
        return Err(rollback_failed_session_creation(
            &history_store,
            &handle,
            previous_active_session_id.as_deref(),
            created_folder_id.as_deref(),
            error,
        ));
    }
    if let Err(error) = history_store.set_session_provider(&handle, &provider_id) {
        return Err(rollback_failed_session_creation(
            &history_store,
            &handle,
            previous_active_session_id.as_deref(),
            created_folder_id.as_deref(),
            error,
        ));
    }
    if let Err(error) =
        history_store.set_session_turn_preference(&handle, &model, Some(reasoning_effort.as_str()))
    {
        return Err(rollback_failed_session_creation(
            &history_store,
            &handle,
            previous_active_session_id.as_deref(),
            created_folder_id.as_deref(),
            error,
        ));
    }

    let session_result = match engine.as_str() {
        "claude-code" => {
            start_claude_with_resume_and_reasoning(
                app,
                handle.clone(),
                bin,
                model.clone(),
                cwd.clone(),
                env,
                reasoning_effort,
                None,
                Vec::new(),
                capability_snapshot.clone(),
                false,
            )
            .await
        }
        "codex" => {
            // API Runtime Profile 由 start_codex_with_reasoning 从 Helm-owned 持久目录解析；
            // Provider Key 只通过进程环境传递，不再创建 Session auth.json。
            start_codex_with_reasoning(
                app,
                handle.clone(),
                bin,
                model.clone(),
                cwd.clone(),
                env,
                vec![],
                None,
                None,
                None,
                None,
                subscription_home,
                capability_snapshot.clone(),
                reasoning_effort,
            )
        }
        _ => Err(format!("暂不支持的引擎：{engine}")),
    };
    let session = match session_result {
        Ok(session) => session,
        Err(error) => {
            return Err(rollback_failed_session_creation(
                &history_store,
                &handle,
                previous_active_session_id.as_deref(),
                created_folder_id.as_deref(),
                error,
            ));
        }
    };
    if let Err(error) = session.set_permission_profile(permission_profile).await {
        return Err(rollback_failed_session_creation(
            &history_store,
            &handle,
            previous_active_session_id.as_deref(),
            created_folder_id.as_deref(),
            error,
        ));
    }
    let owner = RuntimeOwnerRef::session(handle.clone());
    if let Err(error) = runtime_registry
        .register_session(
            owner.clone(),
            session,
            &runtime_route,
            &capability_snapshot,
            &cwd,
        )
        .await
    {
        return Err(rollback_failed_session_creation(
            &history_store,
            &handle,
            previous_active_session_id.as_deref(),
            created_folder_id.as_deref(),
            error,
        ));
    }
    let actor = SessionActorHandle::start(owner, runtime_registry.inner().clone());
    store
        .sessions
        .lock()
        .map_err(|_| "会话表锁中毒".to_string())?
        .insert(handle.clone(), actor);
    store.bind_history_session(&handle, &handle)?;
    // 新会话已落库并注册 Runtime：广播事件让常驻的 Rail 侧栏即时刷新，
    // 无需整页刷新才看到新建任务（修复「打开对话后左侧菜单不实时更新」）。
    let _ = app_for_events.emit("helm-sessions-changed", &handle);
    Ok(handle)
}

fn rollback_failed_session_creation(
    history_store: &SessionHistoryStore,
    session_id: &str,
    previous_active_session_id: Option<&str>,
    created_folder_id: Option<&str>,
    cause: String,
) -> String {
    let mut cleanup_errors = Vec::new();
    if let Err(error) = history_store.delete_session(session_id) {
        cleanup_errors.push(format!("删除未启动 Session 失败：{error}"));
    }
    if let Some(folder_id) = created_folder_id {
        if let Err(error) = history_store.delete_empty_project_folder(folder_id) {
            cleanup_errors.push(format!("删除空项目 Folder 失败：{error}"));
        }
    }
    if let Some(previous_id) = previous_active_session_id {
        if let Err(error) = history_store.set_active_session(previous_id) {
            cleanup_errors.push(format!("恢复原 active Session 失败：{error}"));
        }
    }
    if cleanup_errors.is_empty() {
        cause
    } else {
        format!("{cause}；{}", cleanup_errors.join("；"))
    }
}

#[tauri::command]
pub fn list_sessions(
    history_store: State<'_, SessionHistoryStore>,
) -> Result<Vec<SessionSummary>, String> {
    history_store.list_sessions()
}

#[tauri::command]
pub fn list_folders(
    history_store: State<'_, SessionHistoryStore>,
) -> Result<Vec<SessionFolder>, String> {
    history_store.list_folders()
}

#[tauri::command]
pub fn set_folder_collapsed(
    history_store: State<'_, SessionHistoryStore>,
    folder_id: String,
    collapsed: bool,
) -> Result<(), String> {
    history_store.set_folder_collapsed(&folder_id, collapsed)
}

#[tauri::command]
pub fn get_active_session(
    history_store: State<'_, SessionHistoryStore>,
) -> Result<Option<SessionDetail>, String> {
    history_store.active_session()
}

#[tauri::command]
pub fn get_session_history(
    history_store: State<'_, SessionHistoryStore>,
    session_id: String,
) -> Result<SessionDetail, String> {
    history_store.get_session(&session_id)
}

#[tauri::command]
pub fn list_session_contexts(
    history_store: State<'_, SessionHistoryStore>,
    session_id: String,
) -> Result<Vec<crate::sessions::SessionContextRecord>, String> {
    history_store.list_session_contexts(&session_id)
}

#[tauri::command]
pub fn add_session_context(
    history_store: State<'_, SessionHistoryStore>,
    session_id: String,
    source_path: String,
) -> Result<crate::sessions::SessionContextRecord, String> {
    history_store.add_session_context(&session_id, &source_path)
}

#[tauri::command]
pub fn remove_session_context(
    history_store: State<'_, SessionHistoryStore>,
    session_id: String,
    context_id: String,
) -> Result<(), String> {
    history_store.remove_session_context(&session_id, &context_id)
}

/// 同引擎无损分支结果（十次反馈）：lossless = 已即时创建分支会话（首轮流复制完整历史）；
/// summary = 条件不满足（跨引擎/codex/CLI 无 --fork-session/源无 CLI 会话 id），
/// 自动回退既有摘要派生流程，前端按返回的 operation 走原有轮询。
/// 注意：`rename_all` 只作用变体名（Lossless→lossless），变体字段需要
/// `rename_all_fields`——此前 session_id 以 snake_case 下发、前端读 sessionId
/// 得 undefined，「分叉成功但自动跳转静默失效」（2026-09-04 埋点实证）。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase", rename_all_fields = "camelCase", tag = "mode")]
pub enum BranchForkOutcome {
    Lossless {
        session_id: String,
    },
    Summary {
        operation: crate::operations::BackgroundOperation,
    },
}

/// 分叉任务入口（十次反馈升级）：同引擎 + Claude CLI 支持 `--fork-session` 时走无损
/// 分支——立即创建新会话并登记来源链接，零等待零 token；首次发送时以
/// `--resume <源> --fork-session` 复制完整历史（open 流程消费）。其余情况自动
/// 回退摘要派生，保持既有语义与 UI 轮询不变。
#[tauri::command]
pub async fn start_session_branch(
    app: AppHandle,
    config_store: State<'_, ProviderStore<KeyringSecretStore>>,
    history_store: State<'_, SessionHistoryStore>,
    profiles: State<'_, SubscriptionProfileStore>,
    capability_registry: State<'_, EngineCapabilityRegistry>,
    source_session_id: String,
    source_turn_id: Option<String>,
) -> Result<BranchForkOutcome, String> {
    let fallback_summary = |engine: String| {
        let app = app.clone();
        let source_session_id = source_session_id.clone();
        let source_turn_id = source_turn_id.clone();
        async move {
            let operation = crate::handoff::start_session_fork(
                &app,
                &source_session_id,
                &engine,
                source_turn_id.as_deref(),
            )
            .await?;
            Ok::<BranchForkOutcome, String>(BranchForkOutcome::Summary { operation })
        }
    };
    let detail = history_store.get_session(&source_session_id)?;
    let engine = engine_id_to_string(detail.summary.engine);
    if engine == "codex" {
        // Codex 原生无损分支：首轮由 codex 适配器调 thread/fork 按源线程派生新线程，
        // 此处仅登记分支会话与来源行；源线程 id 取自会话已落库的 cli_session_id。
        let Some(source_thread) = detail.summary.cli_session_id.clone() else {
            return Err(
                "[fork_codex_no_thread] Codex 会话尚未生成原生线程，无法无损分叉；请先运行一轮后再分叉"
                    .to_string(),
            );
        };
        let branch_id = format!("session-{:032x}", rand::random::<u128>());
        history_store.create_session(crate::sessions::NewSessionRecord {
            id: branch_id.clone(),
            engine: detail.summary.engine.clone(),
            model: detail.summary.model.clone(),
            cwd: detail.summary.cwd.clone(),
            // session.created_at/updated_at 约定秒（sessions.rs「维持秒」注释）；
            // 此前误用 now_millis() 导致分叉会话永远置顶且显示「刚刚」。
            created_at: crate::util::now_seconds(),
        })?;
        let branch_title = format!(
            "{}（分叉）",
            if detail.summary.title.trim().is_empty() {
                "未命名会话"
            } else {
                detail.summary.title.trim()
            }
        );
        history_store.rename_session(&branch_id, &branch_title)?;
        // 切点分叉：对话内从某条回答点分叉时，前端带被点回答所属的 Helm turn id。
        // 解析成 Codex 原生轮 id 后登记进来源行——首轮 thread/fork 以此截断到该轮
        // （含），历史复制同样截断；解析不到（老会话未落库 native id）则整段分叉兜底。
        let boundary_native = match source_turn_id.as_deref() {
            Some(turn_id) => history_store
                .native_turn_id_for_session_turn(&source_session_id, turn_id)
                .ok()
                .flatten(),
            None => None,
        };
        history_store.record_session_native_branch(
            &branch_id,
            &source_session_id,
            &source_thread,
            boundary_native.as_deref(),
            source_turn_id
                .as_deref()
                .filter(|_| boundary_native.is_some()),
        )?;
        // 历史可见性：复制源会话消息，分叉出的新会话立即可回看分叉前的对话。
        // 此前历史只存在于 CLI 侧 fork 出的线程里，界面是空的，用户会以为分叉失败。
        // 只复制消息，不复制 turn/用量/检查点（分支自首轮起才记账，避免重复计费用）。
        // 切点存在时按切点截断——只带「这段回答及之前」，该回答之后的轮次不分叉进来。
        let clone_target = source_turn_id
            .as_deref()
            .filter(|_| boundary_native.is_some());
        let copied = history_store
            .clone_messages_into_session_upto(&source_session_id, &branch_id, clone_target)
            .unwrap_or(0);
        eprintln!(
            "[helm] [native_branch_history] Codex 分支 {} 已复制来源 {} 的 {} 条历史消息",
            branch_id, source_session_id, copied
        );
        // 分支会话已落库：广播侧栏刷新（与 new_session 同源修复「左栏不实时更新」），
        // 前端随即跳转分叉会话时无需整页刷新。
        let _ = app.emit("helm-sessions-changed", &branch_id);
        return Ok(BranchForkOutcome::Lossless {
            session_id: branch_id,
        });
    }
    if engine != "claude-code" {
        return fallback_summary(engine).await;
    }
    // 切点分叉目前只有 Codex 支持（thread/fork 带 lastTurnId）。Claude 的
    // --fork-session 只能整段复制，无法截断到某一轮——带切点的请求走摘要派生，
    // 保证分叉内容与「这段回答及之前」一致，宁可多一次摘要也不给错误的无损分支。
    if source_turn_id.is_some() {
        return fallback_summary(engine).await;
    }
    let Some(source_cli) = detail.summary.cli_session_id.clone() else {
        return fallback_summary(engine).await;
    };
    // 解析当前绑定并做能力证明：--fork-session 必须被本机 CLI help 明示（红线：原生
    // resume/fork 需 capability 证明），否则回退摘要。
    let config = config_store.load()?;
    let binding = config
        .bindings
        .iter()
        .find(|binding| binding.engine_id == engine)
        .cloned()
        .ok_or_else(|| format!("引擎还没有配置生效绑定：{engine}"))?;
    let model = resolve_binding_model(&config, &binding, &binding.primary_model);
    let launch_binding = BindingConfig {
        primary_model: model.clone(),
        ..binding.clone()
    };
    ensure_binding_runtime_ready(&profiles, &config, &launch_binding).await?;
    let mut env = config_store.launch_env_for_config(&config, &launch_binding)?;
    let subscription_home = subscription_profile_for_binding(&profiles, &config, &launch_binding)?;
    if subscription_home.is_some() {
        profiles.append_launch_env(&mut env, &engine)?;
    }
    env.extend(agent_environment_from_settings(
        &load_app_settings_from_store(&history_store)?,
    ));
    let bin = config
        .engine_bin(&engine)
        .filter(|bin| !bin.is_empty())
        .unwrap_or("claude")
        .to_string();
    let pricing_profile = config
        .models
        .iter()
        .find(|candidate| candidate.provider_id == binding.provider_id && candidate.id == model)
        .map(|candidate| config_store.model_pricing_profile(&config, candidate))
        .transpose()?
        .flatten();
    let route = build_runtime_route(
        &config,
        &launch_binding,
        &model,
        &bin,
        &env,
        binding.reasoning_effort.unwrap_or_default(),
        pricing_profile,
    )?;
    let capability_snapshot = resolve_engine_capability_snapshot(
        &capability_registry,
        &route,
        &bin,
        &env,
        subscription_home,
    )
    .await?;
    if capability_snapshot.capabilities.native_branch.support
        != crate::capability_registry::CapabilitySupport::Supported
    {
        return fallback_summary(engine).await;
    }
    // 即时创建分支会话：复制源会话的引擎/模型/cwd，标题标注来源；首轮发送时才发生
    // 真实 CLI 调用（同业体验：分叉即时可见，token 成本延后到用户开口）。
    let branch_id = format!("session-{:032x}", rand::random::<u128>());
    history_store.create_session(crate::sessions::NewSessionRecord {
        id: branch_id.clone(),
        engine: detail.summary.engine.clone(),
        model: model.clone(),
        cwd: detail.summary.cwd.clone(),
        // 同上：秒约定，勿改回 now_millis()。
        created_at: crate::util::now_seconds(),
    })?;
    let branch_title = format!(
        "{}（分叉）",
        if detail.summary.title.trim().is_empty() {
            "未命名会话"
        } else {
            detail.summary.title.trim()
        }
    );
    history_store.rename_session(&branch_id, &branch_title)?;
    history_store.record_session_native_branch(
        &branch_id,
        &source_session_id,
        &source_cli,
        None,
        None,
    )?;
    // 同 Codex 分支：复制历史消息让新会话立即可回看分叉前的对话（只复制消息，
    // 不复制 turn/用量/检查点；首轮仍由 --fork-session 在 CLI 侧复制完整上下文）。
    let copied = history_store
        .clone_messages_into_session(&source_session_id, &branch_id)
        .unwrap_or(0);
    eprintln!(
        "[helm] [native_branch_history] Claude 分支 {} 已复制来源 {} 的 {} 条历史消息",
        branch_id, source_session_id, copied
    );
    // 与 Codex 分支一致：广播侧栏即时刷新，前端随后自动跳转分叉会话。
    let _ = app.emit("helm-sessions-changed", &branch_id);
    Ok(BranchForkOutcome::Lossless {
        session_id: branch_id,
    })
}

#[tauri::command]
pub async fn resume_session(
    app: AppHandle,
    store: State<'_, SessionStore>,
    config_store: State<'_, ProviderStore<KeyringSecretStore>>,
    history_store: State<'_, SessionHistoryStore>,
    profiles: State<'_, SubscriptionProfileStore>,
    capability_registry: State<'_, EngineCapabilityRegistry>,
    runtime_registry: State<'_, RuntimeRegistry>,
    session_id: String,
    mode: Option<String>,
) -> Result<String, String> {
    let _initial_mode = TurnMode::parse(mode.as_deref());
    let detail = history_store.get_session(&session_id)?;
    if let Some(actor) = store.actor_for_history_session(&session_id)? {
        let handle = store.next_handle();
        store
            .sessions
            .lock()
            .map_err(|_| "会话表锁中毒".to_string())?
            .insert(handle.clone(), actor);
        store.bind_history_session(&handle, &session_id)?;
        history_store.set_active_session(&session_id)?;
        return Ok(handle);
    }
    let config = config_store.load()?;
    let app_settings = load_app_settings_from_store(&history_store)?;
    sync_history_model_prices(&history_store, &config_store, &config);
    let engine = engine_id_to_string(detail.summary.engine);
    // 诊断（2026-09-04）：分叉会话打不开时前端只能拿到最终 Err 文案，靠它反推不出是哪道
    // 闸门拦的。逐关卡落 helm-runtime.log，末条日志即失败点。
    log_runtime_line(
        "resume",
        &format!(
            "enter session={} engine={} cli={:?}",
            session_id, engine, detail.summary.cli_session_id
        ),
    );
    let binding = config
        .bindings
        .iter()
        .find(|binding| binding.engine_id == engine)
        .cloned()
        .ok_or_else(|| format!("引擎还没有配置生效绑定：{engine}"))?;
    let provider_id = binding.provider_id.clone();
    let requested_model =
        requested_model_for_binding(None, detail.summary.preferred_model.as_deref(), &binding);
    let model = resolve_binding_model(&config, &binding, &requested_model);
    ensure_pricing_allows_turn(
        &history_store.get_budget()?,
        &app_settings,
        &config_store,
        &config,
        &provider_id,
        &model,
    )?;
    let requested_effort = detail
        .summary
        .preferred_reasoning_effort
        .as_deref()
        .map(|value| ReasoningEffort::parse(Some(value)))
        .transpose()?
        .flatten()
        .or(binding.reasoning_effort)
        .unwrap_or_default();
    let launch_binding = BindingConfig {
        primary_model: model.clone(),
        ..binding
    };
    ensure_binding_runtime_ready(&profiles, &config, &launch_binding).await?;
    let mut env = config_store.launch_env_for_config(&config, &launch_binding)?;
    let subscription_home = subscription_profile_for_binding(&profiles, &config, &launch_binding)?;
    if subscription_home.is_some() {
        profiles.append_launch_env(&mut env, &engine)?;
    }
    env.extend(agent_environment_from_settings(&app_settings));
    let bin = config
        .engine_bin(&engine)
        .filter(|bin| !bin.is_empty())
        .unwrap_or(if engine == "codex" { "codex" } else { "claude" })
        .to_string();
    prepare_codex_search_launch(&engine, &bin, &model, &mut env).await?;
    let pricing_profile = config
        .models
        .iter()
        .find(|candidate| candidate.provider_id == provider_id && candidate.id == model)
        .map(|candidate| config_store.model_pricing_profile(&config, candidate))
        .transpose()?
        .flatten();
    let runtime_route = build_runtime_route(
        &config,
        &launch_binding,
        &model,
        &bin,
        &env,
        requested_effort,
        pricing_profile,
    )?;
    let mut capability_snapshot = resolve_engine_capability_snapshot(
        &capability_registry,
        &runtime_route,
        &bin,
        &env,
        subscription_home.clone(),
    )
    .await?;
    apply_provider_declared_model_override(
        &mut capability_snapshot.capabilities.model_override,
        &engine,
        &env,
        launch_binding.primary_model.as_str(),
        &model,
    );
    let reasoning_effort = resolve_routed_effort(&capability_snapshot, requested_effort);
    ensure_requested_runtime_capabilities(&capability_snapshot, reasoning_effort)?;
    let context_messages = ledger_rebuild_messages(&history_store, &session_id)?;
    ensure_ledger_fits_context_window(
        &context_messages,
        "",
        capability_snapshot.capabilities.context_window,
    )?;
    // 同引擎无损分支（十次反馈）：分支会话首轮以「源 CLI 会话 + --fork-session」
    // 复制完整历史；来源链接由 start_session_branch 落库，此处只负责消费。
    let native_branch_row = history_store.load_session_native_branch(&session_id)?;
    log_runtime_line(
        "resume",
        &format!(
            "native_branch session={} row={}",
            session_id,
            match &native_branch_row {
                Some((source, native, boundary, helm_turn)) => format!(
                    "source={} native={} boundary={:?} helm_turn={:?}",
                    source, native, boundary, helm_turn
                ),
                None => "none".to_string(),
            }
        ),
    );
    let native_candidate = match &native_branch_row {
        Some((source_sid, source_cli, _, _)) => {
            eprintln!(
                "[helm] [native_branch_first_turn] Session {} 将以 --resume {} --fork-session 复制来源 {} 的完整历史",
                session_id, source_cli, source_sid
            );
            Some(source_cli.clone())
        }
        None => detail.summary.cli_session_id.clone(),
    };
    let same_launch_profile = match native_candidate.as_deref() {
        Some(native_id) => {
            // 分支的 native ref 登记在源会话名下，按源会话查询归属。
            let owner_sid = native_branch_row
                .as_ref()
                .map(|(source_sid, _, _, _)| source_sid.as_str())
                .unwrap_or(&detail.summary.id);
            history_store.native_resume_profile_matches(
                owner_sid,
                native_id,
                &runtime_route.provider_launch_profile_ref,
                &runtime_route.provider_launch_profile_digest,
            )?
        }
        None => false,
    };
    log_runtime_line(
        "resume",
        &format!(
            "same_launch_profile session={} value={} route_ref={} route_digest={}",
            session_id,
            same_launch_profile,
            runtime_route.provider_launch_profile_ref,
            runtime_route.provider_launch_profile_digest
        ),
    );
    // get_session 已一次性取回全部未回溯消息；serialize_history_prompt 不做 token/条数截断。
    // legacy_unbound 只影响归属精度，不表示这里拿到的是部分历史。
    let complete_ledger_available = true;
    let pending_native_branch = native_branch_row.is_some();
    log_runtime_line(
        "resume",
        &format!(
            "gate session={} pending_native_branch={} native_branch_support={:?} context_ok=true",
            session_id, pending_native_branch, capability_snapshot.capabilities.native_branch.support
        ),
    );
    if pending_native_branch
        && capability_snapshot.capabilities.native_branch.support
            != crate::capability_registry::CapabilitySupport::Supported
    {
        return Err(if engine == "codex" {
            "[capability_native_branch_unsupported] 当前 Codex 不支持 thread/fork，无法无损分支；请更新 Codex CLI，或使用摘要分叉".to_string()
        } else {
            "[capability_native_branch_unsupported] 当前 Claude CLI 不支持 --fork-session，无法无损分支；请更新 CLI，或使用摘要分叉".to_string()
        });
    }
    let native_resume_id = if pending_native_branch {
        // 分支语义必须无损：launch profile 漂移时明确失败，禁止静默降级成空历史重建。
        if !same_launch_profile {
            return Err(
                "[capability_native_branch_profile_mismatch] 分支源会话的启动配置已变化，无法安全无损续接；请改用摘要分叉".to_string(),
            );
        }
        match resume_strategy(
            &capability_snapshot,
            same_launch_profile,
            complete_ledger_available,
        ) {
            ResumeStrategy::Blocked => return Err(
                "[capability_resume_blocked] 原生 resume 不兼容且历史账本不足，拒绝静默截断重建"
                    .to_string(),
            ),
            _ => native_candidate,
        }
    } else {
        match resume_strategy(
            &capability_snapshot,
            same_launch_profile,
            complete_ledger_available,
        ) {
            ResumeStrategy::Native => native_candidate,
            ResumeStrategy::LedgerRebuild => None,
            ResumeStrategy::Blocked => return Err(
                "[capability_resume_blocked] 原生 resume 不兼容且历史账本不足，拒绝静默截断重建"
                    .to_string(),
            ),
        }
    };
    let (codex_native_thread_id, codex_fork_source) = if pending_native_branch {
        (None, native_resume_id.clone())
    } else {
        (native_resume_id.clone(), None)
    };
    if detail.summary.cli_session_id.is_some() && native_resume_id.is_none() {
        eprintln!(
            "[helm] [capability_native_resume_fallback_ledger] Session {} 将从完整 TurnLedger 重建",
            detail.summary.id
        );
    }
    log_runtime_line(
        "resume",
        &format!(
            "spawn session={} engine={} native_resume={:?} codex_fork_source={:?} messages={}",
            session_id,
            engine,
            native_resume_id,
            codex_fork_source,
            context_messages.len()
        ),
    );
    let session = match detail.summary.engine {
        EngineId::ClaudeCode => {
            start_claude_with_resume_and_reasoning(
                app,
                detail.summary.id.clone(),
                bin,
                model.clone(),
                detail.summary.cwd.clone(),
                env,
                reasoning_effort,
                native_resume_id.clone(),
                context_messages,
                capability_snapshot.clone(),
                pending_native_branch,
            )
            .await?
        }
        EngineId::Codex => start_codex_with_reasoning(
            app,
            detail.summary.id.clone(),
            bin,
            model.clone(),
            detail.summary.cwd.clone(),
            env,
            context_messages,
            codex_native_thread_id,
            codex_fork_source,
            // 切点分叉：来源行若登记了截断轮，首轮 thread/fork 带 lastTurnId；
            // 左栏整段分叉为 None，行为与升级前完全一致。
            native_branch_row
                .as_ref()
                .and_then(|(_, _, boundary, _)| boundary.clone()),
            None,
            subscription_home,
            capability_snapshot.clone(),
            reasoning_effort,
        )?,
    };
    let restored_profile =
        crate::adapter::PermissionProfile::parse(&detail.summary.safe_permission_profile)?;
    session.set_permission_profile(restored_profile).await?;
    log_runtime_line(
        "resume",
        &format!("spawned session={} permission_profile=ok", session_id),
    );
    let handle = store.next_handle();
    let owner = RuntimeOwnerRef::session(detail.summary.id.clone());
    runtime_registry
        .register_session(
            owner.clone(),
            session,
            &runtime_route,
            &capability_snapshot,
            &detail.summary.cwd,
        )
        .await?;
    let actor = SessionActorHandle::start(owner, runtime_registry.inner().clone());
    store
        .sessions
        .lock()
        .map_err(|_| "会话表锁中毒".to_string())?
        .insert(handle.clone(), actor);
    store.bind_history_session(&handle, &detail.summary.id)?;
    history_store.set_active_session(&detail.summary.id)?;
    history_store.set_session_provider(&detail.summary.id, &provider_id)?;
    Ok(handle)
}

#[tauri::command]
pub fn get_provider_config(
    store: State<'_, ProviderStore<KeyringSecretStore>>,
) -> Result<AppConfig, String> {
    store.load()
}

#[tauri::command]
pub fn reveal_provider_secret(
    app: AppHandle,
    store: State<'_, ProviderStore<KeyringSecretStore>>,
    provider_id: String,
) -> Result<String, String> {
    use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};

    let provider_name = store
        .load()?
        .providers
        .iter()
        .find(|provider| provider.id == provider_id)
        .map(|provider| provider.name.clone())
        .unwrap_or_else(|| provider_id.clone());
    // 明文密钥出钥匙串前必须经过系统级确认（可靠性检查 P2-7）：
    // 门槛放在后端，前端调用无法绕过。
    let confirmed = app
        .dialog()
        .message(format!(
            "即将在界面上显示服务商「{provider_name}」的明文密钥。\n\n请确认当前没有投屏、录屏或旁观。"
        ))
        .title("显示明文密钥")
        .kind(MessageDialogKind::Warning)
        .buttons(MessageDialogButtons::OkCancelCustom(
            "显示密钥".to_string(),
            "取消".to_string(),
        ))
        .blocking_show();
    if !confirmed {
        return Err("已取消显示密钥".to_string());
    }
    store.provider_secret(&provider_id)
}

#[tauri::command]
pub fn save_provider_config(
    store: State<'_, ProviderStore<KeyringSecretStore>>,
    provider: ProviderConfig,
    api_key: Option<String>,
) -> Result<AppConfig, String> {
    store.save_provider(provider, api_key.as_deref())
}

#[tauri::command]
pub fn delete_provider_config(
    store: State<'_, ProviderStore<KeyringSecretStore>>,
    provider_id: String,
) -> Result<AppConfig, String> {
    store.delete_provider(&provider_id)
}

#[tauri::command]
pub fn save_provider_models_config(
    store: State<'_, ProviderStore<KeyringSecretStore>>,
    sessions: State<'_, SessionHistoryStore>,
    provider_id: String,
    models: Vec<ModelConfig>,
) -> Result<AppConfig, String> {
    let before = store.load()?;
    let config = store.save_models_for_provider(&provider_id, models)?;
    cascade_preferred_model_after_retarget(&sessions, &before, &config, &provider_id);
    Ok(config)
}

#[tauri::command]
pub fn delete_provider_model(
    store: State<'_, ProviderStore<KeyringSecretStore>>,
    sessions: State<'_, SessionHistoryStore>,
    provider_id: String,
    model_id: String,
) -> Result<AppConfig, String> {
    let before = store.load()?;
    let config = store.delete_provider_model(&provider_id, &model_id)?;
    cascade_preferred_model_after_retarget(&sessions, &before, &config, &provider_id);
    Ok(config)
}

#[tauri::command]
pub fn save_engine_config(
    store: State<'_, ProviderStore<KeyringSecretStore>>,
    engine: EngineConfig,
) -> Result<AppConfig, String> {
    store.save_engine(engine)
}

#[tauri::command]
pub fn save_model_config(
    store: State<'_, ProviderStore<KeyringSecretStore>>,
    model: ModelConfig,
) -> Result<AppConfig, String> {
    store.save_model(model)
}

#[tauri::command]
pub fn save_provider_model_selection(
    store: State<'_, ProviderStore<KeyringSecretStore>>,
    sessions: State<'_, SessionHistoryStore>,
    provider_id: String,
    enabled_model_ids: Vec<String>,
) -> Result<AppConfig, String> {
    let before = store.load()?;
    let config = store.save_provider_model_selection(&provider_id, &enabled_model_ids)?;
    cascade_preferred_model_after_retarget(&sessions, &before, &config, &provider_id);
    Ok(config)
}

/// 目录/勾选保存把绑定从旧模型 ID 改到新 ID 后，会话 `preferred_model` 一并跟上。
fn cascade_preferred_model_after_retarget(
    sessions: &SessionHistoryStore,
    before: &AppConfig,
    after: &AppConfig,
    provider_id: &str,
) {
    for after_binding in after
        .bindings
        .iter()
        .filter(|binding| binding.provider_id == provider_id)
    {
        let Some(before_binding) = before
            .bindings
            .iter()
            .find(|binding| binding.engine_id == after_binding.engine_id)
        else {
            continue;
        };
        if before_binding.primary_model != after_binding.primary_model
            && !before_binding.primary_model.trim().is_empty()
        {
            let _ = sessions.rename_session_preferred_model(
                &before_binding.primary_model,
                &after_binding.primary_model,
            );
        }
        let before_fast = before_binding.fast_model.as_deref().unwrap_or("");
        let after_fast = after_binding.fast_model.as_deref().unwrap_or("");
        if before_fast != after_fast && !before_fast.is_empty() && !after_fast.is_empty() {
            let _ = sessions.rename_session_preferred_model(before_fast, after_fast);
        }
    }
}

#[tauri::command]
pub async fn save_binding_config(
    store: State<'_, ProviderStore<KeyringSecretStore>>,
    profiles: State<'_, SubscriptionProfileStore>,
    binding: BindingConfig,
) -> Result<AppConfig, String> {
    let config = store.load()?;
    ensure_binding_runtime_ready(&profiles, &config, &binding).await?;
    store.save_binding(binding)
}

#[tauri::command]
pub fn get_equivalent_env(
    store: State<'_, ProviderStore<KeyringSecretStore>>,
    binding: BindingConfig,
) -> Result<Vec<(String, String)>, String> {
    store.equivalent_env(&binding)
}

#[tauri::command]
pub fn read_engine_config_file(engine_id: String) -> Result<EngineConfigFile, String> {
    read_engine_config_file_from_disk(&engine_id)
}

#[tauri::command]
pub fn write_engine_config_file(
    engine_id: String,
    content: String,
) -> Result<EngineConfigFile, String> {
    write_engine_config_file_to_disk(&engine_id, &content)
}

#[tauri::command]
pub async fn test_provider_config(
    store: State<'_, ProviderStore<KeyringSecretStore>>,
    provider_id: String,
) -> Result<ConnectionResult, String> {
    let result = test_provider_connection(&store, &provider_id).await?;
    let outcome = if !result.verified {
        TestOutcome::Unverified
    } else if result.ok {
        TestOutcome::Ok
    } else {
        TestOutcome::Fail
    };
    let failure_category = classify_failure(&result.message, result.ok, result.verified);
    let test = ProviderTest {
        result: outcome,
        latency_ms: Some(result.latency_ms),
        at: unix_timestamp_seconds()?,
        failure_category,
    };
    store.record_test_result(&provider_id, test)?;
    Ok(result)
}

/// 添加流程「测试连接」：草稿探活（URL + 密钥 + 协议），服务商尚未创建、不落测试记录。
#[tauri::command]
pub async fn test_provider_draft_config(
    base_url: String,
    api_key: String,
    protocol: Protocol,
) -> Result<ConnectionResult, String> {
    test_provider_draft(&base_url, &api_key, &protocol).await
}

/// 「同步模型」候选拉取：只返回远端模型 ID 与最新配置，不把候选写入模型行。
/// 草稿 Base URL / API Key 只用于本次请求，不落库。
#[tauri::command]
pub async fn list_provider_models_config(
    store: State<'_, ProviderStore<KeyringSecretStore>>,
    provider_id: String,
    base_url: Option<String>,
    api_key: Option<String>,
) -> Result<ProviderModelListing, String> {
    list_provider_models(
        &store,
        &provider_id,
        base_url.as_deref(),
        api_key.as_deref(),
    )
    .await
}

fn unix_timestamp_seconds() -> Result<i64, String> {
    let duration = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| format!("系统时间异常：{e}"))?;
    i64::try_from(duration.as_secs()).map_err(|_| "系统时间超出范围".to_string())
}

/// message.ts 用毫秒（变更-07：与 checkpoint.ts 同单位，回溯截断依赖比较）
fn unix_timestamp_millis() -> Result<i64, String> {
    let duration = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| format!("系统时间异常：{e}"))?;
    i64::try_from(duration.as_millis()).map_err(|_| "系统时间超出范围".to_string())
}

#[tauri::command]
pub async fn sync_provider_models_config(
    store: State<'_, ProviderStore<KeyringSecretStore>>,
    profiles: State<'_, SubscriptionProfileStore>,
    sessions: State<'_, SessionHistoryStore>,
    provider_id: String,
) -> Result<AppConfig, String> {
    let config = store.load()?;
    let provider = config
        .providers
        .iter()
        .find(|provider| provider.id == provider_id)
        .ok_or_else(|| format!("找不到服务商：{provider_id}"))?;
    if matches!(provider.kind, ProviderKind::Subscription) {
        let engine = match provider.protocol {
            Protocol::Anthropic => "claude-code",
            Protocol::OpenAiResponses => "codex",
            _ => return Err("订阅服务商的接口规范不受支持".to_string()),
        };
        crate::settings::ensure_subscription_login(&profiles, engine).await?;
        let models = match provider.protocol {
            Protocol::OpenAiResponses => {
                let codex_home = profiles.profile_dir("codex")?;
                discover_codex_subscription_models(&config, &provider_id, &codex_home).await?
            }
            Protocol::Anthropic => crate::providers::subscription_models_for_provider(provider),
            _ => unreachable!("subscription protocol was validated above"),
        };
        let saved = store.save_models_for_provider(&provider_id, models)?;
        cascade_preferred_model_after_retarget(&sessions, &config, &saved, &provider_id);
        return Ok(saved);
    }
    let saved = sync_provider_models(&store, &provider_id).await?;
    cascade_preferred_model_after_retarget(&sessions, &config, &saved, &provider_id);
    Ok(saved)
}

#[tauri::command]
pub async fn test_engine_config(bin: String) -> Result<ConnectionResult, String> {
    Ok(test_engine_connection(&bin).await)
}

/// 读取模型真实支持的推理档位。Claude 由本机 CLI help 与精确模型目录交叉判断；
/// Codex 使用同一 Provider/CODEX_HOME 启动短生命周期 app-server 并调用 model/list。
#[tauri::command]
pub async fn get_reasoning_effort_capability(
    store: State<'_, ProviderStore<KeyringSecretStore>>,
    profiles: State<'_, SubscriptionProfileStore>,
    history_store: State<'_, SessionHistoryStore>,
    capability_registry: State<'_, EngineCapabilityRegistry>,
    engine: String,
    model: String,
    provider_id: Option<String>,
) -> Result<ReasoningEffortCapability, String> {
    let config = store.load()?;
    let bin = config
        .engine_bin(&engine)
        .filter(|bin| !bin.trim().is_empty())
        .unwrap_or(if engine == "codex" { "codex" } else { "claude" })
        .to_string();
    let mut binding = config
        .bindings
        .iter()
        .find(|binding| binding.engine_id == engine)
        .cloned()
        .ok_or_else(|| format!("{engine} 尚未配置生效绑定"))?;
    if let Some(provider_id) = provider_id.filter(|value| !value.trim().is_empty()) {
        binding.provider_id = provider_id;
    }
    let bound_provider_id = binding.provider_id.clone();
    let launch_binding = BindingConfig {
        primary_model: model.clone(),
        ..binding
    };
    let mut env = store.launch_env_for_config(&config, &launch_binding)?;
    let subscription_home = subscription_profile_for_binding(&profiles, &config, &launch_binding)?;
    if subscription_home.is_some() {
        profiles.append_launch_env(&mut env, &engine)?;
    }
    env.extend(agent_environment_from_settings(
        &load_app_settings_from_store(&history_store)?,
    ));
    prepare_codex_search_launch(&engine, &bin, &model, &mut env).await?;
    let route = build_runtime_route(
        &config,
        &launch_binding,
        &model,
        &bin,
        &env,
        ReasoningEffort::Auto,
        None,
    )?;
    let snapshot = resolve_engine_capability_snapshot(
        &capability_registry,
        &route,
        &bin,
        &env,
        subscription_home,
    )
    .await?;
    let support = match snapshot.capabilities.reasoning_effort.support {
        crate::capability_registry::CapabilitySupport::Supported => {
            ReasoningEffortSupport::Supported
        }
        crate::capability_registry::CapabilitySupport::Unsupported => {
            ReasoningEffortSupport::Unsupported
        }
        crate::capability_registry::CapabilitySupport::Degraded
        | crate::capability_registry::CapabilitySupport::Unknown => ReasoningEffortSupport::Unknown,
    };
    Ok(ReasoningEffortCapability {
        support,
        options: snapshot.capabilities.reasoning_efforts,
        default_effort: snapshot.capabilities.default_reasoning_effort,
        source: ReasoningEffortSource::EngineProbe,
    })
}

/// 发送一条用户消息（拉起真实 claude 轮次并流式回传 stdout）。
#[tauri::command]
pub async fn send_message(
    app: AppHandle,
    store: State<'_, SessionStore>,
    config_store: State<'_, ProviderStore<KeyringSecretStore>>,
    history_store: State<'_, SessionHistoryStore>,
    profiles: State<'_, SubscriptionProfileStore>,
    capability_registry: State<'_, EngineCapabilityRegistry>,
    runtime_registry: State<'_, RuntimeRegistry>,
    handle_id: String,
    text: String,
    display_text: Option<String>,
    attachments: Option<Vec<String>>,
    mode: Option<String>,
    model: Option<String>,
    reasoning_effort: Option<String>,
) -> Result<(), String> {
    let attachments = attachments.unwrap_or_default();
    let budget = history_store.get_budget()?;
    ensure_budget_allows_turn(&budget)?;
    let history_session_id = store.history_session_id_for_handle(&handle_id)?;
    let handoff_context = history_store.session_handoff_context(&history_session_id)?;
    let frozen_session_context = history_store.freeze_session_contexts(&history_session_id)?;
    let app_settings = load_app_settings_from_store(&history_store)?;
    // 会话模式（变更-04）：轮次级属性，未知/缺省一律回落构建（兼容旧前端调用）
    let mode = TurnMode::parse(mode.as_deref());
    let requested_reasoning_effort = ReasoningEffort::parse(reasoning_effort.as_deref())?;
    // 历史存用户原文（变更-08）：斜杠命令展开结果只进 CLI，气泡与历史显示 /cmd args 原文
    let record_text = display_text.unwrap_or_else(|| text.clone());
    let session = {
        let sessions = store
            .sessions
            .lock()
            .map_err(|_| "会话表锁中毒".to_string())?;
        sessions
            .get(&handle_id)
            .cloned()
            .ok_or_else(|| format!("找不到会话：{handle_id}"))?
    };
    let permission_profile = session.permission_profile().await?;
    let detail = history_store.get_session(&history_session_id)?;
    let engine = engine_id_to_string(detail.summary.engine);
    let requested_model = model;
    let preferred_model = detail.summary.preferred_model.clone();
    let requested_reasoning_effort = requested_reasoning_effort.or_else(|| {
        detail
            .summary
            .preferred_reasoning_effort
            .as_deref()
            .and_then(|value| ReasoningEffort::parse(Some(value)).ok().flatten())
    });
    let created_at = unix_timestamp_millis()?;
    let mut committed = None;
    for _ in 0..3 {
        let candidate = config_store.route_candidate()?;
        let binding = candidate
            .config
            .bindings
            .iter()
            .find(|binding| binding.engine_id == engine)
            .cloned()
            .ok_or_else(|| format!("引擎还没有配置生效绑定：{engine}"))?;
        let requested_model = requested_model_for_binding(
            requested_model.as_deref(),
            preferred_model.as_deref(),
            &binding,
        );
        let routed_model = resolve_binding_model(&candidate.config, &binding, &requested_model);
        let command = TurnStartCommand {
            history_session_id: history_session_id.clone(),
            display_text: record_text.clone(),
            turn_mode: mode.as_state_str().to_string(),
            permission_profile: permission_profile.as_str().to_string(),
            requested_reasoning_effort,
            requested_model_id: Some(requested_model),
            attachments: attachments.clone(),
            created_at,
        };
        let launch_binding = BindingConfig {
            primary_model: routed_model.clone(),
            ..binding.clone()
        };
        ensure_binding_runtime_ready(&profiles, &candidate.config, &launch_binding).await?;
        let mut env = config_store.launch_env_for_config(&candidate.config, &launch_binding)?;
        let subscription_home =
            subscription_profile_for_binding(&profiles, &candidate.config, &launch_binding)?;
        if subscription_home.is_some() {
            profiles.append_launch_env(&mut env, &engine)?;
        }
        env.extend(agent_environment_from_settings(&app_settings));
        let bin = candidate
            .config
            .engine_bin(&engine)
            .filter(|value| !value.trim().is_empty())
            .unwrap_or(if engine == "codex" { "codex" } else { "claude" })
            .to_string();
        ensure_pricing_allows_turn(
            &budget,
            &app_settings,
            &config_store,
            &candidate.config,
            &binding.provider_id,
            &routed_model,
        )?;
        let pricing_basis = resolve_turn_pricing_basis(
            &config_store,
            &candidate.config,
            &binding.provider_id,
            &routed_model,
        )?;
        let requested_effort = requested_reasoning_effort
            .or(binding.reasoning_effort)
            .unwrap_or_default();
        prepare_codex_search_launch(&engine, &bin, &routed_model, &mut env).await?;
        let mut turn_route = build_runtime_route(
            &candidate.config,
            &launch_binding,
            &routed_model,
            &bin,
            &env,
            requested_effort,
            pricing_basis.profile.clone(),
        )?;
        let mut capability_snapshot = resolve_engine_capability_snapshot(
            &capability_registry,
            &turn_route,
            &bin,
            &env,
            subscription_home.clone(),
        )
        .await?;
        apply_provider_declared_model_override(
            &mut capability_snapshot.capabilities.model_override,
            &engine,
            &env,
            binding.primary_model.as_str(),
            &routed_model,
        );
        let routed_effort = resolve_routed_effort(&capability_snapshot, requested_effort);
        ensure_requested_runtime_capabilities(&capability_snapshot, routed_effort)?;
        turn_route.default_reasoning_effort = routed_effort;
        let mut spec = BindingLiveRouteResolver::resolve(
            &turn_route,
            &binding,
            &capability_snapshot.id,
            &command,
        )?;
        spec.routed_reasoning_effort = routed_effort;
        spec.session_context = frozen_session_context.clone();
        session.reserve_turn().await?;
        match config_store.commit_route_if_unchanged(&candidate.config_digest, |_| {
            history_store.start_turn(&command, spec)
        }) {
            Ok(Some(started)) => {
                committed = Some((
                    started,
                    turn_route,
                    capability_snapshot,
                    bin,
                    env,
                    subscription_home,
                    routed_effort,
                ));
                break;
            }
            Ok(None) => session.release_turn_reservation(),
            Err(error) => {
                session.release_turn_reservation();
                return Err(error);
            }
        }
    }
    let (
        (prepared, spec),
        turn_route,
        capability_snapshot,
        bin,
        env,
        subscription_home,
        routed_effort,
    ) = committed
        .ok_or_else(|| "Provider 配置连续变化，TurnStart 有界重算未能收敛，请重试".to_string())?;
    // 首轮会在 start_turn 内把「未命名会话」改写为用户首行文本（sessions.rs）；主动广播，
    // 让常驻 Rail 侧栏即时刷新标题，无需整页刷新（修复「发送后左侧标题不实时更新」）。
    if detail.summary.title == "未命名会话" {
        let _ = app.emit("helm-sessions-changed", &history_session_id);
    }
    // 状态徽标实时刷新（9/4）：start_turn 已把 session.status 置 'active'（idle→running
    // 起点）。此前只有未命名会话改名才广播，续轮发送时侧栏「运行中」徽标要等首条引擎
    // 事件跨状态边界才点亮——CLI 冷启动/输出静默期是数秒盲区。落库即广播一次。
    let _ = app.emit("helm-sessions-changed", &history_session_id);
    let owner = RuntimeOwnerRef::session(history_session_id.clone());
    let runtime_result: Result<(), String> = async {
        if runtime_registry
            .route_requires_replacement(&owner, &turn_route, &detail.summary.cwd)
            .await?
        {
            let history_messages = ledger_rebuild_messages(&history_store, &history_session_id)?;
            ensure_ledger_fits_context_window(
                &history_messages,
                &record_text,
                capability_snapshot.capabilities.context_window,
            )?;
            let replacement = start_route_runtime(
                app,
                &engine,
                history_session_id.clone(),
                bin,
                turn_route.model_id.clone(),
                detail.summary.cwd.clone(),
                env,
                routed_effort,
                history_messages,
                subscription_home,
                capability_snapshot.clone(),
            )
            .await?;
            runtime_registry
                .replace_reserved_session(
                    &owner,
                    replacement,
                    &turn_route,
                    &capability_snapshot,
                    &detail.summary.cwd,
                )
                .await?;
        } else {
            runtime_registry
                .update_reserved_capability_snapshot(&owner, capability_snapshot.clone())
                .await?;
        }
        history_store.set_session_route_projection(
            &history_session_id,
            &turn_route.provider_id,
            &turn_route.model_id,
        )?;
        Ok(())
    }
    .await;
    if let Err(runtime_error) = runtime_result {
        session.release_turn_reservation();
        return match history_store.rollback_prepared_user_turn(prepared) {
            Ok(()) => Err(runtime_error),
            Err(rollback_error) => Err(format!(
                "{runtime_error}；同时回滚未投递轮次失败：{rollback_error}"
            )),
        };
    }
    let runtime_text = handoff_context
        .map(|context| format!("{context}\n\n[当前用户请求]\n{text}"))
        .unwrap_or(text);
    let send_result = session.send_reserved(runtime_text, attachments, spec).await;
    if let Err(send_error) = send_result {
        session.release_turn_reservation();
        return match history_store.rollback_prepared_user_turn(prepared) {
            Ok(()) => Err(send_error),
            Err(rollback_error) => Err(format!(
                "{send_error}；同时回滚未启动轮次失败：{rollback_error}"
            )),
        };
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn start_route_runtime(
    app: AppHandle,
    engine: &str,
    history_session_id: String,
    bin: String,
    model: String,
    cwd: String,
    env: Vec<(String, String)>,
    reasoning_effort: ReasoningEffort,
    history_messages: Vec<crate::sessions::SessionMessage>,
    subscription_home: Option<std::path::PathBuf>,
    capability_snapshot: EngineCapabilitySnapshot,
) -> Result<crate::adapter::AgentSession, String> {
    match engine {
        "claude-code" => {
            start_claude_with_resume_and_reasoning(
                app,
                history_session_id,
                bin,
                model,
                cwd,
                env,
                reasoning_effort,
                None,
                history_messages,
                capability_snapshot,
                false,
            )
            .await
        }
        "codex" => start_codex_with_reasoning(
            app,
            history_session_id,
            bin,
            model,
            cwd,
            env,
            history_messages,
            None,
            None,
            None,
            None,
            subscription_home,
            capability_snapshot,
            reasoning_effort,
        ),
        _ => Err(format!("暂不支持的引擎：{engine}")),
    }
}

fn ledger_rebuild_messages(
    history_store: &SessionHistoryStore,
    session_id: &str,
) -> Result<Vec<crate::sessions::SessionMessage>, String> {
    use crate::protocol::Role;
    let detail = history_store.get_session(session_id)?;
    let ledger = history_store.get_turn_ledger(session_id)?;
    let mut rebuilt = Vec::new();
    let mut bound_turn_ids = HashSet::new();
    for record in ledger {
        bound_turn_ids.insert(record.turn.id.clone());
        rebuilt.extend(
            record
                .messages
                .iter()
                .filter(|message| !message.reverted)
                .cloned(),
        );
        if record.tool_calls.is_empty()
            && record.approvals.is_empty()
            && record.checkpoints.is_empty()
            && record.attachments.is_empty()
        {
            continue;
        }
        let facts = serde_json::json!({
            "turnId": record.turn.id,
            "tools": record.tool_calls,
            "approvals": record.approvals,
            "checkpoints": record.checkpoints,
            "attachments": record.attachments,
            "contextEvidence": record.session_context,
        });
        rebuilt.push(crate::sessions::SessionMessage {
            role: Role::Assistant,
            text: format!("[Helm TurnLedger 可重放事实]\n{facts}"),
            ts: record.turn.ended_at.unwrap_or(record.turn.started_at),
            reverted: false,
            turn_id: Some(record.turn.id),
            attachments: Vec::new(),
        });
    }
    rebuilt.extend(detail.messages.into_iter().filter(|message| {
        !message.reverted
            && message
                .turn_id
                .as_ref()
                .is_none_or(|turn_id| !bound_turn_ids.contains(turn_id))
    }));
    rebuilt.sort_by_key(|message| message.ts);
    Ok(rebuilt)
}

fn ensure_ledger_fits_context_window(
    history: &[crate::sessions::SessionMessage],
    current_prompt: &str,
    context_window: Option<u64>,
) -> Result<(), String> {
    let Some(context_window) = context_window else {
        return Ok(());
    };
    let bytes = history
        .iter()
        .map(|message| message.text.len())
        .sum::<usize>()
        .saturating_add(current_prompt.len());
    let conservative_tokens = u64::try_from(bytes.saturating_add(2) / 3).unwrap_or(u64::MAX);
    if conservative_tokens > context_window.saturating_mul(9) / 10 {
        return Err(format!(
            "[ledger_context_window_insufficient] 完整历史预计至少 {conservative_tokens} tokens，超过目标模型 {context_window} token 窗口的安全上限；请选择更大窗口模型"
        ));
    }
    Ok(())
}

#[tauri::command]
pub async fn rename_provider_model(
    store: tauri::State<'_, ProviderStore<KeyringSecretStore>>,
    sessions: tauri::State<'_, SessionHistoryStore>,
    provider_id: String,
    old_model_id: String,
    new_model_id: String,
) -> Result<AppConfig, String> {
    let (old_id, new_id) = (old_model_id.clone(), new_model_id.clone());
    let config = {
        let store = store.inner().clone();
        tokio::task::spawn_blocking(move || {
            store.rename_provider_model(&provider_id, &old_id, &new_id)
        })
        .await
        .map_err(|e| format!("模型改名任务失败：{e}"))??
    };
    let changed = {
        let sessions = sessions.inner().clone();
        tokio::task::spawn_blocking(move || {
            sessions.rename_session_preferred_model(&old_model_id, &new_model_id)
        })
        .await
        .map_err(|e| format!("会话偏好级联失败：{e}"))??
    };
    if changed > 0 {
        eprintln!("模型改名级联：{changed} 个会话的 preferred_model 已同步到新 ID");
    }
    Ok(config)
}

#[tauri::command]
pub async fn set_session_turn_preference(
    store: State<'_, SessionStore>,
    history_store: State<'_, SessionHistoryStore>,
    handle_id: String,
    model: String,
    reasoning_effort: Option<String>,
) -> Result<(), String> {
    let session = store
        .sessions
        .lock()
        .map_err(|_| "会话表锁中毒".to_string())?
        .get(&handle_id)
        .cloned()
        .ok_or_else(|| format!("找不到会话：{handle_id}"))?;
    session.reserve_turn().await?;
    let history_session_id = store.history_session_id_for_handle(&handle_id)?;
    let result = history_store.set_session_turn_preference(
        &history_session_id,
        &model,
        reasoning_effort.as_deref(),
    );
    session.release_turn_reservation();
    result
}

#[tauri::command]
pub async fn set_session_permission_profile(
    store: State<'_, SessionStore>,
    history_store: State<'_, SessionHistoryStore>,
    handle_id: String,
    profile: String,
    full_access_confirmed: Option<bool>,
) -> Result<(), String> {
    let profile = crate::adapter::PermissionProfile::parse(&profile)?;
    let session = store
        .sessions
        .lock()
        .map_err(|_| "会话表锁中毒".to_string())?
        .get(&handle_id)
        .cloned()
        .ok_or_else(|| format!("找不到会话：{handle_id}"))?;
    if profile == crate::adapter::PermissionProfile::FullAccess {
        require_full_access_confirmed(full_access_confirmed)?;
    }
    session.set_permission_profile(profile).await?;
    if matches!(
        profile,
        crate::adapter::PermissionProfile::Standard | crate::adapter::PermissionProfile::Auto
    ) {
        let history_session_id = store.history_session_id_for_handle(&handle_id)?;
        history_store.set_safe_permission_profile(&history_session_id, profile.as_str())?;
    }
    Ok(())
}

/// 全部放开只接受前端页内确认卡的显式标记；无标记 fail-closed，禁止 IPC 绕过确认。
fn require_full_access_confirmed(confirmed: Option<bool>) -> Result<(), String> {
    if confirmed == Some(true) {
        Ok(())
    } else {
        Err("开启全部放开需要先在页内确认卡确认".to_string())
    }
}

/// 中断当前轮次（杀掉对应进程树，并合成 turn_complete{interrupted}）。
#[tauri::command]
pub async fn interrupt(store: State<'_, SessionStore>, handle_id: String) -> Result<(), String> {
    let actor = store
        .sessions
        .lock()
        .map_err(|_| "会话表锁中毒".to_string())?
        .get(&handle_id)
        .cloned()
        .ok_or_else(|| format!("找不到会话：{handle_id}"))?;
    actor.interrupt().await
}

/// 触发引擎原生上下文压缩（变更-34/35 · B4）：只有 Codex app-server 提供真实
/// `thread/compact/start` 契约（2026-08-12 更正）；Claude `-p` 返回明确错误。
#[tauri::command]
pub async fn compact_context(
    store: State<'_, SessionStore>,
    handle_id: String,
) -> Result<(), String> {
    let actor = store
        .sessions
        .lock()
        .map_err(|_| "会话表锁中毒".to_string())?
        .get(&handle_id)
        .cloned()
        .ok_or_else(|| format!("找不到会话：{handle_id}"))?;
    actor.compact_context().await
}

#[tauri::command]
pub fn get_background_operation(
    history_store: State<'_, SessionHistoryStore>,
    operation_id: String,
) -> Result<Option<crate::operations::BackgroundOperation>, String> {
    history_store.load_background_operation(&operation_id)
}

#[tauri::command]
pub async fn start_session_fork(
    app: AppHandle,
    source_session_id: String,
    target_engine: String,
    boundary_turn_id: Option<String>,
) -> Result<crate::operations::BackgroundOperation, String> {
    crate::handoff::start_session_fork(
        &app,
        &source_session_id,
        &target_engine,
        boundary_turn_id.as_deref(),
    )
    .await
}

#[tauri::command]
pub async fn cancel_background_operation(
    runtime_registry: State<'_, RuntimeRegistry>,
    operation_id: String,
) -> Result<bool, String> {
    runtime_registry.cancel_operation(&operation_id).await
}

#[tauri::command]
pub async fn retry_background_operation(
    app: AppHandle,
    operation_id: String,
) -> Result<(), String> {
    let history = app
        .try_state::<SessionHistoryStore>()
        .ok_or("历史存储未初始化")?;
    let operation = history
        .load_background_operation(&operation_id)?
        .ok_or_else(|| format!("找不到 BackgroundOperation：{operation_id}"))?;
    match operation.kind.as_str() {
        "auto_title" => crate::titler::retry_background_operation(&app, &operation_id).await,
        "fork_job" => crate::handoff::retry_fork_job(&app, &operation_id).await,
        _ => Err("当前 BackgroundOperation 类型不支持手工重试".to_string()),
    }
}

/// 变更-34 · A3：让 Helm 自评审当前会话的变更（真实 fast model 调用）。
#[tauri::command]
pub async fn review_changes(
    app: AppHandle,
    history_session_id: String,
) -> Result<Vec<crate::self_review::ReviewNoteDto>, String> {
    crate::self_review::review_changes(&app, &history_session_id).await
}

/// 返回后端权威的最近一轮快照，供前端在 Stop、重连或事件丢失后对账。
#[tauri::command]
pub fn get_turn_snapshot(
    store: State<'_, SessionStore>,
    supervisor: State<'_, crate::turn_supervisor::TurnSupervisor>,
    handle_id: String,
) -> Result<Option<crate::turn_supervisor::TurnSnapshot>, String> {
    let history_session_id = store.history_session_id_for_handle(&handle_id)?;
    supervisor.snapshot(&history_session_id)
}

/// 关闭并回收一个会话句柄：终止残留进程、从 SessionStore 移除，防止 runtime 泄漏。
/// 幂等：句柄不存在时静默成功（前端可能重复调用）。
#[tauri::command]
pub async fn close_session(
    store: State<'_, SessionStore>,
    handle_id: String,
) -> Result<(), String> {
    let removed = store
        .sessions
        .lock()
        .map_err(|_| "会话表锁中毒".to_string())?
        .remove(&handle_id);
    store
        .history_session_ids
        .lock()
        .map_err(|_| "会话历史映射锁中毒".to_string())?
        .remove(&handle_id);
    if let Some(actor) = removed {
        let owner_still_referenced = store
            .sessions
            .lock()
            .map_err(|_| "会话表锁中毒".to_string())?
            .values()
            .any(|candidate| candidate.owner() == actor.owner());
        if !owner_still_referenced {
            actor.close().await?;
        }
    }
    Ok(())
}

/// 回应一个审批请求，并用 Claude Code 的 `--resume` 继续被 defer 的工具调用。
trait ApprovalLedger {
    fn mark_applying(
        &self,
        history_session_id: &str,
        approval_id: &str,
        decision: &str,
    ) -> Result<(), String>;
    fn resolve(
        &self,
        history_session_id: &str,
        approval_id: &str,
        decision: &str,
    ) -> Result<(), String>;
    fn fail(&self, history_session_id: &str, approval_id: &str, error: &str) -> Result<(), String>;
}

impl ApprovalLedger for SessionHistoryStore {
    fn mark_applying(
        &self,
        session_id: &str,
        approval_id: &str,
        decision: &str,
    ) -> Result<(), String> {
        self.mark_approval_applying(session_id, approval_id, decision)
    }

    fn resolve(&self, session_id: &str, approval_id: &str, decision: &str) -> Result<(), String> {
        self.resolve_approval_with_decision(session_id, approval_id, decision, None)
    }

    fn fail(&self, session_id: &str, approval_id: &str, error: &str) -> Result<(), String> {
        self.fail_approval(session_id, approval_id, error)
    }
}

async fn apply_approval_response<L, F>(
    history_store: &L,
    history_session_id: &str,
    approval_id: &str,
    decision: ApprovalDecision,
    approve: F,
) -> Result<(), String>
where
    L: ApprovalLedger,
    F: std::future::Future<Output = Result<(), String>>,
{
    let audit_decision = decision.audit_value();
    history_store.mark_applying(history_session_id, approval_id, audit_decision)?;
    match approve.await {
        Ok(()) => match history_store.resolve(history_session_id, approval_id, audit_decision) {
            Ok(()) => Ok(()),
            Err(resolve_error) => {
                let audit_error = format!(
                    "CLI 已接受审批，操作可能已经开始，但账本 resolved 落库失败：{resolve_error}；无法回滚已启动 CLI，请核对实际结果后再决定是否重试"
                );
                match history_store.fail(history_session_id, approval_id, &audit_error) {
                    Ok(()) => Err(audit_error),
                    Err(compensation_error) => Err(format!(
                        "{audit_error}；同时把账本标记为 failed 也失败：{compensation_error}"
                    )),
                }
            }
        },
        Err(error) => {
            if let Err(ledger_error) = history_store.fail(history_session_id, approval_id, &error) {
                eprintln!("[approval] 记录审批失败状态失败：{ledger_error}");
            }
            Err(error)
        }
    }
}

#[tauri::command]
pub async fn approval_response(
    app: AppHandle,
    store: State<'_, SessionStore>,
    history_store: State<'_, SessionHistoryStore>,
    handle_id: String,
    approval_id: String,
    decision: String,
) -> Result<(), String> {
    let decision = match decision.as_str() {
        "allow" => ApprovalDecision::Allow,
        "turn" => ApprovalDecision::Turn,
        "session" => ApprovalDecision::Session,
        "project" => ApprovalDecision::Project,
        "deny" => ApprovalDecision::Deny,
        "always" => ApprovalDecision::Always,
        other => return Err(format!("未知审批决定：{other}")),
    };
    let history_session_id = store.history_session_id_for_handle(&handle_id)?;
    let outcome = apply_approval_response(
        &*history_store,
        &history_session_id,
        &approval_id,
        decision,
        async {
            let session = {
                let sessions = store
                    .sessions
                    .lock()
                    .map_err(|_| "会话表锁中毒".to_string())?;
                sessions
                    .get(&handle_id)
                    .ok_or_else(|| format!("找不到会话：{handle_id}"))?
                    .clone()
            };
            session.approve(approval_id.clone(), decision).await
        },
    )
    .await;
    // 侧栏实时性（9/4）：审批账本已落库（pending → applying → resolved/failed），
    // list_sessions 的 pending_approval 是 SQL 实时派生列，但 Rail/侧栏重拉只认
    // helm-sessions-changed。deny 等场景 turn 状态可能不变、没有恢复轮事件，
    // 不在此确定性广播一次，侧栏「等审批」徽标会一直挂着。
    let _ = app.emit("helm-sessions-changed", &history_session_id);
    outcome
}

/// 删除会话（变更-12）：终止其存活运行时 → 级联删库 → 清理检查点快照文件。
#[tauri::command]
pub async fn delete_session(
    app: AppHandle,
    store: State<'_, SessionStore>,
    history_store: State<'_, SessionHistoryStore>,
    session_id: String,
) -> Result<(), String> {
    // 先回收该会话的所有存活句柄（后台运行中的轮次一并终止——删除是显式破坏性操作）
    let handles: Vec<String> = store
        .history_session_ids
        .lock()
        .map_err(|_| "会话历史映射锁中毒".to_string())?
        .iter()
        .filter(|(_, history_id)| history_id.as_str() == session_id)
        .map(|(handle, _)| handle.clone())
        .collect();
    for handle in handles {
        let removed = store
            .sessions
            .lock()
            .map_err(|_| "会话表锁中毒".to_string())?
            .remove(&handle);
        if let Some(session) = removed {
            let _ = session.close().await;
        }
        store
            .history_session_ids
            .lock()
            .map_err(|_| "会话历史映射锁中毒".to_string())?
            .remove(&handle);
    }
    let snapshot_refs = history_store.delete_session(&session_id)?;
    // 快照文件清理：失败不阻断删除（孤儿文件不影响正确性）
    if let Ok(app_data_dir) = app.path().app_data_dir() {
        let snapshot_store = crate::snapshots::SnapshotStore::new(app_data_dir.join("snapshots"));
        for reference in snapshot_refs {
            let _ = snapshot_store.delete(&reference);
        }
    }
    let _ = app.emit("helm-sessions-changed", &session_id);
    Ok(())
}

/// 重命名会话（变更-12）
#[tauri::command]
pub fn rename_session(
    app: AppHandle,
    history_store: State<'_, SessionHistoryStore>,
    session_id: String,
    title: String,
) -> Result<(), String> {
    history_store.rename_session(&session_id, &title)?;
    let _ = app.emit("helm-sessions-changed", &session_id);
    Ok(())
}

/// 置顶/取消置顶（变更-12）
#[tauri::command]
pub fn set_session_pinned(
    app: AppHandle,
    history_store: State<'_, SessionHistoryStore>,
    session_id: String,
    pinned: bool,
) -> Result<(), String> {
    history_store.set_session_pinned(&session_id, pinned)?;
    let _ = app.emit("helm-sessions-changed", &session_id);
    Ok(())
}

/// 归档/取消归档（变更-34/35 · 切片7 · F1）：可逆，历史与用量保留，区别于删除。
#[tauri::command]
pub fn set_session_archived(
    app: AppHandle,
    history_store: State<'_, SessionHistoryStore>,
    session_id: String,
    archived: bool,
) -> Result<(), String> {
    history_store.set_session_archived(&session_id, archived)?;
    let _ = app.emit("helm-sessions-changed", &session_id);
    Ok(())
}

/// 旁路提问（变更-34 · D3 · SideQuery）：读当前 Session 上下文，跑一次真实 CLI 的
/// 无工具问答，结果直接返回前端。**不写回 SessionContext、不落盘**：不产生任何
/// Turn/Operation/用量记录，SQLite 无旁路提问痕迹（DoD）。
#[tauri::command]
pub async fn side_query(
    store: State<'_, SessionStore>,
    config_store: State<'_, ProviderStore<KeyringSecretStore>>,
    history_store: State<'_, SessionHistoryStore>,
    profiles: State<'_, SubscriptionProfileStore>,
    capability_registry: State<'_, EngineCapabilityRegistry>,
    runtime_registry: State<'_, RuntimeRegistry>,
    handle_id: String,
    text: String,
) -> Result<String, String> {
    let budget = history_store.get_budget()?;
    ensure_budget_allows_turn(&budget)?;
    let history_session_id = store.history_session_id_for_handle(&handle_id)?;
    let detail = history_store.get_session(&history_session_id)?;
    let engine = engine_id_to_string(detail.summary.engine);
    if engine != "claude-code" {
        return Err(
            "[side_query_tools_not_disableable] Codex 当前合同不能关闭全部内建工具，旁路提问不可用"
                .to_string(),
        );
    }
    // 旁路提问只读最近若干条消息作上下文，不修改任何 Session 状态。
    let prompt = build_side_query_prompt(&detail, &text)?;
    let app_settings = load_app_settings_from_store(&history_store)?;
    let admitted_model = detail
        .summary
        .preferred_model
        .as_deref()
        .filter(|model| !model.trim().is_empty())
        .unwrap_or(&detail.summary.model)
        .to_string();
    let mut committed = None;
    for _ in 0..3 {
        let candidate = config_store.route_candidate()?;
        let binding = candidate
            .config
            .bindings
            .iter()
            .find(|binding| binding.engine_id == engine)
            .cloned()
            .ok_or_else(|| format!("引擎还没有配置生效绑定：{engine}"))?;
        let requested_model = requested_model_for_binding(Some(&admitted_model), None, &binding);
        let routed_model = resolve_binding_model(&candidate.config, &binding, &requested_model);
        let launch_binding = BindingConfig {
            primary_model: routed_model.clone(),
            ..binding.clone()
        };
        ensure_binding_runtime_ready(&profiles, &candidate.config, &launch_binding).await?;
        let mut env = config_store.launch_env_for_config(&candidate.config, &launch_binding)?;
        let subscription_home =
            subscription_profile_for_binding(&profiles, &candidate.config, &launch_binding)?;
        if subscription_home.is_some() {
            profiles.append_launch_env(&mut env, &engine)?;
        }
        env.extend(agent_environment_from_settings(&app_settings));
        let bin = candidate
            .config
            .engine_bin(&engine)
            .filter(|value| !value.trim().is_empty())
            .unwrap_or("claude")
            .to_string();
        let requested_effort = binding.reasoning_effort.unwrap_or(ReasoningEffort::Auto);
        let route = build_runtime_route(
            &candidate.config,
            &launch_binding,
            &routed_model,
            &bin,
            &env,
            requested_effort,
            None,
        )?;
        let capability = resolve_engine_capability_snapshot(
            &capability_registry,
            &route,
            &bin,
            &env,
            subscription_home,
        )
        .await?;
        let routed_effort = resolve_routed_effort(&capability, requested_effort);
        // 能力证明（红线）：无 tools 原生合同不成立时（Codex 等）在此 fail-closed
        let policy =
            ModelOnlyOperationPolicy::from_capability(&capability, crate::util::now_millis())?;
        committed = Some((
            route,
            capability.id.clone(),
            policy,
            bin,
            env,
            routed_effort,
        ));
        break;
    }
    let (route, _capability_snapshot_id, policy, bin, env, routed_effort) = committed
        .ok_or_else(|| "Provider 配置连续变化，旁路提问重算未能收敛，请重试".to_string())?;
    // 零持久化：直接跑 transient 调用；预算兜底沿用标准 Turn budget 的 wall/output 上限。
    let turn_budget = TurnBudgetSnapshot::standard(crate::util::now_millis());
    let output_limit = turn_budget
        .limit(BudgetDimension::OutputBytes)
        .map(|limit| limit.limit)
        .unwrap_or(16 * 1024 * 1024);
    let wall_limit = turn_budget
        .limit(BudgetDimension::WallClockMs)
        .map(|limit| limit.limit)
        .unwrap_or(60 * 60 * 1000);
    let output = runtime_registry
        .run_transient_model_only_operation(
            &engine,
            &policy,
            &bin,
            &env,
            std::path::Path::new(&detail.summary.cwd),
            &prompt,
            &route.model_id,
            routed_effort,
            output_limit,
            wall_limit,
        )
        .await?;
    let answer = output.text.trim().to_string();
    if answer.is_empty() {
        return Err("[side_query_empty] 旁路提问未得到模型回复".to_string());
    }
    Ok(answer)
}

/// 旁路提问 prompt：只冻结当前会话可见的用户/助手消息尾巴与工作目录，
/// 不携带任何审批、密钥、工具轨迹，也绝不写回。
fn build_side_query_prompt(detail: &SessionDetail, question: &str) -> Result<String, String> {
    const MAX_CONTEXT_CHARS: usize = 4000;
    const MAX_MESSAGES: usize = 8;
    let role_label = |role: &crate::protocol::Role| match role {
        crate::protocol::Role::User => "用户",
        crate::protocol::Role::Assistant => "助手",
    };
    let mut transcript: Vec<String> = Vec::new();
    let mut chars = 0usize;
    for message in detail.messages.iter().rev().take(MAX_MESSAGES) {
        if message.reverted {
            continue;
        }
        let segment = format!("{}：{}", role_label(&message.role), message.text);
        chars += segment.chars().count();
        if chars > MAX_CONTEXT_CHARS {
            break;
        }
        transcript.push(segment);
    }
    transcript.reverse();
    Ok(format!(
        "下面是当前会话的一段中文对话上下文（可参考但不要重复它），工作目录：{}\n\n{}\n\n请直接回答我的临时提问，不要使用任何工具，也不要提议修改文件：\n{}",
        detail.summary.cwd,
        if transcript.is_empty() {
            "（暂无对话历史）".to_string()
        } else {
            transcript.join("\n")
        },
        question.trim()
    ))
}

/// @文件引用（变更-12）：在工作目录下按名称片段搜索文件，供输入框 @ 菜单联想。
/// 深度/数量双限制 + 跳过依赖与版本库目录，防止大仓库遍历卡顿。
///
/// 2026-08-12 修复（@ 提及搜不到文档）：
/// - 匹配改为作用于完整相对路径（目录名也会命中，`@docs` 能搜到 docs/ 下的文件）；
/// - 跳过 `target-*` 变体构建目录（此前只精确跳过 `target`，displaydoc 等产物污染结果）；
/// - 遍历完再排序截断：此前遍历中途满 30 条就 break，排序只作用于先遇到的根目录文件，
///   子目录里的文档永远进不了菜单。
#[tauri::command]
pub fn search_workspace_files(cwd: String, query: String) -> Result<Vec<String>, String> {
    const SKIP_DIRS: [&str; 8] = [
        "node_modules",
        ".git",
        "target",
        "dist",
        "build",
        ".venv",
        "__pycache__",
        ".next",
    ];
    const MAX_DEPTH: usize = 5;
    const MAX_RESULTS: usize = 30;
    const MAX_SCANNED: usize = 20_000;

    let root = std::path::PathBuf::from(&cwd);
    if !root.is_dir() {
        return Ok(Vec::new());
    }
    let needle = query.trim().to_lowercase();
    let mut results: Vec<String> = Vec::new();
    let mut scanned = 0usize;
    let mut stack: Vec<(std::path::PathBuf, usize)> = vec![(root.clone(), 0)];
    while let Some((dir, depth)) = stack.pop() {
        if scanned >= MAX_SCANNED {
            break;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            scanned += 1;
            if scanned >= MAX_SCANNED {
                break;
            }
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            if path.is_dir() {
                // 目录本身也作为可选中条目返回（尾斜杠约定，前端渲染为「目录」行，
                // 与新任务页文件中心原型一致：可选择文件或目录）。可见性与递归同滤：
                // 隐藏目录 / node_modules / target* 永不出现。
                let visible = depth + 1 <= MAX_DEPTH
                    && !name.starts_with('.')
                    && !SKIP_DIRS.contains(&name.as_str())
                    && !name.starts_with("target-");
                if visible {
                    stack.push((path.clone(), depth + 1));
                    if let Ok(relative) = path.strip_prefix(&root) {
                        let mut relative = relative.to_string_lossy().replace('\\', "/");
                        if needle.is_empty() || relative.to_lowercase().contains(&needle) {
                            relative.push('/');
                            results.push(relative);
                        }
                    }
                }
                continue;
            }
            if let Ok(relative) = path.strip_prefix(&root) {
                let relative = relative.to_string_lossy().replace('\\', "/");
                if needle.is_empty() || relative.to_lowercase().contains(&needle) {
                    results.push(relative);
                }
            }
        }
    }
    results.sort_by_key(|item| item.len());
    results.truncate(MAX_RESULTS);
    Ok(results)
}

/// 粘贴图片附件（变更-12）：剪贴板图片落成临时文件，路径进 attachments。
#[tauri::command]
pub fn save_pasted_image(
    app: AppHandle,
    bytes: Vec<u8>,
    extension: String,
) -> Result<String, String> {
    let ext = match extension.as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" => extension.as_str(),
        _ => "png",
    };
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("获取应用数据目录失败：{e}"))?
        .join("attachments");
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建附件目录失败：{e}"))?;
    let path = dir.join(format!("paste-{}.{ext}", unix_timestamp_millis()?));
    std::fs::write(&path, bytes).map_err(|e| format!("写入图片失败：{e}"))?;
    Ok(path.to_string_lossy().to_string())
}

/// 会话级 MCP 开关（变更-11）：设置停用名单，下一轮启动 CLI 时真实生效。
#[tauri::command]
pub async fn set_session_mcp_disabled(
    store: State<'_, SessionStore>,
    handle_id: String,
    disabled: Vec<String>,
) -> Result<(), String> {
    let actor = store
        .sessions
        .lock()
        .map_err(|_| "会话表锁中毒".to_string())?
        .get(&handle_id)
        .cloned()
        .ok_or_else(|| format!("找不到会话：{handle_id}"))?;
    actor.set_disabled_mcp(disabled).await
}

/// Permission Ledger 结构化规则：读取时顺带完成旧 always-allow 清单的幂等迁移。
#[tauri::command]
pub fn get_permission_rules(
    history_store: State<'_, SessionHistoryStore>,
) -> Result<Vec<crate::permissions::PermissionRule>, String> {
    history_store.migrate_legacy_always_allow_rules()
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateDenyRuleInput {
    engine: Option<String>,
    capability: crate::permissions::Capability,
    operation: Option<String>,
    resource_pattern: Option<String>,
    project_root: Option<String>,
}

fn normalize_manual_deny_operation(
    capability: &crate::permissions::Capability,
    operation: Option<String>,
) -> Option<String> {
    operation
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .and_then(|value| {
            if capability == &crate::permissions::Capability::ProcessExec {
                crate::permissions::command_executable_token(&value).map(str::to_string)
            } else {
                Some(value)
            }
        })
}

/// 创建显式 Deny。凡进入 Helm Permission Kernel 的动作都不能用 Allow 覆盖该规则。
#[tauri::command]
pub fn create_permission_deny_rule(
    history_store: State<'_, SessionHistoryStore>,
    input: CreateDenyRuleInput,
) -> Result<Vec<crate::permissions::PermissionRule>, String> {
    if let Some(engine) = input.engine.as_deref() {
        if !matches!(engine, "claude-code" | "codex") {
            return Err(format!("不支持的引擎：{engine}"));
        }
    }
    let project_root = input
        .project_root
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .map(|value| {
            let path = std::path::PathBuf::from(&value);
            if !path.is_dir() {
                return Err(format!("项目目录不存在：{value}"));
            }
            path.canonicalize()
                .map(|path| path.to_string_lossy().to_string())
                .map_err(|error| format!("无法规范化项目目录：{error}"))
        })
        .transpose()?;
    let scope = if project_root.is_some() {
        crate::permissions::PermissionScope::Project
    } else {
        crate::permissions::PermissionScope::Global
    };
    let now = crate::util::now_millis();
    let operation = normalize_manual_deny_operation(&input.capability, input.operation);
    let rule = crate::permissions::PermissionRule {
        id: format!("manual-deny-{now}-{:016x}", rand::random::<u64>()),
        principal: "main-agent".to_string(),
        effect: crate::permissions::PermissionEffect::Deny,
        scope,
        scope_binding: crate::permissions::PermissionScopeBinding {
            project_root,
            ..Default::default()
        },
        engine: input.engine,
        capability: input.capability,
        operation,
        resource_pattern: input
            .resource_pattern
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty()),
        created_at: now,
        expires_at: None,
        max_uses: None,
        uses: 0,
    };
    history_store.save_permission_rule(&rule)?;
    history_store.list_permission_rules()
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionRuleRemovalResult {
    rules: Vec<crate::permissions::PermissionRule>,
    revocation_too_late_count: usize,
}

/// 按 rule id 撤销，后端同时清理旧兼容值，避免下次迁移把授权复活。
#[tauri::command]
pub fn remove_permission_rule(
    history_store: State<'_, SessionHistoryStore>,
    rule_id: String,
) -> Result<PermissionRuleRemovalResult, String> {
    let revocation_too_late_count =
        history_store.remove_permission_rule_with_legacy_compat(&rule_id)?;
    Ok(PermissionRuleRemovalResult {
        rules: history_store.list_permission_rules()?,
        revocation_too_late_count,
    })
}

#[tauri::command]
pub fn get_permission_audit_summary(
    history_store: State<'_, SessionHistoryStore>,
) -> Result<crate::sessions::PermissionAuditSummary, String> {
    history_store.permission_audit_summary()
}

#[tauri::command]
pub fn clear_permission_audit(
    history_store: State<'_, SessionHistoryStore>,
) -> Result<crate::sessions::PermissionAuditSummary, String> {
    history_store.clear_permission_audit()?;
    history_store.permission_audit_summary()
}

#[tauri::command]
pub fn export_permission_audit(
    app: AppHandle,
    history_store: State<'_, SessionHistoryStore>,
    include_resources: bool,
) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::{DialogExt, FilePath};
    let Some(path) = app
        .dialog()
        .file()
        .set_file_name("helm-permission-audit.json")
        .add_filter("JSON", &["json"])
        .blocking_save_file()
    else {
        return Ok(None);
    };
    let path = match path {
        FilePath::Path(path) => path,
        FilePath::Url(url) => return Err(format!("不支持的审计导出路径：{url}")),
    };
    let content = history_store.export_permission_audit_json(include_resources)?;
    std::fs::write(&path, content).map_err(|error| format!("写入权限审计导出失败：{error}"))?;
    Ok(Some(path.to_string_lossy().to_string()))
}

/// 回溯到某个检查点：还原文件 + 标记回滚 + 重建 Agent 上下文（P2-5 方案 A）
#[tauri::command]
pub async fn restore_checkpoint(
    app: AppHandle,
    store: State<'_, SessionStore>,
    history_store: State<'_, SessionHistoryStore>,
    checkpoint_id: String,
) -> Result<(), String> {
    use crate::snapshots::SnapshotStore;
    use std::path::PathBuf;

    let checkpoint = history_store
        .get_checkpoint(&checkpoint_id)?
        .ok_or_else(|| format!("找不到检查点：{checkpoint_id}"))?;
    if !checkpoint.restorable || checkpoint.file_count == 0 {
        return Err(format!(
            "该检查点不可恢复：{}",
            checkpoint.reason.as_deref().unwrap_or("缺少有效文件快照")
        ));
    }

    let snapshots_dir: PathBuf = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("获取应用数据目录失败：{e}"))?
        .join("snapshots");

    let snapshot_store = SnapshotStore::new(snapshots_dir);
    let snapshot = snapshot_store.load(&checkpoint.snapshot_ref)?;
    if snapshot.files.is_empty() || snapshot.files.len() as u64 != checkpoint.file_count {
        return Err("该检查点不可恢复：文件快照数量与记录不一致".to_string());
    }
    let session = history_store.get_session(&checkpoint.session_id)?;
    let normalize = |value: &str| {
        value
            .replace('\\', "/")
            .trim_start_matches("//?/")
            .trim_end_matches('/')
            .to_ascii_lowercase()
    };
    let cwd = normalize(&session.summary.cwd);
    for file in &snapshot.files {
        let path = normalize(&file.path);
        let file_name = path
            .rsplit('/')
            .next()
            .unwrap_or_default()
            .trim_end_matches('.');
        let device = path == "/dev/null"
            || path.starts_with("//./")
            || matches!(
                file_name,
                "nul"
                    | "con"
                    | "prn"
                    | "aux"
                    | "clock$"
                    | "com1"
                    | "com2"
                    | "com3"
                    | "com4"
                    | "com5"
                    | "com6"
                    | "com7"
                    | "com8"
                    | "com9"
                    | "lpt1"
                    | "lpt2"
                    | "lpt3"
                    | "lpt4"
                    | "lpt5"
                    | "lpt6"
                    | "lpt7"
                    | "lpt8"
                    | "lpt9"
            );
        if device || (path != cwd && !path.starts_with(&format!("{cwd}/"))) {
            return Err("该检查点不可恢复：快照包含设备路径或工作区外文件".to_string());
        }
    }
    snapshot_store.restore_files(&snapshot)?;

    history_store.revert_messages_after(&checkpoint.session_id, checkpoint.ts)?;
    // 回溯语义对齐（P2-5）：检查点之后的消息不再进入 Agent 上下文，
    // 并作废旧 CLI 会话 id——之后不 `--resume`，改用截断历史重新开场。
    history_store.clear_cli_session(&checkpoint.session_id)?;
    reset_live_session_context(&store, &history_store, &checkpoint.session_id).await?;

    Ok(())
}

/// 撤销回溯：恢复标记；Agent 上下文继续用重建模式（旧 CLI 会话已作废，全量历史重新开场）。
/// 按内部句柄定位会话——回溯已把 cli_session_id 置空，不能再用它解析。
#[tauri::command]
pub async fn undo_revert(
    store: State<'_, SessionStore>,
    history_store: State<'_, SessionHistoryStore>,
    handle_id: String,
) -> Result<(), String> {
    let history_session_id = store.history_session_id_for_handle(&handle_id)?;
    history_store.unrevert_messages(&history_session_id)?;
    reset_live_session_context(&store, &history_store, &history_session_id).await
}

/// 把某个历史会话对应的所有运行中句柄的上下文重置为「未回滚消息」的截断历史
async fn reset_live_session_context(
    store: &SessionStore,
    history_store: &SessionHistoryStore,
    history_session_id: &str,
) -> Result<(), String> {
    let detail = history_store.get_session(history_session_id)?;
    let truncated: Vec<crate::sessions::SessionMessage> = detail
        .messages
        .into_iter()
        .filter(|message| !message.reverted)
        .collect();
    let handles: Vec<String> = store
        .history_session_ids
        .lock()
        .map_err(|_| "会话历史映射锁中毒".to_string())?
        .iter()
        .filter(|(_, history_id)| {
            history_id.as_str() == history_session_id
                || history_id.as_str() == detail.summary.id.as_str()
        })
        .map(|(handle, _)| handle.clone())
        .collect();
    let actors = {
        let sessions = store
            .sessions
            .lock()
            .map_err(|_| "会话表锁中毒".to_string())?;
        handles
            .into_iter()
            .filter_map(|handle| sessions.get(&handle).cloned())
            .collect::<Vec<_>>()
    };
    for actor in actors {
        // 会话可能刚结束，重置失败不阻断回溯本身
        let _ = actor.reset_context(truncated.clone()).await;
    }
    Ok(())
}

fn engine_id_from_str(engine: &str) -> Result<EngineId, String> {
    match engine {
        "claude-code" => Ok(EngineId::ClaudeCode),
        "codex" => Ok(EngineId::Codex),
        other => Err(format!("暂不支持的引擎：{other}")),
    }
}

fn engine_id_to_string(engine: EngineId) -> String {
    match engine {
        EngineId::ClaudeCode => "claude-code".to_string(),
        EngineId::Codex => "codex".to_string(),
    }
}

pub(crate) fn sync_history_model_prices(
    history_store: &SessionHistoryStore,
    provider_store: &ProviderStore<KeyringSecretStore>,
    config: &AppConfig,
) {
    for model in &config.models {
        if let Ok(Some(profile)) = provider_store.model_pricing_profile(config, model) {
            history_store.set_model_pricing_profile(&model.provider_id, &model.id, profile);
        }
    }
}

// 用量统计命令
#[tauri::command]
pub async fn get_usage_stats(
    history_store: State<'_, SessionHistoryStore>,
    days: u32,
) -> Result<crate::sessions::UsageStats, String> {
    history_store.get_usage_stats(days)
}

/// S4 冻结的统一分组聚合命令：dimension = model / engine / provider
#[tauri::command]
pub async fn get_usage_breakdown(
    history_store: State<'_, SessionHistoryStore>,
    days: u32,
    dimension: crate::sessions::UsageBreakdownDimension,
) -> Result<Vec<crate::sessions::UsageBreakdownRow>, String> {
    history_store.get_usage_breakdown(days, dimension)
}

#[tauri::command]
pub async fn get_daily_usage(
    history_store: State<'_, SessionHistoryStore>,
    days: u32,
) -> Result<Vec<crate::sessions::DailyUsage>, String> {
    history_store.get_daily_usage(days)
}

#[tauri::command]
pub async fn get_top_sessions(
    history_store: State<'_, SessionHistoryStore>,
    days: u32,
    limit: usize,
) -> Result<Vec<crate::sessions::TopSession>, String> {
    history_store.get_top_sessions(days, limit)
}

#[tauri::command]
pub async fn get_budget(
    history_store: State<'_, SessionHistoryStore>,
) -> Result<crate::sessions::Budget, String> {
    history_store.get_budget()
}

#[tauri::command]
pub async fn set_budget(
    app: AppHandle,
    history_store: State<'_, SessionHistoryStore>,
    monthly_limit: f64,
    alert_at_80: bool,
    stop_at_100: bool,
) -> Result<(), String> {
    history_store.set_budget(monthly_limit, alert_at_80, stop_at_100)?;
    // 预算变化立即反映到托盘（P3-2）
    crate::tray::refresh_usage(&app);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        apply_approval_response, build_side_query_prompt, codex_search_catalog_plan,
        codex_subscription_models_from_response, ensure_budget_allows_turn,
        ensure_discovered_binding_model, ensure_ledger_fits_context_window,
        ensure_pricing_allows_turn, normalize_manual_deny_operation, requested_model_for_binding,
        resolve_binding_model, resolve_routed_effort, resolve_turn_pricing_basis,
        rollback_failed_session_creation, search_workspace_files, subscription_profile_for_binding,
        ApprovalLedger, CodexSearchCatalogPlan,
    };
    use crate::adapter::ApprovalDecision;
    use crate::capability_registry::{CapabilityIdentity, CapabilitySet, EngineCapabilitySnapshot};
    use crate::protocol::{AgentEvent, EngineId, Role};
    use crate::providers::{
        AppConfig, AuthMethod, BindingConfig, KeyringSecretStore, ModelConfig, PriceSource,
        Protocol, ProviderConfig, ProviderKind, ProviderStore,
    };
    use crate::reasoning::ReasoningEffort;
    use crate::sessions::{
        Budget, NewSessionRecord, SessionDetail, SessionHistoryStore, SessionMessage,
        SessionSummary,
    };

    #[test]
    fn branch_fork_outcome_serializes_camel_case_fields() {
        // 回归守卫（2026-09-04 埋点实证）：rename_all 只作用变体名，变体字段需要
        // rename_all_fields——此前 session_id 以 snake_case 下发，前端读 sessionId
        // 得 undefined，分叉自动跳转静默失效。
        let outcome = super::BranchForkOutcome::Lossless {
            session_id: "session-abc".to_string(),
        };
        let json = serde_json::to_value(&outcome).expect("序列化 BranchForkOutcome 失败");
        let lossless = json.as_object().expect("lossless 应为对象");
        assert_eq!(
            lossless.get("mode").and_then(|v| v.as_str()),
            Some("lossless"),
            "变体标签应为 lossless"
        );
        assert_eq!(
            lossless.get("sessionId").and_then(|v| v.as_str()),
            Some("session-abc"),
            "变体字段必须 camelCase（sessionId），实际：{json}"
        );
        assert!(
            lossless.get("session_id").is_none(),
            "不得残留 snake_case 的 session_id 键，实际：{json}"
        );
    }

    #[test]
    fn codex_search_catalog_only_disables_responses_lite_for_the_target_model() {
        let raw = serde_json::to_vec(&serde_json::json!({
            "models": [
                {
                    "slug": "gpt-target",
                    "supports_search_tool": true,
                    "use_responses_lite": true,
                    "web_search_tool_type": "text_and_image",
                    "tool_mode": "code_mode_only",
                    "unknown_future_field": {"kept": true}
                },
                {
                    "slug": "gpt-other",
                    "supports_search_tool": true,
                    "use_responses_lite": true,
                    "web_search_tool_type": "text"
                }
            ],
            "catalog_future_field": [1, 2, 3]
        }))
        .unwrap();
        let CodexSearchCatalogPlan::HostedResponsesCompatibility(encoded) =
            codex_search_catalog_plan(&raw, "gpt-target").unwrap()
        else {
            panic!("Lite hosted model should require a compatibility catalog");
        };
        let catalog: serde_json::Value = serde_json::from_slice(&encoded).unwrap();
        let models = catalog["models"].as_array().unwrap();
        assert_eq!(models[0]["use_responses_lite"], false);
        assert_eq!(models[0]["unknown_future_field"]["kept"], true);
        assert_eq!(models[1]["use_responses_lite"], true);
        assert_eq!(
            catalog["catalog_future_field"],
            serde_json::json!([1, 2, 3])
        );
    }

    #[test]
    fn codex_search_catalog_degrades_unknown_model_and_fails_closed_on_drifted_contract() {
        let hosted = serde_json::to_vec(&serde_json::json!({
            "models": [{
                "slug": "gpt-hosted",
                "supports_search_tool": true,
                "use_responses_lite": false,
                "web_search_tool_type": "text"
            }]
        }))
        .unwrap();
        assert_eq!(
            codex_search_catalog_plan(&hosted, "gpt-hosted").unwrap(),
            CodexSearchCatalogPlan::HostedResponses
        );
        // 未知模型（自定义服务商的模型）能力未知：允许运行时尝试原生 WebSearch，不硬禁
        assert_eq!(
            codex_search_catalog_plan(&hosted, "missing").unwrap(),
            CodexSearchCatalogPlan::Unknown
        );

        let unavailable = serde_json::to_vec(&serde_json::json!({
            "models": [{
                "slug": "gpt-offline",
                "supports_search_tool": false,
                "use_responses_lite": true
            }]
        }))
        .unwrap();
        assert_eq!(
            codex_search_catalog_plan(&unavailable, "gpt-offline").unwrap(),
            CodexSearchCatalogPlan::Unavailable
        );
        assert!(codex_search_catalog_plan(br#"{"models": {}}"#, "gpt")
            .unwrap_err()
            .starts_with("[codex_search_catalog_schema]"));
    }
    #[test]
    fn provider_declared_override_only_for_bound_custom_models() {
        use crate::capability_registry::{CapabilityEvidence, CapabilitySupport};
        let env = vec![(
            "OPENAI_BASE_URL".to_string(),
            "https://gw.example/v1".to_string(),
        )];
        let not_listed = CapabilityEvidence::new(
            CapabilitySupport::Unsupported,
            "codex_model_list",
            "codex_model_not_listed",
        );
        let mut bound = not_listed.clone();
        super::apply_provider_declared_model_override(
            &mut bound,
            "codex",
            &env,
            "m2/glm-5.2-openai",
            "m2/glm-5.2-openai",
        );
        assert_eq!(bound.support, CapabilitySupport::Supported);

        let mut other_model = CapabilityEvidence::new(
            CapabilitySupport::Unsupported,
            "codex_model_list",
            "codex_model_not_listed",
        );
        super::apply_provider_declared_model_override(
            &mut other_model,
            "codex",
            &env,
            "m2/glm-5.2-openai",
            "other",
        );
        assert_eq!(other_model.support, CapabilitySupport::Unsupported);

        let mut official_no_base_url = CapabilityEvidence::new(
            CapabilitySupport::Unsupported,
            "codex_model_list",
            "codex_model_not_listed",
        );
        super::apply_provider_declared_model_override(
            &mut official_no_base_url,
            "codex",
            &[],
            "m2/glm-5.2-openai",
            "m2/glm-5.2-openai",
        );
        assert_eq!(official_no_base_url.support, CapabilitySupport::Unsupported);
    }

    use crate::settings::AppSettings;
    use crate::subscription_profiles::SubscriptionProfileStore;
    use crate::turn_supervisor::TurnSupervisor;
    use std::sync::Mutex;

    #[test]
    fn binding_live_routes_missing_preferences_to_primary_and_preserves_valid_models() {
        let binding = BindingConfig {
            engine_id: "codex".into(),
            provider_id: "provider-new".into(),
            primary_model: "gpt-primary".into(),
            fast_model: None,
            assistant_model_id: None,
            reasoning_effort: None,
            thinking_enabled: None,
            context_1m: None,
            revision: 9,
        };
        let model = |id: &str, provider_id: &str, enabled| ModelConfig {
            id: id.into(),
            provider_id: provider_id.into(),
            display_name: id.into(),
            input_price_per_mtok: 0.0,
            output_price_per_mtok: 0.0,
            cached_input_price_per_mtok: None,
            price_source: None,
            enabled,
            context_window: None,
            capabilities: None,
        };
        let config = AppConfig {
            providers: Vec::new(),
            models: vec![
                model("gpt-primary", "provider-new", true),
                model("gpt-valid", "provider-new", true),
                model("gpt-disabled", "provider-new", false),
                model("gpt-old", "provider-old", true),
            ],
            engines: Vec::new(),
            bindings: vec![binding.clone()],
            default_engine: "codex".into(),
            default_model: "gpt-primary".into(),
        };
        assert_eq!(
            resolve_binding_model(&config, &binding, "gpt-valid"),
            "gpt-valid"
        );
        assert_eq!(
            resolve_binding_model(&config, &binding, "gpt-disabled"),
            "gpt-primary"
        );
        assert_eq!(
            resolve_binding_model(&config, &binding, "gpt-old"),
            "gpt-primary"
        );
        assert_eq!(
            requested_model_for_binding(None, None, &binding),
            "gpt-primary",
            "无偏好时必须读当前 Binding，不能读历史 session.model"
        );
        assert_eq!(
            requested_model_for_binding(None, Some("gpt-valid"), &binding),
            "gpt-valid"
        );
        assert_eq!(
            requested_model_for_binding(Some("gpt-explicit"), Some("gpt-valid"), &binding),
            "gpt-explicit"
        );
    }

    #[test]
    fn binding_live_falls_unsupported_effort_back_to_auto() {
        let mut capabilities = CapabilitySet::unknown("test");
        capabilities.reasoning_efforts = vec![ReasoningEffort::Auto, ReasoningEffort::High];
        let snapshot = EngineCapabilitySnapshot {
            id: "capability-test".into(),
            identity: CapabilityIdentity {
                engine_id: "codex".into(),
                adapter_version: "test".into(),
                binary_identity: "test".into(),
                engine_profile_digest: "test".into(),
                provider_launch_profile_ref: "test".into(),
                provider_launch_profile_digest: "test".into(),
                launch_profile_identity: "test".into(),
                model_capability_key: "gpt".into(),
            },
            capabilities,
            probe_kind: "test".into(),
            probed_at: 1,
        };
        assert_eq!(
            resolve_routed_effort(&snapshot, ReasoningEffort::High),
            ReasoningEffort::High
        );
        assert_eq!(
            resolve_routed_effort(&snapshot, ReasoningEffort::Max),
            ReasoningEffort::Auto
        );
    }

    #[test]
    fn provider_switch_blocks_full_ledger_when_the_target_window_is_too_small() {
        let error =
            ensure_ledger_fits_context_window(&[], &"历史".repeat(1_000), Some(100)).unwrap_err();
        assert!(error.starts_with("[ledger_context_window_insufficient]"));
        ensure_ledger_fits_context_window(&[], "short", Some(10_000)).unwrap();
        ensure_ledger_fits_context_window(&[], &"历史".repeat(1_000), None).unwrap();
    }

    #[test]
    fn codex_subscription_catalog_keeps_visible_account_models() {
        let response = serde_json::json!({
            "data": [
                {"model": "hidden", "hidden": true},
                {"model": "gpt-live", "displayName": "GPT Live", "isDefault": true,
                 "supportedReasoningEfforts": [{"reasoningEffort": "low"}]}
            ]
        });
        let models = codex_subscription_models_from_response("chatgpt", &response).unwrap();
        assert_eq!(models.len(), 1);
        assert!(models[0].0);
        assert_eq!(models[0].1.id, "gpt-live");
        assert!(models[0].1.display_name.contains("账号默认"));
        assert_eq!(
            models[0].1.capabilities.as_deref(),
            Some(["reasoning:low".to_string()].as_slice())
        );
    }

    #[test]
    fn codex_subscription_binding_rejects_stale_model() {
        let response = serde_json::json!({"data": [{"model": "gpt-live"}]});
        let models = codex_subscription_models_from_response("chatgpt", &response)
            .unwrap()
            .into_iter()
            .map(|(_, model)| model)
            .collect::<Vec<_>>();
        ensure_discovered_binding_model(&models, "gpt-live", "主模型").unwrap();
        let error = ensure_discovered_binding_model(&models, "gpt-stale", "主模型").unwrap_err();
        assert!(error.starts_with("[model_unavailable]"));
    }

    #[test]
    fn runtime_selects_helm_profile_only_for_subscription_bindings() {
        let root = std::env::temp_dir().join(format!(
            "helm-command-subscription-profile-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        ));
        let profiles = SubscriptionProfileStore::new(root.clone());
        let binding = BindingConfig {
            engine_id: "codex".to_string(),
            provider_id: "openai".to_string(),
            primary_model: "gpt-test".to_string(),
            fast_model: None,
            assistant_model_id: None,
            reasoning_effort: None,
            thinking_enabled: None,
            context_1m: None,
            revision: 0,
        };
        let provider = |kind| ProviderConfig {
            id: "openai".to_string(),
            name: "OpenAI".to_string(),
            kind,
            base_url: String::new(),
            key_ref: None,
            ready: true,
            last_test: None,
            protocol: Protocol::OpenAiResponses,
            auth_method: AuthMethod::OAuth,
            access_type: None,
            role_models: None,
            last_sync_at: None,
        };
        let config = |kind| AppConfig {
            providers: vec![provider(kind)],
            models: Vec::new(),
            engines: Vec::new(),
            bindings: vec![binding.clone()],
            default_engine: "codex".to_string(),
            default_model: "gpt-test".to_string(),
        };

        let subscription = subscription_profile_for_binding(
            &profiles,
            &config(ProviderKind::Subscription),
            &binding,
        )
        .unwrap();
        assert_eq!(
            subscription,
            Some(root.join("cli-profiles/codex-subscription"))
        );
        assert_eq!(
            subscription_profile_for_binding(&profiles, &config(ProviderKind::Api), &binding)
                .unwrap(),
            None
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn manual_process_deny_normalizes_to_the_runtime_executable_token() {
        assert_eq!(
            normalize_manual_deny_operation(
                &crate::permissions::Capability::ProcessExec,
                Some("cargo test --workspace".to_string()),
            )
            .as_deref(),
            Some("cargo")
        );
        assert_eq!(
            normalize_manual_deny_operation(
                &crate::permissions::Capability::ProcessExec,
                Some("\"C:\\Program Files\\Tool\\tool.exe\" --check".to_string()),
            )
            .as_deref(),
            Some("C:\\Program Files\\Tool\\tool.exe")
        );
        assert_eq!(
            normalize_manual_deny_operation(
                &crate::permissions::Capability::McpInvoke,
                Some("mcp__server__tool".to_string()),
            )
            .as_deref(),
            Some("mcp__server__tool")
        );
        assert_eq!(
            normalize_manual_deny_operation(
                &crate::permissions::Capability::ProcessExec,
                Some("   ".to_string()),
            ),
            None
        );
    }

    #[test]
    fn new_session_history_exists_before_runtime_bootstrap_and_rolls_back_on_failure() {
        let root = std::env::temp_dir().join(format!(
            "helm-command-create-order-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let store = SessionHistoryStore::new(root.join("sessions.sqlite"));
        for id in ["previous", "new"] {
            store
                .create_session(NewSessionRecord {
                    id: id.to_string(),
                    engine: EngineId::ClaudeCode,
                    model: "claude-test".to_string(),
                    cwd: root.to_string_lossy().to_string(),
                    created_at: 1,
                })
                .unwrap();
        }
        store.set_active_session("previous").unwrap();
        store.set_safe_permission_profile("new", "auto").unwrap();
        store.set_session_provider("new", "provider-test").unwrap();

        // Runtime constructors call latest_turn_epoch immediately. The row and its
        // routing metadata must already exist before they are invoked.
        assert_eq!(store.latest_turn_epoch("new").unwrap(), 0);
        assert_eq!(store.session_provider_id("new").unwrap(), "provider-test");

        let error = rollback_failed_session_creation(
            &store,
            "new",
            Some("previous"),
            None,
            "runtime bootstrap failed".to_string(),
        );
        assert_eq!(error, "runtime bootstrap failed");
        assert!(store.get_session("new").is_err());
        assert_eq!(
            store.active_session().unwrap().unwrap().summary.id,
            "previous"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn failed_runtime_bootstrap_removes_only_the_project_folder_created_for_that_attempt() {
        let root = std::env::temp_dir().join(format!(
            "helm-command-folder-rollback-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let store = SessionHistoryStore::new(root.join("history.sqlite"));

        let (_, created_folder_id) = store
            .create_session_for_cwd_tracked(
                NewSessionRecord {
                    id: "new".to_string(),
                    engine: EngineId::ClaudeCode,
                    model: "claude-test".to_string(),
                    cwd: root.to_string_lossy().to_string(),
                    created_at: 1,
                },
                None,
            )
            .unwrap();
        assert!(created_folder_id.is_some());

        let error = rollback_failed_session_creation(
            &store,
            "new",
            None,
            created_folder_id.as_deref(),
            "runtime bootstrap failed".to_string(),
        );
        assert_eq!(error, "runtime bootstrap failed");
        assert_eq!(store.list_folders().unwrap().len(), 1);

        let (_, original_folder_id) = store
            .create_session_for_cwd_tracked(
                NewSessionRecord {
                    id: "seed".to_string(),
                    engine: EngineId::ClaudeCode,
                    model: "claude-test".to_string(),
                    cwd: root.to_string_lossy().to_string(),
                    created_at: 2,
                },
                None,
            )
            .unwrap();
        assert!(original_folder_id.is_some());
        store.delete_session("seed").unwrap();

        let (_, reused_folder_id) = store
            .create_session_for_cwd_tracked(
                NewSessionRecord {
                    id: "retry".to_string(),
                    engine: EngineId::ClaudeCode,
                    model: "claude-test".to_string(),
                    cwd: root.to_string_lossy().to_string(),
                    created_at: 3,
                },
                None,
            )
            .unwrap();
        assert!(reused_folder_id.is_none());
        rollback_failed_session_creation(
            &store,
            "retry",
            None,
            reused_folder_id.as_deref(),
            "runtime bootstrap failed".to_string(),
        );
        assert_eq!(store.list_folders().unwrap().len(), 2);

        let _ = std::fs::remove_dir_all(root);
    }

    #[derive(Default)]
    struct ResolveFailLedger {
        transitions: Mutex<Vec<String>>,
    }

    impl ApprovalLedger for ResolveFailLedger {
        fn mark_applying(&self, _: &str, _: &str, _: &str) -> Result<(), String> {
            self.transitions
                .lock()
                .unwrap()
                .push("applying".to_string());
            Ok(())
        }

        fn resolve(&self, _: &str, _: &str, _: &str) -> Result<(), String> {
            self.transitions.lock().unwrap().push("resolve".to_string());
            Err("数据库写入失败".to_string())
        }

        fn fail(&self, _: &str, _: &str, error: &str) -> Result<(), String> {
            self.transitions
                .lock()
                .unwrap()
                .push(format!("failed:{error}"));
            Ok(())
        }
    }

    fn approval_store(name: &str) -> (std::path::PathBuf, SessionHistoryStore) {
        let root = std::env::temp_dir().join(format!(
            "helm-command-approval-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let store = SessionHistoryStore::new(root.join("sessions.sqlite"));
        store
            .create_session(NewSessionRecord {
                id: "history-1".to_string(),
                engine: EngineId::ClaudeCode,
                model: "claude-test".to_string(),
                cwd: root.to_string_lossy().to_string(),
                created_at: 1,
            })
            .unwrap();
        TurnSupervisor::new(store.clone()).begin(
            "history-1",
            "turn-approval-test",
            1,
            "build",
            "standard",
        );
        store
            .record_event_for_session_in_turn(
                "history-1",
                Some("turn-approval-test"),
                &AgentEvent::ApprovalRequest {
                    session_id: "cli-1".to_string(),
                    id: "approval-1".to_string(),
                    action: "Write".to_string(),
                    detail: "写文件".to_string(),
                    input: None,
                    available_decisions: vec![],
                    persistent_label: None,
                    matcher_summary: None,
                },
            )
            .unwrap();
        (root, store)
    }

    #[tokio::test]
    async fn approval_ledger_transitions_applying_to_resolved_after_manager_ack() {
        let (root, store) = approval_store("resolved");

        apply_approval_response(
            &store,
            "history-1",
            "approval-1",
            ApprovalDecision::Always,
            async { Ok(()) },
        )
        .await
        .unwrap();

        let approval = store.get_session("history-1").unwrap().approvals.remove(0);
        assert_eq!(approval.status, "resolved");
        assert_eq!(approval.decision.as_deref(), Some("always"));
        assert!(approval.error.is_none());
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn approval_ledger_transitions_applying_to_failed_on_manager_error() {
        let (root, store) = approval_store("failed");

        let error = apply_approval_response(
            &store,
            "history-1",
            "approval-1",
            ApprovalDecision::Deny,
            async { Err("manager 拒绝应用".to_string()) },
        )
        .await
        .expect_err("manager 失败必须返回给命令调用方");

        assert_eq!(error, "manager 拒绝应用");
        let approval = store.get_session("history-1").unwrap().approvals.remove(0);
        assert_eq!(approval.status, "failed");
        assert_eq!(approval.decision.as_deref(), Some("deny"));
        assert_eq!(approval.error.as_deref(), Some("manager 拒绝应用"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn manager_ack_then_resolve_failure_is_compensated_with_explicit_audit_error() {
        let ledger = ResolveFailLedger::default();

        let error = apply_approval_response(
            &ledger,
            "history-1",
            "approval-1",
            ApprovalDecision::Allow,
            async { Ok(()) },
        )
        .await
        .expect_err("resolved 落库失败必须向调用方返回错误");

        assert!(error.contains("CLI 已接受"));
        assert!(error.contains("可能已经开始"));
        let transitions = ledger.transitions.lock().unwrap();
        assert_eq!(transitions[0], "applying");
        assert_eq!(transitions[1], "resolve");
        assert!(transitions[2].starts_with("failed:"));
    }

    #[test]
    fn existing_session_cannot_start_turn_after_hard_budget_limit() {
        let budget = Budget {
            monthly_limit: 10.0,
            alert_at_80: true,
            stop_at_100: true,
            current_month_cost: 10.0,
            percentage: 100.0,
        };

        let error = ensure_budget_allows_turn(&budget).expect_err("达到硬上限时必须拒绝新轮次");

        assert!(error.contains("$10.00 / $10.00"));
        assert!(error.contains("用量与成本"));
    }

    #[test]
    fn zero_monthly_limit_does_not_block_turns() {
        let budget = Budget {
            monthly_limit: 0.0,
            alert_at_80: true,
            stop_at_100: true,
            current_month_cost: 42.0,
            percentage: 0.0,
        };

        ensure_budget_allows_turn(&budget).expect("0 表示未设置预算上限");
    }

    #[test]
    fn strict_budget_blocks_unknown_price_but_allows_builtin_and_warn_modes() {
        let root =
            std::env::temp_dir().join(format!("helm-command-pricing-{}", std::process::id()));
        let provider_store = ProviderStore::new(root.join("providers.json"), KeyringSecretStore);
        let provider = ProviderConfig {
            id: "openai-gateway".to_string(),
            name: "OpenAI 网关".to_string(),
            kind: ProviderKind::Api,
            base_url: "https://example.invalid/v1".to_string(),
            key_ref: None,
            ready: false,
            last_test: None,
            protocol: Protocol::OpenAiResponses,
            auth_method: AuthMethod::ApiKey,
            access_type: None,
            role_models: None,
            last_sync_at: None,
        };
        let provider_id = provider.id.clone();
        let model = |id: &str| ModelConfig {
            id: id.to_string(),
            provider_id: provider_id.clone(),
            display_name: id.to_string(),
            input_price_per_mtok: 0.0,
            output_price_per_mtok: 0.0,
            cached_input_price_per_mtok: None,
            price_source: Some(PriceSource::Unknown),
            enabled: true,
            context_window: None,
            capabilities: None,
        };
        let models = vec![model("unknown-model"), model("gpt-5.6-sol")];
        let config = AppConfig {
            providers: vec![provider],
            models,
            engines: Vec::new(),
            bindings: Vec::new(),
            default_engine: String::new(),
            default_model: String::new(),
        };
        let unknown_basis =
            resolve_turn_pricing_basis(&provider_store, &config, "openai-gateway", "unknown-model")
                .unwrap();
        assert!(unknown_basis.profile.is_none());
        let catalog_basis =
            resolve_turn_pricing_basis(&provider_store, &config, "openai-gateway", "gpt-5.6-sol")
                .unwrap();
        assert!(catalog_basis.profile.is_some());
        let budget = Budget {
            monthly_limit: 10.0,
            alert_at_80: true,
            stop_at_100: false,
            current_month_cost: 0.0,
            percentage: 0.0,
        };
        let mut settings = AppSettings::default();
        settings.general.pricing_unknown_policy = "block".to_string();

        let error = ensure_pricing_allows_turn(
            &budget,
            &settings,
            &provider_store,
            &config,
            "openai-gateway",
            "unknown-model",
        )
        .expect_err("严格预算必须阻止缺价模型");
        assert!(error.contains("严格预算模式"));

        ensure_pricing_allows_turn(
            &budget,
            &settings,
            &provider_store,
            &config,
            "openai-gateway",
            "gpt-5.6-sol",
        )
        .expect("内置目录中的模型应允许发送");

        settings.general.pricing_unknown_policy = "warn".to_string();
        ensure_pricing_allows_turn(
            &budget,
            &settings,
            &provider_store,
            &config,
            "openai-gateway",
            "unknown-model",
        )
        .expect("提醒模式不应阻断发送");
    }

    fn side_query_detail(cwd: &str, messages: Vec<SessionMessage>) -> SessionDetail {
        SessionDetail {
            summary: SessionSummary {
                id: "s-test".to_string(),
                cli_session_id: None,
                title: "测试".to_string(),
                engine: EngineId::ClaudeCode,
                model: "claude-opus-4.7".to_string(),
                cwd: cwd.to_string(),
                status: crate::sessions::SessionStatus::Active,
                message_count: messages.len() as u32,
                input_tokens: 0,
                output_tokens: 0,
                cost_usd: 0.0,
                created_at: 0,
                updated_at: 0,
                summary: None,
                pinned: false,
                runtime_capabilities: None,
                safe_permission_profile: "standard".to_string(),
                folder_id: String::new(),
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
            messages,
            tool_calls: Vec::new(),
            checkpoints: Vec::new(),
            approvals: Vec::new(),
            turns: Vec::new(),
            session_context: Vec::new(),
            fork: None,
        }
    }

    #[test]
    fn side_query_prompt_freezes_recent_context_and_skips_reverted() {
        let detail = side_query_detail(
            "D:/work",
            vec![
                SessionMessage {
                    role: Role::User,
                    text: "帮我修登录".to_string(),
                    ts: 1,
                    reverted: false,
                    turn_id: None,
                    attachments: Vec::new(),
                },
                SessionMessage {
                    role: Role::Assistant,
                    text: "好的，正在排查".to_string(),
                    ts: 2,
                    reverted: true,
                    turn_id: None,
                    attachments: Vec::new(),
                },
                SessionMessage {
                    role: Role::Assistant,
                    text: "已定位到 token 刷新逻辑".to_string(),
                    ts: 3,
                    reverted: false,
                    turn_id: None,
                    attachments: Vec::new(),
                },
            ],
        );
        let prompt = build_side_query_prompt(&detail, "  下一步怎么做？  ").unwrap();
        assert!(prompt.contains("工作目录：D:/work"));
        assert!(prompt.contains("用户：帮我修登录"));
        assert!(prompt.contains("助手：已定位到 token 刷新逻辑"));
        assert!(!prompt.contains("正在排查"), "reverted 消息必须剔除");
        assert!(prompt.ends_with("下一步怎么做？"));
        assert!(prompt.contains("不要使用任何工具"));
    }

    #[test]
    fn side_query_prompt_falls_back_to_empty_transcript() {
        let detail = side_query_detail("D:/work", Vec::new());
        let prompt = build_side_query_prompt(&detail, "什么是 Helm？").unwrap();
        assert!(prompt.contains("（暂无对话历史）"));
        assert!(prompt.contains("什么是 Helm？"));
    }

    #[test]
    fn search_workspace_files_matches_directory_names_and_skips_target_variants() {
        let root = std::env::temp_dir().join(format!(
            "helm-command-search-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        ));
        let write = |relative: &str| {
            let path = root.join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, "x").unwrap();
        };
        // 文档目录（目录名 docs 本身不带 doc 后缀的文件名）
        write("docs/实施切片计划.md");
        write("docs/已知限制.md");
        // target-* 变体构建目录：displaydoc 产物文件名含 doc，必须被跳过
        write("target-change31/debug/deps/displaydoc-12e117d2cc5dc8bd.d");
        write("target-change30/debug/deps/displaydoc-12e117d2cc5dc8bd.d");
        // 根目录普通文件
        write("README.md");
        write("src/main.rs");

        // @docs：目录名命中，应带出目录条目本身（尾斜杠）与其下所有文件
        let docs =
            search_workspace_files(root.to_string_lossy().to_string(), "docs".to_string()).unwrap();
        assert_eq!(
            docs.len(),
            3,
            "@docs 应返回 docs/ 目录行 + 其下全部文件：{docs:?}"
        );
        assert!(
            docs.iter().any(|p| p == "docs/"),
            "目录条目必须以尾斜杠返回：{docs:?}"
        );
        assert!(docs.iter().any(|p| p == "docs/实施切片计划.md"));
        assert!(docs.iter().any(|p| p == "docs/已知限制.md"));

        // @doc：路径匹配同样命中 docs/ 文件，且绝不含 target-* 产物
        let doc =
            search_workspace_files(root.to_string_lossy().to_string(), "doc".to_string()).unwrap();
        assert!(
            doc.iter().all(|p| !p.starts_with("target-")),
            "target-* 变体产物不得出现在结果里：{doc:?}"
        );
        assert!(doc.iter().any(|p| p.starts_with("docs/")));

        // 空查询：返回全部文件与目录条目，按路径长度升序（根目录目录行最短靠前）
        let all =
            search_workspace_files(root.to_string_lossy().to_string(), String::new()).unwrap();
        assert_eq!(all.len(), 6, "4 文件 + docs/ src/ 两个目录行：{all:?}");
        assert_eq!(all[0], "src/");
        assert_eq!(all[1], "docs/");
        assert_eq!(all[2], "README.md");

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn search_workspace_files_truncates_to_30_shortest_paths() {
        let root = std::env::temp_dir().join(format!(
            "helm-command-search-limit-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        ));
        std::fs::create_dir_all(&root).unwrap();
        for i in 0..60 {
            std::fs::write(root.join(format!("file-{i:02}.md")), "x").unwrap();
        }
        let results =
            search_workspace_files(root.to_string_lossy().to_string(), String::new()).unwrap();
        // 只截断到 30 条，且按长度排序（全部等长时保留遍历顺序前 30）
        assert_eq!(results.len(), 30);
        std::fs::remove_dir_all(&root).ok();
    }
}

// ============================================================
// 扩展管理命令
// ============================================================

#[tauri::command]
pub async fn list_skills(
    engine: Option<String>,
    project_dir: Option<String>,
) -> Result<Vec<crate::extensions::Skill>, String> {
    crate::extensions::list_skills(engine, project_dir)
}

#[tauri::command]
pub async fn toggle_skill(
    skill_id: String,
    enabled: bool,
    project_dir: Option<String>,
) -> Result<(), String> {
    crate::extensions::toggle_skill(&skill_id, enabled, project_dir)
}

#[tauri::command]
pub async fn create_skill(
    request: crate::extensions::CreateSkillRequest,
    project_dir: Option<String>,
) -> Result<crate::extensions::Skill, String> {
    crate::extensions::create_skill(request, project_dir)
}

#[tauri::command]
pub async fn read_skill_source(
    skill_id: String,
    engine: String,
    project_dir: Option<String>,
) -> Result<crate::extensions::SkillSourceFile, String> {
    tokio::task::spawn_blocking(move || {
        crate::extensions::read_skill_source(&skill_id, &engine, project_dir)
    })
    .await
    .map_err(|e| format!("读取技能源码任务失败: {e}"))?
}

#[tauri::command]
pub async fn delete_skill(
    skill_id: String,
    engine: String,
    project_dir: Option<String>,
) -> Result<(), String> {
    crate::extensions::delete_skill(&skill_id, &engine, project_dir)
}

#[tauri::command]
pub async fn list_mcp_servers() -> Result<Vec<crate::extensions::McpServer>, String> {
    crate::extensions::list_mcp_servers()
}

#[tauri::command]
pub async fn set_mcp_server_enabled(name: String, enabled: bool) -> Result<(), String> {
    crate::extensions::set_mcp_server_enabled(&name, enabled)
}

#[tauri::command]
pub async fn import_mcp_servers(
    json: String,
) -> Result<Vec<crate::extensions::McpImportItemResult>, String> {
    crate::extensions::import_mcp_servers(json).await
}

#[tauri::command]
pub async fn list_mcp_credential_status(
    name: String,
) -> Result<Vec<crate::extensions::McpCredentialStatus>, String> {
    crate::extensions::list_mcp_credential_status(&name)
}

#[tauri::command]
pub async fn test_mcp_connection(
    server: crate::extensions::McpServer,
) -> Result<Vec<crate::extensions::McpTool>, String> {
    let result = crate::extensions::test_mcp_connection(&server).await;
    // 最近一次连接状态持久化（变更-05）：成功失败都记录，跨重启可见
    crate::extensions::record_mcp_status(&server.name, &result);
    result
}

#[tauri::command]
pub async fn save_mcp_server(server: crate::extensions::McpServer) -> Result<(), String> {
    // 凭证类字段值同步进系统钥匙串；被移除的旧凭证字段一并清理
    let previous_keys = crate::extensions::list_mcp_servers()
        .ok()
        .and_then(|list| {
            list.into_iter()
                .find(|existing| existing.name == server.name)
        })
        .map(|existing| crate::extensions::credential_keys_of(&existing))
        .unwrap_or_default();
    crate::extensions::save_mcp_server_with_credential_sync(&server, &previous_keys)
}

#[tauri::command]
pub async fn delete_mcp_server(name: String) -> Result<(), String> {
    crate::extensions::delete_mcp_server(&name)?;
    crate::extensions::forget_mcp_status(&name);
    Ok(())
}

#[tauri::command]
pub async fn list_subagents(
    project_dir: Option<String>,
) -> Result<Vec<crate::extensions::Subagent>, String> {
    crate::extensions::list_subagents(project_dir)
}

#[tauri::command]
pub async fn save_subagent(
    subagent: crate::extensions::Subagent,
    project_dir: Option<String>,
) -> Result<(), String> {
    crate::extensions::save_subagent(subagent, project_dir)
}

#[tauri::command]
pub async fn delete_subagent(id: String, project_dir: Option<String>) -> Result<(), String> {
    crate::extensions::delete_subagent(&id, project_dir)
}

#[tauri::command]
pub async fn list_slash_commands(
    engine: Option<String>,
    cwd: Option<String>,
) -> Result<Vec<crate::extensions::SlashCommand>, String> {
    crate::extensions::list_slash_commands(engine, cwd)
}

#[tauri::command]
pub async fn save_slash_command(
    command: crate::extensions::SlashCommand,
    project_dir: Option<String>,
) -> Result<(), String> {
    crate::extensions::save_slash_command(command, project_dir)
}

#[tauri::command]
pub async fn delete_slash_command(id: String, project_dir: Option<String>) -> Result<(), String> {
    crate::extensions::delete_slash_command(&id, project_dir)
}

#[tauri::command]
pub async fn list_hooks(
    project_dir: Option<String>,
) -> Result<Vec<crate::extensions::Hook>, String> {
    crate::extensions::list_hooks(project_dir)
}

#[tauri::command]
pub async fn save_hook(
    hook: crate::extensions::Hook,
    project_dir: Option<String>,
) -> Result<(), String> {
    crate::extensions::save_hook(hook, project_dir)
}

#[tauri::command]
pub async fn delete_hook(id: String, project_dir: Option<String>) -> Result<(), String> {
    crate::extensions::delete_hook(&id, project_dir)
}

#[tauri::command]
pub async fn market_search_skills(
    query: String,
) -> Result<Vec<crate::extensions::MarketSkill>, String> {
    crate::extensions::market_search_skills(&query).await
}

#[tauri::command]
pub async fn market_install_skill(
    source: String,
    skill_id: String,
    scope: crate::extensions::SkillScope,
    project_dir: Option<String>,
) -> Result<(), String> {
    crate::extensions::market_install_skill(&source, &skill_id, scope, project_dir).await
}
