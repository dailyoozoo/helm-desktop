use serde::{de::Error as _, Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::io::Read as _;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalStatus {
    Requested,
    AwaitingUser,
    Applying,
    Permitted,
    Denied,
    Failed,
    Expired,
    Executing,
    Completed,
}

impl ApprovalStatus {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Denied | Self::Expired | Self::Completed)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PermissionEffect {
    Allow,
    Ask,
    Deny,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PermissionScope {
    Once,
    Turn,
    Session,
    Project,
    Global,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Capability {
    FileRead,
    DirectoryList,
    FileWrite,
    ProcessExec,
    NetworkRequest,
    McpInvoke,
    Unknown(String),
}

impl Serialize for Capability {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let value = match self {
            Self::FileRead => "file_read",
            Self::DirectoryList => "directory_list",
            Self::FileWrite => "file_write",
            Self::ProcessExec => "process_exec",
            Self::NetworkRequest => "network_request",
            Self::McpInvoke => "mcp_invoke",
            Self::Unknown(name) => return serializer.serialize_str(&format!("unknown:{name}")),
        };
        serializer.serialize_str(value)
    }
}

impl<'de> Deserialize<'de> for Capability {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        match value.as_str() {
            "file_read" => Ok(Self::FileRead),
            "directory_list" => Ok(Self::DirectoryList),
            "file_write" => Ok(Self::FileWrite),
            "process_exec" => Ok(Self::ProcessExec),
            "network_request" => Ok(Self::NetworkRequest),
            "mcp_invoke" => Ok(Self::McpInvoke),
            value if value.starts_with("unknown:") => {
                Ok(Self::Unknown(value["unknown:".len()..].to_string()))
            }
            _ => Err(D::Error::custom(format!(
                "unsupported capability string: {value}"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionDescriptor {
    pub engine: String,
    pub session_id: String,
    pub turn_id: String,
    pub tool_call_id: String,
    pub principal: String,
    pub capability: Capability,
    pub operation: String,
    pub resources: Vec<String>,
    pub cwd: Option<String>,
    pub raw_input: Value,
    pub invalid_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeGrantMatcher {
    pub kind: &'static str,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeGrantDisplay {
    pub persistent_label: String,
    pub matcher_summary: String,
}

pub const RUNTIME_GRANT_CEILING_VERSION: &str = "runtime-safe-v1";
const PROCESS_EXEC_MATCHER_PREFIX: &str = "helm:process-exec:v1:";
/// Session 范围的 ProcessExec 授权按"可执行文件身份"（而不是精确 argv）匹配，见 ADR 0016。
/// 只用于 `PermissionScope::Session`；Turn/Project/Global/Always 仍用精确 argv 的
/// `process-exec:v1`。跨会话持久授权不放宽。
const PROCESS_EXEC_SESSION_MATCHER_PREFIX: &str = "helm:process-exec-session:v1:";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProcessExecMatcherV1 {
    canonical_executable: String,
    executable_sha256: String,
    argv_sha256: String,
    stdin_sha256: String,
}

/// Session 范围 ProcessExec 授权 matcher：只绑定可执行文件身份与引擎/工作目录，
/// 不含 argv/stdin，因此同一可执行文件在本会话内的命令可复用授权（ADR 0016）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProcessExecSessionMatcherV1 {
    canonical_executable: String,
    executable_sha256: String,
    engine: String,
    /// 授权时的工作目录；匹配时若动作 cwd 不同则 fail-closed 回退 Ask。None 表示不校验 cwd。
    cwd: Option<String>,
}

pub fn runtime_approval_adapter_version(engine: &str) -> Option<&'static str> {
    match engine {
        "claude-code" => Some("claude-runtime-hook-v1"),
        "codex" => Some("codex-app-server-v1"),
        _ => None,
    }
}

/// Builds a matcher from stable request facts only. Turn/session/tool-call ids are
/// deliberately excluded so a grant can be reused without widening its action.
pub fn runtime_grant_matcher(action: &ActionDescriptor) -> Option<RuntimeGrantMatcher> {
    if action.invalid_reason.is_some() || runtime_approval_adapter_version(&action.engine).is_none()
    {
        return None;
    }
    if action.operation == "WebSearch" {
        return safe_network_read_action_is_eligible(action).then(|| RuntimeGrantMatcher {
            kind: "tool_family",
            value: "WebSearch".to_string(),
        });
    }
    if action.operation == "WebFetch" {
        if !safe_network_read_action_is_eligible(action) {
            return None;
        }
        let raw = action.raw_input.get("url")?.as_str()?;
        let url = reqwest::Url::parse(raw).ok()?;
        let host = url.host_str()?.to_ascii_lowercase();
        let port = url.port_or_known_default()?;
        return Some(RuntimeGrantMatcher {
            kind: "method_origin",
            value: format!("GET_HEAD:https://{host}:{port}"),
        });
    }
    if action.capability == Capability::NetworkRequest {
        return None;
    }
    if action.capability == Capability::ProcessExec {
        return Some(RuntimeGrantMatcher {
            kind: "process_exec_v1",
            value: process_exec_matcher_pattern(action)?,
        });
    }

    let stable = serde_json::json!({
        "engine": action.engine,
        "principal": action.principal,
        "capability": action.capability,
        "operation": action.operation,
        "resources": action.resources,
        "rawInput": canonical_json(&action.raw_input),
    });
    let bytes = serde_json::to_vec(&stable).ok()?;
    Some(RuntimeGrantMatcher {
        kind: "exact_action",
        value: hex_digest(Sha256::digest(bytes)),
    })
}

/// User-facing copy derived from the same normalized matcher that is persisted.
/// Keeping this in the backend prevents UI labels from accidentally widening scope.
pub fn runtime_grant_display(action: &ActionDescriptor) -> Option<RuntimeGrantDisplay> {
    let matcher = runtime_grant_matcher(action)?;
    let (persistent_label, matcher_summary) = match matcher.kind {
        "tool_family" => (
            format!("此项目始终允许 {}", matcher.value),
            format!("当前引擎 + {} 工具族", matcher.value),
        ),
        "method_origin" => {
            let origin = matcher
                .value
                .strip_prefix("GET_HEAD:")
                .unwrap_or(&matcher.value);
            (
                format!("此项目始终允许读取 {origin}"),
                format!("当前引擎 + GET/HEAD + {origin}"),
            )
        }
        "process_exec_v1" => {
            let executable = process_exec_matcher_executable_label(action)
                .unwrap_or_else(|| "该程序".to_string());
            (
                format!("此项目永久允许执行 {executable}"),
                format!("当前引擎 + {executable}"),
            )
        }
        _ => {
            let target = action
                .resources
                .first()
                .map(String::as_str)
                .unwrap_or("当前请求");
            (
                "此项目永久允许此操作".to_string(),
                format!("当前引擎 + {} + {target}", action.operation),
            )
        }
    };
    Some(RuntimeGrantDisplay {
        persistent_label,
        matcher_summary,
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionIdentity {
    pub native_tool_call_id: String,
    pub tool_call_id: String,
    pub turn_epoch: u64,
}

impl ActionIdentity {
    pub fn derive(session_nonce: &str, turn_epoch: u64, native_tool_call_id: &str) -> Self {
        let digest = Sha256::digest(
            format!("{session_nonce}\0{turn_epoch}\0{native_tool_call_id}").as_bytes(),
        );
        Self {
            native_tool_call_id: native_tool_call_id.to_string(),
            tool_call_id: hex_digest(digest),
            turn_epoch,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ActionDigest(pub String);

impl ActionDigest {
    pub fn from_action(action: &ActionDescriptor) -> Self {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct DigestFacts<'a> {
            engine: &'a str,
            session_id: &'a str,
            turn_id: &'a str,
            tool_call_id: &'a str,
            principal: &'a str,
            capability: &'a Capability,
            operation: &'a str,
            resources: &'a [String],
            cwd: Option<&'a str>,
            raw_input: Value,
            invalid_reason: Option<&'a str>,
        }

        let facts = DigestFacts {
            engine: &action.engine,
            session_id: &action.session_id,
            turn_id: &action.turn_id,
            tool_call_id: &action.tool_call_id,
            principal: &action.principal,
            capability: &action.capability,
            operation: &action.operation,
            resources: &action.resources,
            cwd: action.cwd.as_deref(),
            raw_input: canonical_json(&action.raw_input),
            invalid_reason: action.invalid_reason.as_deref(),
        };
        let bytes = serde_json::to_vec(&facts).unwrap_or_default();
        Self(hex_digest(Sha256::digest(bytes)))
    }
}

fn canonical_json(value: &Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(values.iter().map(canonical_json).collect()),
        Value::Object(values) => {
            let mut keys = values.keys().collect::<Vec<_>>();
            keys.sort_unstable();
            let mut canonical = serde_json::Map::new();
            for key in keys {
                canonical.insert(key.clone(), canonical_json(&values[key]));
            }
            Value::Object(canonical)
        }
        value => value.clone(),
    }
}

fn hex_digest(bytes: impl AsRef<[u8]>) -> String {
    bytes
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn process_exec_matcher_pattern(action: &ActionDescriptor) -> Option<String> {
    let matcher = process_exec_matcher(action)?;
    Some(format!(
        "{PROCESS_EXEC_MATCHER_PREFIX}{}",
        serde_json::to_string(&matcher).ok()?
    ))
}

pub(crate) fn process_exec_rule_matches(pattern: &str, action: &ActionDescriptor) -> bool {
    if action.capability != Capability::ProcessExec {
        return false;
    }
    let Some(encoded) = pattern.strip_prefix(PROCESS_EXEC_MATCHER_PREFIX) else {
        return false;
    };
    let Ok(expected) = serde_json::from_str::<ProcessExecMatcherV1>(encoded) else {
        return false;
    };
    process_exec_matcher(action).is_some_and(|actual| actual == expected)
}

/// 从命令里提取可执行文件名，用于审批卡等用户可见文案。
fn process_exec_matcher_executable_label(action: &ActionDescriptor) -> Option<String> {
    let command = action.raw_input.get("command")?.as_str()?;
    let (executable, _) = command_executable_and_tail(command)?;
    let path = std::path::Path::new(executable);
    let name = path.file_name()?.to_string_lossy().into_owned();
    Some(if name.is_empty() {
        executable.to_string()
    } else {
        name
    })
}

/// Session 范围 ProcessExec 授权 matcher 的生成（ADR 0016）。只绑定可执行文件身份 +
/// 引擎 + 授权时 cwd，不含 argv/stdin，允许同一可执行文件的本会话命令复用授权。
pub(crate) fn process_exec_session_matcher_pattern(action: &ActionDescriptor) -> Option<String> {
    if action.invalid_reason.is_some() || action.capability != Capability::ProcessExec {
        return None;
    }
    let command = action.raw_input.get("command")?.as_str()?;
    let (executable, _) = command_executable_and_tail(command)?;
    let canonical = resolve_command_executable(executable, action.cwd.as_deref())?;
    let canonical_executable = canonical_executable_string(&canonical);
    let executable_sha256 = sha256_file(&canonical)?;
    let matcher = ProcessExecSessionMatcherV1 {
        canonical_executable,
        executable_sha256,
        engine: action.engine.clone(),
        cwd: action.cwd.clone(),
    };
    Some(format!(
        "{PROCESS_EXEC_SESSION_MATCHER_PREFIX}{}",
        serde_json::to_string(&matcher).ok()?
    ))
}

/// Session 范围 ProcessExec 授权 matcher 的匹配：不比较 argv/stdin，但必须同一
/// 可执行文件、同一引擎、同一 cwd（cwd 授权后改变则 fail-closed 回退 Ask）。
pub(crate) fn process_exec_session_rule_matches(pattern: &str, action: &ActionDescriptor) -> bool {
    if action.capability != Capability::ProcessExec {
        return false;
    }
    let Some(encoded) = pattern.strip_prefix(PROCESS_EXEC_SESSION_MATCHER_PREFIX) else {
        return false;
    };
    let Ok(expected) = serde_json::from_str::<ProcessExecSessionMatcherV1>(encoded) else {
        return false;
    };
    if action.engine != expected.engine {
        return false;
    }
    if expected.cwd.is_some()
        && !session_cwd_matches(expected.cwd.as_deref(), action.cwd.as_deref())
    {
        return false;
    }
    let Some(command) = action.raw_input.get("command").and_then(Value::as_str) else {
        return false;
    };
    let Some((executable, _)) = command_executable_and_tail(command) else {
        return false;
    };
    let Some(canonical) = resolve_command_executable(executable, action.cwd.as_deref()) else {
        return false;
    };
    canonical_executable_string(&canonical) == expected.canonical_executable
        && sha256_file(&canonical).as_deref() == Some(expected.executable_sha256.as_str())
}

fn session_cwd_matches(expected: Option<&str>, actual: Option<&str>) -> bool {
    match (expected, actual) {
        (Some(expected), Some(actual)) => {
            normalize_session_path(expected) == normalize_session_path(actual)
        }
        (None, None) => true,
        _ => false,
    }
}

fn normalize_session_path(value: &str) -> String {
    value.replace('\\', "/").to_ascii_lowercase()
}

fn process_exec_matcher(action: &ActionDescriptor) -> Option<ProcessExecMatcherV1> {
    if action.invalid_reason.is_some() || action.capability != Capability::ProcessExec {
        return None;
    }
    let command = action.raw_input.get("command")?.as_str()?;
    let (executable, argv_tail) = command_executable_and_tail(command)?;
    let canonical = resolve_command_executable(executable, action.cwd.as_deref())?;
    let canonical_executable = canonical_executable_string(&canonical);
    let executable_sha256 = sha256_file(&canonical)?;
    let argv_bytes = serde_json::to_vec(&serde_json::json!({
        "canonicalExecutable": canonical_executable,
        "argvTail": argv_tail,
    }))
    .ok()?;
    let stdin = canonical_json(action.raw_input.get("stdin").unwrap_or(&Value::Null));
    let stdin_bytes = serde_json::to_vec(&stdin).ok()?;
    Some(ProcessExecMatcherV1 {
        canonical_executable,
        executable_sha256,
        argv_sha256: hex_digest(Sha256::digest(argv_bytes)),
        stdin_sha256: hex_digest(Sha256::digest(stdin_bytes)),
    })
}

fn command_executable_and_tail(command: &str) -> Option<(&str, &str)> {
    let command = command.trim_start();
    if let Some(rest) = command.strip_prefix('"') {
        let end = rest.find('"')?;
        return Some((&rest[..end], rest[end + 1..].trim_start()));
    }
    let end = command.find(char::is_whitespace).unwrap_or(command.len());
    let executable = &command[..end];
    (!executable.is_empty()).then(|| (executable, command[end..].trim_start()))
}

fn resolve_command_executable(executable: &str, cwd: Option<&str>) -> Option<PathBuf> {
    let path = Path::new(executable);
    if path.is_absolute() || executable.contains(['/', '\\']) {
        let candidate = if path.is_absolute() {
            path.to_path_buf()
        } else {
            Path::new(cwd?).join(path)
        };
        return canonical_executable(candidate);
    }

    let path_env = std::env::var_os("PATH")?;
    for directory in std::env::split_paths(&path_env) {
        for candidate in executable_candidates(&directory, executable) {
            if let Some(canonical) = canonical_executable(candidate) {
                return Some(canonical);
            }
        }
    }
    None
}

fn executable_candidates(directory: &Path, executable: &str) -> Vec<PathBuf> {
    let direct = directory.join(executable);
    #[cfg(windows)]
    {
        if Path::new(executable).extension().is_some() {
            return vec![direct];
        }
        let extensions =
            std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_string());
        let mut candidates = vec![direct];
        candidates.extend(
            extensions
                .split(';')
                .filter(|extension| !extension.is_empty())
                .map(|extension| directory.join(format!("{executable}{extension}"))),
        );
        candidates
    }
    #[cfg(not(windows))]
    {
        vec![direct]
    }
}

fn canonical_executable(candidate: PathBuf) -> Option<PathBuf> {
    let canonical = candidate.canonicalize().ok()?;
    canonical.is_file().then_some(canonical)
}

fn canonical_executable_string(path: &Path) -> String {
    let value = path.to_string_lossy().replace('\\', "/");
    value.strip_prefix("//?/").unwrap_or(&value).to_string()
}

fn sha256_file(path: &Path) -> Option<String> {
    let mut file = std::fs::File::open(path).ok()?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).ok()?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Some(hex_digest(digest.finalize()))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionScopeBinding {
    pub tool_call_id: Option<String>,
    pub turn_id: Option<String>,
    pub session_id: Option<String>,
    pub project_root: Option<String>,
}

impl Default for PermissionScopeBinding {
    fn default() -> Self {
        Self {
            tool_call_id: None,
            turn_id: None,
            session_id: None,
            project_root: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionRule {
    pub id: String,
    #[serde(default = "default_principal")]
    pub principal: String,
    pub effect: PermissionEffect,
    pub scope: PermissionScope,
    #[serde(default)]
    pub scope_binding: PermissionScopeBinding,
    pub engine: Option<String>,
    pub capability: Capability,
    pub operation: Option<String>,
    pub resource_pattern: Option<String>,
    pub created_at: i64,
    pub expires_at: Option<i64>,
    pub max_uses: Option<u32>,
    #[serde(default)]
    pub uses: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionDecision {
    pub effect: PermissionEffect,
    pub reason: String,
    pub rule_id: Option<String>,
    pub policy_version: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineCapabilityManifest {
    pub engine: String,
    pub version: String,
    pub supports_defer: bool,
    pub supports_parallel_tool_approval: bool,
    pub supports_native_sandbox: bool,
    pub verified: bool,
}

fn default_principal() -> String {
    "main-agent".to_string()
}

pub fn build_once_rule_from_action(action: &ActionDescriptor, created_at: i64) -> PermissionRule {
    PermissionRule {
        id: format!(
            "approval-once:{}:{}:{}",
            action.session_id, action.turn_id, action.tool_call_id
        ),
        principal: action.principal.clone(),
        effect: PermissionEffect::Allow,
        scope: PermissionScope::Once,
        scope_binding: PermissionScopeBinding {
            tool_call_id: Some(action.tool_call_id.clone()),
            turn_id: Some(action.turn_id.clone()),
            session_id: Some(action.session_id.clone()),
            project_root: None,
        },
        engine: Some(action.engine.clone()),
        capability: action.capability.clone(),
        operation: Some(action.operation.clone()),
        resource_pattern: action.resources.first().cloned(),
        created_at,
        expires_at: None,
        max_uses: Some(1),
        uses: 0,
    }
}

pub fn build_always_rule_from_action(action: &ActionDescriptor, created_at: i64) -> PermissionRule {
    let resource_pattern = rule_resource_pattern(action);
    let fingerprint = serde_json::to_vec(&serde_json::json!({
        "principal": action.principal,
        "engine": action.engine,
        "capability": action.capability,
        "operation": action.operation,
        "resource": resource_pattern,
    }))
    .map(|bytes| {
        Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    })
    .unwrap_or_else(|_| "invalid".to_string());
    PermissionRule {
        id: format!("approval-always:{fingerprint}"),
        principal: action.principal.clone(),
        effect: PermissionEffect::Allow,
        scope: PermissionScope::Global,
        scope_binding: PermissionScopeBinding::default(),
        engine: Some(action.engine.clone()),
        capability: action.capability.clone(),
        operation: Some(action.operation.clone()),
        resource_pattern,
        created_at,
        expires_at: None,
        max_uses: None,
        uses: 0,
    }
}

pub fn build_session_rule_from_action(
    action: &ActionDescriptor,
    created_at: i64,
) -> PermissionRule {
    let resource_pattern = if action.capability == Capability::ProcessExec {
        // ADR 0016：Session 范围 ProcessExec 授权只可执行文件，见下方说明。
        process_exec_session_matcher_pattern(action)
    } else {
        rule_resource_pattern(action)
    };
    let fingerprint = serde_json::to_vec(&serde_json::json!({
        "principal": action.principal,
        "sessionId": action.session_id,
        "engine": action.engine,
        "capability": action.capability,
        "operation": action.operation,
        "resource": resource_pattern,
    }))
    .map(|bytes| hex_digest(Sha256::digest(bytes)))
    .unwrap_or_else(|_| "invalid".to_string());
    PermissionRule {
        id: format!("approval-session:{fingerprint}"),
        principal: action.principal.clone(),
        effect: PermissionEffect::Allow,
        scope: PermissionScope::Session,
        scope_binding: PermissionScopeBinding {
            tool_call_id: None,
            turn_id: None,
            session_id: Some(action.session_id.clone()),
            project_root: None,
        },
        engine: Some(action.engine.clone()),
        capability: action.capability.clone(),
        operation: Some(action.operation.clone()),
        resource_pattern,
        created_at,
        expires_at: None,
        max_uses: None,
        uses: 0,
    }
}

pub fn build_turn_rule_from_action(action: &ActionDescriptor, created_at: i64) -> PermissionRule {
    let resource_pattern = rule_resource_pattern(action);
    let fingerprint = serde_json::to_vec(&serde_json::json!({
        "principal": action.principal,
        "turnId": action.turn_id,
        "engine": action.engine,
        "capability": action.capability,
        "operation": action.operation,
        "resource": resource_pattern,
    }))
    .map(|bytes| hex_digest(Sha256::digest(bytes)))
    .unwrap_or_else(|_| "invalid".to_string());
    PermissionRule {
        id: format!("approval-turn:{fingerprint}"),
        principal: action.principal.clone(),
        effect: PermissionEffect::Allow,
        scope: PermissionScope::Turn,
        scope_binding: PermissionScopeBinding {
            tool_call_id: None,
            turn_id: Some(action.turn_id.clone()),
            session_id: Some(action.session_id.clone()),
            project_root: None,
        },
        engine: Some(action.engine.clone()),
        capability: action.capability.clone(),
        operation: Some(action.operation.clone()),
        resource_pattern,
        created_at,
        expires_at: None,
        max_uses: None,
        uses: 0,
    }
}

pub fn build_project_rule_from_action(
    action: &ActionDescriptor,
    created_at: i64,
) -> Result<PermissionRule, String> {
    let project_root = action
        .cwd
        .clone()
        .ok_or_else(|| "project approval requires a stable policy cwd".to_string())?;
    let resource_pattern = rule_resource_pattern(action);
    let fingerprint = hex_digest(Sha256::digest(
        serde_json::to_vec(&serde_json::json!({
            "principal": action.principal,
            "engine": action.engine,
            "capability": action.capability,
            "operation": action.operation,
            "resource": resource_pattern,
            "projectRoot": project_root,
        }))
        .map_err(|error| error.to_string())?,
    ));
    Ok(PermissionRule {
        id: format!("approval-project:{fingerprint}"),
        principal: action.principal.clone(),
        effect: PermissionEffect::Allow,
        scope: PermissionScope::Project,
        scope_binding: PermissionScopeBinding {
            tool_call_id: None,
            turn_id: None,
            session_id: None,
            project_root: Some(project_root),
        },
        engine: Some(action.engine.clone()),
        capability: action.capability.clone(),
        operation: Some(action.operation.clone()),
        resource_pattern,
        created_at,
        expires_at: None,
        max_uses: None,
        uses: 0,
    })
}

fn rule_resource_pattern(action: &ActionDescriptor) -> Option<String> {
    if action.capability == Capability::ProcessExec {
        process_exec_matcher_pattern(action)
    } else {
        action.resources.first().cloned()
    }
}

pub fn normalize_tool_action(
    engine: &str,
    session_id: &str,
    turn_id: &str,
    tool_call_id: &str,
    tool_name: &str,
    input: &Value,
    cwd: Option<&str>,
) -> ActionDescriptor {
    normalize_tool_action_for_principal(
        engine,
        session_id,
        turn_id,
        tool_call_id,
        "main-agent",
        tool_name,
        input,
        cwd,
    )
}

pub(crate) fn sensitive_path_is_denied(path: &str) -> bool {
    path.replace('\\', "/").split('/').any(|component| {
        let component = component.trim_end_matches(['.', ' ']).to_ascii_lowercase();
        component.starts_with(".env")
            || matches!(
                component.as_str(),
                ".ssh" | ".aws" | ".azure" | ".gnupg" | ".git-credentials" | "credentials.json"
            )
    })
}

/// `SafeRead` 只覆盖可证明落在当前策略 cwd 内、且未命中保护路径的结构化读取。
pub(crate) fn safe_read_action_is_eligible(action: &ActionDescriptor) -> bool {
    if action.invalid_reason.is_some()
        || !matches!(
            action.capability,
            Capability::FileRead | Capability::DirectoryList
        )
    {
        return false;
    }
    let Some(cwd) = action.cwd.as_deref().filter(|cwd| !cwd.is_empty()) else {
        return false;
    };
    let resources = if action.resources.is_empty() {
        if !matches!(
            action.operation.to_ascii_lowercase().as_str(),
            "glob" | "grep" | "ls" | "list"
        ) {
            return false;
        }
        vec!["."]
    } else {
        action.resources.iter().map(String::as_str).collect()
    };
    resources.into_iter().all(|resource| {
        !resource.is_empty()
            && !path_uses_alternate_data_stream(resource)
            && resolve_workspace_path(cwd, cwd, resource)
                .is_some_and(|resolved| !sensitive_path_is_denied(&resolved))
    })
}

/// 判定结构化写目标是否可安全放行：所有资源都可证明落在 `workspace_root` 内、
/// 且不命中保护路径、不是 ADS。任一资源越界/敏感即返回 false（fail-closed）。
pub(crate) fn safe_file_write_resources_within(
    action: &ActionDescriptor,
    workspace_root: &str,
) -> bool {
    if action.invalid_reason.is_some() || action.capability != Capability::FileWrite {
        return false;
    }
    if workspace_root.is_empty() || action.resources.is_empty() {
        return false;
    }
    action.resources.iter().all(|resource| {
        !resource.is_empty()
            && !path_uses_alternate_data_stream(resource)
            && resolve_workspace_path(workspace_root, workspace_root, resource)
                .is_some_and(|resolved| !sensitive_path_is_denied(&resolved))
    })
}

pub(crate) fn safe_network_read_action_is_eligible(action: &ActionDescriptor) -> bool {
    if action.invalid_reason.is_some() || action.capability != Capability::NetworkRequest {
        return false;
    }
    if action.operation == "WebSearch" {
        return ["query", "q", "search_term"]
            .iter()
            .filter_map(|key| action.raw_input.get(*key).and_then(Value::as_str))
            .next()
            .is_some_and(|query| {
                !query.trim().is_empty() && !query.contains("-----BEGIN") && !query.contains("sk-")
            });
    }
    if action.operation != "WebFetch" {
        return false;
    }
    let Some(raw) = action.raw_input.get("url").and_then(Value::as_str) else {
        return false;
    };
    let Ok(url) = reqwest::Url::parse(raw) else {
        return false;
    };
    if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some() {
        return false;
    }
    let Some(host) = url.host_str().map(str::to_ascii_lowercase) else {
        return false;
    };
    if host == "localhost"
        || host.ends_with(".localhost")
        || host.ends_with(".local")
        || host == "metadata.google.internal"
        || host == "169.254.169.254"
    {
        return false;
    }
    match host.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V4(ip)) => {
            !(ip.is_private() || ip.is_loopback() || ip.is_link_local() || ip.is_unspecified())
        }
        Ok(std::net::IpAddr::V6(ip)) => {
            !(ip.is_loopback() || ip.is_unspecified() || ip.is_unique_local())
        }
        Err(_) => true,
    }
}

#[cfg(windows)]
fn path_is_unsafe_windows_namespace(path: &str) -> bool {
    let normalized = path.replace('\\', "/");
    if normalized.starts_with("//") {
        return true;
    }
    normalized.split('/').enumerate().any(|(index, component)| {
        if component.is_empty()
            || matches!(component, "." | "..")
            || (index == 0 && component.len() == 2 && component.as_bytes().get(1) == Some(&b':'))
        {
            return false;
        }
        if component.ends_with(['.', ' ']) {
            return true;
        }
        let device_name = component
            .trim_end_matches(['.', ' '])
            .split('.')
            .next()
            .unwrap_or_default()
            .to_ascii_uppercase();
        matches!(
            device_name.as_str(),
            "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
        ) || device_name
            .strip_prefix("COM")
            .or_else(|| device_name.strip_prefix("LPT"))
            .is_some_and(|suffix| suffix.len() == 1 && matches!(suffix.as_bytes()[0], b'1'..=b'9'))
    })
}

#[cfg(not(windows))]
fn path_is_unsafe_windows_namespace(_: &str) -> bool {
    false
}

#[cfg(windows)]
fn path_uses_alternate_data_stream(path: &str) -> bool {
    let path = path.replace('\\', "/");
    let path = if path.as_bytes().get(1).is_some_and(|byte| *byte == b':') {
        &path[2..]
    } else {
        path.as_str()
    };
    path.split('/').any(|component| component.contains(':'))
}

#[cfg(not(windows))]
fn path_uses_alternate_data_stream(_: &str) -> bool {
    false
}

fn resolve_workspace_path(cwd: &str, workspace_root: &str, requested: &str) -> Option<String> {
    if path_is_unsafe_windows_namespace(requested) {
        return None;
    }
    let requested = requested.replace('\\', "/");
    let combined = if requested.starts_with('/')
        || requested
            .as_bytes()
            .get(1)
            .is_some_and(|byte| *byte == b':')
    {
        requested
    } else {
        format!("{}/{}", cwd.trim_end_matches(['/', '\\']), requested)
    };
    let resource = normalize_lexical_path(&combined);
    let root = normalize_lexical_path(workspace_root);
    let resource_lower = resource.to_lowercase();
    let root_lower = root.trim_end_matches('/').to_lowercase();
    (resource_lower == root_lower || resource_lower.starts_with(&format!("{root_lower}/")))
        .then_some(resource)
}

fn normalize_lexical_path(path: &str) -> String {
    let path = path.replace('\\', "/");
    let path = path.strip_prefix("//?/").unwrap_or(&path).to_string();
    let mut prefix = String::new();
    let mut remainder = path.as_str();
    if path.as_bytes().get(1).is_some_and(|byte| *byte == b':') {
        prefix = path[..2].to_string();
        remainder = path[2..].trim_start_matches('/');
    } else if path.starts_with('/') {
        prefix = "/".to_string();
        remainder = path.trim_start_matches('/');
    }
    let mut parts = Vec::new();
    for part in remainder.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            part => parts.push(part),
        }
    }
    if prefix == "/" {
        format!("/{}", parts.join("/"))
    } else if prefix.is_empty() {
        parts.join("/")
    } else if parts.is_empty() {
        format!("{prefix}/")
    } else {
        format!("{prefix}/{}", parts.join("/"))
    }
}

pub fn normalize_tool_action_for_principal(
    engine: &str,
    session_id: &str,
    turn_id: &str,
    tool_call_id: &str,
    principal: &str,
    tool_name: &str,
    input: &Value,
    cwd: Option<&str>,
) -> ActionDescriptor {
    let (mut capability, operation, resource_key, resource_required) = match tool_name {
        "Read" | "Glob" | "Grep" | "LS" => (
            Capability::FileRead,
            tool_name.to_string(),
            file_resource_key(input),
            false,
        ),
        "Write" | "Edit" | "MultiEdit" | "NotebookEdit" => (
            Capability::FileWrite,
            tool_name.to_string(),
            file_resource_key(input),
            true,
        ),
        "Bash" => (
            Capability::ProcessExec,
            input
                .get("command")
                .and_then(Value::as_str)
                .and_then(command_executable_token)
                .unwrap_or(tool_name)
                .to_string(),
            None,
            false,
        ),
        "WebFetch" => (
            Capability::NetworkRequest,
            tool_name.to_string(),
            Some("url"),
            true,
        ),
        "WebSearch" => (
            Capability::NetworkRequest,
            tool_name.to_string(),
            None,
            false,
        ),
        name if name.starts_with("mcp__") => (Capability::McpInvoke, name.to_string(), None, false),
        name => (
            Capability::Unknown(name.to_string()),
            name.to_string(),
            None,
            false,
        ),
    };

    let resources = resource_key
        .and_then(|key| input.get(key))
        .and_then(Value::as_str)
        .filter(|resource| !resource.is_empty())
        .map(|resource| vec![resource.to_string()])
        .unwrap_or_default();
    let invalid_reason = if resource_required && resources.is_empty() {
        capability = Capability::Unknown(format!("invalid:{tool_name}"));
        Some(format!("{tool_name} requires a non-empty string resource"))
    } else {
        None
    };

    ActionDescriptor {
        engine: engine.to_string(),
        session_id: session_id.to_string(),
        turn_id: turn_id.to_string(),
        tool_call_id: tool_call_id.to_string(),
        principal: principal.to_string(),
        capability,
        operation,
        resources,
        cwd: cwd.map(str::to_string),
        raw_input: input.clone(),
        invalid_reason,
    }
}

fn file_resource_key(input: &Value) -> Option<&'static str> {
    ["file_path", "path", "notebook_path"]
        .into_iter()
        .find(|key| input.get(key).and_then(Value::as_str).is_some())
}

/// Preserve a quoted Windows executable path instead of truncating it at whitespace.
pub(crate) fn command_executable_token(command: &str) -> Option<&str> {
    let command = command.trim_start();
    if let Some(rest) = command.strip_prefix('"') {
        return rest.find('"').map(|end| &rest[..end]);
    }
    command.split_whitespace().next()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn p95(mut samples: Vec<std::time::Duration>) -> std::time::Duration {
        samples.sort_unstable();
        samples[(samples.len() * 95 / 100).min(samples.len() - 1)]
    }

    #[test]
    fn process_exec_preserves_quoted_windows_executable_path() {
        let action = normalize_tool_action(
            "codex",
            "session-1",
            "turn-1",
            "tool-1",
            "Bash",
            &json!({
                "command": "\"C:\\Program Files\\PowerShell\\7\\pwsh.exe\" -Command echo ok"
            }),
            Some("C:/workspace"),
        );
        assert_eq!(
            action.operation,
            "C:\\Program Files\\PowerShell\\7\\pwsh.exe"
        );
    }

    #[test]
    fn permission_classification_fast_paths_meet_local_p95_budgets() {
        let mut read_samples = Vec::with_capacity(2_000);
        let mut network_samples = Vec::with_capacity(2_000);
        for index in 0..2_000 {
            let started = std::time::Instant::now();
            let read = normalize_tool_action(
                "claude-code",
                "history",
                "turn-1",
                &format!("read-{index}"),
                "Read",
                &json!({"file_path":"D:/repo/src/main.rs"}),
                Some("D:/repo"),
            );
            assert!(safe_read_action_is_eligible(&read));
            read_samples.push(started.elapsed());

            let started = std::time::Instant::now();
            let network = normalize_tool_action(
                "claude-code",
                "history",
                "turn-1",
                &format!("network-{index}"),
                "WebFetch",
                &json!({"url":"https://example.com/docs?q=helm","method":"GET"}),
                Some("D:/repo"),
            );
            assert!(safe_network_read_action_is_eligible(&network));
            network_samples.push(started.elapsed());
        }
        let read_p95 = p95(read_samples);
        let network_p95 = p95(network_samples);
        eprintln!(
            "permission-fast-path-p95 safe_read_us={} safe_network_read_us={}",
            read_p95.as_micros(),
            network_p95.as_micros()
        );
        assert!(read_p95 < std::time::Duration::from_millis(3));
        assert!(network_p95 < std::time::Duration::from_millis(5));
    }

    #[test]
    fn action_identity_is_derived_from_session_nonce_turn_epoch_and_native_call_id() {
        let identity = ActionIdentity::derive("session-nonce", 4, "call-7");

        assert_eq!(identity.native_tool_call_id, "call-7");
        assert_ne!(identity.tool_call_id, "call-7");
        assert_eq!(identity.tool_call_id.len(), 64);
        assert_eq!(identity.turn_epoch, 4);
    }

    #[test]
    fn action_digest_changes_when_any_approved_execution_fact_changes() {
        let first = ActionDescriptor {
            engine: "codex".to_string(),
            session_id: "session-1".to_string(),
            turn_id: "turn-1".to_string(),
            tool_call_id: "tool-1".to_string(),
            principal: "main-agent".to_string(),
            capability: Capability::FileRead,
            operation: "read_file".to_string(),
            resources: vec!["D:/repo/a.txt".to_string()],
            cwd: Some("D:/repo".to_string()),
            raw_input: json!({"path":"D:/repo/a.txt"}),
            invalid_reason: None,
        };
        let mut second = first.clone();
        second.resources = vec!["D:/repo/b.txt".to_string()];

        assert_ne!(
            ActionDigest::from_action(&first),
            ActionDigest::from_action(&second)
        );
    }

    #[test]
    fn safe_read_eligibility_requires_workspace_bounded_non_sensitive_resources() {
        let inside = normalize_tool_action(
            "claude-code",
            "session-1",
            "turn-1",
            "tool-1",
            "Read",
            &json!({"file_path":"src/lib.rs"}),
            Some("D:/repo"),
        );
        assert!(safe_read_action_is_eligible(&inside));

        for (tool_name, input) in [
            ("Read", json!({"file_path":"D:/outside.txt"})),
            ("Read", json!({"file_path":".env.local"})),
            ("Read", json!({})),
            ("Bash", json!({"command":"ls -la"})),
        ] {
            let action = normalize_tool_action(
                "claude-code",
                "session-1",
                "turn-1",
                "tool-denied",
                tool_name,
                &input,
                Some("D:/repo"),
            );
            assert!(
                !safe_read_action_is_eligible(&action),
                "unsafe action was classified as SafeRead: {tool_name} {input}"
            );
        }

        for tool_name in ["Glob", "Grep", "LS"] {
            let action = normalize_tool_action(
                "claude-code",
                "session-1",
                "turn-1",
                "tool-default-cwd",
                tool_name,
                &json!({}),
                Some("D:/repo"),
            );
            assert!(
                safe_read_action_is_eligible(&action),
                "{tool_name} without an explicit path should remain bounded to cwd"
            );
        }
    }

    #[test]
    fn safe_file_write_accepts_bounded_non_sensitive_targets_and_rejects_outside_protected() {
        let root = "D:/repo";
        let write_action = |resource: &str| {
            let mut action = normalize_tool_action(
                "codex",
                "session-1",
                "turn-1",
                "tool-1",
                "Write",
                &json!({"file_path": resource}),
                Some(root),
            );
            action.operation = "fileChange".to_string();
            action.resources = vec![resource.to_string()];
            action
        };
        assert!(crate::permissions::safe_file_write_resources_within(
            &write_action("src/lib.rs"),
            root,
        ));
        assert!(crate::permissions::safe_file_write_resources_within(
            &write_action("D:/repo/new-file.txt"),
            root,
        ));
        for resource in [
            "D:/outside.txt",
            ".env.local",
            ".ssh/config",
            "D:/repo/../secret.txt",
            "nul",
            "D:/repo/data:stream",
        ] {
            let action = write_action(resource);
            assert!(
                !crate::permissions::safe_file_write_resources_within(&action, root),
                "unsafe file write target was auto-approved: {resource}"
            );
        }
        let empty_root = write_action("src/lib.rs");
        assert!(!crate::permissions::safe_file_write_resources_within(
            &empty_root,
            "",
        ));
    }

    #[test]
    fn safe_network_read_accepts_public_structured_reads_and_rejects_sensitive_targets() {
        let action = |tool: &str, input: serde_json::Value| {
            normalize_tool_action(
                "claude-code",
                "session-1",
                "turn-1",
                "tool-1",
                tool,
                &input,
                Some("D:/repo"),
            )
        };
        assert!(safe_network_read_action_is_eligible(&action(
            "WebSearch",
            json!({"query":"Rust tokio docs"}),
        )));
        assert!(safe_network_read_action_is_eligible(&action(
            "WebFetch",
            json!({"url":"https://example.com/docs?q=rust"}),
        )));
        for url in [
            "http://example.com",
            "https://localhost/data",
            "https://127.0.0.1/data",
            "https://169.254.169.254/latest/meta-data",
            "https://user:pass@example.com/data",
        ] {
            assert!(!safe_network_read_action_is_eligible(&action(
                "WebFetch",
                json!({"url":url}),
            )));
        }
    }

    #[test]
    fn runtime_grant_display_is_derived_from_the_same_safe_matcher() {
        let action = normalize_tool_action(
            "claude-code",
            "session-1",
            "turn-1",
            "tool-1",
            "WebFetch",
            &json!({"url":"https://example.com/docs"}),
            Some("D:/repo"),
        );
        let matcher = runtime_grant_matcher(&action).expect("safe WebFetch should have a matcher");
        let display =
            runtime_grant_display(&action).expect("safe WebFetch should have display copy");
        assert_eq!(matcher.kind, "method_origin");
        assert!(display.persistent_label.contains("example.com"));
        assert!(display.matcher_summary.contains("GET/HEAD"));
        assert!(!display.matcher_summary.contains("当前项目"));

        let unsafe_action = normalize_tool_action(
            "claude-code",
            "session-1",
            "turn-1",
            "tool-2",
            "WebFetch",
            &json!({"url":"http://127.0.0.1:8080/secret"}),
            Some("D:/repo"),
        );
        assert!(runtime_grant_display(&unsafe_action).is_none());
    }

    #[test]
    fn approval_status_only_marks_completed_or_unsuccessful_outcomes_terminal() {
        for status in [
            ApprovalStatus::Requested,
            ApprovalStatus::AwaitingUser,
            ApprovalStatus::Applying,
            ApprovalStatus::Permitted,
            ApprovalStatus::Failed,
            ApprovalStatus::Executing,
        ] {
            assert!(
                !status.is_terminal(),
                "{status:?} should remain non-terminal"
            );
        }

        for status in [
            ApprovalStatus::Denied,
            ApprovalStatus::Expired,
            ApprovalStatus::Completed,
        ] {
            assert!(status.is_terminal(), "{status:?} should be terminal");
        }
    }

    #[test]
    fn approval_status_serializes_and_deserializes_exact_snake_case_values() {
        let cases = [
            (ApprovalStatus::Requested, "requested"),
            (ApprovalStatus::AwaitingUser, "awaiting_user"),
            (ApprovalStatus::Applying, "applying"),
            (ApprovalStatus::Permitted, "permitted"),
            (ApprovalStatus::Denied, "denied"),
            (ApprovalStatus::Failed, "failed"),
            (ApprovalStatus::Expired, "expired"),
            (ApprovalStatus::Executing, "executing"),
            (ApprovalStatus::Completed, "completed"),
        ];

        for (status, encoded) in cases {
            assert_eq!(serde_json::to_value(status).unwrap(), json!(encoded));
            assert_eq!(
                serde_json::from_value::<ApprovalStatus>(json!(encoded)).unwrap(),
                status
            );
        }
    }

    #[test]
    fn capability_serializes_as_stable_strings_and_round_trips() {
        let cases = [
            (Capability::FileRead, "file_read"),
            (Capability::FileWrite, "file_write"),
            (Capability::ProcessExec, "process_exec"),
            (Capability::NetworkRequest, "network_request"),
            (Capability::McpInvoke, "mcp_invoke"),
            (
                Capability::Unknown("CustomTool".to_string()),
                "unknown:CustomTool",
            ),
        ];

        for (capability, encoded) in cases {
            assert_eq!(serde_json::to_value(&capability).unwrap(), json!(encoded));
            assert_eq!(
                serde_json::from_value::<Capability>(json!(encoded)).unwrap(),
                capability
            );
        }
    }

    #[test]
    fn ls_tool_and_bash_ls_preserve_distinct_action_semantics() {
        let file_action = normalize_tool_action(
            "claude-code",
            "session-1",
            "turn-1",
            "tool-1",
            "LS",
            &json!({ "path": "src" }),
            Some("D:/repo"),
        );
        let process_action = normalize_tool_action(
            "claude-code",
            "session-1",
            "turn-1",
            "tool-2",
            "Bash",
            &json!({ "command": "ls -la src" }),
            Some("D:/repo"),
        );

        assert_eq!(file_action.capability, Capability::FileRead);
        assert_eq!(file_action.operation, "LS");
        assert_eq!(file_action.resources, vec!["src"]);
        assert_eq!(file_action.invalid_reason, None);
        assert_eq!(process_action.capability, Capability::ProcessExec);
        assert_eq!(process_action.operation, "ls");
        assert_eq!(process_action.resources, Vec::<String>::new());
        assert_eq!(process_action.raw_input["command"], "ls -la src");
        assert_eq!(process_action.invalid_reason, None);
    }

    #[test]
    fn notebook_edit_extracts_notebook_path_as_a_file_resource() {
        let action = normalize_tool_action(
            "claude-code",
            "session-1",
            "turn-1",
            "tool-1",
            "NotebookEdit",
            &json!({ "notebook_path": "notes/demo.ipynb", "cell_id": "cell-1" }),
            Some("D:/repo"),
        );

        assert_eq!(action.capability, Capability::FileWrite);
        assert_eq!(action.resources, vec!["notes/demo.ipynb"]);
        assert_eq!(action.invalid_reason, None);
    }

    #[test]
    fn mcp_tool_keeps_the_full_tool_name_as_its_operation() {
        let action = normalize_tool_action(
            "claude-code",
            "session-1",
            "turn-1",
            "tool-1",
            "mcp__github__create_issue",
            &json!({ "repo": "helm" }),
            None,
        );

        assert_eq!(action.capability, Capability::McpInvoke);
        assert_eq!(action.operation, "mcp__github__create_issue");
        assert_eq!(action.invalid_reason, None);
    }

    #[test]
    fn unknown_tool_preserves_its_name_and_identity_context() {
        let action = normalize_tool_action(
            "codex",
            "session-9",
            "turn-4",
            "tool-7",
            "CustomTool",
            &json!({ "value": 1 }),
            Some("D:/repo"),
        );

        assert_eq!(
            action.capability,
            Capability::Unknown("CustomTool".to_string())
        );
        assert_eq!(action.engine, "codex");
        assert_eq!(action.session_id, "session-9");
        assert_eq!(action.turn_id, "turn-4");
        assert_eq!(action.tool_call_id, "tool-7");
        assert_eq!(action.principal, "main-agent");
        assert_eq!(action.cwd.as_deref(), Some("D:/repo"));
        assert_eq!(action.invalid_reason, None);
    }

    #[test]
    fn web_fetch_extracts_url_as_network_resource() {
        let action = normalize_tool_action(
            "claude-code",
            "session-1",
            "turn-1",
            "tool-1",
            "WebFetch",
            &json!({ "url": "https://example.com/docs" }),
            None,
        );

        assert_eq!(action.capability, Capability::NetworkRequest);
        assert_eq!(action.resources, vec!["https://example.com/docs"]);
        assert_eq!(action.invalid_reason, None);
    }

    #[test]
    fn write_without_a_string_path_fails_closed() {
        for input in [json!({}), json!({ "file_path": 42 })] {
            let action = normalize_tool_action(
                "claude-code",
                "session-1",
                "turn-1",
                "tool-1",
                "Write",
                &input,
                Some("D:/repo"),
            );

            assert_eq!(
                action.capability,
                Capability::Unknown("invalid:Write".to_string())
            );
            assert_eq!(action.operation, "Write");
            assert!(action.resources.is_empty());
            assert!(action.invalid_reason.is_some());
        }
    }

    #[test]
    fn web_fetch_without_a_string_url_fails_closed() {
        let action = normalize_tool_action(
            "claude-code",
            "session-1",
            "turn-1",
            "tool-1",
            "WebFetch",
            &json!({ "url": ["https://example.com"] }),
            None,
        );

        assert_eq!(
            action.capability,
            Capability::Unknown("invalid:WebFetch".to_string())
        );
        assert_eq!(action.operation, "WebFetch");
        assert!(action.resources.is_empty());
        assert!(action.invalid_reason.is_some());
    }

    #[test]
    fn explicit_principal_is_preserved_for_non_main_agent_actions() {
        let action = normalize_tool_action_for_principal(
            "claude-code",
            "session-1",
            "turn-1",
            "tool-1",
            "subagent:reviewer",
            "Read",
            &json!({ "file_path": "src/lib.rs" }),
            Some("D:/repo"),
        );

        assert_eq!(action.principal, "subagent:reviewer");
        assert_eq!(action.capability, Capability::FileRead);
        assert_eq!(action.invalid_reason, None);
    }

    #[test]
    fn permission_rule_serializes_field_names_as_camel_case() {
        let rule = PermissionRule {
            id: "rule-1".to_string(),
            principal: "main-agent".to_string(),
            effect: PermissionEffect::Allow,
            scope: PermissionScope::Project,
            scope_binding: PermissionScopeBinding {
                tool_call_id: None,
                turn_id: None,
                session_id: None,
                project_root: Some("D:/repo".to_string()),
            },
            engine: Some("claude-code".to_string()),
            capability: Capability::FileWrite,
            operation: Some("Edit".to_string()),
            resource_pattern: Some("D:/repo/**".to_string()),
            created_at: 1_752_314_400_000,
            expires_at: Some(1_752_318_000_000),
            max_uses: Some(10),
            uses: 0,
        };

        let value = serde_json::to_value(rule).expect("permission rule should serialize");

        assert_eq!(value["resourcePattern"], "D:/repo/**");
        assert_eq!(value["createdAt"], 1_752_314_400_000_i64);
        assert_eq!(value["expiresAt"], 1_752_318_000_000_i64);
        assert_eq!(value["maxUses"], 10);
        assert!(value.get("resource_pattern").is_none());
        assert!(value.get("created_at").is_none());
        assert!(value.get("expires_at").is_none());
        assert!(value.get("max_uses").is_none());
    }

    #[test]
    fn permission_decision_and_engine_manifest_have_stable_wire_shapes() {
        let decision = PermissionDecision {
            effect: PermissionEffect::Deny,
            reason: "unknown capability".to_string(),
            rule_id: None,
            policy_version: 7,
        };
        let manifest = EngineCapabilityManifest {
            engine: "claude-code".to_string(),
            version: "2.1.207".to_string(),
            supports_defer: true,
            supports_parallel_tool_approval: false,
            supports_native_sandbox: false,
            verified: true,
        };

        let decision_json = serde_json::to_value(decision).unwrap();
        let manifest_json = serde_json::to_value(manifest).unwrap();

        assert_eq!(decision_json["policyVersion"], 7);
        assert_eq!(decision_json["ruleId"], serde_json::Value::Null);
        assert_eq!(manifest_json["supportsDefer"], true);
        assert_eq!(manifest_json["supportsParallelToolApproval"], false);
        assert_eq!(manifest_json["verified"], true);
    }

    #[test]
    fn approval_rules_are_built_from_exact_actions_without_crossing_engines() {
        let action = normalize_tool_action(
            "codex",
            "history-1",
            "turn-1",
            "approval-1",
            "Bash",
            &serde_json::json!({"command":"cargo test"}),
            Some("D:/repo"),
        );
        let once = super::build_once_rule_from_action(&action, 10);
        assert_eq!(once.engine.as_deref(), Some("codex"));
        assert_eq!(once.scope, PermissionScope::Once);
        assert_eq!(once.max_uses, Some(1));

        let turn = super::build_turn_rule_from_action(&action, 10);
        assert_eq!(turn.scope, PermissionScope::Turn);
        assert_eq!(turn.scope_binding.turn_id.as_deref(), Some("turn-1"));
        assert_eq!(turn.scope_binding.session_id.as_deref(), Some("history-1"));
        assert_eq!(
            once.scope_binding.tool_call_id.as_deref(),
            Some("approval-1")
        );

        let always = super::build_always_rule_from_action(&action, 11);
        assert_eq!(always.engine.as_deref(), Some("codex"));
        assert_eq!(always.scope, PermissionScope::Global);
        assert_eq!(always.operation.as_deref(), Some("cargo"));
        assert_eq!(always.max_uses, None);
        assert_ne!(always.id, once.id);

        let session = super::build_session_rule_from_action(&action, 12);
        assert_eq!(session.scope, PermissionScope::Session);
        assert_eq!(
            session.scope_binding.session_id.as_deref(),
            Some("history-1")
        );
        // ADR 0016：Session 授权按可执行文件身份，同一可执行文件的不同 argv 复用同一规则。
        let mut other_resource = action.clone();
        other_resource.raw_input = serde_json::json!({"command":"cargo check"});
        assert_eq!(
            session.id,
            super::build_session_rule_from_action(&other_resource, 12).id,
            "cargo test 与 cargo check 解析到同一可执行文件，Session 授权应复用同一规则"
        );
        // 换引擎后不应复用同一规则。
        let mut other_engine = action.clone();
        other_engine.engine = "claude-code".to_string();
        assert_ne!(
            session.id,
            super::build_session_rule_from_action(&other_engine, 12).id
        );
        let project = super::build_project_rule_from_action(&action, 13).unwrap();
        assert_eq!(project.scope, PermissionScope::Project);
        assert_eq!(
            project.scope_binding.project_root.as_deref(),
            Some("D:/repo")
        );
    }

    fn process_exec_fixture() -> (std::path::PathBuf, std::path::PathBuf) {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "helm process matcher {} {nonce}",
            std::process::id()
        ));
        let bin_dir = root.join("bin folder");
        std::fs::create_dir_all(&bin_dir).unwrap();
        let executable = bin_dir.join("tool.bin");
        std::fs::write(&executable, b"binary-v1").unwrap();
        (root, executable)
    }

    fn process_action(
        executable: &std::path::Path,
        session_id: &str,
        turn_id: &str,
        tool_call_id: &str,
        argv_tail: &str,
        stdin: serde_json::Value,
    ) -> ActionDescriptor {
        normalize_tool_action(
            "claude-code",
            session_id,
            turn_id,
            tool_call_id,
            "Bash",
            &json!({
                "command": format!("\"{}\" {argv_tail}", executable.display()),
                "stdin": stdin,
            }),
            executable.parent().and_then(std::path::Path::to_str),
        )
    }

    #[test]
    fn process_exec_allow_binds_exact_argv_for_persistent_but_not_session() {
        let (root, executable) = process_exec_fixture();
        let approved = process_action(
            &executable,
            "session-1",
            "turn-1",
            "tool-1",
            "alpha beta",
            json!("input-a"),
        );
        let turn = build_turn_rule_from_action(&approved, 1);
        let project = build_project_rule_from_action(&approved, 1).unwrap();
        let always = build_always_rule_from_action(&approved, 1);

        // 持久范围（Turn/Project/Always）必须精确 argv+stdin 命中，见红线。
        for rule in [&turn, &project, &always] {
            assert!(rule
                .resource_pattern
                .as_deref()
                .is_some_and(|pattern| pattern.starts_with(PROCESS_EXEC_MATCHER_PREFIX)));
            assert!(rule
                .resource_pattern
                .as_deref()
                .is_some_and(|pattern| !pattern.starts_with(PROCESS_EXEC_SESSION_MATCHER_PREFIX)));
            assert_eq!(
                crate::permission_kernel::evaluate_action(
                    &approved,
                    std::slice::from_ref(rule),
                    2,
                    1
                )
                .effect,
                PermissionEffect::Allow
            );
            let changed_argv = process_action(
                &executable,
                "session-1",
                "turn-1",
                "tool-2",
                "alpha changed",
                json!("input-a"),
            );
            assert_eq!(
                crate::permission_kernel::evaluate_action(
                    &changed_argv,
                    std::slice::from_ref(rule),
                    2,
                    1,
                )
                .effect,
                PermissionEffect::Ask
            );
            let changed_stdin = process_action(
                &executable,
                "session-1",
                "turn-1",
                "tool-3",
                "alpha beta",
                json!("input-b"),
            );
            assert_eq!(
                crate::permission_kernel::evaluate_action(
                    &changed_stdin,
                    std::slice::from_ref(rule),
                    2,
                    1,
                )
                .effect,
                PermissionEffect::Ask
            );
        }

        // Session 范围（ADR 0016）：只绑定可执行文件身份，但不跨 argv/cwd/engine/可执行文件。
        let session = build_session_rule_from_action(&approved, 1);
        assert!(session
            .resource_pattern
            .as_deref()
            .is_some_and(|pattern| pattern.starts_with(PROCESS_EXEC_SESSION_MATCHER_PREFIX)));
        let changed_argv = process_action(
            &executable,
            "session-1",
            "turn-1",
            "tool-2",
            "alpha changed",
            json!("input-a"),
        );
        assert_eq!(
            crate::permission_kernel::evaluate_action(
                &changed_argv,
                std::slice::from_ref(&session),
                2,
                1
            )
            .effect,
            PermissionEffect::Allow,
            "同 executable 不同 argv 在本会话内应继续放行"
        );
        let changed_stdin = process_action(
            &executable,
            "session-1",
            "turn-1",
            "tool-3",
            "alpha beta",
            json!("input-b"),
        );
        assert_eq!(
            crate::permission_kernel::evaluate_action(
                &changed_stdin,
                std::slice::from_ref(&session),
                2,
                1
            )
            .effect,
            PermissionEffect::Allow,
            "同 executable 不同 stdin 在本会话内应继续放行"
        );
        // 换个可执行文件（非法路径）不命中。
        let other_exe = root.join("bin folder").join("other.bin");
        std::fs::write(&other_exe, b"binary-v1").unwrap();
        let other_action = process_action(
            &other_exe,
            "session-1",
            "turn-1",
            "tool-4",
            "alpha",
            Value::Null,
        );
        assert_eq!(
            crate::permission_kernel::evaluate_action(
                &other_action,
                std::slice::from_ref(&session),
                2,
                1,
            )
            .effect,
            PermissionEffect::Ask
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn process_exec_matcher_canonicalizes_aliases_and_detects_binary_drift() {
        let (root, executable) = process_exec_fixture();
        let approved = process_action(
            &executable,
            "session-1",
            "turn-1",
            "tool-1",
            "alpha",
            Value::Null,
        );
        let rule = build_session_rule_from_action(&approved, 1);
        let alias = executable
            .parent()
            .unwrap()
            .join("child")
            .join("..")
            .join(executable.file_name().unwrap());
        std::fs::create_dir_all(executable.parent().unwrap().join("child")).unwrap();
        let mut aliased_action = process_action(
            &alias,
            "session-1",
            "turn-2",
            "tool-2",
            "alpha",
            Value::Null,
        );
        // cwd 由工具真实工作目录驱动，别名路径解析到同一可执行文件，身份应与批准一致。
        aliased_action.cwd = approved.cwd.clone();
        assert_eq!(
            crate::permission_kernel::evaluate_action(&aliased_action, &[rule.clone()], 2, 1)
                .effect,
            PermissionEffect::Allow
        );

        std::fs::write(&executable, b"binary-v2").unwrap();
        assert_eq!(
            crate::permission_kernel::evaluate_action(&aliased_action, &[rule], 3, 1).effect,
            PermissionEffect::Ask
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn legacy_process_exec_allow_without_a_structured_matcher_fails_closed() {
        let (root, executable) = process_exec_fixture();
        let action = process_action(
            &executable,
            "session-1",
            "turn-1",
            "tool-1",
            "alpha",
            Value::Null,
        );
        let mut legacy = build_session_rule_from_action(&action, 1);
        legacy.resource_pattern = None;
        assert_eq!(
            crate::permission_kernel::evaluate_action(&action, &[legacy], 2, 1).effect,
            PermissionEffect::Ask
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
