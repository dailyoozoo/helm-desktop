use crate::util::now_millis;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, oneshot, Mutex};

const MAX_PENDING_REQUESTS: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum JsonRpcRequestId {
    Number(i64),
    String(String),
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandApprovalRequest {
    pub request_id: JsonRpcRequestId,
    pub thread_id: String,
    pub turn_id: String,
    pub item_id: String,
    pub approval_id: Option<String>,
    pub started_at_ms: i64,
    pub command: Option<String>,
    pub cwd: Option<String>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileChangeApprovalRequest {
    pub request_id: JsonRpcRequestId,
    pub thread_id: String,
    pub turn_id: String,
    pub item_id: String,
    pub started_at_ms: i64,
    pub grant_root: Option<String>,
    pub reason: Option<String>,
    #[serde(default)]
    pub correlated_paths: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionsApprovalRequest {
    pub request_id: JsonRpcRequestId,
    pub thread_id: String,
    pub turn_id: String,
    pub item_id: String,
    pub started_at_ms: i64,
    pub cwd: String,
    pub permissions: serde_json::Value,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CodexApprovalRequest {
    Command(CommandApprovalRequest),
    FileChange(FileChangeApprovalRequest),
    Permissions(PermissionsApprovalRequest),
}

impl CodexApprovalRequest {
    pub(crate) fn request_id(&self) -> JsonRpcRequestId {
        match self {
            Self::Command(request) => request.request_id.clone(),
            Self::FileChange(request) => request.request_id.clone(),
            Self::Permissions(request) => request.request_id.clone(),
        }
    }

    pub(crate) fn native_turn_id(&self) -> &str {
        match self {
            Self::Command(request) => &request.turn_id,
            Self::FileChange(request) => &request.turn_id,
            Self::Permissions(request) => &request.turn_id,
        }
    }
}

#[derive(Debug, Clone)]
pub struct CodexPendingApproval {
    pub request: CodexApprovalRequest,
    pub action: crate::permissions::ActionDescriptor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodexUserDecision {
    Allow,
    Turn,
    Session,
    Project,
    Deny,
    Always,
}

#[derive(Deserialize)]
struct ServerRequestEnvelope {
    id: JsonRpcRequestId,
    method: String,
    params: serde_json::Value,
}

pub fn parse_approval_request(value: serde_json::Value) -> Result<CodexApprovalRequest, String> {
    let envelope: ServerRequestEnvelope =
        serde_json::from_value(value).map_err(|error| error.to_string())?;
    match envelope.method.as_str() {
        "item/commandExecution/requestApproval" => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct Params {
                thread_id: String,
                turn_id: String,
                item_id: String,
                approval_id: Option<String>,
                started_at_ms: i64,
                command: Option<String>,
                cwd: Option<String>,
                reason: Option<String>,
            }
            let params: Params =
                serde_json::from_value(envelope.params).map_err(|error| error.to_string())?;
            Ok(CodexApprovalRequest::Command(CommandApprovalRequest {
                request_id: envelope.id,
                thread_id: params.thread_id,
                turn_id: params.turn_id,
                item_id: params.item_id,
                approval_id: params.approval_id,
                started_at_ms: params.started_at_ms,
                command: params.command,
                cwd: params.cwd,
                reason: params.reason,
            }))
        }
        "item/fileChange/requestApproval" => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct Params {
                thread_id: String,
                turn_id: String,
                item_id: String,
                started_at_ms: i64,
                grant_root: Option<String>,
                reason: Option<String>,
            }
            let params: Params =
                serde_json::from_value(envelope.params).map_err(|error| error.to_string())?;
            Ok(CodexApprovalRequest::FileChange(
                FileChangeApprovalRequest {
                    request_id: envelope.id,
                    thread_id: params.thread_id,
                    turn_id: params.turn_id,
                    item_id: params.item_id,
                    started_at_ms: params.started_at_ms,
                    grant_root: params.grant_root,
                    reason: params.reason,
                    correlated_paths: Vec::new(),
                },
            ))
        }
        "item/permissions/requestApproval" => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct Params {
                thread_id: String,
                turn_id: String,
                item_id: String,
                started_at_ms: i64,
                cwd: String,
                permissions: serde_json::Value,
                reason: Option<String>,
            }
            let params: Params =
                serde_json::from_value(envelope.params).map_err(|error| error.to_string())?;
            Ok(CodexApprovalRequest::Permissions(
                PermissionsApprovalRequest {
                    request_id: envelope.id,
                    thread_id: params.thread_id,
                    turn_id: params.turn_id,
                    item_id: params.item_id,
                    started_at_ms: params.started_at_ms,
                    cwd: params.cwd,
                    permissions: params.permissions,
                    reason: params.reason,
                },
            ))
        }
        method => Err(format!("unsupported Codex server request method: {method}")),
    }
}

fn server_request_contract_error(value: &serde_json::Value) -> String {
    match value.get("method").and_then(serde_json::Value::as_str) {
        Some("execCommandApproval") => "[codex_legacy_exec_approval_unhandled]".to_string(),
        Some("applyPatchApproval") => "[codex_legacy_patch_approval_unhandled]".to_string(),
        Some(
            "item/commandExecution/requestApproval"
            | "item/fileChange/requestApproval"
            | "item/permissions/requestApproval",
        ) => "[codex_approval_contract_unrecognized]".to_string(),
        Some("item/tool/requestUserInput") => "[codex_request_user_input_unmanaged]".to_string(),
        Some("mcpServer/elicitation/request") => "[codex_mcp_elicitation_unmanaged]".to_string(),
        Some("item/tool/call") => "[codex_dynamic_tool_call_unmanaged]".to_string(),
        Some("account/chatgptAuthTokens/refresh") => "[codex_auth_refresh_unmanaged]".to_string(),
        Some("attestation/generate") => "[codex_attestation_unmanaged]".to_string(),
        Some("currentTime/read") => "[codex_current_time_unmanaged]".to_string(),
        Some("unauthorized") => "[codex_unauthorized_request]".to_string(),
        _ => "[codex_server_request_unrecognized]".to_string(),
    }
}

pub fn normalize_approval_actions(
    history_session_id: &str,
    request: &CodexApprovalRequest,
    file_changes_by_item: &HashMap<String, Vec<String>>,
) -> Result<Vec<crate::permissions::ActionDescriptor>, String> {
    normalize_approval_actions_for_turn(
        history_session_id,
        request.native_turn_id(),
        request,
        file_changes_by_item,
    )
}

pub fn normalize_approval_actions_for_turn(
    history_session_id: &str,
    helm_turn_id: &str,
    request: &CodexApprovalRequest,
    file_changes_by_item: &HashMap<String, Vec<String>>,
) -> Result<Vec<crate::permissions::ActionDescriptor>, String> {
    use crate::permissions::{normalize_tool_action, ActionDescriptor, Capability};
    match request {
        CodexApprovalRequest::Command(request) => {
            let tool_call_id = request.approval_id.as_deref().unwrap_or(&request.item_id);
            let input = serde_json::json!({"command": request.command});
            Ok(vec![normalize_tool_action(
                "codex",
                history_session_id,
                helm_turn_id,
                tool_call_id,
                "Bash",
                &input,
                request.cwd.as_deref(),
            )])
        }
        CodexApprovalRequest::FileChange(request) => {
            let resources = (!request.correlated_paths.is_empty())
                .then(|| request.correlated_paths.clone())
                .or_else(|| file_changes_by_item.get(&request.item_id).cloned())
                .filter(|resources| !resources.is_empty())
                .ok_or_else(|| {
                    format!(
                        "missing file-change correlation for Codex itemId {}",
                        request.item_id
                    )
                })?;
            Ok(vec![ActionDescriptor {
                engine: "codex".to_string(),
                session_id: history_session_id.to_string(),
                turn_id: helm_turn_id.to_string(),
                tool_call_id: request.item_id.clone(),
                principal: "main-agent".to_string(),
                capability: Capability::FileWrite,
                operation: "fileChange".to_string(),
                resources: resources.clone(),
                cwd: request.grant_root.clone(),
                raw_input: serde_json::json!({
                    "paths": resources,
                    "grantRoot": request.grant_root,
                }),
                invalid_reason: None,
            }])
        }
        CodexApprovalRequest::Permissions(request) => {
            let network_requested = request
                .permissions
                .pointer("/network/enabled")
                .and_then(serde_json::Value::as_bool)
                == Some(true);
            let filesystem_requested = request
                .permissions
                .get("fileSystem")
                .is_some_and(|value| !value.is_null());
            if network_requested && filesystem_requested {
                return Err(
                    "mixed Codex permission profile requires atomic sub-actions".to_string()
                );
            }
            if network_requested {
                return Ok(vec![ActionDescriptor {
                    engine: "codex".to_string(),
                    session_id: history_session_id.to_string(),
                    turn_id: helm_turn_id.to_string(),
                    tool_call_id: request.item_id.clone(),
                    principal: "main-agent".to_string(),
                    capability: Capability::NetworkRequest,
                    operation: "requestPermissions".to_string(),
                    resources: vec!["network:*".to_string()],
                    cwd: Some(request.cwd.clone()),
                    raw_input: request.permissions.clone(),
                    invalid_reason: None,
                }]);
            }
            Err("unsupported or empty Codex permission profile".to_string())
        }
    }
}

pub fn evaluate_approval_with_kernel(
    store: &crate::sessions::SessionHistoryStore,
    history_session_id: &str,
    request: &CodexApprovalRequest,
    file_changes_by_item: &HashMap<String, Vec<String>>,
) -> Result<crate::permissions::PermissionDecision, String> {
    let actions = normalize_approval_actions(history_session_id, request, file_changes_by_item)?;
    evaluate_normalized_actions_with_kernel(store, &actions)
}

pub fn evaluate_normalized_actions_with_kernel(
    store: &crate::sessions::SessionHistoryStore,
    actions: &[crate::permissions::ActionDescriptor],
) -> Result<crate::permissions::PermissionDecision, String> {
    let mut selected: Option<crate::permissions::PermissionDecision> = None;
    for action in actions {
        let decision = store.evaluate_permission_action(action)?;
        let replace = match selected.as_ref() {
            None => true,
            Some(current) => {
                decision_precedence(decision.effect) > decision_precedence(current.effect)
            }
        };
        if replace {
            selected = Some(decision);
        }
    }
    selected.ok_or_else(|| "Codex approval produced no normalized actions".to_string())
}

fn decision_precedence(effect: crate::permissions::PermissionEffect) -> u8 {
    use crate::permissions::PermissionEffect;
    match effect {
        PermissionEffect::Allow => 1,
        PermissionEffect::Ask => 2,
        PermissionEffect::Deny => 3,
    }
}

pub fn automatic_approval_response(
    request: &CodexApprovalRequest,
    decision: &crate::permissions::PermissionDecision,
) -> Option<serde_json::Value> {
    use crate::permissions::PermissionEffect;
    match decision.effect {
        PermissionEffect::Ask => None,
        PermissionEffect::Allow => Some(match request {
            CodexApprovalRequest::Command(_) | CodexApprovalRequest::FileChange(_) => {
                serde_json::json!({"decision":"accept"})
            }
            CodexApprovalRequest::Permissions(_) => serde_json::json!({
                "permissions": {"network":{"enabled":true}},
                "scope": "turn",
                "strictAutoReview": true
            }),
        }),
        PermissionEffect::Deny => Some(denied_approval_response(request)),
    }
}

pub fn denied_approval_response(request: &CodexApprovalRequest) -> serde_json::Value {
    match request {
        CodexApprovalRequest::Command(_) | CodexApprovalRequest::FileChange(_) => {
            serde_json::json!({"decision":"decline"})
        }
        CodexApprovalRequest::Permissions(_) => serde_json::json!({
            "permissions": {},
            "scope": "turn",
            "strictAutoReview": true
        }),
    }
}

pub async fn apply_codex_user_decision(
    store: &crate::sessions::SessionHistoryStore,
    rpc: &CodexRpcClient,
    pending: &CodexPendingApproval,
    user_decision: CodexUserDecision,
) -> Result<(), String> {
    use crate::permissions::{PermissionDecision, PermissionEffect};
    if user_decision != CodexUserDecision::Deny {
        let current = store.evaluate_permission_action(&pending.action)?;
        if current.effect != PermissionEffect::Ask {
            let response = automatic_approval_response(&pending.request, &current)
                .ok_or_else(|| "Codex current policy unexpectedly remained ask".to_string())?;
            return rpc
                .respond(pending.request.request_id(), response)
                .await
                .map_err(|error| format!("[approval_delivery_unknown] {error}"));
        }
    }
    let rule = match user_decision {
        CodexUserDecision::Allow => Some((
            crate::permissions::build_once_rule_from_action(&pending.action, now_millis()),
            true,
        )),
        CodexUserDecision::Always => Some((
            crate::permissions::build_always_rule_from_action(&pending.action, now_millis()),
            false,
        )),
        CodexUserDecision::Turn => Some((
            crate::permissions::build_turn_rule_from_action(&pending.action, now_millis()),
            false,
        )),
        CodexUserDecision::Session => Some((
            crate::permissions::build_session_rule_from_action(&pending.action, now_millis()),
            false,
        )),
        CodexUserDecision::Project => Some((
            crate::permissions::build_project_rule_from_action(&pending.action, now_millis())?,
            false,
        )),
        CodexUserDecision::Deny => None,
    };
    let mut created_rule_id = None;
    if let Some((rule, consumed)) = rule {
        let existed = store
            .list_permission_rules()?
            .iter()
            .any(|existing| existing.id == rule.id);
        let persistent = matches!(
            rule.scope,
            crate::permissions::PermissionScope::Project
                | crate::permissions::PermissionScope::Global
        );
        if consumed {
            store.save_consumed_permission_rule(&rule)?;
        } else {
            store.save_permission_rule(&rule)?;
        }
        if persistent {
            if let Err(error) = store.save_runtime_grant_for_action(&rule, &pending.action) {
                if !existed {
                    let _ = store.remove_permission_rule(&rule.id);
                }
                return Err(format!("无法保存 Runtime 永久授权：{error}"));
            }
        }
        if !existed {
            created_rule_id = Some(rule.id);
        }
    }
    let effect = if user_decision == CodexUserDecision::Deny {
        PermissionEffect::Deny
    } else {
        PermissionEffect::Allow
    };
    let response = automatic_approval_response(
        &pending.request,
        &PermissionDecision {
            effect,
            reason: "user decided Codex approval in Helm".to_string(),
            rule_id: created_rule_id.clone(),
            policy_version: store.permission_policy_version()?,
        },
    )
    .ok_or_else(|| "Codex user decision unexpectedly remained ask".to_string())?;
    if let Err(error) = rpc.respond(pending.request.request_id(), response).await {
        // The RPC write may have reached Runtime even when its response was
        // lost. Keep the decision and mark delivery unknown; a blind rollback
        // or automatic retry could execute the same side effect twice.
        return Err(format!("[approval_delivery_unknown] {error}"));
    }
    Ok(())
}

struct OutboundMessage {
    value: serde_json::Value,
    written: oneshot::Sender<Result<(), String>>,
}

struct CodexRpcInner {
    outbound: mpsc::UnboundedSender<OutboundMessage>,
    pending: Mutex<HashMap<u64, oneshot::Sender<Result<serde_json::Value, String>>>>,
    approvals: Mutex<mpsc::UnboundedReceiver<Result<CodexApprovalRequest, String>>>,
    notifications: Mutex<mpsc::UnboundedReceiver<Result<serde_json::Value, String>>>,
    next_id: AtomicU64,
}

fn protocol_trace_enabled() -> bool {
    std::env::var_os("HELM_CODEX_PROTOCOL_TRACE").is_some_and(|value| value != "0")
}

fn trace_outbound(value: &serde_json::Value) {
    if !protocol_trace_enabled() {
        return;
    }
    let id = value.get("id").map(ToString::to_string).unwrap_or_default();
    if let Some(method) = value.get("method").and_then(serde_json::Value::as_str) {
        eprintln!("[codex-trace] outbound request id={id} method={method}");
    } else if value.get("result").is_some() {
        let decision = value
            .pointer("/result/decision")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("none");
        eprintln!("[codex-trace] outbound response id={id} result=present decision={decision}");
    } else if value.get("error").is_some() {
        eprintln!("[codex-trace] outbound response id={id} result=error");
    }
}

fn trace_inbound(value: &serde_json::Value) {
    if !protocol_trace_enabled() {
        return;
    }
    let id = value.get("id").map(ToString::to_string).unwrap_or_default();
    if let Some(method) = value.get("method").and_then(serde_json::Value::as_str) {
        if value.get("id").is_some() {
            let command_len = value
                .pointer("/params/command")
                .and_then(serde_json::Value::as_str)
                .map(str::len)
                .unwrap_or(0);
            let command_targets_probe_file = value
                .pointer("/params/command")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|command| command.contains("codex-command-approval-check"));
            eprintln!(
                "[codex-trace] inbound server_request id={id} method={method} command_len={command_len} targets_probe_file={command_targets_probe_file}"
            );
        } else {
            let item_type = value
                .pointer("/params/item/type")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("none");
            let status = value
                .pointer("/params/turn/status")
                .or_else(|| value.pointer("/params/item/status"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or("none");
            eprintln!(
                "[codex-trace] inbound notification method={method} item={item_type} status={status}"
            );
        }
    } else if value.get("result").is_some() {
        eprintln!("[codex-trace] inbound response id={id} result=present");
    } else if value.get("error").is_some() {
        eprintln!("[codex-trace] inbound response id={id} result=error");
    }
}

#[derive(Clone)]
pub struct CodexRpcClient {
    inner: Arc<CodexRpcInner>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodexApprovalPolicy {
    Untrusted,
    OnRequest,
    Never,
}

impl CodexApprovalPolicy {
    fn as_str(self) -> &'static str {
        match self {
            Self::Untrusted => "untrusted",
            Self::OnRequest => "on-request",
            Self::Never => "never",
        }
    }
}

pub struct CodexAppServerProcess {
    pub rpc: CodexRpcClient,
    child: Mutex<Child>,
    transport_tasks: Mutex<Vec<tokio::task::JoinHandle<()>>>,
}

pub fn resolve_codex_native_executable(configured_bin: &str) -> Result<std::path::PathBuf, String> {
    let configured = std::path::PathBuf::from(configured_bin);
    let wrapper = if configured.is_file() {
        configured
            .canonicalize()
            .map_err(|error| format!("解析 Codex 可执行文件失败：{error}"))?
    } else {
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            let lookup = std::process::Command::new("where.exe")
                .arg(if configured_bin.eq_ignore_ascii_case("codex") {
                    "codex.cmd"
                } else {
                    configured_bin
                })
                .creation_flags(0x08000000)
                .output()
                .map_err(|error| format!("定位 Codex 安装失败：{error}"))?;
            if !lookup.status.success() {
                return Err("未找到 Codex 原生可执行文件".to_string());
            }
            String::from_utf8_lossy(&lookup.stdout)
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(std::path::PathBuf::from)
                .find(|path| path.is_file())
                .ok_or_else(|| "未找到 Codex 原生可执行文件".to_string())?
                .canonicalize()
                .map_err(|error| format!("解析 Codex wrapper 失败：{error}"))?
        }
        #[cfg(not(windows))]
        {
            return Err("当前平台不需要解析 Windows Codex 原生可执行文件".to_string());
        }
    };
    if wrapper
        .extension()
        .and_then(std::ffi::OsStr::to_str)
        .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"))
    {
        return Ok(wrapper);
    }
    let npm_root = wrapper
        .parent()
        .ok_or_else(|| "Codex wrapper 缺少父目录".to_string())?;
    let native = npm_root
        .join("node_modules")
        .join("@openai")
        .join("codex")
        .join("node_modules")
        .join("@openai")
        .join("codex-win32-x64")
        .join("vendor")
        .join("x86_64-pc-windows-msvc")
        .join("bin")
        .join("codex.exe");
    native
        .canonicalize()
        .map_err(|error| format!("未找到官方 Codex Windows 原生执行器：{error}"))
}

impl CodexAppServerProcess {
    pub async fn shutdown(&self) {
        let mut child = self.child.lock().await;
        let pid = child.id();
        let _ = crate::adapter::terminate_child_bounded(
            &mut child,
            pid,
            std::time::Duration::from_secs(2),
        )
        .await;
        drop(child);
        let tasks = std::mem::take(&mut *self.transport_tasks.lock().await);
        shutdown_rpc_tasks(tasks).await;
    }
}

pub async fn spawn_codex_app_server(mut command: Command) -> Result<CodexAppServerProcess, String> {
    command
        .args(["app-server", "--stdio"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|error| format!("启动 Codex app-server 失败：{error}"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "Codex app-server stdout unavailable".to_string())?;
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| "Codex app-server stdin unavailable".to_string())?;
    let mut transport_tasks = Vec::new();
    if let Some(stderr) = child.stderr.take() {
        transport_tasks.push(tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while matches!(lines.next_line().await, Ok(Some(_))) {}
        }));
    }
    let (rpc, rpc_tasks) = start_rpc_transport_tracked(stdout, stdin);
    transport_tasks.extend(rpc_tasks);
    let initialized = match rpc
        .request(
            "initialize",
            serde_json::json!({
                "clientInfo": {
                    "name": "helm",
                    "title": "Helm",
                    "version": env!("CARGO_PKG_VERSION")
                },
                "capabilities": {"experimentalApi": true}
            }),
        )
        .await
    {
        Ok(initialized) => initialized,
        Err(error) => {
            crate::adapter::kill_tree(child.id()).await;
            let _ = child.wait().await;
            shutdown_rpc_tasks(transport_tasks).await;
            return Err(error);
        }
    };
    for required in ["userAgent", "codexHome", "platformFamily", "platformOs"] {
        if initialized
            .get(required)
            .and_then(serde_json::Value::as_str)
            .is_none()
        {
            crate::adapter::kill_tree(child.id()).await;
            let _ = child.wait().await;
            shutdown_rpc_tasks(transport_tasks).await;
            return Err(format!(
                "Codex app-server initialize response missing {required}"
            ));
        }
    }
    Ok(CodexAppServerProcess {
        rpc,
        child: Mutex::new(child),
        transport_tasks: Mutex::new(transport_tasks),
    })
}

impl CodexRpcClient {
    pub async fn model_list(&self, cursor: Option<&str>) -> Result<serde_json::Value, String> {
        self.model_list_with_visibility(cursor, true).await
    }

    pub async fn visible_model_list(
        &self,
        cursor: Option<&str>,
    ) -> Result<serde_json::Value, String> {
        self.model_list_with_visibility(cursor, false).await
    }

    async fn model_list_with_visibility(
        &self,
        cursor: Option<&str>,
        include_hidden: bool,
    ) -> Result<serde_json::Value, String> {
        self.request(
            "model/list",
            serde_json::json!({
                "cursor": cursor,
                "includeHidden": include_hidden,
                "limit": 1000
            }),
        )
        .await
    }

    pub async fn start_thread(
        &self,
        cwd: &str,
        model: &str,
        sandbox: &str,
    ) -> Result<String, String> {
        self.start_thread_with_policy(cwd, model, sandbox, CodexApprovalPolicy::OnRequest)
            .await
    }

    pub async fn start_thread_with_policy(
        &self,
        cwd: &str,
        model: &str,
        sandbox: &str,
        approval_policy: CodexApprovalPolicy,
    ) -> Result<String, String> {
        let result = self
            .request(
                "thread/start",
                serde_json::json!({
                    "cwd": cwd,
                    "model": model,
                    "sandbox": sandbox,
                    "approvalPolicy": approval_policy.as_str(),
                    "approvalsReviewer": "user",
                    "experimentalRawEvents": false
                }),
            )
            .await?;
        required_nested_id(&result, "/thread/id", "thread/start")
    }

    pub async fn resume_thread(
        &self,
        thread_id: &str,
        cwd: &str,
        model: &str,
        sandbox: &str,
    ) -> Result<String, String> {
        self.resume_thread_with_policy(
            thread_id,
            cwd,
            model,
            sandbox,
            CodexApprovalPolicy::OnRequest,
        )
        .await
    }

    pub async fn resume_thread_with_policy(
        &self,
        thread_id: &str,
        cwd: &str,
        model: &str,
        sandbox: &str,
        approval_policy: CodexApprovalPolicy,
    ) -> Result<String, String> {
        let result = self
            .request(
                "thread/resume",
                serde_json::json!({
                    "threadId": thread_id,
                    "cwd": cwd,
                    "model": model,
                    "sandbox": sandbox,
                    "approvalPolicy": approval_policy.as_str(),
                    "approvalsReviewer": "user"
                }),
            )
            .await?;
        required_nested_id(&result, "/thread/id", "thread/resume")
    }

    pub async fn start_turn(
        &self,
        thread_id: &str,
        prompt: &str,
        sandbox: &str,
        workspace_root: &str,
        network_allowed: bool,
        effort: Option<&str>,
    ) -> Result<String, String> {
        self.start_turn_with_policy(
            thread_id,
            prompt,
            sandbox,
            workspace_root,
            network_allowed,
            effort,
            CodexApprovalPolicy::OnRequest,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn start_turn_with_policy(
        &self,
        thread_id: &str,
        prompt: &str,
        sandbox: &str,
        workspace_root: &str,
        network_allowed: bool,
        effort: Option<&str>,
        approval_policy: CodexApprovalPolicy,
    ) -> Result<String, String> {
        self.start_turn_with_context_policy(
            thread_id,
            prompt,
            "",
            sandbox,
            workspace_root,
            network_allowed,
            effort,
            approval_policy,
            &[],
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn start_turn_with_context_policy(
        &self,
        thread_id: &str,
        prompt: &str,
        model: &str,
        sandbox: &str,
        workspace_root: &str,
        network_allowed: bool,
        effort: Option<&str>,
        approval_policy: CodexApprovalPolicy,
        session_context: &[crate::turn_start::FrozenSessionContext],
    ) -> Result<String, String> {
        let sandbox_policy = match sandbox {
            "read-only" => serde_json::json!({"type":"readOnly","networkAccess":false}),
            "workspace-write" => serde_json::json!({
                "type":"workspaceWrite",
                "writableRoots":[workspace_root],
                "networkAccess":network_allowed
            }),
            "danger-full-access" => serde_json::json!({"type":"dangerFullAccess"}),
            other => return Err(format!("unsupported Helm Codex sandbox ceiling: {other}")),
        };
        let mut input = vec![serde_json::json!({
            "type": "text",
            "text": prompt,
            "text_elements": []
        })];
        let mut runtime_workspace_roots = vec![workspace_root.to_string()];
        for context in session_context {
            if context.kind == "file" {
                input.push(serde_json::json!({
                    "type": "mention",
                    "name": std::path::Path::new(&context.canonical_path)
                        .file_name()
                        .and_then(|value| value.to_str())
                        .unwrap_or("context"),
                    "path": context.canonical_path
                }));
            } else if context.kind == "directory" {
                runtime_workspace_roots.push(context.canonical_path.clone());
            }
        }
        runtime_workspace_roots.sort();
        runtime_workspace_roots.dedup();
        let result = self
            .request(
                "turn/start",
                serde_json::json!({
                    "threadId": thread_id,
                    "model": model,
                    "input": input,
                    "runtimeWorkspaceRoots": runtime_workspace_roots,
                    "sandboxPolicy": sandbox_policy,
                    "approvalPolicy": approval_policy.as_str(),
                    "approvalsReviewer": "user",
                    // `null` 明确清除 thread 上一轮的 sticky override，恢复模型默认；
                    // 省略字段会沿用上一轮，不能表达 Helm 的 `auto`。
                    "effort": effort
                }),
            )
            .await?;
        required_nested_id(&result, "/turn/id", "turn/start")
    }

    pub async fn request(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let id = self.inner.next_id.fetch_add(1, Ordering::AcqRel);
        let (response_tx, response_rx) = oneshot::channel();
        {
            let mut pending = self.inner.pending.lock().await;
            if pending.len() >= MAX_PENDING_REQUESTS {
                return Err("Codex app-server has too many pending client requests".to_string());
            }
            pending.insert(id, response_tx);
        }
        let write_result = self
            .write(serde_json::json!({"id": id, "method": method, "params": params}))
            .await;
        if let Err(error) = write_result {
            self.inner.pending.lock().await.remove(&id);
            return Err(error);
        }
        response_rx
            .await
            .map_err(|_| "Codex app-server response channel closed".to_string())?
    }

    pub async fn respond(
        &self,
        id: JsonRpcRequestId,
        result: serde_json::Value,
    ) -> Result<(), String> {
        self.write(serde_json::json!({"id": id, "result": result}))
            .await
    }

    pub async fn next_approval_request(&self) -> Option<Result<CodexApprovalRequest, String>> {
        self.inner.approvals.lock().await.recv().await
    }

    pub async fn next_notification(&self) -> Option<Result<serde_json::Value, String>> {
        self.inner.notifications.lock().await.recv().await
    }

    async fn write(&self, value: serde_json::Value) -> Result<(), String> {
        let (written, acknowledged) = oneshot::channel();
        self.inner
            .outbound
            .send(OutboundMessage { value, written })
            .map_err(|_| "Codex app-server writer is closed".to_string())?;
        acknowledged
            .await
            .map_err(|_| "Codex app-server writer acknowledgement was dropped".to_string())?
    }
}

fn required_nested_id(
    value: &serde_json::Value,
    pointer: &str,
    method: &str,
) -> Result<String, String> {
    value
        .pointer(pointer)
        .and_then(serde_json::Value::as_str)
        .filter(|id| !id.is_empty())
        .map(ToString::to_string)
        .ok_or_else(|| format!("Codex {method} response missing id at {pointer}"))
}

fn start_rpc_transport_tracked<R, W>(
    reader: R,
    mut writer: W,
) -> (CodexRpcClient, Vec<tokio::task::JoinHandle<()>>)
where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let (outbound_tx, mut outbound_rx) = mpsc::unbounded_channel::<OutboundMessage>();
    let (approval_tx, approval_rx) =
        mpsc::unbounded_channel::<Result<CodexApprovalRequest, String>>();
    let (notification_tx, notification_rx) =
        mpsc::unbounded_channel::<Result<serde_json::Value, String>>();
    let inner = Arc::new(CodexRpcInner {
        outbound: outbound_tx,
        pending: Mutex::new(HashMap::new()),
        approvals: Mutex::new(approval_rx),
        notifications: Mutex::new(notification_rx),
        next_id: AtomicU64::new(1),
    });

    let writer_task = tokio::spawn(async move {
        while let Some(message) = outbound_rx.recv().await {
            let result = async {
                trace_outbound(&message.value);
                let mut bytes = serde_json::to_vec(&message.value).map_err(|e| e.to_string())?;
                bytes.push(b'\n');
                writer.write_all(&bytes).await.map_err(|e| e.to_string())?;
                writer.flush().await.map_err(|e| e.to_string())
            }
            .await;
            let failed = result.is_err();
            let _ = message.written.send(result);
            if failed {
                break;
            }
        }
    });

    let reader_inner = inner.clone();
    let reader_task = tokio::spawn(async move {
        let mut lines = BufReader::new(reader).lines();
        let mut file_paths_by_item = HashMap::<String, Vec<String>>::new();
        loop {
            let line = match lines.next_line().await {
                Ok(Some(line)) => line,
                Ok(None) => break,
                Err(error) => {
                    let message = format!("Codex app-server stdout read failed: {error}");
                    let _ = approval_tx.send(Err(message.clone()));
                    let _ = notification_tx.send(Err(message));
                    break;
                }
            };
            let event: serde_json::Value = match serde_json::from_str(&line) {
                Ok(event) => event,
                Err(error) => {
                    let message = format!("Codex app-server emitted invalid JSON: {error}");
                    let _ = approval_tx.send(Err(message.clone()));
                    let _ = notification_tx.send(Err(message));
                    break;
                }
            };
            trace_inbound(&event);
            if event.get("method").is_some() && event.get("id").is_some() {
                let contract_error = server_request_contract_error(&event);
                let request_id = event.get("id").cloned();
                let mut parsed = parse_approval_request(event).map_err(|_| contract_error.clone());
                if parsed.is_err() {
                    if let Some(request_id) = request_id {
                        let code = if contract_error == "[codex_server_request_unrecognized]" {
                            -32601
                        } else {
                            -32602
                        };
                        let (written, acknowledged) = oneshot::channel();
                        let response = serde_json::json!({
                            "id": request_id,
                            "error": {"code": code, "message": contract_error}
                        });
                        if reader_inner
                            .outbound
                            .send(OutboundMessage {
                                value: response,
                                written,
                            })
                            .is_ok()
                        {
                            let _ = acknowledged.await;
                        }
                    }
                    let _ = approval_tx.send(Err(contract_error.clone()));
                    let _ = notification_tx.send(Err(contract_error));
                    break;
                }
                if let Ok(CodexApprovalRequest::FileChange(request)) = &mut parsed {
                    request.correlated_paths = file_paths_by_item
                        .get(&request.item_id)
                        .cloned()
                        .unwrap_or_default();
                }
                let _ = approval_tx.send(parsed);
                continue;
            }
            if event.get("method").is_some() {
                if let Some((item_id, paths)) = file_paths_from_notification(&event) {
                    file_paths_by_item.insert(item_id, paths);
                }
                let _ = notification_tx.send(Ok(event));
                continue;
            }
            let Some(id) = event.get("id").and_then(serde_json::Value::as_u64) else {
                continue;
            };
            let Some(waiter) = reader_inner.pending.lock().await.remove(&id) else {
                continue;
            };
            let response = if let Some(result) = event.get("result") {
                Ok(result.clone())
            } else {
                Err(event
                    .get("error")
                    .map(ToString::to_string)
                    .unwrap_or_else(|| "Codex app-server response has no result".to_string()))
            };
            let _ = waiter.send(response);
        }
        let waiters = {
            let mut pending = reader_inner.pending.lock().await;
            pending
                .drain()
                .map(|(_, waiter)| waiter)
                .collect::<Vec<_>>()
        };
        for waiter in waiters {
            let _ = waiter.send(Err("Codex app-server connection closed".to_string()));
        }
    });

    (CodexRpcClient { inner }, vec![writer_task, reader_task])
}

async fn shutdown_rpc_tasks(tasks: Vec<tokio::task::JoinHandle<()>>) {
    for task in &tasks {
        task.abort();
    }
    for task in tasks {
        let _ = task.await;
    }
}

pub fn start_rpc_transport<R, W>(reader: R, writer: W) -> CodexRpcClient
where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    start_rpc_transport_tracked(reader, writer).0
}

fn file_paths_from_notification(event: &serde_json::Value) -> Option<(String, Vec<String>)> {
    if !matches!(
        event.get("method").and_then(serde_json::Value::as_str),
        Some("item/started" | "item/completed")
    ) {
        return None;
    }
    let item = event.pointer("/params/item")?;
    if item.get("type").and_then(serde_json::Value::as_str) != Some("fileChange") {
        return None;
    }
    let item_id = item.get("id")?.as_str()?.to_string();
    let paths = item
        .get("changes")?
        .as_array()?
        .iter()
        .filter_map(|change| {
            change
                .get("path")
                .and_then(serde_json::Value::as_str)
                .map(ToString::to_string)
        })
        .collect::<Vec<_>>();
    (!paths.is_empty()).then_some((item_id, paths))
}

#[cfg(test)]
mod tests {
    use super::{
        parse_approval_request, resolve_codex_native_executable, shutdown_rpc_tasks,
        start_rpc_transport, start_rpc_transport_tracked, CodexApprovalRequest,
        MAX_PENDING_REQUESTS,
    };
    use crate::permissions::Capability;
    use std::collections::HashMap;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    #[tokio::test]
    async fn tracked_rpc_transport_shutdown_aborts_idle_io_tasks() {
        let (client_read, _server_write) = tokio::io::duplex(1024);
        let (_server_read, client_write) = tokio::io::duplex(1024);
        let (_client, tasks) = start_rpc_transport_tracked(client_read, client_write);

        assert_eq!(tasks.len(), 2);
        tokio::time::timeout(std::time::Duration::from_secs(1), shutdown_rpc_tasks(tasks))
            .await
            .expect("tracked RPC tasks must stop within the shutdown bound");
    }

    #[test]
    fn resolves_direct_and_official_npm_codex_native_executables() {
        let temp = std::env::temp_dir().join(format!(
            "helm-codex-native-resolver-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        ));
        std::fs::create_dir_all(&temp).unwrap();
        let direct = temp.join("codex.exe");
        std::fs::write(&direct, b"").unwrap();
        assert_eq!(
            resolve_codex_native_executable(direct.to_str().unwrap()).unwrap(),
            direct.canonicalize().unwrap()
        );

        let npm = temp.join("npm");
        std::fs::create_dir_all(&npm).unwrap();
        let wrapper = npm.join("codex.cmd");
        std::fs::write(&wrapper, b"@echo off").unwrap();
        let native = npm
            .join("node_modules/@openai/codex/node_modules/@openai/codex-win32-x64")
            .join("vendor/x86_64-pc-windows-msvc/bin/codex.exe");
        std::fs::create_dir_all(native.parent().unwrap()).unwrap();
        std::fs::write(&native, b"").unwrap();
        assert_eq!(
            resolve_codex_native_executable(wrapper.to_str().unwrap()).unwrap(),
            native.canonicalize().unwrap()
        );
        let extensionless = npm.join("codex");
        std::fs::write(&extensionless, b"#!/bin/sh\n").unwrap();
        assert_eq!(
            resolve_codex_native_executable(extensionless.to_str().unwrap()).unwrap(),
            native.canonicalize().unwrap()
        );
        std::fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn parses_all_verified_approval_request_families() {
        let command = parse_approval_request(serde_json::json!({
            "id": 7,
            "method": "item/commandExecution/requestApproval",
            "params": {
                "threadId": "thread-1",
                "turnId": "turn-1",
                "itemId": "item-1",
                "approvalId": "approval-1",
                "startedAtMs": 12,
                "command": "cargo test",
                "cwd": "D:/repo"
            }
        }))
        .unwrap();
        assert!(matches!(command, CodexApprovalRequest::Command(_)));

        let file = parse_approval_request(serde_json::json!({
            "id": "request-2",
            "method": "item/fileChange/requestApproval",
            "params": {
                "threadId": "thread-1",
                "turnId": "turn-1",
                "itemId": "item-2",
                "startedAtMs": 13,
                "grantRoot": "D:/repo"
            }
        }))
        .unwrap();
        assert!(matches!(file, CodexApprovalRequest::FileChange(_)));

        let permissions = parse_approval_request(serde_json::json!({
            "id": 9,
            "method": "item/permissions/requestApproval",
            "params": {
                "threadId": "thread-1",
                "turnId": "turn-1",
                "itemId": "item-3",
                "startedAtMs": 14,
                "cwd": "D:/repo",
                "permissions": {"network": {"enabled": true}}
            }
        }))
        .unwrap();
        assert!(matches!(permissions, CodexApprovalRequest::Permissions(_)));
    }

    #[test]
    fn rejects_unknown_methods_and_missing_action_identity() {
        let unknown = parse_approval_request(serde_json::json!({
            "id": 1,
            "method": "item/unknown/requestApproval",
            "params": {}
        }))
        .unwrap_err();
        assert!(unknown.contains("unsupported"));

        let missing = parse_approval_request(serde_json::json!({
            "id": 2,
            "method": "item/commandExecution/requestApproval",
            "params": {"threadId": "thread-1", "turnId": "turn-1", "startedAtMs": 1}
        }))
        .unwrap_err();
        assert!(missing.contains("itemId"));
    }

    #[tokio::test]
    async fn rpc_transport_routes_responses_server_approvals_and_write_acknowledgements() {
        let (client_stream, server_stream) = tokio::io::duplex(16 * 1024);
        let (client_read, client_write) = tokio::io::split(client_stream);
        let (server_read, mut server_write) = tokio::io::split(server_stream);
        let client = start_rpc_transport(client_read, client_write);
        let request_client = client.clone();
        let request = tokio::spawn(async move {
            request_client
                .request(
                    "initialize",
                    serde_json::json!({"clientInfo":{"name":"helm"}}),
                )
                .await
        });

        let mut server_lines = BufReader::new(server_read).lines();
        let initialize: serde_json::Value =
            serde_json::from_str(&server_lines.next_line().await.unwrap().unwrap()).unwrap();
        assert_eq!(initialize["method"], "initialize");
        let request_id = initialize["id"].clone();
        server_write
            .write_all(
                format!("{{\"id\":{request_id},\"result\":{{\"userAgent\":\"codex\"}}}}\n")
                    .as_bytes(),
            )
            .await
            .unwrap();
        assert_eq!(request.await.unwrap().unwrap()["userAgent"], "codex");

        server_write
            .write_all(
                b"{\"id\":91,\"method\":\"item/commandExecution/requestApproval\",\"params\":{\"threadId\":\"thread-1\",\"turnId\":\"turn-1\",\"itemId\":\"item-1\",\"startedAtMs\":1,\"command\":\"cargo test\",\"cwd\":\"D:/repo\"}}\n",
            )
            .await
            .unwrap();
        let approval = client.next_approval_request().await.unwrap().unwrap();
        let response_id = match approval {
            CodexApprovalRequest::Command(request) => request.request_id,
            other => panic!("unexpected request: {other:?}"),
        };
        client
            .respond(response_id, serde_json::json!({"decision":"accept"}))
            .await
            .unwrap();
        let response: serde_json::Value =
            serde_json::from_str(&server_lines.next_line().await.unwrap().unwrap()).unwrap();
        assert_eq!(response["id"], 91);
        assert_eq!(response["result"]["decision"], "accept");
    }

    #[tokio::test]
    async fn reader_correlates_file_paths_before_dispatching_the_following_approval() {
        let (client_stream, mut server_stream) = tokio::io::duplex(4096);
        let (read, write) = tokio::io::split(client_stream);
        let client = start_rpc_transport(read, write);
        server_stream
            .write_all(
                b"{\"method\":\"item/started\",\"params\":{\"threadId\":\"thread-1\",\"turnId\":\"turn-1\",\"startedAtMs\":1,\"item\":{\"id\":\"file-1\",\"type\":\"fileChange\",\"status\":\"inProgress\",\"changes\":[{\"path\":\"src/main.rs\",\"diff\":\"x\",\"kind\":{\"type\":\"update\"}}]}}}\n{\"id\":81,\"method\":\"item/fileChange/requestApproval\",\"params\":{\"threadId\":\"thread-1\",\"turnId\":\"turn-1\",\"itemId\":\"file-1\",\"startedAtMs\":2}}\n",
            )
            .await
            .unwrap();
        let request = client.next_approval_request().await.unwrap().unwrap();
        let CodexApprovalRequest::FileChange(request) = request else {
            panic!("expected file approval")
        };
        assert_eq!(request.correlated_paths, vec!["src/main.rs"]);
    }

    #[tokio::test]
    async fn rpc_transport_forwards_notifications_and_fails_pending_requests_on_protocol_exit() {
        let (client_stream, server_stream) = tokio::io::duplex(4096);
        let (client_read, client_write) = tokio::io::split(client_stream);
        let (server_read, mut server_write) = tokio::io::split(server_stream);
        let client = start_rpc_transport(client_read, client_write);
        server_write
            .write_all(
                b"{\"method\":\"thread/started\",\"params\":{\"thread\":{\"id\":\"thread-1\"}}}\n",
            )
            .await
            .unwrap();
        let notification = client.next_notification().await.unwrap().unwrap();
        assert_eq!(notification["method"], "thread/started");

        let request_client = client.clone();
        let pending = tokio::spawn(async move {
            request_client
                .request("thread/read", serde_json::json!({"threadId":"thread-1"}))
                .await
        });
        let mut server_lines = BufReader::new(server_read).lines();
        assert!(server_lines.next_line().await.unwrap().is_some());
        drop(server_write);
        drop(server_lines);
        let error = pending.await.unwrap().unwrap_err();
        assert!(error.contains("connection closed"));

        let (client_stream, mut server_stream) = tokio::io::duplex(512);
        let (client_read, client_write) = tokio::io::split(client_stream);
        let client = start_rpc_transport(client_read, client_write);
        server_stream.write_all(b"not-json\n").await.unwrap();
        let error = client.next_approval_request().await.unwrap().unwrap_err();
        assert!(error.contains("invalid JSON"));
    }

    #[tokio::test]
    async fn rpc_transport_scrubs_unrecognized_approval_contract_details() {
        let (client_stream, server_stream) = tokio::io::duplex(1024);
        let (client_read, client_write) = tokio::io::split(client_stream);
        let (server_read, mut server_write) = tokio::io::split(server_stream);
        let client = start_rpc_transport(client_read, client_write);
        server_write
            .write_all(
                b"{\"id\":91,\"method\":\"item/future/requestApproval\",\"params\":{\"secret\":\"raw-secret\"}}\n",
            )
            .await
            .unwrap();

        let error = client.next_approval_request().await.unwrap().unwrap_err();
        assert_eq!(error, "[codex_server_request_unrecognized]");
        assert!(!error.contains("raw-secret"));
        let mut response_lines = BufReader::new(server_read).lines();
        let response: serde_json::Value =
            serde_json::from_str(&response_lines.next_line().await.unwrap().unwrap()).unwrap();
        assert_eq!(response["id"], 91);
        assert_eq!(response["error"]["code"], -32601);
        assert_eq!(
            response["error"]["message"],
            "[codex_server_request_unrecognized]"
        );
        assert!(!response.to_string().contains("raw-secret"));
    }

    #[tokio::test]
    async fn rpc_transport_classifies_legacy_approval_methods_without_payload_details() {
        let (client_stream, mut server_stream) = tokio::io::duplex(1024);
        let (client_read, client_write) = tokio::io::split(client_stream);
        let client = start_rpc_transport(client_read, client_write);
        server_stream
            .write_all(
                b"{\"id\":91,\"method\":\"execCommandApproval\",\"params\":{\"command\":[\"raw-secret\"]}}\n",
            )
            .await
            .unwrap();

        let error = client.next_approval_request().await.unwrap().unwrap_err();
        assert_eq!(error, "[codex_legacy_exec_approval_unhandled]");
        assert!(!error.contains("raw-secret"));
    }

    #[tokio::test]
    async fn rpc_transport_bounds_pending_client_requests() {
        let (client_stream, server_stream) = tokio::io::duplex(64 * 1024);
        let (client_read, client_write) = tokio::io::split(client_stream);
        let (server_read, _server_write) = tokio::io::split(server_stream);
        let client = start_rpc_transport(client_read, client_write);
        tokio::spawn(async move {
            let mut lines = BufReader::new(server_read).lines();
            while matches!(lines.next_line().await, Ok(Some(_))) {}
        });
        let mut pending = Vec::new();
        for index in 0..MAX_PENDING_REQUESTS {
            let client = client.clone();
            pending.push(tokio::spawn(async move {
                client
                    .request(
                        "thread/read",
                        serde_json::json!({"threadId":index.to_string()}),
                    )
                    .await
            }));
        }
        for _ in 0..100 {
            if client.inner.pending.lock().await.len() == MAX_PENDING_REQUESTS {
                break;
            }
            tokio::task::yield_now().await;
        }
        let error = client
            .request("thread/read", serde_json::json!({"threadId":"overflow"}))
            .await
            .unwrap_err();
        assert!(error.contains("too many pending"));
        for task in pending {
            task.abort();
        }
    }

    #[tokio::test]
    async fn app_server_methods_start_resume_threads_and_start_turns_with_helm_review() {
        let (client_stream, server_stream) = tokio::io::duplex(32 * 1024);
        let (client_read, client_write) = tokio::io::split(client_stream);
        let (server_read, mut server_write) = tokio::io::split(server_stream);
        let client = start_rpc_transport(client_read, client_write);
        let mut server_lines = BufReader::new(server_read).lines();

        let model_client = client.clone();
        let models = tokio::spawn(async move { model_client.model_list(None).await });
        let request: serde_json::Value =
            serde_json::from_str(&server_lines.next_line().await.unwrap().unwrap()).unwrap();
        assert_eq!(request["method"], "model/list");
        assert_eq!(request["params"]["includeHidden"], true);
        server_write
            .write_all(
                format!(
                    "{{\"id\":{},\"result\":{{\"data\":[{{\"id\":\"gpt-5.5\",\"model\":\"gpt-5.5\",\"defaultReasoningEffort\":\"medium\",\"supportedReasoningEfforts\":[{{\"reasoningEffort\":\"low\",\"description\":\"\"}}]}}]}}}}\n",
                    request["id"]
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        assert_eq!(models.await.unwrap().unwrap()["data"][0]["id"], "gpt-5.5");

        let start_client = client.clone();
        let started = tokio::spawn(async move {
            start_client
                .start_thread("D:/repo", "gpt-5", "workspace-write")
                .await
        });
        let request: serde_json::Value =
            serde_json::from_str(&server_lines.next_line().await.unwrap().unwrap()).unwrap();
        assert_eq!(request["method"], "thread/start");
        assert_eq!(request["params"]["approvalsReviewer"], "user");
        assert_eq!(request["params"]["approvalPolicy"], "on-request");
        assert!(request["params"].get("environments").is_none());
        server_write
            .write_all(
                format!(
                    "{{\"id\":{},\"result\":{{\"thread\":{{\"id\":\"thread-1\"}}}}}}\n",
                    request["id"]
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        assert_eq!(started.await.unwrap().unwrap(), "thread-1");

        let resume_client = client.clone();
        let resumed = tokio::spawn(async move {
            resume_client
                .resume_thread("thread-1", "D:/repo", "gpt-5", "read-only")
                .await
        });
        let request: serde_json::Value =
            serde_json::from_str(&server_lines.next_line().await.unwrap().unwrap()).unwrap();
        assert_eq!(request["method"], "thread/resume");
        assert_eq!(request["params"]["approvalPolicy"], "on-request");
        assert!(request["params"].get("environments").is_none());
        server_write
            .write_all(
                format!(
                    "{{\"id\":{},\"result\":{{\"thread\":{{\"id\":\"thread-1\"}}}}}}\n",
                    request["id"]
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        assert_eq!(resumed.await.unwrap().unwrap(), "thread-1");

        let turn_client = client.clone();
        let turn = tokio::spawn(async move {
            turn_client
                .start_turn_with_context_policy(
                    "thread-1",
                    "只发送当前问题",
                    "gpt-5.5",
                    "workspace-write",
                    "D:/repo",
                    true,
                    Some("high"),
                    super::CodexApprovalPolicy::OnRequest,
                    &[
                        crate::turn_start::FrozenSessionContext {
                            id: "context-file".into(),
                            kind: "file".into(),
                            canonical_path: "D:/repo/guide.md".into(),
                            canonical_path_digest: "sha256:file".into(),
                            identity_digest: "sha256:identity-file".into(),
                        },
                        crate::turn_start::FrozenSessionContext {
                            id: "context-dir".into(),
                            kind: "directory".into(),
                            canonical_path: "D:/repo/docs".into(),
                            canonical_path_digest: "sha256:dir".into(),
                            identity_digest: "sha256:identity-dir".into(),
                        },
                    ],
                )
                .await
        });
        let request: serde_json::Value =
            serde_json::from_str(&server_lines.next_line().await.unwrap().unwrap()).unwrap();
        assert_eq!(request["method"], "turn/start");
        assert_eq!(request["params"]["model"], "gpt-5.5");
        assert_eq!(request["params"]["input"][0]["text"], "只发送当前问题");
        assert_eq!(request["params"]["input"][1]["type"], "mention");
        assert_eq!(request["params"]["input"][1]["path"], "D:/repo/guide.md");
        assert_eq!(
            request["params"]["runtimeWorkspaceRoots"],
            serde_json::json!(["D:/repo", "D:/repo/docs"])
        );
        assert_eq!(request["params"]["approvalPolicy"], "on-request");
        assert_eq!(
            request["params"]["sandboxPolicy"]["writableRoots"],
            serde_json::json!(["D:/repo"])
        );
        assert_eq!(request["params"]["sandboxPolicy"]["networkAccess"], true);
        assert_eq!(request["params"]["effort"], "high");
        server_write
            .write_all(
                format!(
                    "{{\"id\":{},\"result\":{{\"turn\":{{\"id\":\"turn-1\"}}}}}}\n",
                    request["id"]
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        assert_eq!(turn.await.unwrap().unwrap(), "turn-1");

        let auto_turn_client = client.clone();
        let auto_turn = tokio::spawn(async move {
            auto_turn_client
                .start_turn(
                    "thread-1",
                    "恢复模型默认推理强度",
                    "read-only",
                    "D:/repo",
                    false,
                    None,
                )
                .await
        });
        let request: serde_json::Value =
            serde_json::from_str(&server_lines.next_line().await.unwrap().unwrap()).unwrap();
        assert_eq!(request["method"], "turn/start");
        assert!(request["params"]["effort"].is_null());
        server_write
            .write_all(
                format!(
                    "{{\"id\":{},\"result\":{{\"turn\":{{\"id\":\"turn-2\"}}}}}}\n",
                    request["id"]
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        assert_eq!(auto_turn.await.unwrap().unwrap(), "turn-2");
    }

    #[test]
    fn codex_approval_uses_the_shared_kernel_and_never_persists_engine_session_acceptance() {
        use crate::permissions::PermissionEffect;
        use crate::sessions::SessionHistoryStore;
        let root =
            std::env::temp_dir().join(format!("helm-codex-kernel-route-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let store = SessionHistoryStore::new(root.join("db.sqlite"));
        let request = parse_approval_request(serde_json::json!({
            "id": 51,
            "method": "item/commandExecution/requestApproval",
            "params": {
                "threadId": "thread-1", "turnId": "turn-1", "itemId": "item-1",
                "startedAtMs": 1, "command": "cargo test", "cwd": "D:/repo"
            }
        }))
        .unwrap();

        let ask =
            super::evaluate_approval_with_kernel(&store, "history-1", &request, &HashMap::new())
                .unwrap();
        assert_eq!(ask.effect, PermissionEffect::Ask);
        assert!(super::automatic_approval_response(&request, &ask).is_none());

        let action = super::normalize_approval_actions("history-1", &request, &HashMap::new())
            .unwrap()
            .remove(0);
        let rule = crate::permissions::build_always_rule_from_action(&action, 1);
        let rule_id = rule.id.clone();
        store.save_permission_rule(&rule).unwrap();
        store.save_runtime_grant_for_action(&rule, &action).unwrap();
        let allow =
            super::evaluate_approval_with_kernel(&store, "history-1", &request, &HashMap::new())
                .unwrap();
        assert_eq!(allow.effect, PermissionEffect::Allow);
        let response = super::automatic_approval_response(&request, &allow).unwrap();
        assert_eq!(response["decision"], "accept");
        assert_ne!(response["decision"], "acceptForSession");

        store.remove_permission_rule(&rule_id).unwrap();
        store
            .save_permission_rule(&crate::permissions::PermissionRule {
                id: "deny-cargo".to_string(),
                principal: action.principal.clone(),
                effect: crate::permissions::PermissionEffect::Deny,
                scope: crate::permissions::PermissionScope::Global,
                scope_binding: Default::default(),
                engine: Some(action.engine.clone()),
                capability: action.capability.clone(),
                operation: Some(action.operation.clone()),
                resource_pattern: None,
                created_at: 1,
                expires_at: None,
                max_uses: None,
                uses: 0,
            })
            .unwrap();
        let deny =
            super::evaluate_approval_with_kernel(&store, "history-1", &request, &HashMap::new())
                .unwrap();
        assert_eq!(deny.effect, PermissionEffect::Deny);
        assert_eq!(
            super::automatic_approval_response(&request, &deny).unwrap()["decision"],
            "decline"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn codex_user_grants_are_engine_neutral_and_delivery_unknown_is_not_rolled_back() {
        use super::{apply_codex_user_decision, CodexPendingApproval, CodexUserDecision};
        use crate::permissions::PermissionScope;
        use crate::sessions::SessionHistoryStore;
        let root =
            std::env::temp_dir().join(format!("helm-codex-user-grant-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let store = SessionHistoryStore::new(root.join("db.sqlite"));
        let request = parse_approval_request(serde_json::json!({
            "id": 71,
            "method": "item/commandExecution/requestApproval",
            "params": {
                "threadId": "thread-1", "turnId": "turn-1", "itemId": "item-1",
                "startedAtMs": 1, "command": "cargo test", "cwd": "D:/repo"
            }
        }))
        .unwrap();
        let action = super::normalize_approval_actions("history-1", &request, &HashMap::new())
            .unwrap()
            .remove(0);
        let pending = CodexPendingApproval {
            request: request.clone(),
            action: action.clone(),
        };

        let (client_stream, server_stream) = tokio::io::duplex(256);
        drop(server_stream);
        let (read, write) = tokio::io::split(client_stream);
        let failed_client = start_rpc_transport(read, write);
        let error =
            apply_codex_user_decision(&store, &failed_client, &pending, CodexUserDecision::Allow)
                .await
                .unwrap_err();
        assert!(error.starts_with("[approval_delivery_unknown]"));
        assert_eq!(store.list_permission_rules().unwrap().len(), 1);
        store
            .remove_permission_rule(&store.list_permission_rules().unwrap()[0].id)
            .unwrap();

        let (client_stream, server_stream) = tokio::io::duplex(4096);
        let (read, write) = tokio::io::split(client_stream);
        let (_server_read, _server_write) = tokio::io::split(server_stream);
        let client = start_rpc_transport(read, write);
        apply_codex_user_decision(&store, &client, &pending, CodexUserDecision::Allow)
            .await
            .unwrap();
        let rule = store.list_permission_rules().unwrap().remove(0);
        assert_eq!(rule.scope, PermissionScope::Once);
        assert_eq!(rule.uses, 1, "Codex 直接 accept 时 Once 必须同步消费");
        assert_eq!(rule.engine.as_deref(), Some("codex"));

        store.remove_permission_rule(&rule.id).unwrap();
        apply_codex_user_decision(&store, &client, &pending, CodexUserDecision::Always)
            .await
            .unwrap();
        let rule = store.list_permission_rules().unwrap().remove(0);
        assert_eq!(rule.scope, PermissionScope::Global);
        assert_eq!(rule.uses, 0);
        assert_eq!(rule.engine.as_deref(), Some("codex"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn codex_pending_allow_cannot_bypass_a_newer_kernel_deny_ceiling() {
        use super::{apply_codex_user_decision, CodexPendingApproval, CodexUserDecision};
        use crate::sessions::SessionHistoryStore;
        let root = std::env::temp_dir().join(format!(
            "helm-codex-policy-version-race-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let store = SessionHistoryStore::new(root.join("db.sqlite"));
        let request = parse_approval_request(serde_json::json!({
            "id": 81,
            "method": "item/commandExecution/requestApproval",
            "params": {
                "threadId": "thread-1", "turnId": "turn-1", "itemId": "item-1",
                "startedAtMs": 1, "command": "cargo test", "cwd": "D:/repo"
            }
        }))
        .unwrap();
        let action = super::normalize_approval_actions("history-1", &request, &HashMap::new())
            .unwrap()
            .remove(0);
        let pending = CodexPendingApproval { request, action };

        let initial = store.evaluate_permission_action(&pending.action).unwrap();
        assert_eq!(initial.effect, crate::permissions::PermissionEffect::Ask);
        store
            .save_permission_rule(&crate::permissions::PermissionRule {
                id: "deny-cargo-race".to_string(),
                principal: pending.action.principal.clone(),
                effect: crate::permissions::PermissionEffect::Deny,
                scope: crate::permissions::PermissionScope::Global,
                scope_binding: Default::default(),
                engine: Some(pending.action.engine.clone()),
                capability: pending.action.capability.clone(),
                operation: Some(pending.action.operation.clone()),
                resource_pattern: None,
                created_at: 1,
                expires_at: None,
                max_uses: None,
                uses: 0,
            })
            .unwrap();

        let (client_stream, server_stream) = tokio::io::duplex(4096);
        let (read, write) = tokio::io::split(client_stream);
        let (server_read, _server_write) = tokio::io::split(server_stream);
        let client = start_rpc_transport(read, write);
        apply_codex_user_decision(&store, &client, &pending, CodexUserDecision::Allow)
            .await
            .unwrap();

        let mut lines = BufReader::new(server_read).lines();
        let response: serde_json::Value =
            serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
        assert_eq!(response["result"]["decision"], "decline");
        let rules = store.list_permission_rules().unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].effect, crate::permissions::PermissionEffect::Deny);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn normalizes_approval_facts_and_requires_file_item_correlation() {
        let command = parse_approval_request(serde_json::json!({
            "id": 1,
            "method": "item/commandExecution/requestApproval",
            "params": {
                "threadId": "thread-1", "turnId": "turn-1", "itemId": "item-1",
                "approvalId": "approval-1", "startedAtMs": 1,
                "command": "cargo test", "cwd": "D:/repo"
            }
        }))
        .unwrap();
        let actions =
            super::normalize_approval_actions("history-1", &command, &HashMap::new()).unwrap();
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].capability, Capability::ProcessExec);
        assert_eq!(actions[0].tool_call_id, "approval-1");
        assert_eq!(actions[0].operation, "cargo");
        let helm_scoped = super::normalize_approval_actions_for_turn(
            "history-1",
            "turn-helm-global",
            &command,
            &HashMap::new(),
        )
        .unwrap();
        assert_eq!(helm_scoped[0].turn_id, "turn-helm-global");
        assert_ne!(helm_scoped[0].turn_id, command.native_turn_id());

        let file = parse_approval_request(serde_json::json!({
            "id": 2,
            "method": "item/fileChange/requestApproval",
            "params": {
                "threadId": "thread-1", "turnId": "turn-1", "itemId": "item-2",
                "startedAtMs": 2, "grantRoot": "D:/repo"
            }
        }))
        .unwrap();
        assert!(super::normalize_approval_actions("history-1", &file, &HashMap::new()).is_err());
        let correlations = HashMap::from([(
            "item-2".to_string(),
            vec![
                "D:/repo/src/main.rs".to_string(),
                "D:/repo/Cargo.toml".to_string(),
            ],
        )]);
        let actions = super::normalize_approval_actions("history-1", &file, &correlations).unwrap();
        assert_eq!(actions[0].capability, Capability::FileWrite);
        assert_eq!(actions[0].resources.len(), 2);

        let network = parse_approval_request(serde_json::json!({
            "id": 3,
            "method": "item/permissions/requestApproval",
            "params": {
                "threadId": "thread-1", "turnId": "turn-1", "itemId": "item-3",
                "startedAtMs": 3, "cwd": "D:/repo",
                "permissions": {"network":{"enabled":true}}
            }
        }))
        .unwrap();
        let actions =
            super::normalize_approval_actions("history-1", &network, &HashMap::new()).unwrap();
        assert_eq!(actions[0].capability, Capability::NetworkRequest);

        let mixed = parse_approval_request(serde_json::json!({
            "id": 4,
            "method": "item/permissions/requestApproval",
            "params": {
                "threadId": "thread-1", "turnId": "turn-1", "itemId": "item-4",
                "startedAtMs": 4, "cwd": "D:/repo",
                "permissions": {"network":{"enabled":true},"fileSystem":{"write":["D:/outside"]}}
            }
        }))
        .unwrap();
        let error =
            super::normalize_approval_actions("history-1", &mixed, &HashMap::new()).unwrap_err();
        assert!(error.contains("mixed"));
    }
}
