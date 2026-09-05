use crate::protocol::{RuntimeCapabilityAvailability, RuntimeCapabilitySnapshot};
use crate::reasoning::{ReasoningEffort, ReasoningEffortCapability, ReasoningEffortSupport};
use crate::sessions::SessionHistoryStore;
use crate::turn_start::{digest_json, RuntimeRoute};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub const CAPABILITY_PROBE_DEADLINE: Duration = Duration::from_secs(15);
pub const CAPABILITY_PROBE_OUTPUT_LIMIT: usize = 256 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CapabilitySupport {
    Supported,
    Degraded,
    Unsupported,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityEvidence {
    pub support: CapabilitySupport,
    pub source: String,
    pub diagnostic: String,
}

impl CapabilityEvidence {
    pub fn new(
        support: CapabilitySupport,
        source: impl Into<String>,
        diagnostic: impl Into<String>,
    ) -> Self {
        Self {
            support,
            source: source.into(),
            diagnostic: diagnostic.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilitySet {
    pub model_override: CapabilityEvidence,
    pub reasoning_effort: CapabilityEvidence,
    pub native_resume: CapabilityEvidence,
    /// 同引擎无损分支（十次反馈）：`--resume <sid> --fork-session` 可把完整历史
    /// 复制进新 CLI 会话；缺失时分支分叉回退摘要派生。
    pub native_branch: CapabilityEvidence,
    pub approval: CapabilityEvidence,
    pub search: CapabilityEvidence,
    pub fetch: CapabilityEvidence,
    pub usage: CapabilityEvidence,
    pub interrupt: CapabilityEvidence,
    pub model_only_operation: CapabilityEvidence,
    #[serde(default = "unknown_auto_approval")]
    pub auto_approval: CapabilityEvidence,
    #[serde(default)]
    pub reasoning_efforts: Vec<ReasoningEffort>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_reasoning_effort: Option<ReasoningEffort>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
}

impl CapabilitySet {
    pub fn unknown(source: &str) -> Self {
        let evidence = |diagnostic: &str| {
            CapabilityEvidence::new(CapabilitySupport::Unknown, source, diagnostic)
        };
        Self {
            model_override: evidence("model_override_unknown"),
            reasoning_effort: evidence("reasoning_effort_unknown"),
            native_resume: evidence("native_resume_unknown"),
            native_branch: evidence("native_branch_unknown"),
            approval: evidence("approval_unknown"),
            search: evidence("search_unknown"),
            fetch: evidence("fetch_unknown"),
            usage: evidence("usage_unknown"),
            interrupt: evidence("interrupt_unknown"),
            model_only_operation: evidence("model_only_operation_unknown"),
            auto_approval: evidence("auto_approval_runtime_observation_required"),
            reasoning_efforts: vec![ReasoningEffort::Auto],
            default_reasoning_effort: None,
            context_window: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityIdentity {
    pub engine_id: String,
    pub adapter_version: String,
    pub binary_identity: String,
    pub engine_profile_digest: String,
    pub provider_launch_profile_ref: String,
    pub provider_launch_profile_digest: String,
    pub launch_profile_identity: String,
    pub model_capability_key: String,
}

impl CapabilityIdentity {
    pub fn from_route(
        route: &RuntimeRoute,
        binary_identity: String,
        launch_profile_identity: String,
    ) -> Self {
        let adapter_version = if route.engine_id == "codex" {
            // +codex-branch-v4：2026-09-02 Codex 无损分支改由 app-server `thread/fork` 提供，
            // native_branch 探测结论从 Unsupported 翻转为 Supported。该值不进 cache_key，
            // 旧快照（probed 于翻转前，仍记 nativeBranch=unsupported）会持续命中，令
            // resume_session 的分支闸门直接把分叉会话判死、界面上「点了打不开」且无报错
            // （错误只落在收起的会话抽屉里）。换盐使旧探测快照整体失效重探。
            format!(
                "{}+codex-search-v3+codex-branch-v4",
                env!("CARGO_PKG_VERSION")
            )
        } else {
            // +no-tools-contract-v2：`claude --help` 的 no-tools 合同匹配升级为折叠
            // 空白后再匹配（CLI 折行会打断连续短语）。该盐使旧探测快照失效重探。
            format!("{}+no-tools-contract-v2", env!("CARGO_PKG_VERSION"))
        };
        Self {
            engine_id: route.engine_id.clone(),
            adapter_version,
            binary_identity,
            engine_profile_digest: route.engine_profile_digest.clone(),
            provider_launch_profile_ref: route.provider_launch_profile_ref.clone(),
            provider_launch_profile_digest: route.provider_launch_profile_digest.clone(),
            launch_profile_identity,
            model_capability_key: route.model_id.clone(),
        }
    }

    pub fn cache_key(&self) -> Result<String, String> {
        digest_json(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineCapabilitySnapshot {
    pub id: String,
    pub identity: CapabilityIdentity,
    pub capabilities: CapabilitySet,
    pub probe_kind: String,
    pub probed_at: i64,
}

impl EngineCapabilitySnapshot {
    pub fn runtime_projection(&self) -> RuntimeCapabilitySnapshot {
        RuntimeCapabilitySnapshot {
            web_search: availability(self.capabilities.search.support),
            web_fetch: availability(self.capabilities.fetch.support),
            approval_contract_version: self.capabilities.approval.diagnostic.clone(),
            capability_snapshot_id: Some(self.id.clone()),
            auto_review_strategy: Some(
                match self.capabilities.auto_approval.support {
                    CapabilitySupport::Supported => "native",
                    CapabilitySupport::Degraded => "compatible",
                    CapabilitySupport::Unsupported => "unavailable",
                    CapabilitySupport::Unknown => "unknown",
                }
                .to_string(),
            ),
        }
    }
}

fn unknown_auto_approval() -> CapabilityEvidence {
    CapabilityEvidence::new(
        CapabilitySupport::Unknown,
        "legacy_capability_snapshot",
        "auto_approval_runtime_observation_required",
    )
}

fn availability(support: CapabilitySupport) -> RuntimeCapabilityAvailability {
    match support {
        CapabilitySupport::Supported => RuntimeCapabilityAvailability::Available,
        CapabilitySupport::Unsupported => RuntimeCapabilityAvailability::Unavailable,
        CapabilitySupport::Degraded | CapabilitySupport::Unknown => {
            RuntimeCapabilityAvailability::Unknown
        }
    }
}

#[derive(Clone)]
pub struct EngineCapabilityRegistry {
    history: SessionHistoryStore,
    memory: Arc<Mutex<HashMap<String, EngineCapabilitySnapshot>>>,
}

impl EngineCapabilityRegistry {
    pub fn new(history: SessionHistoryStore) -> Self {
        Self {
            history,
            memory: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub async fn resolve<F, Fut>(
        &self,
        identity: CapabilityIdentity,
        probe: F,
    ) -> Result<EngineCapabilitySnapshot, String>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<(CapabilitySet, String), String>>,
    {
        let cache_key = identity.cache_key()?;
        if let Some(snapshot) = self
            .memory
            .lock()
            .map_err(|_| "Capability Registry 内存缓存锁中毒".to_string())?
            .get(&cache_key)
            .cloned()
        {
            return Ok(snapshot);
        }
        if let Some(snapshot) = self.history.load_capability_snapshot(&cache_key)? {
            self.memory
                .lock()
                .map_err(|_| "Capability Registry 内存缓存锁中毒".to_string())?
                .insert(cache_key, snapshot.clone());
            return Ok(snapshot);
        }
        let (capabilities, probe_kind) = tokio::time::timeout(CAPABILITY_PROBE_DEADLINE, probe())
            .await
            .map_err(|_| "[capability_probe_timeout] Engine 能力握手超时".to_string())??;
        let probed_at = crate::util::now_millis();
        let id = digest_json(&(&identity, &capabilities, &probe_kind, probed_at))?;
        let snapshot = EngineCapabilitySnapshot {
            id,
            identity,
            capabilities,
            probe_kind,
            probed_at,
        };
        self.history
            .save_capability_snapshot(&cache_key, &snapshot)?;
        let snapshot = self
            .history
            .load_capability_snapshot(&cache_key)?
            .ok_or_else(|| "CapabilitySnapshot 持久化后不可见".to_string())?;
        self.memory
            .lock()
            .map_err(|_| "Capability Registry 内存缓存锁中毒".to_string())?
            .insert(cache_key, snapshot.clone());
        Ok(snapshot)
    }

    pub fn record_auto_review_degraded(
        &self,
        snapshot: &EngineCapabilitySnapshot,
        evidence_code: &str,
    ) -> Result<EngineCapabilitySnapshot, String> {
        if !matches!(
            evidence_code,
            "automode-unavailable" | "automode-parsing-error"
        ) {
            return Err("拒绝用非兼容性拒绝污染 Auto capability".to_string());
        }
        let cache_key = snapshot.identity.cache_key()?;
        let mut updated = snapshot.clone();
        updated.capabilities.auto_approval = CapabilityEvidence::new(
            CapabilitySupport::Degraded,
            "claude_runtime_denial",
            evidence_code,
        );
        updated.probed_at = crate::util::now_millis();
        self.history
            .update_capability_snapshot(&cache_key, &updated)?;
        self.memory
            .lock()
            .map_err(|_| "Capability Registry 内存缓存锁中毒".to_string())?
            .insert(cache_key, updated.clone());
        Ok(updated)
    }

    pub fn record_auto_review_native(
        &self,
        snapshot: &EngineCapabilitySnapshot,
    ) -> Result<EngineCapabilitySnapshot, String> {
        let cache_key = snapshot.identity.cache_key()?;
        let mut updated = snapshot.clone();
        updated.capabilities.auto_approval = CapabilityEvidence::new(
            CapabilitySupport::Supported,
            "claude_runtime_success",
            "claude_native_auto_turn_completed",
        );
        updated.probed_at = crate::util::now_millis();
        self.history
            .update_capability_snapshot(&cache_key, &updated)?;
        self.memory
            .lock()
            .map_err(|_| "Capability Registry 内存缓存锁中毒".to_string())?
            .insert(cache_key, updated.clone());
        Ok(updated)
    }

    pub fn record_web_search_native(
        &self,
        snapshot: &EngineCapabilitySnapshot,
    ) -> Result<EngineCapabilitySnapshot, String> {
        let cache_key = snapshot.identity.cache_key()?;
        let mut updated = snapshot.clone();
        updated.capabilities.search = CapabilityEvidence::new(
            CapabilitySupport::Supported,
            "codex_runtime_observation",
            "codex_native_web_search_item_observed",
        );
        updated.probed_at = crate::util::now_millis();
        self.history
            .update_capability_snapshot(&cache_key, &updated)?;
        self.memory
            .lock()
            .map_err(|_| "Capability Registry 内存缓存锁中毒".to_string())?
            .insert(cache_key, updated.clone());
        Ok(updated)
    }

    pub fn record_web_search_unavailable(
        &self,
        snapshot: &EngineCapabilitySnapshot,
        diagnostic: &str,
    ) -> Result<EngineCapabilitySnapshot, String> {
        let cache_key = snapshot.identity.cache_key()?;
        let mut updated = snapshot.clone();
        updated.capabilities.search = CapabilityEvidence::new(
            CapabilitySupport::Unsupported,
            "codex_runtime_observation",
            diagnostic,
        );
        updated.probed_at = crate::util::now_millis();
        self.history
            .update_capability_snapshot(&cache_key, &updated)?;
        self.memory
            .lock()
            .map_err(|_| "Capability Registry 内存缓存锁中毒".to_string())?
            .insert(cache_key, updated.clone());
        Ok(updated)
    }
}

pub fn binary_identity(configured_bin: &str) -> Result<String, String> {
    let path = resolve_binary_path(configured_bin)?;
    let metadata = std::fs::metadata(&path)
        .map_err(|error| format!("读取 Engine 二进制元数据失败：{error}"))?;
    if metadata.len() > 128 * 1024 * 1024 {
        return Err("Engine 二进制超过 capability identity 读取上限".to_string());
    }
    let bytes = std::fs::read(&path).map_err(|error| format!("读取 Engine 二进制失败：{error}"))?;
    let canonical = path
        .canonicalize()
        .unwrap_or(path)
        .to_string_lossy()
        .to_string();
    let modified = metadata
        .modified()
        .ok()
        .and_then(|value| value.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|value| value.as_nanos())
        .unwrap_or_default();
    Ok(format!(
        "{}:{}:{}:sha256:{:x}",
        canonical,
        metadata.len(),
        modified,
        Sha256::digest(bytes)
    ))
}

pub fn launch_profile_identity(
    route: &RuntimeRoute,
    profile_home: Option<&Path>,
) -> Result<String, String> {
    let Some(profile_home) = profile_home else {
        return digest_json(&(
            &route.provider_launch_profile_ref,
            &route.launch_config_digest,
        ));
    };
    let mut evidence = Vec::new();
    for name in [
        "auth.json",
        ".credentials.json",
        "settings.json",
        "config.toml",
    ] {
        let path = profile_home.join(name);
        let Ok(metadata) = std::fs::metadata(&path) else {
            continue;
        };
        let modified = metadata
            .modified()
            .ok()
            .and_then(|value| value.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|value| value.as_nanos())
            .unwrap_or_default();
        evidence.push((name, metadata.len(), modified));
    }
    digest_json(&(
        &route.provider_launch_profile_ref,
        &route.launch_config_digest,
        profile_home.to_string_lossy(),
        evidence,
    ))
}

fn resolve_binary_path(configured_bin: &str) -> Result<PathBuf, String> {
    let candidate = Path::new(configured_bin);
    if candidate.is_file() {
        return Ok(candidate.to_path_buf());
    }
    let output = if cfg!(windows) {
        let mut probe = std::process::Command::new("where.exe");
        probe.arg(configured_bin);
        use std::os::windows::process::CommandExt as _;
        probe.creation_flags(0x0800_0000);
        probe.output()
    } else {
        std::process::Command::new("which")
            .arg(configured_bin)
            .output()
    }
    .map_err(|error| format!("定位 Engine 二进制失败：{error}"))?;
    if !output.status.success() {
        return Err(format!("找不到 Engine 二进制：{configured_bin}"));
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(PathBuf::from)
        .find(|path| path.is_file())
        .ok_or_else(|| format!("找不到 Engine 二进制：{configured_bin}"))
}

pub fn bounded_probe_output(stdout: &[u8], stderr: &[u8]) -> Result<String, String> {
    if stdout.len().saturating_add(stderr.len()) > CAPABILITY_PROBE_OUTPUT_LIMIT {
        return Err("[capability_probe_output_limit] Engine 能力握手输出超过上限".to_string());
    }
    Ok(format!(
        "{}\n{}",
        String::from_utf8_lossy(stdout),
        String::from_utf8_lossy(stderr)
    ))
}

pub fn claude_capabilities_from_help(
    help: &str,
    reasoning: &ReasoningEffortCapability,
    model_only_launch_verified: bool,
) -> CapabilitySet {
    let has = |flag: &str| help.contains(flag);
    let supported = |condition: bool, diagnostic: &str| {
        CapabilityEvidence::new(
            if condition {
                CapabilitySupport::Supported
            } else {
                CapabilitySupport::Unsupported
            },
            "claude_help_contract",
            diagnostic,
        )
    };
    CapabilitySet {
        model_override: supported(has("--model"), "claude_flag_model"),
        reasoning_effort: reasoning_evidence(reasoning, "claude_help_model_contract"),
        native_resume: supported(has("--resume"), "claude_flag_resume"),
        native_branch: supported(has("--fork-session"), "claude_flag_fork_session"),
        approval: CapabilityEvidence::new(
            if has("--permission-prompt-tool") {
                CapabilitySupport::Degraded
            } else {
                CapabilitySupport::Unknown
            },
            "claude_help_contract",
            "claude_defer_requires_live_turn_observation",
        ),
        search: CapabilityEvidence::new(
            CapabilitySupport::Unknown,
            "runtime_observation_required",
            "claude_search_not_advertised_by_local_handshake",
        ),
        fetch: CapabilityEvidence::new(
            CapabilitySupport::Unknown,
            "runtime_observation_required",
            "claude_fetch_not_advertised_by_local_handshake",
        ),
        usage: supported(has("stream-json"), "claude_stream_json_usage"),
        interrupt: CapabilityEvidence::new(
            CapabilitySupport::Supported,
            "helm_process_supervisor",
            "bounded_process_tree_interrupt",
        ),
        model_only_operation: CapabilityEvidence::new(
            if model_only_launch_verified {
                CapabilitySupport::Supported
            } else if has("--tools") {
                CapabilitySupport::Unknown
            } else {
                CapabilitySupport::Unsupported
            },
            "claude_no_tools_launch_contract",
            if model_only_launch_verified {
                "claude_empty_tools_launch_verified"
            } else {
                "claude_empty_tools_launch_not_verified"
            },
        ),
        auto_approval: CapabilityEvidence::new(
            CapabilitySupport::Unknown,
            "runtime_observation_required",
            "claude_auto_review_runtime_observation_required",
        ),
        reasoning_efforts: reasoning.options.clone(),
        default_reasoning_effort: reasoning.default_effort,
        context_window: None,
    }
}

pub fn claude_model_only_contract_from_help(help: &str) -> bool {
    // CLI 的 --help 会对长描述折行（如 "Use \"\" to disable all\n tools"），
    // 先折叠空白再匹配，避免换行打断连续短语。
    let normalized: String = help.split_whitespace().collect::<Vec<_>>().join(" ");
    normalized.contains("--tools")
        && normalized.contains("disable all tools")
        && normalized.contains("--disable-slash-commands")
        && normalized.contains("--strict-mcp-config")
        && normalized.contains("--no-session-persistence")
}

pub fn codex_capabilities_from_handshake(
    model: &str,
    model_list: &serde_json::Value,
    reasoning: &ReasoningEffortCapability,
    native_search_enabled: bool,
    provider_search_capability: Option<bool>,
) -> CapabilitySet {
    let model_entry = model_list
        .get("data")
        .and_then(serde_json::Value::as_array)
        .and_then(|models| {
            models.iter().find(|entry| {
                entry.get("id").and_then(serde_json::Value::as_str) == Some(model)
                    || entry.get("model").and_then(serde_json::Value::as_str) == Some(model)
            })
        });
    let supported = |diagnostic: &str| {
        CapabilityEvidence::new(
            CapabilitySupport::Supported,
            "codex_app_server_handshake",
            diagnostic,
        )
    };
    let optional_bool = |camel: &str, snake: &str, diagnostic: &str| {
        model_entry
            .and_then(|entry| entry.get(camel).or_else(|| entry.get(snake)))
            .and_then(serde_json::Value::as_bool)
            .map(|available| {
                CapabilityEvidence::new(
                    if available {
                        CapabilitySupport::Supported
                    } else {
                        CapabilitySupport::Unsupported
                    },
                    "codex_model_list",
                    diagnostic,
                )
            })
            .unwrap_or_else(|| {
                CapabilityEvidence::new(
                    CapabilitySupport::Unknown,
                    "codex_model_list",
                    format!("{diagnostic}_not_advertised"),
                )
            })
    };
    CapabilitySet {
        model_override: if model_entry.is_some() {
            supported("codex_turn_start_model")
        } else {
            CapabilityEvidence::new(
                CapabilitySupport::Unsupported,
                "codex_model_list",
                "codex_model_not_listed",
            )
        },
        reasoning_effort: reasoning_evidence(reasoning, "codex_model_list"),
        native_resume: CapabilityEvidence::new(
            CapabilitySupport::Degraded,
            "codex_app_server_handshake",
            "codex_resume_same_launch_profile_only",
        ),
        // 2026-09-02：Codex 无损分支改由 app-server `thread/fork` 提供（codex_app_server::fork_thread），
        // 不再依赖「只能摘要派生」。fork 失败按错误透出，禁止静默降级为摘要（语义红线）；
        // 同 launch profile 约束由 `resume_session` 的 native_resume_profile 校验兜底。
        native_branch: CapabilityEvidence::new(
            CapabilitySupport::Supported,
            "codex_app_server_thread_fork",
            "codex_native_branch_thread_fork",
        ),
        approval: CapabilityEvidence::new(
            CapabilitySupport::Degraded,
            "codex_app_server_handshake",
            "codex_server_request_requires_live_observation",
        ),
        search: if provider_search_capability == Some(true) {
            // 网关在 model_provider_capabilities 里明确声明支持联网搜索：直接判可用。
            CapabilityEvidence::new(
                CapabilitySupport::Supported,
                "codex_model_provider_capabilities",
                "codex_provider_web_search_enabled",
            )
        } else {
            // 网关声明不支持(webSearch:false)或未知：不硬禁，交给真实 Runtime 观察——
            // 目录命中按 supportsWebSearch 裁决；否则按原生搜索开关降级为 Degraded(Unknown)，
            // 由运行时真实 WebSearch 工具是否存在决定可用与否（缺工具再 fail-closed，不冒充联网）。
            model_entry
                .and_then(|entry| {
                    entry
                        .get("supportsWebSearch")
                        .or_else(|| entry.get("supports_web_search"))
                })
                .and_then(serde_json::Value::as_bool)
                .map(|available| {
                    CapabilityEvidence::new(
                        if available {
                            CapabilitySupport::Supported
                        } else {
                            CapabilitySupport::Unsupported
                        },
                        "codex_model_list",
                        "codex_web_search",
                    )
                })
                .unwrap_or_else(|| {
                    if native_search_enabled {
                        CapabilityEvidence::new(
                            CapabilitySupport::Degraded,
                            "codex_search_launch_flag",
                            "codex_web_search_enabled_requires_runtime_observation",
                        )
                    } else {
                        CapabilityEvidence::new(
                            CapabilitySupport::Unknown,
                            "codex_model_list",
                            "codex_web_search_not_enabled",
                        )
                    }
                })
        },
        fetch: optional_bool("supportsWebFetch", "supports_web_fetch", "codex_web_fetch"),
        usage: supported("codex_turn_usage_events"),
        interrupt: supported("codex_turn_interrupt_rpc"),
        model_only_operation: CapabilityEvidence::new(
            CapabilitySupport::Unsupported,
            "codex_app_server_handshake",
            "codex_no_native_disable_all_tools_contract",
        ),
        auto_approval: CapabilityEvidence::new(
            CapabilitySupport::Supported,
            "codex_app_server_handshake",
            "codex_workspace_sandbox_and_server_request",
        ),
        reasoning_efforts: reasoning.options.clone(),
        default_reasoning_effort: reasoning.default_effort,
        context_window: model_entry
            .and_then(|entry| {
                entry
                    .get("contextWindow")
                    .or_else(|| entry.get("context_window"))
            })
            .and_then(serde_json::Value::as_u64),
    }
}

fn reasoning_evidence(capability: &ReasoningEffortCapability, source: &str) -> CapabilityEvidence {
    let support = match capability.support {
        ReasoningEffortSupport::Supported => CapabilitySupport::Supported,
        ReasoningEffortSupport::Unsupported => CapabilitySupport::Unsupported,
        ReasoningEffortSupport::Unknown => CapabilitySupport::Unknown,
    };
    CapabilityEvidence::new(support, source, "model_scoped_reasoning_effort")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResumeStrategy {
    Native,
    LedgerRebuild,
    Blocked,
}

pub fn resume_strategy(
    snapshot: &EngineCapabilitySnapshot,
    same_launch_profile: bool,
    complete_ledger_available: bool,
) -> ResumeStrategy {
    if same_launch_profile
        && matches!(
            snapshot.capabilities.native_resume.support,
            CapabilitySupport::Supported | CapabilitySupport::Degraded
        )
    {
        ResumeStrategy::Native
    } else if complete_ledger_available {
        ResumeStrategy::LedgerRebuild
    } else {
        ResumeStrategy::Blocked
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reasoning::{ReasoningEffortSource, ReasoningEffortSupport};

    fn reasoning() -> ReasoningEffortCapability {
        ReasoningEffortCapability {
            support: ReasoningEffortSupport::Supported,
            options: vec![ReasoningEffort::Auto, ReasoningEffort::High],
            default_effort: None,
            source: ReasoningEffortSource::EngineProbe,
        }
    }

    #[test]
    fn claude_help_never_upgrades_no_tools_without_launch_proof() {
        let capabilities = claude_capabilities_from_help(
            "--model --resume --effort --tools --output-format stream-json",
            &reasoning(),
            false,
        );
        assert_eq!(
            capabilities.model_only_operation.support,
            CapabilitySupport::Unknown
        );
        assert_eq!(
            capabilities.model_override.support,
            CapabilitySupport::Supported
        );
    }

    #[test]
    fn claude_native_branch_requires_fork_session_flag() {
        // 十次反馈：无损分支只在 CLI help 明示 --fork-session 时放行；
        // 缺失时回退摘要分叉，不得凭 --resume 单独推断。
        let with_branch = claude_capabilities_from_help(
            "--model --resume --fork-session --tools --output-format stream-json",
            &reasoning(),
            false,
        );
        assert_eq!(
            with_branch.native_branch.support,
            CapabilitySupport::Supported
        );
        let without_branch = claude_capabilities_from_help(
            "--model --resume --tools --output-format stream-json",
            &reasoning(),
            false,
        );
        assert_eq!(
            without_branch.native_branch.support,
            CapabilitySupport::Unsupported
        );
    }

    #[test]
    fn codex_native_branch_supported_via_thread_fork() {
        // 2026-09-02 回归守卫：Codex 无损分支由 app-server `thread/fork` 提供，
        // 不依赖 claude 的 --fork-session。该值若回退为 Unsupported，resume_session
        // 会对分支会话直接拒绝打开，表现为「左栏分叉出的新任务点不开」。
        let model_list = serde_json::json!({ "data": [{ "id": "gpt-5" }] });
        let capabilities =
            codex_capabilities_from_handshake("gpt-5", &model_list, &reasoning(), false, None);
        assert_eq!(
            capabilities.native_branch.support,
            CapabilitySupport::Supported
        );
    }

    #[test]
    fn claude_model_only_contract_requires_every_isolation_flag() {
        let complete = "--tools Use empty to disable all tools --disable-slash-commands \
                        --strict-mcp-config --no-session-persistence";
        assert!(claude_model_only_contract_from_help(complete));
        assert!(!claude_model_only_contract_from_help(
            "--tools disable all tools --strict-mcp-config --no-session-persistence"
        ));
    }

    #[test]
    fn claude_model_only_contract_survives_help_wrapping() {
        // 真实 CLI 会把长描述折行（"disable all\n tools"），连续短语被换行打断，
        // 必须折叠空白后仍能匹配，否则 no-tools 合同被误判为不可用。
        let wrapped = "--tools <tools...>  Specify the list of available tools from\n\
                       the built-in set. Use \"\" to disable all\n\
                       tools, \"default\" to use all tools, or\n\
                       specify tool names (e.g. \"Bash,Edit,Read\").\n\
                       --disable-slash-commands  Disable all skills\n\
                       --strict-mcp-config\n\
                       --no-session-persistence";
        assert!(
            claude_model_only_contract_from_help(wrapped),
            "折行的 --help 必须仍能验证 no-tools 合同"
        );
    }

    #[test]
    fn codex_cache_facts_are_model_scoped() {
        let response = serde_json::json!({"data":[
            {"model":"gpt-a","supportedReasoningEfforts":[{"reasoningEffort":"high"}]},
            {"model":"gpt-b","supportsWebSearch":false}
        ]});
        let a = codex_capabilities_from_handshake("gpt-a", &response, &reasoning(), false, None);
        let b = codex_capabilities_from_handshake("gpt-b", &response, &reasoning(), false, None);
        assert_eq!(a.search.support, CapabilitySupport::Unknown);
        assert_eq!(b.search.support, CapabilitySupport::Unsupported);
        assert_eq!(
            b.model_only_operation.support,
            CapabilitySupport::Unsupported
        );
    }

    #[test]
    fn codex_search_launch_flag_is_degraded_until_runtime_observation() {
        let response = serde_json::json!({"data":[{"model":"gpt-a"}]});
        // 原生搜索开启、provider 未显式声明 → 降级为 Degraded，交运行时观察（缺工具再 fail-closed）。
        let capabilities =
            codex_capabilities_from_handshake("gpt-a", &response, &reasoning(), true, None);
        assert_eq!(capabilities.search.support, CapabilitySupport::Degraded);
        assert_eq!(
            capabilities.search.diagnostic,
            "codex_web_search_enabled_requires_runtime_observation"
        );
    }

    #[test]
    fn codex_provider_capability_web_search_resolution() {
        let response = serde_json::json!({"data":[{"model":"gpt-a"}]});
        // provider 显式 true → 直接判可用。
        let enabled =
            codex_capabilities_from_handshake("gpt-a", &response, &reasoning(), true, Some(true));
        assert_eq!(enabled.search.support, CapabilitySupport::Supported);
        assert_eq!(enabled.search.source, "codex_model_provider_capabilities");
        // provider 显式 false 或未知：不再硬禁，交运行时观察（原生开启即 Degraded）。
        let disabled =
            codex_capabilities_from_handshake("gpt-a", &response, &reasoning(), true, Some(false));
        assert_eq!(disabled.search.support, CapabilitySupport::Degraded);
        assert_eq!(disabled.search.source, "codex_search_launch_flag");
    }

    #[test]
    fn cache_identity_separates_provider_profile_and_model() {
        let base = CapabilityIdentity {
            engine_id: "codex".into(),
            adapter_version: "0.1.0".into(),
            binary_identity: "codex:sha256:a".into(),
            engine_profile_digest: "sha256:engine".into(),
            provider_launch_profile_ref: "provider:a:api".into(),
            provider_launch_profile_digest: "sha256:provider-a".into(),
            launch_profile_identity: "sha256:launch-a".into(),
            model_capability_key: "gpt-a".into(),
        };
        let provider_b = CapabilityIdentity {
            provider_launch_profile_ref: "provider:b:api".into(),
            provider_launch_profile_digest: "sha256:provider-b".into(),
            ..base.clone()
        };
        let model_b = CapabilityIdentity {
            model_capability_key: "gpt-b".into(),
            ..base.clone()
        };
        assert_ne!(base.cache_key().unwrap(), provider_b.cache_key().unwrap());
        assert_ne!(base.cache_key().unwrap(), model_b.cache_key().unwrap());
    }

    #[test]
    fn optional_extensions_are_accepted_but_required_identity_is_not() {
        let valid = serde_json::json!({
            "id":"snapshot-1",
            "identity":{
                "engineId":"codex",
                "adapterVersion":"test",
                "binaryIdentity":"sha256:a",
                "engineProfileDigest":"sha256:e",
                "providerLaunchProfileRef":"provider:a:api",
                "providerLaunchProfileDigest":"sha256:p",
                "launchProfileIdentity":"sha256:launch",
                "modelCapabilityKey":"gpt-a"
            },
            "capabilities": serde_json::to_value(CapabilitySet::unknown("fixture")).unwrap(),
            "probeKind":"fixture",
            "probedAt":1,
            "futureOptionalField":true
        });
        assert!(serde_json::from_value::<EngineCapabilitySnapshot>(valid.clone()).is_ok());
        let mut missing_required = valid;
        missing_required.as_object_mut().unwrap().remove("identity");
        assert!(serde_json::from_value::<EngineCapabilitySnapshot>(missing_required).is_err());
    }

    #[tokio::test]
    async fn persisted_cache_avoids_a_second_probe() {
        let path = std::env::temp_dir().join(format!(
            "helm-capability-registry-{}-{}.sqlite",
            std::process::id(),
            rand::random::<u64>()
        ));
        let store = SessionHistoryStore::new(path.clone());
        let identity = CapabilityIdentity {
            engine_id: "codex".into(),
            adapter_version: "test".into(),
            binary_identity: "codex:sha256:a".into(),
            engine_profile_digest: "sha256:engine".into(),
            provider_launch_profile_ref: "provider:a:api".into(),
            provider_launch_profile_digest: "sha256:provider".into(),
            launch_profile_identity: "sha256:launch".into(),
            model_capability_key: "gpt-a".into(),
        };
        let first = EngineCapabilityRegistry::new(store.clone())
            .resolve(identity.clone(), || async {
                Ok((CapabilitySet::unknown("first"), "fixture".into()))
            })
            .await
            .unwrap();
        let second = EngineCapabilityRegistry::new(store)
            .resolve(identity, || async {
                panic!("persisted cache miss");
                #[allow(unreachable_code)]
                Ok((CapabilitySet::unknown("second"), "fixture".into()))
            })
            .await
            .unwrap();
        assert_eq!(first, second);
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn auto_review_degradation_is_identity_scoped_and_blocked_does_not_pollute_it() {
        let path = std::env::temp_dir().join(format!(
            "helm-auto-capability-{}-{}.sqlite",
            std::process::id(),
            rand::random::<u64>()
        ));
        let store = SessionHistoryStore::new(path.clone());
        let registry = EngineCapabilityRegistry::new(store.clone());
        let identity = CapabilityIdentity {
            engine_id: "claude-code".into(),
            adapter_version: "test".into(),
            binary_identity: "claude:sha256:a".into(),
            engine_profile_digest: "sha256:engine".into(),
            provider_launch_profile_ref: "provider:a:api".into(),
            provider_launch_profile_digest: "sha256:provider".into(),
            launch_profile_identity: "sha256:launch".into(),
            model_capability_key: "mimo-v2.5-pro".into(),
        };
        let initial = registry
            .resolve(identity.clone(), || async {
                Ok((CapabilitySet::unknown("fixture"), "fixture".into()))
            })
            .await
            .unwrap();
        assert!(registry
            .record_auto_review_degraded(&initial, "automode-blocked")
            .is_err());
        let degraded = registry
            .record_auto_review_degraded(&initial, "automode-unavailable")
            .unwrap();
        assert_eq!(
            degraded.capabilities.auto_approval.support,
            CapabilitySupport::Degraded
        );
        let restored = EngineCapabilityRegistry::new(store)
            .resolve(identity, || async {
                panic!("degraded evidence must be persisted");
                #[allow(unreachable_code)]
                Ok((CapabilitySet::unknown("miss"), "fixture".into()))
            })
            .await
            .unwrap();
        assert_eq!(
            restored.capabilities.auto_approval.support,
            CapabilitySupport::Degraded
        );
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn observed_codex_web_search_is_persisted_for_the_same_identity() {
        let path = std::env::temp_dir().join(format!(
            "helm-search-capability-{}-{}.sqlite",
            std::process::id(),
            rand::random::<u64>()
        ));
        let store = SessionHistoryStore::new(path.clone());
        let registry = EngineCapabilityRegistry::new(store.clone());
        let identity = CapabilityIdentity {
            engine_id: "codex".into(),
            adapter_version: "test".into(),
            binary_identity: "codex:sha256:a".into(),
            engine_profile_digest: "sha256:engine".into(),
            provider_launch_profile_ref: "provider:a:api".into(),
            provider_launch_profile_digest: "sha256:provider".into(),
            launch_profile_identity: "sha256:launch".into(),
            model_capability_key: "gpt-a".into(),
        };
        let initial = registry
            .resolve(identity.clone(), || async {
                Ok((CapabilitySet::unknown("fixture"), "fixture".into()))
            })
            .await
            .unwrap();
        let observed = registry.record_web_search_native(&initial).unwrap();
        assert_eq!(
            observed.capabilities.search.support,
            CapabilitySupport::Supported
        );

        let restored = EngineCapabilityRegistry::new(store)
            .resolve(identity, || async {
                panic!("observed search evidence must be persisted");
                #[allow(unreachable_code)]
                Ok((CapabilitySet::unknown("miss"), "fixture".into()))
            })
            .await
            .unwrap();
        assert_eq!(
            restored.capabilities.search.support,
            CapabilitySupport::Supported
        );
        let _ = std::fs::remove_file(path);
    }
}
