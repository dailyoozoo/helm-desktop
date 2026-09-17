use crate::probe_cache::{AsyncProbeCache, PROBE_INVALIDATED};
use crate::protocol::{RuntimeCapabilityAvailability, RuntimeCapabilitySnapshot};
use crate::reasoning::{ReasoningEffort, ReasoningEffortCapability, ReasoningEffortSupport};
use crate::sessions::SessionHistoryStore;
use crate::turn_start::{digest_json, RuntimeRoute};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

pub const CAPABILITY_PROBE_DEADLINE: Duration = Duration::from_secs(15);
pub const CAPABILITY_PROBE_OUTPUT_LIMIT: usize = 256 * 1024;
const CAPABILITY_CACHE_TTL: Duration = Duration::from_secs(300);
const BINARY_IDENTITY_CACHE_TTL: Duration = Duration::from_secs(30);
const PROBE_CACHE_CAPACITY: usize = 64;

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
    probes: AsyncProbeCache<EngineCapabilitySnapshot>,
    invalidated_at: Arc<Mutex<HashMap<String, i64>>>,
}

impl EngineCapabilityRegistry {
    pub fn new(history: SessionHistoryStore) -> Self {
        Self {
            history,
            probes: AsyncProbeCache::new(CAPABILITY_CACHE_TTL, PROBE_CACHE_CAPACITY),
            invalidated_at: Arc::new(Mutex::new(HashMap::new())),
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
        let memory_key = format!("{}:{cache_key}", identity.engine_id);
        self.probes
            .get_or_probe_if(
                memory_key,
                |token| async move {
                    let cached = {
                        let invalidated = self
                            .invalidated_at
                            .lock()
                            .map_err(|_| "Capability Registry 发布锁中毒".to_string())?;
                        self.history
                            .load_capability_snapshot(&cache_key)?
                            .filter(|snapshot| {
                                capability_snapshot_is_fresh(
                                    snapshot,
                                    invalidated.get(&identity.engine_id).copied(),
                                )
                            })
                    };
                    if let Some(snapshot) = cached {
                        return Ok(snapshot);
                    }
                    let (mut capabilities, probe_kind) =
                        tokio::time::timeout(CAPABILITY_PROBE_DEADLINE, probe())
                            .await
                            .map_err(|_| {
                                "[capability_probe_timeout] Engine 能力握手超时".to_string()
                            })??;
                    let invalidated = self
                        .invalidated_at
                        .lock()
                        .map_err(|_| "Capability Registry 发布锁中毒".to_string())?;
                    if !token.is_current() {
                        return Err(PROBE_INVALIDATED.to_string());
                    }
                    let previous = self.history.load_capability_snapshot(&cache_key)?;
                    if let Some(previous) = &previous {
                        preserve_runtime_evidence(&mut capabilities, &previous.capabilities);
                    }
                    let probed_at = crate::util::now_millis().max(
                        invalidated
                            .get(&identity.engine_id)
                            .copied()
                            .unwrap_or_default()
                            .saturating_add(1),
                    );
                    let id = match &previous {
                        Some(previous) => previous.id.clone(),
                        None => digest_json(&(&identity, &capabilities, &probe_kind, probed_at))?,
                    };
                    let snapshot = EngineCapabilitySnapshot {
                        id,
                        identity,
                        capabilities,
                        probe_kind,
                        probed_at,
                    };
                    if previous.is_some() {
                        self.history
                            .update_capability_snapshot(&cache_key, &snapshot)?;
                    } else {
                        self.history
                            .save_capability_snapshot(&cache_key, &snapshot)?;
                    }
                    Ok(snapshot)
                },
                |snapshot| capability_snapshot_is_fresh(snapshot, None),
            )
            .await
    }

    pub fn invalidate_engine(&self, engine: &str) -> Result<(), String> {
        if !matches!(engine, "claude-code" | "codex") {
            return Err(format!("未知引擎：{engine}"));
        }
        let mut invalidated = self
            .invalidated_at
            .lock()
            .map_err(|_| "Capability Registry 发布锁中毒".to_string())?;
        let invalidated_at = crate::util::now_millis().max(
            invalidated
                .get(engine)
                .copied()
                .unwrap_or_default()
                .saturating_add(1),
        );
        invalidated.insert(engine.to_string(), invalidated_at);
        let prefix = format!("{engine}:");
        self.probes.invalidate_where(|key| key.starts_with(&prefix))
    }

    fn record_runtime_observation(
        &self,
        snapshot: &EngineCapabilitySnapshot,
        update: impl FnOnce(&mut CapabilitySet),
    ) -> Result<EngineCapabilitySnapshot, String> {
        let cache_key = snapshot.identity.cache_key()?;
        let invalidated = self
            .invalidated_at
            .lock()
            .map_err(|_| "Capability Registry 发布锁中毒".to_string())?;
        let mut updated = self
            .history
            .load_capability_snapshot(&cache_key)?
            .ok_or_else(|| "CapabilitySnapshot 更新目标不存在".to_string())?;
        update(&mut updated.capabilities);
        updated.probed_at = crate::util::now_millis().max(
            invalidated
                .get(&updated.identity.engine_id)
                .copied()
                .unwrap_or_default()
                .saturating_add(1),
        );
        self.history
            .update_capability_snapshot(&cache_key, &updated)?;
        self.probes.put(
            format!("{}:{cache_key}", updated.identity.engine_id),
            updated.clone(),
        )?;
        Ok(updated)
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
        self.record_runtime_observation(snapshot, |capabilities| {
            capabilities.auto_approval = CapabilityEvidence::new(
                CapabilitySupport::Degraded,
                "claude_runtime_denial",
                evidence_code,
            );
        })
    }

    pub fn record_auto_review_native(
        &self,
        snapshot: &EngineCapabilitySnapshot,
    ) -> Result<EngineCapabilitySnapshot, String> {
        self.record_runtime_observation(snapshot, |capabilities| {
            capabilities.auto_approval = CapabilityEvidence::new(
                CapabilitySupport::Supported,
                "claude_runtime_success",
                "claude_native_auto_turn_completed",
            );
        })
    }

    pub fn record_web_search_native(
        &self,
        snapshot: &EngineCapabilitySnapshot,
    ) -> Result<EngineCapabilitySnapshot, String> {
        self.record_runtime_observation(snapshot, |capabilities| {
            capabilities.search = CapabilityEvidence::new(
                CapabilitySupport::Supported,
                "codex_runtime_observation",
                "codex_native_web_search_item_observed",
            );
        })
    }

    pub fn record_web_search_unavailable(
        &self,
        snapshot: &EngineCapabilitySnapshot,
        diagnostic: &str,
    ) -> Result<EngineCapabilitySnapshot, String> {
        self.record_runtime_observation(snapshot, |capabilities| {
            capabilities.search = CapabilityEvidence::new(
                CapabilitySupport::Unsupported,
                "codex_runtime_observation",
                diagnostic,
            );
        })
    }
}

fn capability_snapshot_is_fresh(
    snapshot: &EngineCapabilitySnapshot,
    invalidated_at: Option<i64>,
) -> bool {
    let now = crate::util::now_millis();
    snapshot.probed_at <= now.saturating_add(1000)
        && now.saturating_sub(snapshot.probed_at) < CAPABILITY_CACHE_TTL.as_millis() as i64
        && invalidated_at.is_none_or(|invalidated| snapshot.probed_at > invalidated)
}

fn preserve_runtime_evidence(capabilities: &mut CapabilitySet, previous: &CapabilitySet) {
    if matches!(
        previous.auto_approval.source.as_str(),
        "claude_runtime_denial" | "claude_runtime_success"
    ) {
        capabilities.auto_approval = previous.auto_approval.clone();
    }
    if previous.search.source == "codex_runtime_observation" {
        capabilities.search = previous.search.clone();
    }
}

type BinaryIdentityKey = (PathBuf, u64, SystemTime);

fn binary_identity_cache() -> &'static Mutex<HashMap<BinaryIdentityKey, (String, Instant)>> {
    static CACHE: std::sync::OnceLock<Mutex<HashMap<BinaryIdentityKey, (String, Instant)>>> =
        std::sync::OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn binary_identity(configured_bin: &str) -> Result<String, String> {
    let path = resolve_binary_path(configured_bin)?
        .canonicalize()
        .map_err(|error| format!("解析 Engine 二进制路径失败：{error}"))?;
    let metadata = std::fs::metadata(&path)
        .map_err(|error| format!("读取 Engine 二进制元数据失败：{error}"))?;
    if metadata.len() > 128 * 1024 * 1024 {
        return Err("Engine 二进制超过 capability identity 读取上限".to_string());
    }
    let modified = metadata
        .modified()
        .map_err(|error| format!("读取 Engine 二进制时间失败：{error}"))?;
    let key = (path.clone(), metadata.len(), modified);
    let mut cache = binary_identity_cache()
        .lock()
        .map_err(|_| "Engine 二进制身份缓存锁中毒".to_string())?;
    cache.retain(|_, (_, checked_at)| checked_at.elapsed() < BINARY_IDENTITY_CACHE_TTL);
    if let Some((identity, _)) = cache.get(&key) {
        return Ok(identity.clone());
    }
    let mut file =
        std::fs::File::open(&path).map_err(|error| format!("读取 Engine 二进制失败：{error}"))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    let mut read_bytes = 0_u64;
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|error| format!("读取 Engine 二进制失败：{error}"))?;
        if count == 0 {
            break;
        }
        read_bytes = read_bytes.saturating_add(count as u64);
        if read_bytes > 128 * 1024 * 1024 {
            return Err("Engine 二进制超过 capability identity 读取上限".to_string());
        }
        hasher.update(&buffer[..count]);
    }
    let latest = file
        .metadata()
        .map_err(|error| format!("读取 Engine 二进制元数据失败：{error}"))?;
    if read_bytes != metadata.len()
        || latest.len() != metadata.len()
        || latest.modified().ok() != Some(modified)
    {
        return Err("Engine 二进制在探测期间发生变化，请重试".to_string());
    }
    let identity = format!(
        "{}:{}:{}:sha256:{:x}",
        path.to_string_lossy(),
        metadata.len(),
        modified
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
        hasher.finalize(),
    );
    if cache.len() >= PROBE_CACHE_CAPACITY {
        if let Some(oldest) = cache
            .iter()
            .min_by_key(|(_, (_, checked_at))| *checked_at)
            .map(|(key, _)| key.clone())
        {
            cache.remove(&oldest);
        }
    }
    cache.insert(key, (identity.clone(), Instant::now()));
    Ok(identity)
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

pub(crate) fn resolve_binary_path(configured_bin: &str) -> Result<PathBuf, String> {
    let candidate = Path::new(configured_bin);
    if candidate.is_file() {
        return Ok(candidate.to_path_buf());
    }
    if configured_bin.trim().is_empty() || candidate.components().count() != 1 {
        return Err(format!("找不到 Engine 二进制：{configured_bin}"));
    }
    let extensions = if cfg!(windows) && candidate.extension().is_none() {
        let mut extensions = std::env::var("PATHEXT")
            .unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_string())
            .split(';')
            .filter(|extension| extension.starts_with('.'))
            .map(str::to_string)
            .collect::<Vec<_>>();
        extensions.push(String::new());
        extensions
    } else {
        vec![String::new()]
    };
    if let Some(search_path) = std::env::var_os("PATH") {
        for directory in std::env::split_paths(&search_path) {
            for extension in &extensions {
                let path = directory.join(format!("{configured_bin}{extension}"));
                if path.is_file() {
                    return Ok(path);
                }
            }
        }
    }
    Err(format!("找不到 Engine 二进制：{configured_bin}"))
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

    fn probe_registry() -> (EngineCapabilityRegistry, CapabilityIdentity, PathBuf) {
        let path = std::env::temp_dir().join(format!(
            "helm-probe-cache-{}-{}.sqlite",
            std::process::id(),
            rand::random::<u64>()
        ));
        let registry = EngineCapabilityRegistry::new(SessionHistoryStore::new(path.clone()));
        let identity = CapabilityIdentity {
            engine_id: "codex".into(),
            adapter_version: "test".into(),
            binary_identity: "configured:sha256:test".into(),
            engine_profile_digest: "engine:test".into(),
            provider_launch_profile_ref: "provider:test".into(),
            provider_launch_profile_digest: "provider:digest".into(),
            launch_profile_identity: "launch:test".into(),
            model_capability_key: "test-model".into(),
        };
        (registry, identity, path)
    }

    #[tokio::test]
    async fn capability_probes_merge_concurrent_requests() {
        let (registry, identity, path) = probe_registry();
        let calls = std::sync::atomic::AtomicUsize::new(0);
        let probe = || async {
            calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            tokio::task::yield_now().await;
            Ok((CapabilitySet::unknown("shared"), "test".into()))
        };
        let (first, second) = tokio::join!(
            registry.resolve(identity.clone(), probe),
            registry.resolve(identity, probe)
        );
        assert_eq!(first.unwrap(), second.unwrap());
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn isolated_auth_updates_do_not_reuse_another_account_capabilities() {
        let (registry, _, database_path) = probe_registry();
        let config_dir = database_path.with_extension("profiles");
        let profiles =
            crate::subscription_profiles::SubscriptionProfileStore::new(config_dir.clone());
        let unrelated_profile = config_dir.join("unrelated-profile");
        std::fs::create_dir_all(&unrelated_profile).unwrap();

        for (engine, auth_file) in [("codex", "auth.json"), ("claude-code", ".credentials.json")] {
            let profile_home = profiles.profile_dir(engine).unwrap();
            let auth_path = profile_home.join(auth_file);
            let route = RuntimeRoute {
                engine_id: engine.into(),
                provider_id: "subscription".into(),
                provider_kind: "subscription".into(),
                provider_display_name: "Subscription".into(),
                route_label_snapshot: "Subscription / test-model".into(),
                model_id: "test-model".into(),
                model_label_snapshot: "Test Model".into(),
                default_reasoning_effort: ReasoningEffort::Auto,
                engine_profile_digest: "sha256:engine".into(),
                provider_launch_profile_ref: "provider:subscription:subscription".into(),
                provider_launch_profile_digest: "sha256:provider".into(),
                launch_config_digest: "sha256:launch".into(),
                pricing_basis_snapshot: crate::turn_start::PricingBasisSnapshot { profile: None },
            };
            let current_identity = || {
                CapabilityIdentity::from_route(
                    &route,
                    "configured:sha256:test".into(),
                    launch_profile_identity(&route, Some(&profile_home)).unwrap(),
                )
            };
            let unsigned_identity = current_identity();
            std::fs::write(&auth_path, "synthetic-first-account").unwrap();
            let first_identity = current_identity();
            assert_ne!(
                unsigned_identity.cache_key().unwrap(),
                first_identity.cache_key().unwrap()
            );
            let first = registry
                .resolve(first_identity.clone(), || async {
                    Ok((CapabilitySet::unknown("first-account"), "test".into()))
                })
                .await
                .unwrap();

            std::fs::write(unrelated_profile.join(auth_file), "unrelated-sentinel").unwrap();
            assert_eq!(first_identity, current_identity());
            let cached = registry
                .resolve(current_identity(), || async {
                    Err("unchanged account should use cached capabilities".into())
                })
                .await
                .unwrap();
            assert_eq!(first, cached);

            std::fs::write(&auth_path, "synthetic-second-account-with-a-new-length").unwrap();
            let changed_identity = current_identity();
            assert_ne!(
                first_identity.cache_key().unwrap(),
                changed_identity.cache_key().unwrap()
            );
            let refreshed = registry
                .resolve(changed_identity.clone(), || async {
                    Ok((CapabilitySet::unknown("second-account"), "test".into()))
                })
                .await
                .unwrap();
            assert_ne!(first.id, refreshed.id);
            assert_eq!(refreshed.capabilities.search.source, "second-account");

            let reopened = EngineCapabilityRegistry::new(registry.history.clone());
            let persisted = reopened
                .resolve(changed_identity, || async {
                    Err("updated account should use its own persisted capabilities".into())
                })
                .await
                .unwrap();
            assert_eq!(refreshed, persisted);

            std::fs::remove_file(auth_path).unwrap();
            assert_eq!(unsigned_identity, current_identity());
        }

        std::fs::remove_dir_all(config_dir).unwrap();
        let _ = std::fs::remove_file(database_path);
    }

    #[tokio::test]
    async fn failed_capability_probe_is_not_cached_or_persisted() {
        let (registry, identity, path) = probe_registry();
        assert!(registry
            .resolve(identity.clone(), || async { Err("unavailable".into()) })
            .await
            .is_err());
        assert!(registry
            .history
            .load_capability_snapshot(&identity.cache_key().unwrap())
            .unwrap()
            .is_none());
        let resolved = registry
            .resolve(identity, || async {
                Ok((CapabilitySet::unknown("retry"), "test".into()))
            })
            .await
            .unwrap();
        assert_eq!(resolved.capabilities.search.source, "retry");
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn expired_persisted_snapshot_is_refreshed_without_changing_its_reference() {
        let (registry, identity, path) = probe_registry();
        let mut previous = registry
            .resolve(identity.clone(), || async {
                Ok((CapabilitySet::unknown("old"), "test".into()))
            })
            .await
            .unwrap();
        previous.probed_at =
            crate::util::now_millis() - CAPABILITY_CACHE_TTL.as_millis() as i64 - 1;
        registry
            .history
            .update_capability_snapshot(&identity.cache_key().unwrap(), &previous)
            .unwrap();
        let reopened = EngineCapabilityRegistry::new(registry.history.clone());
        let next = reopened
            .resolve(identity, || async {
                Ok((CapabilitySet::unknown("fresh"), "test".into()))
            })
            .await
            .unwrap();
        assert_eq!(next.id, previous.id);
        assert_eq!(next.capabilities.search.source, "fresh");
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn explicit_refresh_does_not_reuse_a_fresh_persisted_snapshot() {
        let (registry, identity, path) = probe_registry();
        let previous = registry
            .resolve(identity.clone(), || async {
                Ok((CapabilitySet::unknown("old"), "test".into()))
            })
            .await
            .unwrap();
        registry.invalidate_engine("codex").unwrap();
        let next = registry
            .resolve(identity, || async {
                Ok((CapabilitySet::unknown("fresh"), "test".into()))
            })
            .await
            .unwrap();
        assert_eq!(next.id, previous.id);
        assert_eq!(next.capabilities.search.source, "fresh");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn explicit_invalidations_advance_even_within_one_clock_tick() {
        let (registry, _, path) = probe_registry();
        let previous = crate::util::now_millis() + 1000;
        registry
            .invalidated_at
            .lock()
            .unwrap()
            .insert("codex".into(), previous);
        registry.invalidate_engine("codex").unwrap();
        assert!(registry.invalidated_at.lock().unwrap()["codex"] > previous);
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn runtime_observation_wins_when_an_older_probe_finishes_late() {
        let (registry, identity, path) = probe_registry();
        let original = registry
            .resolve(identity.clone(), || async {
                Ok((CapabilitySet::unknown("initial"), "test".into()))
            })
            .await
            .unwrap();
        registry.invalidate_engine("codex").unwrap();
        let started = tokio::sync::Notify::new();
        let release = tokio::sync::Notify::new();
        let (resolved, observed) = tokio::join!(
            registry.resolve(identity.clone(), || async {
                started.notify_one();
                release.notified().await;
                Ok((CapabilitySet::unknown("late"), "test".into()))
            }),
            async {
                started.notified().await;
                let observed = registry.record_web_search_native(&original).unwrap();
                release.notify_one();
                observed
            }
        );
        assert_eq!(resolved.unwrap(), observed);
        let persisted = registry
            .history
            .load_capability_snapshot(&identity.cache_key().unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(
            persisted.capabilities.search.support,
            CapabilitySupport::Supported
        );
        assert_eq!(
            persisted.capabilities.search.source,
            "codex_runtime_observation"
        );
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn stale_observation_input_does_not_erase_other_runtime_evidence() {
        let (registry, identity, path) = probe_registry();
        let original = registry
            .resolve(identity, || async {
                Ok((CapabilitySet::unknown("initial"), "test".into()))
            })
            .await
            .unwrap();
        registry
            .record_auto_review_degraded(&original, "automode-unavailable")
            .unwrap();
        let observed = registry.record_web_search_native(&original).unwrap();
        assert_eq!(
            observed.capabilities.auto_approval.support,
            CapabilitySupport::Degraded
        );
        assert_eq!(
            observed.capabilities.search.support,
            CapabilitySupport::Supported
        );
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn invalidated_probe_does_not_publish_to_the_persistent_cache() {
        let (registry, identity, path) = probe_registry();
        let started = tokio::sync::Notify::new();
        let release = tokio::sync::Notify::new();
        let (result, ()) = tokio::join!(
            registry.resolve(identity.clone(), || async {
                started.notify_one();
                release.notified().await;
                Ok((CapabilitySet::unknown("late"), "test".into()))
            }),
            async {
                started.notified().await;
                registry.invalidate_engine("codex").unwrap();
                release.notify_one();
            }
        );
        assert_eq!(result.unwrap_err(), PROBE_INVALIDATED);
        assert!(registry
            .history
            .load_capability_snapshot(&identity.cache_key().unwrap())
            .unwrap()
            .is_none());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn binary_identity_reuses_hash_until_metadata_or_ttl_changes() {
        let path = std::env::temp_dir().join(format!(
            "helm-binary-identity-{}-{}.bin",
            std::process::id(),
            rand::random::<u64>()
        ));
        std::fs::write(&path, "first").unwrap();
        let first = binary_identity(path.to_str().unwrap()).unwrap();
        let canonical = path.canonicalize().unwrap();
        let checked_at = binary_identity_cache()
            .lock()
            .unwrap()
            .iter()
            .find(|(key, _)| key.0 == canonical)
            .unwrap()
            .1
             .1;
        assert_eq!(first, binary_identity(path.to_str().unwrap()).unwrap());
        assert_eq!(
            binary_identity_cache()
                .lock()
                .unwrap()
                .iter()
                .find(|(key, _)| key.0 == canonical)
                .unwrap()
                .1
                 .1,
            checked_at
        );
        std::fs::write(&path, "second-longer").unwrap();
        let second = binary_identity(path.to_str().unwrap()).unwrap();
        assert_ne!(first, second);
        {
            let mut cache = binary_identity_cache().lock().unwrap();
            for (key, (_, checked_at)) in cache.iter_mut().filter(|(key, _)| key.0 == canonical) {
                assert_eq!(key.0, canonical);
                *checked_at = Instant::now() - BINARY_IDENTITY_CACHE_TTL - Duration::from_secs(1);
            }
        }
        assert_eq!(second, binary_identity(path.to_str().unwrap()).unwrap());
        assert!(
            binary_identity_cache()
                .lock()
                .unwrap()
                .iter()
                .find(|(key, _)| key.0 == canonical)
                .unwrap()
                .1
                 .1
                .elapsed()
                < BINARY_IDENTITY_CACHE_TTL
        );
        std::fs::remove_file(path).unwrap();
    }
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
