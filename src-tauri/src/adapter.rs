//! Claude Code 引擎适配器（Rust/tokio 版）：拉起真实 `claude` 子进程，以 stream-json
//! 模式读取输出，逐行归一化为 `AgentEvent` 并推给前端。
//!
//! Slice 2 的审批不再依赖不可见的内置 TTY prompt：Claude Code 2.1.x 在 headless
//! 模式支持 `PreToolUse` hook 返回 `permissionDecision:"defer"`，随后可用
//! `claude -p --resume <sessionId>` 重新评估同一个工具调用。Helm 用这个真实 CLI
//! 能力实现审批卡：先 defer → UI 批准/拒绝 → 写入 hook 状态 → resume 继续原会话。

use crate::codex_app_server::{
    apply_codex_user_decision, automatic_approval_response, denied_approval_response,
    evaluate_normalized_actions_with_kernel, normalize_approval_actions_for_turn,
    spawn_codex_app_server, CodexAppServerProcess, CodexApprovalPolicy, CodexPendingApproval,
    CodexUserDecision,
};
use crate::parse::parse_claude_line;
use crate::permission_service::{PermissionService, PermissionSessionContext};
use crate::protocol::{
    AgentEvent, ApprovalDecisionOption, CallStatus, Diff, DiffHunk, DiffKind, DiffLine, EngineId,
    PlanStatus, PlanStep, Role, StopReason, ToolStatus, TurnStage,
};
use crate::reasoning::ReasoningEffort;
use crate::sessions::SessionHistoryStore;
use crate::settings::AppSettings;
use crate::util::now_millis;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::{broadcast, mpsc, oneshot, Mutex, Notify};

const EVENT_NAME: &str = "agent-event";
#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
static TEMP_DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

/// 全局运行中 CLI 进程注册表：应用退出时必须整棵杀掉，否则 Windows 上经
/// `cmd /C` 包装启动的 node 子进程会成为孤儿（kill_on_drop 只杀 cmd 层）。
static RUNNING_PIDS: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<u32>>> =
    std::sync::OnceLock::new();

fn running_pid_registry() -> &'static std::sync::Mutex<std::collections::HashSet<u32>> {
    RUNNING_PIDS.get_or_init(|| std::sync::Mutex::new(std::collections::HashSet::new()))
}

/// 更新会话的运行 pid，并同步维护全局注册表。所有对 `running_pid` 槽位的写入都必须走这里。
async fn set_running_pid(slot: &Mutex<Option<u32>>, pid: Option<u32>) {
    let mut guard = slot.lock().await;
    if let Ok(mut registry) = running_pid_registry().lock() {
        if let Some(old) = *guard {
            registry.remove(&old);
        }
        if let Some(new) = pid {
            registry.insert(new);
        }
    }
    *guard = pid;
}

/// 是否还有 CLI 进程在运行（变更-12：关闭窗口前确认用）
pub fn has_running_processes() -> bool {
    running_pid_registry()
        .lock()
        .map(|registry| !registry.is_empty())
        .unwrap_or(false)
}

/// 应用退出时同步杀掉所有仍在运行的 CLI 进程树。
/// 必须是同步实现：退出路径上 tokio runtime 可能已不再调度任务。
pub fn kill_all_running_processes() {
    let pids: Vec<u32> = match running_pid_registry().lock() {
        Ok(mut registry) => registry.drain().collect(),
        Err(_) => return,
    };
    for pid in pids {
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            let _ = std::process::Command::new("taskkill")
                .args(["/F", "/T", "/PID", &pid.to_string()])
                .creation_flags(CREATE_NO_WINDOW)
                .output();
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = std::process::Command::new("kill")
                .args(["-TERM", &pid.to_string()])
                .output();
        }
    }
}

/// manager task 接收的指令。
enum SessionCmd {
    Send {
        text: String,
        attachments: Vec<String>,
        spec: crate::turn_start::TurnExecutionSpec,
    },
    Approve {
        request_id: String,
        decision: ApprovalDecision,
        responder: oneshot::Sender<Result<(), String>>,
    },
    /// 检查点回溯后重建上下文（P2-5）：作废 CLI 会话 id，下一轮用截断历史重新开场
    ResetContext {
        messages: Vec<crate::sessions::SessionMessage>,
    },
    /// 会话级 MCP 开关（变更-11）：设置停用名单，下一轮生效
    SetDisabledMcp {
        disabled: Vec<String>,
        responder: oneshot::Sender<Result<(), String>>,
    },
    SetPermissionProfile {
        profile: PermissionProfile,
        responder: oneshot::Sender<Result<(), String>>,
    },
    Interrupt,
}

/// 会话模式（变更-04）：轮次级属性，随每条消息下发。
/// 构建 = 现状默认行为；计划 = 只研究产出方案；询问 = 只读问答。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TurnMode {
    #[default]
    Build,
    Plan,
    Ask,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionProfile {
    #[default]
    Standard,
    Auto,
    FullAccess,
}

impl PermissionProfile {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "standard" => Ok(Self::Standard),
            "auto" => Ok(Self::Auto),
            "full_access" => Ok(Self::FullAccess),
            _ => Err(format!("未知权限档位：{value}")),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::Auto => "auto",
            Self::FullAccess => "full_access",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FullAccessLease {
    app_instance_id: String,
    session_id: String,
    engine: String,
    canonical_cwd: String,
}

fn app_instance_id() -> &'static str {
    static INSTANCE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    INSTANCE.get_or_init(|| format!("{}-{}", std::process::id(), now_millis()))
}

fn full_access_lease(session_id: &str, engine: &str, cwd: &str) -> FullAccessLease {
    FullAccessLease {
        app_instance_id: app_instance_id().to_string(),
        session_id: session_id.to_string(),
        engine: engine.to_string(),
        canonical_cwd: cwd.to_string(),
    }
}

fn lease_is_valid(lease: &FullAccessLease, session_id: &str, engine: &str, cwd: &str) -> bool {
    lease.app_instance_id == app_instance_id()
        && lease.session_id == session_id
        && lease.engine == engine
        && lease.canonical_cwd.eq_ignore_ascii_case(cwd)
}

async fn terminal_turn_outcome(
    terminals: &Mutex<HashMap<String, Result<(), String>>>,
    turn_id: &str,
) -> Option<Result<(), String>> {
    terminals.lock().await.get(turn_id).cloned()
}

impl TurnMode {
    pub fn parse(value: Option<&str>) -> Self {
        match value {
            Some("plan") => TurnMode::Plan,
            Some("ask") => TurnMode::Ask,
            _ => TurnMode::Build,
        }
    }

    pub(crate) fn as_state_str(self) -> &'static str {
        match self {
            TurnMode::Build => "build",
            TurnMode::Plan => "plan",
            TurnMode::Ask => "ask",
        }
    }
}

/// 询问模式的软约束层（C.1 实测：与 --settings 同用正常）。硬约束在审批 hook 里。
const ASK_MODE_APPEND_PROMPT: &str =
    "当前为询问模式：只回答问题、解释代码，不要尝试修改文件或执行有副作用的命令。";

/// Codex 无原生计划模式，计划轮 = read-only 沙箱 + 本段 prompt 前缀（降级近似，变更-04 A.3）。
/// 只注入发给 CLI 的 prompt，Helm 历史仍存用户原文。
const CODEX_PLAN_PROMPT_PREFIX: &str =
    "【计划模式】本轮请先调研并输出分步实施方案，不要执行任何修改（当前沙箱为只读）。";

#[derive(Debug, Clone, Copy)]
pub enum ApprovalDecision {
    Allow,
    Turn,
    Session,
    Project,
    Deny,
    Always,
}

impl ApprovalDecision {
    fn hook_decision(self) -> &'static str {
        match self {
            ApprovalDecision::Allow
            | ApprovalDecision::Turn
            | ApprovalDecision::Session
            | ApprovalDecision::Project
            | ApprovalDecision::Always => "allow",
            ApprovalDecision::Deny => "deny",
        }
    }

    pub fn audit_value(self) -> &'static str {
        match self {
            ApprovalDecision::Allow => "allow",
            ApprovalDecision::Turn => "turn",
            ApprovalDecision::Session => "session",
            ApprovalDecision::Project => "project",
            ApprovalDecision::Deny => "deny",
            ApprovalDecision::Always => "always",
        }
    }
}

pub(crate) fn available_approval_decisions(
    action: Option<&crate::permissions::ActionDescriptor>,
) -> Vec<ApprovalDecisionOption> {
    let mut decisions = vec![ApprovalDecisionOption::Allow];
    if let Some(action) =
        action.filter(|action| crate::permissions::runtime_grant_display(action).is_some())
    {
        if !action.turn_id.is_empty() && !action.session_id.is_empty() {
            decisions.push(ApprovalDecisionOption::Turn);
        }
        if !action.session_id.is_empty() {
            decisions.push(ApprovalDecisionOption::Session);
        }
        if action.cwd.as_deref().is_some_and(|cwd| !cwd.is_empty()) {
            decisions.push(ApprovalDecisionOption::Project);
        }
        decisions.push(ApprovalDecisionOption::Always);
    }
    decisions.push(ApprovalDecisionOption::Deny);
    decisions
}

fn validate_approval_decision(
    decision: ApprovalDecision,
    available_decisions: &[ApprovalDecisionOption],
) -> Result<(), String> {
    let option = match decision {
        ApprovalDecision::Allow => ApprovalDecisionOption::Allow,
        ApprovalDecision::Turn => ApprovalDecisionOption::Turn,
        ApprovalDecision::Session => ApprovalDecisionOption::Session,
        ApprovalDecision::Project => ApprovalDecisionOption::Project,
        ApprovalDecision::Always => ApprovalDecisionOption::Always,
        ApprovalDecision::Deny => ApprovalDecisionOption::Deny,
    };
    if available_decisions.contains(&option) {
        Ok(())
    } else {
        Err(format!(
            "[approval_decision_unavailable] 当前审批不允许决定：{}",
            decision.audit_value()
        ))
    }
}

/// hook 状态文件。hook 子进程只读它；Helm 后端在用户点击审批按钮后写入。
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApprovalState {
    decisions: HashMap<String, String>,
    /// 当前轮次内被拒绝的操作目标（文件路径等），防止换工具重试
    denied_targets: Vec<String>,
    /// 当前轮次的会话模式（变更-04）：ask 时 hook 以最高优先级拒绝写操作
    #[serde(default)]
    turn_mode: String,
}

/// 逐轮解析 Codex sandbox（变更-04）：计划/询问强制只读（取更严值），构建沿用设置映射。
pub fn codex_sandbox_for_mode(settings_sandbox: &str, mode: TurnMode) -> Result<String, String> {
    match mode {
        TurnMode::Plan | TurnMode::Ask => Ok("read-only".to_string()),
        TurnMode::Build if matches!(settings_sandbox, "read-only" | "workspace-write") => {
            Ok(settings_sandbox.to_string())
        }
        TurnMode::Build => Err(format!(
            "unsupported Helm Codex sandbox ceiling: {settings_sandbox}"
        )),
    }
}

pub fn agent_environment_from_settings(settings: &AppSettings) -> Vec<(String, String)> {
    let mut env = vec![(
        "HELM_ANONYMOUS_ANALYTICS".to_string(),
        if settings.general.anonymous_analytics {
            "1".to_string()
        } else {
            "0".to_string()
        },
    )];
    if !settings.general.anonymous_analytics {
        env.push(("DO_NOT_TRACK".to_string(), "1".to_string()));
        env.push(("HELM_TELEMETRY_DISABLED".to_string(), "1".to_string()));
    }
    env
}

struct ApprovalHookFiles {
    settings_path: PathBuf,
    state_path: PathBuf,
}

/// 待审批工具的信息
#[derive(Debug, Clone)]
struct PendingToolInfo {
    name: String,
    input: serde_json::Value,
}

#[derive(Debug, Clone)]
struct PreparedApproval {
    pending_tool: Option<PendingToolInfo>,
}

#[derive(Debug, Default)]
struct CommittedApprovalGrants {
    rule_ids: Vec<String>,
}

struct SessionRuntime {
    app: AppHandle,
    history_session_id: String,
    bin: String,
    model: Mutex<String>,
    cwd: String,
    policy_cwd: String,
    env: Vec<(String, String)>,
    use_user_setting_source: bool,
    capability_snapshot: Mutex<crate::capability_registry::EngineCapabilitySnapshot>,
    /// 当前轮次的会话模式（变更-04）：Send 时写入；审批恢复轮沿用发起轮的值
    turn_mode: Mutex<TurnMode>,
    /// 当前 Session 的推理强度；每个新 Turn 可覆盖，审批恢复轮沿用发起值。
    reasoning_effort: Mutex<ReasoningEffort>,
    settings_path: PathBuf,
    state_path: PathBuf,
    session_id: Mutex<Option<String>>,
    running_pid: Mutex<Option<u32>>,
    turn_lock: Mutex<()>,
    /// 同一 Session 的审批事务必须串行，避免 Always 提交/回滚覆盖另一笔审批。
    approval_lock: Mutex<()>,
    permission_endpoint: String,
    permission_token: String,
    current_turn_id: Mutex<String>,
    current_turn_spec: Mutex<Option<crate::turn_start::TurnExecutionSpec>>,
    current_session_context: Mutex<Vec<crate::turn_start::FrozenSessionContext>>,
    busy: AtomicBool,
    idle_notify: Notify,
    interrupted: AtomicBool,
    auto_compat_attempted: AtomicBool,
    pending_tools: Mutex<HashMap<String, PendingToolInfo>>,
    user_approved_tools: Mutex<HashSet<String>>,
    /// 回溯/恢复时的重建历史（P2-5）：没有 CLI 会话可 --resume 时，
    /// 下一轮把这份截断历史序列化进 prompt 重新开场，用后即清。
    rebuild_history: Mutex<Vec<crate::sessions::SessionMessage>>,
    /// 会话级停用的 MCP 服务器名单（变更-11）：非空时下一轮以
    /// `--strict-mcp-config --mcp-config <过滤后配置>` 启动，真实生效。
    disabled_mcp: std::sync::Mutex<Vec<String>>,
    permission_profile: Mutex<PermissionProfile>,
    full_access_lease: Mutex<Option<FullAccessLease>>,
}

/// 一个 Claude 会话句柄。每个用户轮次会拉起一次真实 `claude -p`；会话连续性通过
/// Claude Code 的 sessionId + `--resume` 保持。
#[derive(Clone)]
pub struct ClaudeSession {
    tx: mpsc::UnboundedSender<SessionCmd>,
    cwd: String,
    control: Option<Arc<SessionRuntime>>,
}

#[derive(Clone)]
pub struct CodexSession {
    app: AppHandle,
    history_session_id: String,
    bin: String,
    model: Arc<Mutex<String>>,
    cwd: String,
    execution_cwd: Arc<Mutex<Option<String>>>,
    policy_cwd: Arc<Mutex<String>>,
    env: Vec<(String, String)>,
    running_pid: Arc<Mutex<Option<u32>>>,
    /// 每轮序列化进 prompt 的历史消息；检查点回溯会整体替换为截断后的清单（P2-5）
    history_messages: Arc<std::sync::Mutex<Vec<crate::sessions::SessionMessage>>>,
    /// 轮次互斥（变更-06）：与 Claude 的 busy CAS 对齐，防止前端状态失真时双进程并发同一会话
    busy: Arc<AtomicBool>,
    /// 用户中断标志（变更-09）：中断导致的进程退出不渲染为错误卡，与 Claude 路径对齐
    interrupted: Arc<AtomicBool>,
    /// 会话级停用的 MCP 服务器名单（变更-11）：API Key 模式下过滤临时 CODEX_HOME 配置
    disabled_mcp: Arc<std::sync::Mutex<Vec<String>>>,
    /// Codex CLI 原生 thread id；普通后续轮直接 `exec resume`，不再重复拼接完整历史。
    thread_id: Arc<std::sync::Mutex<Option<String>>>,
    /// API Key 模式的临时 CODEX_HOME 归 Session 所有，Turn 只借用路径。
    auth_home: Arc<std::sync::Mutex<Option<CodexAuthHome>>>,
    /// Session 实际使用的 CODEX_HOME。API 接入指向受 Session 所有的快照；
    /// subscription 指向 Helm-owned 持久 Profile。
    effective_home: Arc<std::sync::Mutex<Option<PathBuf>>>,
    /// 回溯后下一轮必须丢弃旧 thread，并用截断历史新建一次 thread。
    force_history_rebuild: Arc<AtomicBool>,
    /// 每个 Helm Codex Session 持有一个 app-server；旧 exec 路径仅作回退。
    app_server: Arc<Mutex<Option<Arc<CodexAppServerProcess>>>>,
    app_server_thread_ready: Arc<AtomicBool>,
    /// 等待用户裁决的 Codex server requests，以 Helm approval id 索引。
    pending_approvals: Arc<Mutex<HashMap<String, CodexPendingApproval>>>,
    /// app-server 文件审批请求不带路径，通过同 itemId 的通知关联。
    file_changes_by_item: Arc<Mutex<HashMap<String, Vec<String>>>>,
    /// Tool items must complete before a Codex Turn can be accepted as successful.
    pending_tool_items: Arc<Mutex<HashSet<String>>>,
    /// 通知循环向当前 Turn 等待者广播完成或协议错误。
    turn_completions: broadcast::Sender<Result<String, String>>,
    terminal_turns: Arc<Mutex<HashMap<String, Result<(), String>>>>,
    terminal_notify: Arc<Notify>,
    /// Helm 控制面分配的全局 TurnId；与 app-server 原生 turn id 分离。
    current_helm_turn_id: Arc<Mutex<Option<String>>>,
    current_app_server_turn_id: Arc<Mutex<Option<String>>>,
    native_turn_contexts: Arc<Mutex<CodexTurnContextIndex>>,
    permission_profile: Arc<std::sync::Mutex<PermissionProfile>>,
    full_access_lease: Arc<std::sync::Mutex<Option<FullAccessLease>>>,
    capability_snapshot: Arc<Mutex<crate::capability_registry::EngineCapabilitySnapshot>>,
}

#[derive(Default)]
struct CodexTurnContextIndex {
    contexts: HashMap<String, (String, u64)>,
    order: VecDeque<String>,
}

impl CodexTurnContextIndex {
    fn insert(&mut self, native_turn_id: String, helm_turn_id: String, turn_epoch: u64) {
        const MAX_CONTEXTS: usize = 32;
        if !self.contexts.contains_key(&native_turn_id) {
            self.order.push_back(native_turn_id.clone());
        }
        self.contexts
            .insert(native_turn_id, (helm_turn_id, turn_epoch));
        while self.order.len() > MAX_CONTEXTS {
            if let Some(expired) = self.order.pop_front() {
                self.contexts.remove(&expired);
            }
        }
    }

    fn resolve(&self, native_turn_id: &str) -> Option<(String, u64)> {
        self.contexts.get(native_turn_id).cloned()
    }
}

pub(crate) struct CodexAuthHome {
    pub(crate) path: PathBuf,
}

impl Drop for CodexAuthHome {
    fn drop(&mut self) {
        const ATTEMPTS: usize = 100;
        for attempt in 0..ATTEMPTS {
            match fs::remove_dir_all(&self.path) {
                Ok(()) => return,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
                Err(error) if attempt + 1 == ATTEMPTS => {
                    eprintln!(
                        "[helm] 无法清理 Codex 临时认证目录（已重试 {ATTEMPTS} 次）：{}：{error}",
                        self.path.display()
                    );
                }
                Err(_) => std::thread::sleep(Duration::from_millis(50)),
            }
        }
    }
}

#[derive(Clone)]
pub enum AgentSession {
    Claude(ClaudeSession),
    Codex(CodexSession),
}

impl AgentSession {
    pub fn history_session_id(&self) -> &str {
        match self {
            AgentSession::Claude(session) => session
                .control
                .as_ref()
                .map(|runtime| runtime.history_session_id.as_str())
                .unwrap_or_default(),
            AgentSession::Codex(session) => &session.history_session_id,
        }
    }

    pub async fn native_session_ref(&self) -> Result<Option<String>, String> {
        match self {
            AgentSession::Claude(session) => Ok(session
                .control
                .as_ref()
                .ok_or_else(|| "Claude Runtime 控制面不可用".to_string())?
                .session_id
                .lock()
                .await
                .clone()),
            AgentSession::Codex(session) => session
                .thread_id
                .lock()
                .map(|thread_id| thread_id.clone())
                .map_err(|_| "Codex thread id 锁中毒".to_string()),
        }
    }

    pub async fn set_turn_capability_snapshot(
        &self,
        snapshot: crate::capability_registry::EngineCapabilitySnapshot,
    ) -> Result<(), String> {
        match self {
            AgentSession::Claude(session) => {
                if snapshot.identity.engine_id != "claude-code" {
                    return Err("Claude Runtime 收到其他 Engine 的 CapabilitySnapshot".to_string());
                }
                let control = session
                    .control
                    .as_ref()
                    .ok_or_else(|| "Claude Runtime 控制面不可用".to_string())?;
                *control.capability_snapshot.lock().await = snapshot;
            }
            AgentSession::Codex(session) => {
                if snapshot.identity.engine_id != "codex" {
                    return Err("Codex Runtime 收到其他 Engine 的 CapabilitySnapshot".to_string());
                }
                *session.capability_snapshot.lock().await = snapshot;
            }
        }
        Ok(())
    }

    pub fn reserve_turn(&self) -> Result<(), String> {
        let busy = match self {
            AgentSession::Claude(session) => {
                &session
                    .control
                    .as_ref()
                    .ok_or_else(|| "Claude Runtime 控制面不可用".to_string())?
                    .busy
            }
            AgentSession::Codex(session) => session.busy.as_ref(),
        };
        reserve_turn_flag(busy)
    }

    pub fn release_turn_reservation(&self) {
        match self {
            AgentSession::Claude(session) => {
                if let Some(control) = &session.control {
                    control.busy.store(false, Ordering::Release);
                    control.idle_notify.notify_waiters();
                }
            }
            AgentSession::Codex(session) => {
                session.busy.store(false, Ordering::Release);
            }
        }
    }

    pub async fn send_reserved(
        &self,
        text: String,
        attachments: Vec<String>,
        spec: crate::turn_start::TurnExecutionSpec,
    ) -> Result<(), String> {
        if matches!(self, AgentSession::Claude(_))
            && !spec.routed_reasoning_effort.is_claude_level()
        {
            return Err("Claude Code 不支持该推理强度".to_string());
        }
        let route_matches = match self {
            AgentSession::Claude(session) => {
                let control = session
                    .control
                    .as_ref()
                    .ok_or_else(|| "Claude Runtime 控制面不可用".to_string())?;
                spec.engine_id == "claude-code"
                    && spec.permission_profile == control.permission_profile.lock().await.as_str()
            }
            AgentSession::Codex(session) => {
                spec.engine_id == "codex"
                    && spec.permission_profile
                        == session
                            .permission_profile
                            .lock()
                            .map_err(|_| "Codex 权限档位锁中毒".to_string())?
                            .as_str()
            }
        };
        if !route_matches {
            self.release_turn_reservation();
            return Err("TurnExecutionSpec 与已启动 Runtime 路由不一致".to_string());
        }
        match self {
            AgentSession::Claude(session) => session
                .tx
                .send(SessionCmd::Send {
                    text,
                    attachments,
                    spec,
                })
                .map_err(|_| {
                    self.release_turn_reservation();
                    "会话已结束，无法发送".to_string()
                }),
            AgentSession::Codex(session) => {
                session.send_reserved(text, attachments, spec);
                Ok(())
            }
        }
    }

    pub async fn permission_profile(&self) -> Result<PermissionProfile, String> {
        match self {
            AgentSession::Claude(session) => {
                let control = session
                    .control
                    .as_ref()
                    .ok_or_else(|| "Claude Runtime 控制面不可用".to_string())?;
                Ok(*control.permission_profile.lock().await)
            }
            AgentSession::Codex(session) => session
                .permission_profile
                .lock()
                .map(|profile| *profile)
                .map_err(|_| "Codex 权限档位锁中毒".to_string()),
        }
    }

    pub async fn approve(
        &self,
        request_id: String,
        decision: ApprovalDecision,
    ) -> Result<(), String> {
        match self {
            AgentSession::Claude(session) => {
                let (responder, response) = oneshot::channel();
                session
                    .tx
                    .send(SessionCmd::Approve {
                        request_id,
                        decision,
                        responder,
                    })
                    .map_err(|_| "会话已结束，无法审批".to_string())?;
                response
                    .await
                    .map_err(|_| "审批协调器已结束，无法确认审批结果".to_string())?
            }
            AgentSession::Codex(session) => session.approve(request_id, decision).await,
        }
    }

    pub async fn set_permission_profile(&self, profile: PermissionProfile) -> Result<(), String> {
        match self {
            AgentSession::Claude(session) => {
                let (responder, response) = oneshot::channel();
                session
                    .tx
                    .send(SessionCmd::SetPermissionProfile { profile, responder })
                    .map_err(|_| "会话已结束，无法更新权限档位".to_string())?;
                response
                    .await
                    .map_err(|_| "会话已结束，无法确认权限档位".to_string())?
            }
            AgentSession::Codex(session) => {
                if session.busy.load(Ordering::Acquire) {
                    return Err("轮次进行中，结束后才能切换权限档位".to_string());
                }
                *session
                    .permission_profile
                    .lock()
                    .map_err(|_| "Codex 权限档位锁中毒".to_string())? = profile;
                *session
                    .full_access_lease
                    .lock()
                    .map_err(|_| "Codex FullAccessLease 锁中毒".to_string())? = (profile
                    == PermissionProfile::FullAccess)
                    .then(|| full_access_lease(&session.history_session_id, "codex", &session.cwd));
                if let Some(process) = session.app_server.lock().await.take() {
                    process.shutdown().await;
                }
                session
                    .app_server_thread_ready
                    .store(false, Ordering::Release);
                Ok(())
            }
        }
    }

    pub fn permission_confirmation_context(&self) -> (String, String) {
        match self {
            AgentSession::Claude(session) => ("Claude Code".to_string(), session.cwd.clone()),
            AgentSession::Codex(session) => ("Codex".to_string(), session.cwd.clone()),
        }
    }

    pub fn interrupt(&self) -> Result<(), String> {
        match self {
            AgentSession::Claude(session) => session
                .tx
                .send(SessionCmd::Interrupt)
                .map_err(|_| "会话已结束，无法中断".to_string()),
            AgentSession::Codex(session) => session.interrupt(),
        }
    }

    pub fn close(&self) {
        let _ = self.interrupt();
        if let AgentSession::Codex(session) = self {
            let app_server = session.app_server.clone();
            spawn_agent_task(async move {
                if let Some(process) = app_server.lock().await.take() {
                    process.shutdown().await;
                }
            });
        }
    }

    pub async fn shutdown(&self) {
        let _ = self.interrupt();
        if let AgentSession::Codex(session) = self {
            if let Some(process) = session.app_server.lock().await.take() {
                process.shutdown().await;
            }
            session
                .app_server_thread_ready
                .store(false, Ordering::Release);
        }
    }

    /// 检查点回溯后重建 Agent 上下文（P2-5）：传入截断后的历史消息。
    /// Claude：作废 CLI 会话 id，下一轮以序列化历史重新开场；
    /// Codex：替换每轮序列化的历史快照。
    pub fn reset_context(
        &self,
        messages: Vec<crate::sessions::SessionMessage>,
    ) -> Result<(), String> {
        match self {
            AgentSession::Claude(session) => session
                .tx
                .send(SessionCmd::ResetContext { messages })
                .map_err(|_| "会话已结束，无法重建上下文".to_string()),
            AgentSession::Codex(session) => {
                reset_codex_context_state(
                    &session.thread_id,
                    &session.force_history_rebuild,
                    &session.history_messages,
                    messages,
                )?;
                session
                    .app_server_thread_ready
                    .store(false, Ordering::Release);
                Ok(())
            }
        }
    }

    /// 会话级 MCP 开关（变更-11）：设置停用名单，下一轮真实生效。
    /// Claude：下一轮 `--strict-mcp-config --mcp-config <过滤后配置>`；
    /// Codex：API Key 模式下过滤临时 CODEX_HOME 的 config.toml（订阅登录暂不支持）。
    pub async fn set_disabled_mcp(&self, disabled: Vec<String>) -> Result<(), String> {
        match self {
            AgentSession::Claude(session) => {
                let (responder, response) = oneshot::channel();
                session
                    .tx
                    .send(SessionCmd::SetDisabledMcp {
                        disabled,
                        responder,
                    })
                    .map_err(|_| "会话已结束，无法更新 MCP 开关".to_string())?;
                response
                    .await
                    .map_err(|_| "会话未返回 MCP 配置更新结果".to_string())?
            }
            AgentSession::Codex(session) => {
                if session.busy.load(Ordering::Acquire) {
                    return Err("轮次进行中，结束后才能更新 MCP 开关".to_string());
                }
                // API Key Session 只有在用户显式变更 MCP 开关时才重建临时 HOME；
                // subscription 没有临时认证目录，继续复用现有 Helm Profile。
                let replacement = create_codex_auth_home(&session.env, &disabled)?;
                let replacement_path = replacement.as_ref().map(|home| home.path.clone());
                let existing_home = session
                    .effective_home
                    .lock()
                    .map_err(|_| "Codex CODEX_HOME 锁中毒".to_string())?
                    .clone();
                let effective_home =
                    select_effective_codex_home(replacement_path.as_ref(), existing_home.as_ref());
                *session
                    .auth_home
                    .lock()
                    .map_err(|_| "Codex CODEX_HOME 锁中毒".to_string())? = replacement;
                *session
                    .effective_home
                    .lock()
                    .map_err(|_| "Codex CODEX_HOME 锁中毒".to_string())? = effective_home;
                *session
                    .disabled_mcp
                    .lock()
                    .map_err(|_| "Codex MCP 开关锁中毒".to_string())? = disabled;
                if let Some(process) = session.app_server.lock().await.take() {
                    process.shutdown().await;
                }
                session
                    .app_server_thread_ready
                    .store(false, Ordering::Release);
                Ok(())
            }
        }
    }
}

fn reserve_turn_flag(busy: &AtomicBool) -> Result<(), String> {
    busy.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .map(|_| ())
        .map_err(|_| "当前会话已有轮次正在运行，请等待完成或先停止".to_string())
}

/// 按平台构造 `claude` 命令。Windows 优先解析官方 npm wrapper 指向的原生
/// `claude.exe`，避免 `cmd /C` 在超时/中断时留下持有管道的孤儿子进程。
fn validate_engine_bin(bin: &str) -> Result<(), String> {
    let trimmed = bin.trim();
    if trimmed.is_empty() {
        return Err("引擎可执行文件路径不能为空".to_string());
    }
    if trimmed.chars().any(|ch| {
        matches!(
            ch,
            '&' | '|' | '<' | '>' | '^' | '%' | '!' | '"' | '\r' | '\n'
        )
    }) {
        return Err("引擎可执行文件路径包含不安全的命令控制字符".to_string());
    }
    Ok(())
}

fn filter_inherited_agent_environment<I>(vars: I) -> Vec<(String, String)>
where
    I: IntoIterator<Item = (String, String)>,
{
    const ALLOWED: &[&str] = &[
        "PATH",
        "PATHEXT",
        "SYSTEMROOT",
        "WINDIR",
        "COMSPEC",
        "TEMP",
        "TMP",
        "TMPDIR",
        "USERPROFILE",
        "HOMEDRIVE",
        "HOMEPATH",
        "HOME",
        "APPDATA",
        "LOCALAPPDATA",
        "PROGRAMDATA",
        "PROGRAMFILES",
        "PROGRAMFILES(X86)",
        "COMMONPROGRAMFILES",
        "COMMONPROGRAMFILES(X86)",
        "USER",
        "USERNAME",
        "LOGNAME",
        "SHELL",
        "LANG",
        "LANGUAGE",
        "LC_ALL",
        "LC_CTYPE",
        "TERM",
        "COLORTERM",
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "NO_PROXY",
        "ALL_PROXY",
        "SSL_CERT_FILE",
        "SSL_CERT_DIR",
        "NODE_EXTRA_CA_CERTS",
        "SSH_AUTH_SOCK",
        "GIT_SSH",
        "GIT_SSH_COMMAND",
        "CODEX_HOME",
        "CLAUDE_CONFIG_DIR",
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "XDG_CACHE_HOME",
    ];

    vars.into_iter()
        .filter(|(key, _)| ALLOWED.contains(&key.to_ascii_uppercase().as_str()))
        .collect()
}

pub(crate) fn apply_inherited_agent_environment(cmd: &mut Command) {
    cmd.env_clear();
    cmd.envs(filter_inherited_agent_environment(std::env::vars()));
}

fn official_claude_npm_binary(wrapper: &Path) -> Option<PathBuf> {
    wrapper.parent().map(|parent| {
        parent
            .join("node_modules")
            .join("@anthropic-ai")
            .join("claude-code")
            .join("bin")
            .join("claude.exe")
    })
}

fn resolve_claude_binary_path(configured_bin: &str) -> Result<PathBuf, String> {
    let configured = PathBuf::from(configured_bin);
    #[cfg(windows)]
    if configured.is_file() && configured.extension().is_none() {
        if let Some(native) = official_claude_npm_binary(&configured).filter(|path| path.is_file())
        {
            return native
                .canonicalize()
                .map_err(|error| format!("resolve Claude native binary failed: {error}"));
        }
        for extension in ["exe", "cmd", "bat", "com"] {
            let candidate = configured.with_extension(extension);
            if candidate.is_file() {
                return resolve_claude_binary_path(&candidate.to_string_lossy());
            }
        }
    }
    let mut candidates = if configured.is_file() {
        vec![configured]
    } else {
        #[cfg(windows)]
        {
            let output = std::process::Command::new("where.exe")
                .arg(configured_bin)
                .output()
                .map_err(|error| format!("resolve Claude binary failed: {error}"))?;
            if !output.status.success() {
                return Err("configured Claude binary is unavailable".to_string());
            }
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(PathBuf::from)
                .collect::<Vec<_>>()
        }
        #[cfg(not(windows))]
        {
            let output = std::process::Command::new("which")
                .arg(configured_bin)
                .output()
                .map_err(|error| format!("resolve Claude binary failed: {error}"))?;
            if !output.status.success() {
                return Err("configured Claude binary is unavailable".to_string());
            }
            vec![PathBuf::from(
                String::from_utf8_lossy(&output.stdout).trim(),
            )]
        }
    };
    #[cfg(windows)]
    candidates.sort_by_key(|path| {
        match path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str()
        {
            "exe" => 0,
            "cmd" => 1,
            "bat" => 2,
            "com" => 3,
            _ => 4,
        }
    });
    let selected = candidates
        .into_iter()
        .find(|path| path.is_file())
        .ok_or_else(|| "configured Claude binary is unavailable".to_string())?;
    if selected
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case("cmd"))
    {
        if let Some(native) = official_claude_npm_binary(&selected).filter(|path| path.is_file()) {
            return native
                .canonicalize()
                .map_err(|error| format!("resolve Claude native binary failed: {error}"));
        }
    }
    selected
        .canonicalize()
        .map_err(|error| format!("resolve Claude binary failed: {error}"))
}

pub(crate) fn build_command(bin: &str) -> Command {
    #[cfg(target_os = "windows")]
    {
        if let Ok(native) = resolve_claude_binary_path(bin) {
            let mut command = Command::new(native);
            command.creation_flags(CREATE_NO_WINDOW);
            return command;
        }
        let mut c = Command::new("cmd");
        c.arg("/C").arg(bin).creation_flags(CREATE_NO_WINDOW);
        c
    }
    #[cfg(not(target_os = "windows"))]
    {
        Command::new(bin)
    }
}

pub(crate) fn build_claude_model_only_command(
    bin: &str,
    model: &str,
    env: &[(String, String)],
    cwd: &std::path::Path,
    prompt: &str,
    reasoning_effort: ReasoningEffort,
) -> Result<Command, String> {
    let mut command = build_command(bin);
    apply_inherited_agent_environment(&mut command);
    for (key, value) in env {
        if !key.starts_with("HELM_") {
            command.env(key, value);
        }
    }
    command
        .current_dir(cwd)
        .kill_on_drop(true)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .args([
            "--print",
            "--output-format",
            "json",
            "--model",
            model,
            "--tools",
            "",
            "--disable-slash-commands",
            "--strict-mcp-config",
            "--mcp-config",
            "{}",
            "--no-session-persistence",
            "--permission-mode",
            "plan",
            "--safe-mode",
            "--setting-sources",
            "",
        ]);
    if reasoning_effort != ReasoningEffort::Auto {
        command.args(["--effort", reasoning_effort.as_str()]);
        command.env("CLAUDE_CODE_EFFORT_LEVEL", reasoning_effort.as_str());
    } else {
        command.env_remove("CLAUDE_CODE_EFFORT_LEVEL");
    }
    command.arg(prompt);
    Ok(command)
}

pub(crate) fn parse_claude_model_only_output(
    stdout: &[u8],
) -> Result<crate::operations::ModelOnlyOperationOutput, String> {
    let payload: serde_json::Value = serde_json::from_slice(stdout)
        .map_err(|error| format!("[operation_invalid_output] Claude JSON 解析失败：{error}"))?;
    if model_only_payload_contains_forbidden_runtime_event(&payload) {
        return Err(
            "[operation_tool_not_allowed] ModelOnlyOperation 收到意外工具或审批事件".to_string(),
        );
    }
    let text = payload
        .get("result")
        .or_else(|| payload.get("output_text"))
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "[operation_invalid_output] Claude 输出缺少 result".to_string())?;
    let usage = payload.get("usage").unwrap_or(&serde_json::Value::Null);
    Ok(crate::operations::ModelOnlyOperationOutput {
        text: text.to_string(),
        input_tokens: usage
            .get("input_tokens")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or_default(),
        cached_input_tokens: usage
            .get("cache_read_input_tokens")
            .or_else(|| usage.get("cached_input_tokens"))
            .and_then(serde_json::Value::as_u64)
            .unwrap_or_default(),
        cache_write_input_tokens: usage
            .get("cache_creation_input_tokens")
            .or_else(|| usage.get("cache_write_input_tokens"))
            .and_then(serde_json::Value::as_u64)
            .unwrap_or_default(),
        output_tokens: usage
            .get("output_tokens")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or_default(),
        reported_cost_usd: payload
            .get("total_cost_usd")
            .and_then(serde_json::Value::as_f64),
        service_tier: usage
            .get("service_tier")
            .and_then(serde_json::Value::as_str)
            .map(ToString::to_string),
        observed_model_id: payload
            .get("model")
            .and_then(serde_json::Value::as_str)
            .map(ToString::to_string),
    })
}

fn model_only_payload_contains_forbidden_runtime_event(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Object(fields) => fields.iter().any(|(key, value)| {
            let normalized = key.to_ascii_lowercase().replace(['-', '_'], "");
            let forbidden_key = matches!(
                normalized.as_str(),
                "tooluse"
                    | "tooluses"
                    | "toolcall"
                    | "toolcalls"
                    | "approval"
                    | "approvalrequest"
                    | "permissiondenial"
                    | "permissiondenials"
            );
            (forbidden_key && runtime_event_value_is_present(value))
                || model_only_payload_contains_forbidden_runtime_event(value)
        }),
        serde_json::Value::Array(values) => values
            .iter()
            .any(model_only_payload_contains_forbidden_runtime_event),
        serde_json::Value::String(value) => matches!(
            value.to_ascii_lowercase().replace(['-', '_'], "").as_str(),
            "tooluse" | "toolcall" | "approvalrequest" | "permissiondenial"
        ),
        _ => false,
    }
}

fn runtime_event_value_is_present(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Null => false,
        serde_json::Value::Bool(value) => *value,
        serde_json::Value::String(value) => !value.trim().is_empty(),
        serde_json::Value::Array(value) => !value.is_empty(),
        serde_json::Value::Object(value) => !value.is_empty(),
        serde_json::Value::Number(_) => true,
    }
}

pub(crate) fn claude_uses_user_setting_source(env: &[(String, String)]) -> bool {
    !env.iter().any(|(key, value)| {
        matches!(
            key.to_ascii_uppercase().as_str(),
            "ANTHROPIC_API_KEY" | "ANTHROPIC_AUTH_TOKEN"
        ) && !value.trim().is_empty()
    })
}

fn apply_claude_setting_source_policy(command: &mut Command, use_user_setting_source: bool) {
    if !use_user_setting_source {
        // API Binding 必须完全由 Helm 注入的 Provider/Model 配置决定，不能被
        // ~/.claude 或项目设置中的 env/model 覆盖；显式 --settings 仍会加载审批 hook。
        command.args(["--setting-sources", ""]);
    }
}

pub(crate) fn build_codex_command(bin: &str) -> Command {
    #[cfg(target_os = "windows")]
    {
        if let Ok(native) = crate::codex_app_server::resolve_codex_native_executable(bin) {
            let mut command = Command::new(native);
            command.creation_flags(CREATE_NO_WINDOW);
            return command;
        }
        let mut c = Command::new("cmd");
        c.arg("/C").arg(bin).creation_flags(CREATE_NO_WINDOW);
        c
    }
    #[cfg(not(target_os = "windows"))]
    {
        Command::new(bin)
    }
}

/// 把历史消息序列化成新进程可读的开场 prompt（Codex 续聊与 Claude 回溯重建共用，P2-5）。
fn serialize_history_prompt(
    history: &[crate::sessions::SessionMessage],
    current_prompt: &str,
) -> String {
    if history.is_empty() {
        return current_prompt.to_string();
    }
    let mut context = String::from("之前的对话历史：\n\n");
    for msg in history {
        let role = match msg.role {
            Role::User => "用户",
            Role::Assistant => "助手",
        };
        context.push_str(&format!("{}: {}\n\n", role, msg.text));
    }
    context.push_str(&format!("用户: {current_prompt}"));
    context
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CodexAppServerThreadPlan {
    Start,
    Resume(String),
}

fn codex_app_server_thread_plan(
    native_thread_id: Option<&str>,
    force_history_rebuild: bool,
) -> CodexAppServerThreadPlan {
    if force_history_rebuild {
        CodexAppServerThreadPlan::Start
    } else if let Some(thread_id) = native_thread_id.filter(|id| !id.is_empty()) {
        CodexAppServerThreadPlan::Resume(thread_id.to_string())
    } else {
        CodexAppServerThreadPlan::Start
    }
}

fn codex_app_server_prompt(
    force_history_rebuild: bool,
    history: &[crate::sessions::SessionMessage],
    current_prompt: &str,
) -> String {
    if force_history_rebuild {
        serialize_history_prompt(history, current_prompt)
    } else {
        current_prompt.to_string()
    }
}

fn is_codex_thread_missing_error(message: &str) -> bool {
    let normalized = message.to_ascii_lowercase();
    normalized.contains("no rollout found for thread id")
        || ((normalized.contains("thread") || normalized.contains("session"))
            && (normalized.contains("not found") || normalized.contains("does not exist")))
}

fn reset_codex_context_state(
    thread_id: &Arc<std::sync::Mutex<Option<String>>>,
    force_history_rebuild: &Arc<AtomicBool>,
    history_messages: &Arc<std::sync::Mutex<Vec<crate::sessions::SessionMessage>>>,
    messages: Vec<crate::sessions::SessionMessage>,
) -> Result<(), String> {
    *thread_id
        .lock()
        .map_err(|_| "Codex thread id 锁中毒".to_string())? = None;
    *history_messages
        .lock()
        .map_err(|_| "Codex 历史锁中毒".to_string())? = messages;
    force_history_rebuild.store(true, Ordering::Release);
    Ok(())
}

pub(crate) fn codex_provider_config_args(env: &[(String, String)]) -> Vec<String> {
    let Some((_, base_url)) = env.iter().find(|(key, _)| key == "OPENAI_BASE_URL") else {
        // subscription 必须使用 Codex first-party Provider；不能让用户全局 config.toml
        // 中残留的 custom model_provider 把官方 OAuth Binding 偷换成第三方路由。
        return vec!["model_provider=openai".to_string()];
    };
    let wire_api = env
        .iter()
        .find(|(key, _)| key == "HELM_CODEX_WIRE_API")
        .map(|(_, value)| value.as_str())
        .unwrap_or("responses");
    vec![
        "model_provider=helm".to_string(),
        "model_providers.helm.name=helm".to_string(),
        format!("model_providers.helm.base_url={base_url}"),
        format!("model_providers.helm.wire_api={wire_api}"),
        "model_providers.helm.requires_openai_auth=true".to_string(),
    ]
}

fn spawn_agent_task<F>(future: F)
where
    F: Future<Output = ()> + Send + 'static,
{
    tauri::async_runtime::spawn(future);
}

fn set_event_turn_context(history_session_id: &str, turn_id: &str, epoch: u64) {
    event_turn_contexts()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(history_session_id.to_string(), (turn_id.to_string(), epoch));
}

fn event_turn_context(history_session_id: &str) -> Option<(String, u64)> {
    event_turn_contexts()
        .lock()
        .ok()
        .and_then(|contexts| contexts.get(history_session_id).cloned())
}

fn event_turn_contexts() -> &'static std::sync::Mutex<HashMap<String, (String, u64)>> {
    static CONTEXTS: std::sync::OnceLock<std::sync::Mutex<HashMap<String, (String, u64)>>> =
        std::sync::OnceLock::new();
    CONTEXTS.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}

fn begin_turn_supervisor(
    app: &AppHandle,
    history_session_id: &str,
    turn_id: &str,
    turn_epoch: u64,
    mode: TurnMode,
    permission_profile: PermissionProfile,
) {
    if let Some(supervisor) = app.try_state::<crate::turn_supervisor::TurnSupervisor>() {
        supervisor.begin(
            history_session_id,
            turn_id,
            turn_epoch,
            mode.as_state_str(),
            permission_profile.as_str(),
        );
    }
}

fn normalize_runtime_search_tool_result(event: &AgentEvent) -> AgentEvent {
    let AgentEvent::ToolResult {
        session_id,
        id,
        status: ToolStatus::Error,
        output: Some(output),
        diff,
        ..
    } = event
    else {
        return event.clone();
    };
    let lower = output.to_ascii_lowercase();
    let search_name = if lower.contains("websearch") || lower.contains("web search") {
        Some("WebSearch")
    } else if lower.contains("webfetch") || lower.contains("web fetch") {
        Some("WebFetch")
    } else {
        None
    };
    let unavailable = lower.contains("unknown tool")
        || lower.contains("tool not found")
        || lower.contains("not available")
        || lower.contains("does not exist")
        || lower.contains("unavailable");
    if let Some(name) = search_name.filter(|_| unavailable) {
        return AgentEvent::ToolResult {
            session_id: session_id.clone(),
            id: id.clone(),
            status: ToolStatus::Error,
            output: Some(format!(
                "[runtime_web_search_unavailable] 当前 Runtime/Provider 未提供 {name}，未切换到其他网络服务。"
            )),
            diff: diff.clone(),
            outcome: Some(crate::protocol::ToolOutcomeKind::RuntimeDenied),
            started: Some(false),
            has_output: Some(true),
            retryable: Some(false),
            denial_source: Some(crate::protocol::ToolDenialSource::Runtime),
            native_denial_code: Some("runtime_web_search_unavailable".to_string()),
        };
    }
    event.clone()
}

fn emit_agent_event(app: &AppHandle, history_session_id: &str, event: &AgentEvent) {
    let context = event_turn_context(history_session_id);
    emit_agent_event_in_turn(
        app,
        history_session_id,
        context.as_ref().map(|(turn_id, _)| turn_id.as_str()),
        context.as_ref().map(|(_, epoch)| *epoch),
        event,
    );
}

fn emit_agent_event_in_turn(
    app: &AppHandle,
    history_session_id: &str,
    explicit_turn_id: Option<&str>,
    explicit_turn_epoch: Option<u64>,
    event: &AgentEvent,
) {
    let event =
        crate::redaction::sanitize_agent_event(&normalize_runtime_search_tool_result(event));
    let turn_id = explicit_turn_id.map(ToString::to_string);
    let turn_epoch = explicit_turn_epoch;
    if let Some(supervisor) = app.try_state::<crate::turn_supervisor::TurnSupervisor>() {
        let _ = supervisor.submit_event(history_session_id, turn_id.as_deref(), turn_epoch, event);
        return;
    }
    // 只供未安装 Tauri state 的旧测试壳使用；生产 setup 始终安装 Supervisor。
    if let Some(store) = app.try_state::<SessionHistoryStore>() {
        if let Err(err) =
            store.record_event_for_session_in_turn(history_session_id, turn_id.as_deref(), &event)
        {
            eprintln!("[helm] 会话历史写入失败：{err}");
        }
    }
    let _ = app.emit(
        EVENT_NAME,
        serde_json::json!({
            "historyId": history_session_id,
            "eventSeq": 0,
            "turnId": turn_id,
            "turnEpoch": turn_epoch,
            "event": event,
        }),
    );
}

/// delta 合批（变更-09）：event 与缓冲中的 delta 同类且同会话时并入缓冲，返回 true。
/// 只合并 MessageDelta / ThinkingDelta——其他事件必须保序发出。
fn merge_pending_delta(pending: &mut Option<AgentEvent>, event: &AgentEvent) -> bool {
    match (pending.as_mut(), event) {
        (
            Some(AgentEvent::MessageDelta {
                session_id: buf_sid,
                role: buf_role,
                text: buf,
            }),
            AgentEvent::MessageDelta {
                session_id,
                role,
                text,
            },
        ) if buf_sid == session_id && buf_role == role => {
            buf.push_str(text);
            true
        }
        (
            Some(AgentEvent::ThinkingDelta {
                session_id: buf_sid,
                text: buf,
            }),
            AgentEvent::ThinkingDelta { session_id, text },
        ) if buf_sid == session_id => {
            buf.push_str(text);
            true
        }
        _ => false,
    }
}

pub(crate) fn create_codex_auth_home(
    env: &[(String, String)],
    disabled_mcp: &[String],
) -> Result<Option<CodexAuthHome>, String> {
    let real_codex_dir = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .ok()
        .map(|home| PathBuf::from(home).join(".codex"));
    create_codex_auth_home_with_source(env, real_codex_dir.as_deref(), disabled_mcp)
}

fn copy_directory_recursive(source: &Path, destination: &Path) -> Result<(), String> {
    fs::create_dir_all(destination).map_err(|e| format!("创建继承目录失败：{e}"))?;
    for entry in fs::read_dir(source).map_err(|e| format!("读取继承目录失败：{e}"))? {
        let entry = entry.map_err(|e| format!("读取继承目录项失败：{e}"))?;
        let file_type = entry
            .file_type()
            .map_err(|e| format!("读取继承目录项类型失败：{e}"))?;
        let from = entry.path();
        let to = destination.join(entry.file_name());
        if file_type.is_symlink() {
            let target = fs::canonicalize(&from)
                .map_err(|e| format!("解析 Codex Skill 链接失败（{}）：{e}", from.display()))?;
            let metadata = fs::metadata(&target)
                .map_err(|e| format!("读取 Codex Skill 链接目标失败（{}）：{e}", from.display()))?;
            if metadata.is_dir() {
                copy_directory_recursive(&target, &to)?;
            } else if metadata.is_file() {
                fs::copy(&target, &to)
                    .map_err(|e| format!("继承 Codex Skill 链接文件失败：{e}"))?;
            }
        } else if file_type.is_dir() {
            copy_directory_recursive(&from, &to)?;
        } else if file_type.is_file() {
            fs::copy(&from, &to).map_err(|e| format!("继承 Codex Skill 文件失败：{e}"))?;
        }
    }
    Ok(())
}

/// 临时 CODEX_HOME 必须继承真实 ~/.codex 的 config.toml 与 prompts/，
/// 否则用户已有的 MCP、模型配置、custom prompts 会在 API key 启动时全部丢失（变更-03 A.1）。
/// disabled_mcp 非空时在继承的 config.toml 里剔除对应 `[mcp_servers.<name>]`（变更-11）。
fn create_codex_auth_home_with_source(
    env: &[(String, String)],
    source_dir: Option<&Path>,
    disabled_mcp: &[String],
) -> Result<Option<CodexAuthHome>, String> {
    let Some((_, api_key)) = env
        .iter()
        .find(|(key, value)| key == "OPENAI_API_KEY" && !value.trim().is_empty())
    else {
        return Ok(None);
    };
    let path = unique_temp_dir("helm-codex-home");
    fs::create_dir_all(&path).map_err(|e| format!("创建 Codex 临时认证目录失败：{e}"))?;
    let home = CodexAuthHome { path };
    if let Some(source) = source_dir {
        let config = source.join("config.toml");
        if config.is_file() {
            if disabled_mcp.is_empty() {
                fs::copy(&config, home.path.join("config.toml"))
                    .map_err(|e| format!("继承 Codex 配置失败：{e}"))?;
            } else {
                let raw =
                    fs::read_to_string(&config).map_err(|e| format!("读取 Codex 配置失败：{e}"))?;
                let filtered = filter_codex_mcp_servers(&raw, disabled_mcp)?;
                fs::write(home.path.join("config.toml"), filtered)
                    .map_err(|e| format!("写入 Codex 配置失败：{e}"))?;
            }
        }
        let prompts = source.join("prompts");
        if prompts.is_dir() {
            let dest = home.path.join("prompts");
            fs::create_dir_all(&dest).map_err(|e| format!("创建 Codex prompts 目录失败：{e}"))?;
            let entries =
                fs::read_dir(&prompts).map_err(|e| format!("读取 Codex prompts 失败：{e}"))?;
            for entry in entries {
                let entry = entry.map_err(|e| format!("读取 Codex prompts 失败：{e}"))?;
                let from = entry.path();
                if from.extension().and_then(|ext| ext.to_str()) != Some("md") {
                    continue;
                }
                let Some(name) = from.file_name() else {
                    continue;
                };
                fs::copy(&from, dest.join(name))
                    .map_err(|e| format!("继承 Codex prompt 失败：{e}"))?;
            }
        }
        let skills = source.join("skills");
        if skills.is_dir() {
            copy_directory_recursive(&skills, &home.path.join("skills"))?;
        }
    }
    // auth.json 最后写入：即使继承来源里有旧 auth 也以本次注入的 key 为准。
    let auth_text = serde_json::to_string(&json!({ "OPENAI_API_KEY": api_key }))
        .map_err(|e| format!("序列化 Codex 临时认证失败：{e}"))?;
    fs::write(home.path.join("auth.json"), auth_text)
        .map_err(|e| format!("写入 Codex 临时认证失败：{e}"))?;
    Ok(Some(home))
}

/// 从 Codex config.toml 文本中剔除停用的 `[mcp_servers.<name>]` 段（变更-11）。
fn filter_codex_mcp_servers(raw: &str, disabled: &[String]) -> Result<String, String> {
    let mut value: toml::Value =
        toml::from_str(raw).map_err(|e| format!("解析 Codex 配置失败：{e}"))?;
    if let Some(servers) = value
        .get_mut("mcp_servers")
        .and_then(|item| item.as_table_mut())
    {
        for name in disabled {
            servers.remove(name);
        }
    }
    toml::to_string_pretty(&value).map_err(|e| format!("序列化 Codex 配置失败：{e}"))
}

fn claude_permission_mode(mode: TurnMode, profile: PermissionProfile) -> &'static str {
    match mode {
        TurnMode::Plan => "plan",
        TurnMode::Ask => "manual",
        TurnMode::Build => match profile {
            PermissionProfile::Standard => "manual",
            PermissionProfile::Auto => "auto",
            PermissionProfile::FullAccess => "bypassPermissions",
        },
    }
}

fn claude_permission_mode_for_capability(
    mode: TurnMode,
    profile: PermissionProfile,
    auto_support: crate::capability_registry::CapabilitySupport,
) -> &'static str {
    if mode == TurnMode::Build
        && profile == PermissionProfile::Auto
        && auto_support == crate::capability_registry::CapabilitySupport::Degraded
    {
        "acceptEdits"
    } else {
        claude_permission_mode(mode, profile)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AutoFallbackDecision {
    None,
    RetryCompatible,
    Fuse,
}

fn auto_fallback_decision(
    profile: PermissionProfile,
    outcome: Option<crate::protocol::ToolOutcomeKind>,
    already_attempted: bool,
    saw_executed_tool: bool,
) -> AutoFallbackDecision {
    if profile != PermissionProfile::Auto
        || !matches!(
            outcome,
            Some(
                crate::protocol::ToolOutcomeKind::AutoReviewUnavailable
                    | crate::protocol::ToolOutcomeKind::AutoReviewParseError
            )
        )
    {
        return AutoFallbackDecision::None;
    }
    if already_attempted || saw_executed_tool {
        AutoFallbackDecision::Fuse
    } else {
        AutoFallbackDecision::RetryCompatible
    }
}

fn codex_runtime_profile_policy(
    mode: TurnMode,
    profile: PermissionProfile,
) -> (&'static str, bool, CodexApprovalPolicy) {
    if mode != TurnMode::Build {
        return ("read-only", false, CodexApprovalPolicy::Untrusted);
    }
    match profile {
        PermissionProfile::Standard => ("workspace-write", false, CodexApprovalPolicy::Untrusted),
        PermissionProfile::Auto => ("workspace-write", true, CodexApprovalPolicy::OnRequest),
        PermissionProfile::FullAccess => ("danger-full-access", true, CodexApprovalPolicy::Never),
    }
}

/// 会话级 MCP 开关（变更-11）：从真实 `~/.claude/settings.json` 读出 mcpServers，
/// 剔除停用项后写成一份临时 mcp-config。配合 `--strict-mcp-config` 让本轮
/// 只加载过滤后的集合（CLI 官方支持的完全控制路径）。
fn restrict_claude_runtime_mcp_servers(
    servers: serde_json::Map<String, serde_json::Value>,
    disabled: &[String],
) -> serde_json::Map<String, serde_json::Value> {
    servers
        .into_iter()
        .filter(|(name, _)| name != "helm" && !disabled.iter().any(|item| item == name))
        .collect()
}

fn build_claude_mcp_config_file(disabled: &[String], out_dir: &Path) -> Result<PathBuf, String> {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map_err(|_| "无法解析用户主目录".to_string())?;
    let settings_path = PathBuf::from(home).join(".claude").join("settings.json");
    let servers = fs::read_to_string(&settings_path)
        .ok()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        .and_then(|settings| settings.get("mcpServers").cloned())
        .and_then(|value| match value {
            serde_json::Value::Object(map) => Some(map),
            _ => None,
        })
        .unwrap_or_default();
    let filtered = restrict_claude_runtime_mcp_servers(servers, disabled);
    let config = serde_json::json!({ "mcpServers": filtered });
    let path = out_dir.join("mcp-config.json");
    fs::write(
        &path,
        serde_json::to_string_pretty(&config).map_err(|e| format!("序列化 MCP 配置失败：{e}"))?,
    )
    .map_err(|e| format!("写入 MCP 配置失败：{e}"))?;
    Ok(path)
}

fn prompt_with_attachments(text: &str, attachments: &[String]) -> String {
    let mounted_paths = attachments
        .iter()
        .map(|path| path.trim())
        .filter(|path| !path.is_empty())
        .collect::<Vec<_>>();
    if mounted_paths.is_empty() {
        return text.to_string();
    }

    let mut prompt = String::from(text);
    prompt.push_str("\n\n已挂载上下文：\n");
    for path in mounted_paths {
        prompt.push_str("- ");
        prompt.push_str(path);
        prompt.push('\n');
    }
    prompt.push_str("\n请在需要时直接读取以上本地路径。");
    prompt
}

fn apply_claude_session_context_args(
    cmd: &mut Command,
    cwd: &str,
    session_context: &[crate::turn_start::FrozenSessionContext],
) {
    for context in session_context {
        let allowed_dir = if context.kind == "directory" {
            Path::new(&context.canonical_path)
        } else {
            Path::new(&context.canonical_path)
                .parent()
                .unwrap_or_else(|| Path::new(cwd))
        };
        cmd.arg("--add-dir").arg(allowed_dir);
    }
}

const PROCESS_TREE_CLEANUP_TIMEOUT: Duration = Duration::from_secs(5);

async fn run_process_command_with_timeout(mut command: Command, timeout: Duration) -> bool {
    command.kill_on_drop(true);
    matches!(
        tokio::time::timeout(timeout, command.output()).await,
        Ok(Ok(_))
    )
}

/// 按 pid 杀掉整棵进程树（中断用）。Windows 走 `taskkill /T`，Unix 走 `kill -TERM`。
/// 清理命令本身也属于不可信外部进程，必须有硬超时，避免中断路径永久挂死。
pub(crate) async fn kill_tree(pid: Option<u32>) {
    let Some(pid) = pid else {
        return;
    };
    #[cfg(target_os = "windows")]
    {
        let mut command = Command::new("taskkill");
        command
            .args(["/F", "/T", "/PID", &pid.to_string()])
            .creation_flags(CREATE_NO_WINDOW);
        let _ = run_process_command_with_timeout(command, PROCESS_TREE_CLEANUP_TIMEOUT).await;
    }
    #[cfg(not(target_os = "windows"))]
    {
        let mut command = Command::new("kill");
        command.args(["-TERM", &pid.to_string()]);
        let _ = run_process_command_with_timeout(command, PROCESS_TREE_CLEANUP_TIMEOUT).await;
    }
}

pub(crate) async fn terminate_child_bounded(
    child: &mut tokio::process::Child,
    pid: Option<u32>,
    wait_timeout: Duration,
) -> bool {
    if pid.is_some() {
        kill_tree(pid).await;
        let exited = matches!(
            tokio::time::timeout(wait_timeout, child.wait()).await,
            Ok(Ok(_))
        );
        if exited {
            return true;
        }
    }
    let _ = child.start_kill();
    matches!(
        tokio::time::timeout(wait_timeout, child.wait()).await,
        Ok(Ok(_))
    )
}

/// 校验工作目录：为空或不存在都不允许启动进程（S8/S9 场景守卫）。
fn validate_cwd(cwd: &str) -> Result<(), String> {
    if cwd.trim().is_empty() {
        return Err(
            "未设置工作目录：请先在「设置 → 通用」中选择默认工作目录，再发送消息".to_string(),
        );
    }
    if !Path::new(cwd).is_dir() {
        return Err(format!("工作目录不存在：{cwd}。请重新选择一个有效目录"));
    }
    Ok(())
}

fn unique_temp_dir(prefix: &str) -> PathBuf {
    let counter = TEMP_DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "{}-{}-{}-{}",
        prefix,
        std::process::id(),
        now_millis(),
        counter
    ))
}

#[cfg(target_os = "windows")]
fn hook_script_name() -> &'static str {
    "approval-hook.ps1"
}

#[cfg(not(target_os = "windows"))]
fn hook_script_name() -> &'static str {
    "approval-hook.py"
}

#[cfg(target_os = "windows")]
fn runtime_hook_command(script: &Path, state: &Path) -> String {
    let _ = script;
    let executable = std::env::current_exe()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|_| "helm.exe".to_string());
    format!(
        "\"{executable}\" --helm-runtime-hook \"{}\"",
        state.display()
    )
}

#[cfg(not(target_os = "windows"))]
fn runtime_hook_command(script: &Path, state: &Path) -> String {
    format!("python3 \"{}\" \"{}\"", script.display(), state.display())
}

fn create_runtime_approval_hook_files() -> Result<ApprovalHookFiles, String> {
    let root = unique_temp_dir("helm-runtime-approval");
    fs::create_dir_all(&root).map_err(|e| format!("创建 Runtime 审批目录失败：{e}"))?;
    let state_path = root.join("approval-state.json");
    write_approval_state(&state_path, &ApprovalState::default())?;
    let script_path = root.join(hook_script_name());
    crate::claude_permission_hook::write_runtime_hook_script(&script_path)?;
    let settings_path = root.join("claude-settings.json");
    let settings = json!({
        "hooks": {
            "PreToolUse": [{
                "matcher": "Read|Glob|Grep|LS|Write|Edit|MultiEdit|NotebookEdit|Bash|WebFetch|WebSearch|mcp__.*",
                "hooks": [{
                    "type": "command",
                    "command": runtime_hook_command(&script_path, &state_path),
                    "timeout": 10
                }]
            }]
        }
    });
    fs::write(
        &settings_path,
        serde_json::to_string_pretty(&settings).map_err(|e| e.to_string())?,
    )
    .map_err(|e| format!("写入 Runtime 审批设置失败：{e}"))?;
    Ok(ApprovalHookFiles {
        settings_path,
        state_path,
    })
}

fn read_approval_state(path: &Path) -> ApprovalState {
    fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str::<ApprovalState>(&raw).ok())
        .unwrap_or_default()
}

fn write_approval_state(path: &Path, state: &ApprovalState) -> Result<(), String> {
    let text = serde_json::to_string(state).map_err(|e| format!("序列化审批状态失败：{e}"))?;
    fs::write(path, text).map_err(|e| format!("写入审批状态失败：{e}"))
}

/// 从工具调用输入中提取操作目标（文件路径等），用于防止换工具重试
fn extract_tool_target(tool_name: &str, input: &serde_json::Value) -> Option<String> {
    match tool_name {
        "Write" | "Edit" | "MultiEdit" | "Read" => {
            input.get("file_path")?.as_str().map(|s| s.to_string())
        }
        // NotebookEdit 的目标参数是 notebook_path（变更-07：此前漏检导致 notebook 改动无检查点）
        "NotebookEdit" => input
            .get("notebook_path")
            .or_else(|| input.get("file_path"))?
            .as_str()
            .map(|s| s.to_string()),
        "Bash" => {
            let cmd = input.get("command")?.as_str()?;
            use regex::Regex;
            // `>{1,2}`：覆盖追加重定向 `>>`（此前 `>>` 会把第二个 `>` 一起捕进目标路径）
            let redirect = Regex::new(r#">{1,2}\s*["']?([^\s"'>][^\s"']*)"#).ok()?;
            if let Some(target) = redirect
                .captures(cmd)
                .and_then(|caps| caps.get(1))
                .map(|m| m.as_str().to_string())
            {
                return Some(target);
            }

            let command_target =
                Regex::new(r#"\b(?:rm|rmdir|mv|dd|chmod)\b\s+(?:-[^\s]+\s+)*["']?([^\s"']+)"#)
                    .ok()?;
            command_target
                .captures(cmd)
                .and_then(|caps| caps.get(1))
                .map(|m| m.as_str().to_string())
        }
        _ => None,
    }
}

fn checkpoint_target_path(
    tool_name: &str,
    input: &serde_json::Value,
    cwd: &Path,
) -> Option<PathBuf> {
    match tool_name {
        "Write" | "Edit" | "MultiEdit" | "NotebookEdit" => {
            let target = extract_tool_target(tool_name, input)?;
            let target = target.trim();
            if target.is_empty() || target.eq_ignore_ascii_case("null") {
                return None;
            }
            let path = PathBuf::from(target);
            if path
                .components()
                .any(|component| matches!(component, std::path::Component::ParentDir))
            {
                return None;
            }
            let path = if path.is_absolute() {
                path
            } else {
                cwd.join(path)
            };
            let normalized = path
                .to_string_lossy()
                .replace('\\', "/")
                .to_ascii_lowercase();
            let file_name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default()
                .trim_end_matches('.')
                .to_ascii_lowercase();
            if normalized == "/dev/null"
                || normalized.starts_with("//./")
                || normalized.starts_with("//?/")
                || matches!(
                    file_name.as_str(),
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
                )
            {
                return None;
            }
            let canonical_cwd = cwd.canonicalize().ok()?;
            let scoped_path = if path.exists() {
                path.canonicalize().ok()?
            } else {
                let parent = path.parent()?.canonicalize().ok()?;
                parent.join(path.file_name()?)
            };
            let scope_key = |value: &Path| {
                value
                    .to_string_lossy()
                    .replace('\\', "/")
                    .trim_end_matches('/')
                    .to_ascii_lowercase()
            };
            let cwd_key = scope_key(&canonical_cwd);
            let target_key = scope_key(&scoped_path);
            if target_key != cwd_key && !target_key.starts_with(&format!("{cwd_key}/")) {
                return None;
            }
            Some(scoped_path)
        }
        _ => None,
    }
}

fn checkpoint_id_for_tool(tool_id: &str) -> String {
    let safe = tool_id
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    if safe.is_empty() {
        format!("ckpt-{}", now_millis())
    } else {
        format!("ckpt-{safe}")
    }
}

fn create_auto_checkpoint_for_tool(
    history_store: &SessionHistoryStore,
    snapshots_dir: &Path,
    history_session_id: &str,
    cli_session_id: &str,
    cwd: &Path,
    tool_id: &str,
    tool_name: &str,
    input: &serde_json::Value,
    turn_id: &str,
) -> Result<Option<AgentEvent>, String> {
    let Some(target) = checkpoint_target_path(tool_name, input, cwd) else {
        return Ok(None);
    };

    let checkpoint_id = checkpoint_id_for_tool(tool_id);
    let snapshot_store = crate::snapshots::SnapshotStore::new(snapshots_dir.to_path_buf());
    let snapshot = snapshot_store.capture_files(std::slice::from_ref(&target))?;
    if snapshot.files.is_empty() {
        return Ok(None);
    }
    snapshot_store.save(&checkpoint_id, &snapshot)?;

    let label_target = target
        .file_name()
        .and_then(|name| name.to_str())
        .map(ToString::to_string)
        .unwrap_or_else(|| target.to_string_lossy().to_string());
    let ts = now_millis();
    history_store.save_checkpoint(
        &checkpoint_id,
        history_session_id,
        0,
        &format!("改动前：{label_target}"),
        &checkpoint_id,
        ts,
        turn_id,
        true,
        snapshot.files.len() as u64,
        None,
    )?;

    Ok(Some(AgentEvent::Checkpoint {
        session_id: cli_session_id.to_string(),
        id: checkpoint_id,
        label: format!("改动前：{label_target}"),
        ts,
        restorable: true,
        file_count: snapshot.files.len() as u64,
        reason: None,
    }))
}

#[cfg(test)]
mod tests {
    use super::{
        agent_environment_from_settings, auto_fallback_decision, build_approval_action,
        checkpoint_target_path, claude_exit_disposition, claude_permission_mode,
        claude_permission_mode_for_capability, codex_provider_config_args,
        codex_runtime_profile_policy, codex_sandbox_for_mode, create_auto_checkpoint_for_tool,
        create_codex_auth_home, create_codex_auth_home_with_source,
        create_runtime_approval_hook_files, extract_tool_target,
        filter_inherited_agent_environment, finish_codex_interrupt_terminal, full_access_lease,
        lease_is_valid, merge_pending_delta, normalize_runtime_search_tool_result,
        parse_codex_line, prompt_with_attachments, record_approval_state, reserve_turn_flag,
        rollback_prepared_approval_state, run_serialized_approval, should_process_claude_event,
        spawn_agent_task, terminal_turn_outcome, validate_engine_bin, wait_until_idle_and_begin,
        write_approval_state, AgentSession, ApprovalDecision, ApprovalState, AutoFallbackDecision,
        ClaudeExitDisposition, ClaudeSession, PendingToolInfo, PermissionProfile, SessionCmd,
        TurnMode,
    };
    use crate::codex_app_server::CodexApprovalPolicy;
    use crate::protocol::{AgentEvent, EngineId, Role, StopReason, ToolStatus};
    use crate::sessions::{NewSessionRecord, SessionHistoryStore};
    use crate::settings::AppSettings;
    use serde_json::json;
    use std::collections::HashMap;
    use std::fs;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::sync::{mpsc, Mutex, Notify};

    #[test]
    fn missing_runtime_search_tool_is_normalized_without_fallback_service() {
        let normalized = normalize_runtime_search_tool_result(&AgentEvent::ToolResult {
            session_id: "cli".into(),
            id: "tool-1".into(),
            status: ToolStatus::Error,
            output: Some("Unknown tool WebSearch: tool not found".into()),
            diff: None,
            outcome: None,
            started: None,
            has_output: None,
            retryable: None,
            denial_source: None,
            native_denial_code: None,
        });
        let AgentEvent::ToolResult { output, .. } = normalized else {
            panic!("expected tool result");
        };
        assert!(output
            .unwrap()
            .starts_with("[runtime_web_search_unavailable]"));
    }

    #[test]
    fn claude_terminal_candidate_is_committed_only_after_successful_exit() {
        assert_eq!(
            claude_exit_disposition(false, false, false, true, true, false),
            ClaudeExitDisposition::EmitCandidate
        );
        // 有 terminal_candidate 时即使 exit code!=0 也优先 emit（如 API 403 错误场景）
        assert_eq!(
            claude_exit_disposition(false, false, false, false, true, false),
            ClaudeExitDisposition::EmitCandidate
        );
        // 无 terminal_candidate 且 exit code!=0 → ProcessError
        assert_eq!(
            claude_exit_disposition(false, false, false, false, false, false),
            ClaudeExitDisposition::ProcessError
        );
    }

    #[test]
    fn claude_successful_exit_without_result_fails_closed_unless_approval_is_deferred() {
        assert_eq!(
            claude_exit_disposition(false, false, false, true, false, false),
            ClaudeExitDisposition::MissingResult
        );
        assert_eq!(
            claude_exit_disposition(false, false, false, true, false, true),
            ClaudeExitDisposition::ApprovalDeferred
        );
        assert_eq!(
            claude_exit_disposition(false, false, false, true, true, true),
            ClaudeExitDisposition::ApprovalDeferred
        );
    }

    #[test]
    fn claude_resume_ignores_tool_and_approval_replay_before_session_started() {
        let approval = AgentEvent::ApprovalRequest {
            session_id: "native".to_string(),
            id: "old-approval".to_string(),
            action: "Bash".to_string(),
            detail: "echo old".to_string(),
            input: None,
            available_decisions: Vec::new(),
            persistent_label: None,
            matcher_summary: None,
        };
        assert!(!should_process_claude_event(false, false, &approval));
        assert!(should_process_claude_event(true, false, &approval));
        let approved_result = AgentEvent::ToolResult {
            session_id: "native".to_string(),
            id: "current-approved".to_string(),
            status: ToolStatus::Success,
            output: Some("ok".to_string()),
            diff: None,
            outcome: None,
            started: Some(true),
            has_output: Some(true),
            retryable: Some(false),
            denial_source: None,
            native_denial_code: None,
        };
        assert!(should_process_claude_event(false, true, &approved_result));
        assert!(!should_process_claude_event(false, false, &approved_result));
        assert!(should_process_claude_event(
            false,
            false,
            &AgentEvent::SessionStarted {
                session_id: "native".to_string(),
                engine: EngineId::ClaudeCode,
                model: "claude".to_string(),
                cwd: "D:/work".to_string(),
                ts: 1,
                capabilities: None,
            }
        ));
        assert!(should_process_claude_event(
            false,
            false,
            &AgentEvent::Error {
                session_id: None,
                message: "launch failed".to_string(),
                recoverable: false,
                kind: None,
            }
        ));
    }

    #[test]
    fn claude_interruption_and_auto_fallback_own_their_terminal_flow() {
        assert_eq!(
            claude_exit_disposition(true, false, false, false, true, false),
            ClaudeExitDisposition::Return
        );
        assert_eq!(
            claude_exit_disposition(false, true, false, false, true, false),
            ClaudeExitDisposition::Return
        );
        assert_eq!(
            claude_exit_disposition(false, false, true, false, false, false),
            ClaudeExitDisposition::Return
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn codex_auth_home_drop_retries_until_a_short_lived_exclusive_lock_releases() {
        use std::os::windows::fs::OpenOptionsExt;

        let path = super::unique_temp_dir("helm-codex-drop-retry");
        std::fs::create_dir_all(&path).unwrap();
        let auth_path = path.join("auth.json");
        std::fs::write(&auth_path, r#"{"OPENAI_API_KEY":"test-only"}"#).unwrap();
        let (locked_tx, locked_rx) = std::sync::mpsc::channel();
        let lock_path = auth_path.clone();
        let holder = std::thread::spawn(move || {
            let file = std::fs::OpenOptions::new()
                .read(true)
                .share_mode(0)
                .open(lock_path)
                .unwrap();
            locked_tx.send(()).unwrap();
            std::thread::sleep(Duration::from_millis(200));
            drop(file);
        });
        locked_rx.recv().unwrap();

        drop(super::CodexAuthHome { path: path.clone() });
        holder.join().unwrap();

        assert!(
            !path.exists(),
            "短暂文件锁释放后必须删除包含临时凭证的 CODEX_HOME"
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn codex_auth_home_drop_tolerates_windows_locks_longer_than_one_second() {
        use std::os::windows::fs::OpenOptionsExt;

        let path = super::unique_temp_dir("helm-codex-drop-long-retry");
        std::fs::create_dir_all(&path).unwrap();
        let auth_path = path.join("auth.json");
        std::fs::write(&auth_path, r#"{"OPENAI_API_KEY":"test-only"}"#).unwrap();
        let (locked_tx, locked_rx) = std::sync::mpsc::channel();
        let lock_path = auth_path.clone();
        let holder = std::thread::spawn(move || {
            let file = std::fs::OpenOptions::new()
                .read(true)
                .share_mode(0)
                .open(lock_path)
                .unwrap();
            locked_tx.send(()).unwrap();
            std::thread::sleep(Duration::from_millis(1_500));
            drop(file);
        });
        locked_rx.recv().unwrap();

        drop(super::CodexAuthHome { path: path.clone() });
        holder.join().unwrap();

        let removed = !path.exists();
        if !removed {
            std::fs::remove_dir_all(&path).unwrap();
        }
        assert!(
            removed,
            "Windows 扫描器或日志句柄占用超过一秒时仍必须删除临时 CODEX_HOME"
        );
    }

    #[test]
    fn effective_codex_home_prefers_session_snapshot_and_falls_back_to_subscription_home() {
        let session = std::path::PathBuf::from("D:/session-codex-home");
        let subscription = std::path::PathBuf::from(
            "C:/Users/test/AppData/Roaming/Helm/cli-profiles/codex-subscription",
        );

        assert_eq!(
            super::select_effective_codex_home(Some(&session), Some(&subscription)),
            Some(session)
        );
        assert_eq!(
            super::select_effective_codex_home(None, Some(&subscription)),
            Some(subscription)
        );
    }

    #[cfg(target_os = "windows")]
    #[tokio::test]
    async fn process_tree_cleanup_command_has_a_hard_timeout() {
        let mut command = tokio::process::Command::new("powershell");
        command
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "Start-Sleep -Seconds 30",
            ])
            .creation_flags(super::CREATE_NO_WINDOW);
        let started = std::time::Instant::now();

        let completed =
            super::run_process_command_with_timeout(command, Duration::from_millis(50)).await;

        assert!(!completed, "挂住的清理命令不得被当作成功完成");
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "清理命令必须在硬上限内返回，且测试需容忍 Windows 并行调度延迟"
        );
    }

    #[cfg(target_os = "windows")]
    #[tokio::test]
    async fn bounded_child_reap_falls_back_when_tree_kill_does_not_exit_target() {
        let mut command = tokio::process::Command::new("powershell");
        command
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "Start-Sleep -Seconds 30",
            ])
            .kill_on_drop(true);
        let mut child = command.spawn().unwrap();
        let started = std::time::Instant::now();

        let reaped = super::terminate_child_bounded(&mut child, None, Duration::from_secs(1)).await;

        assert!(
            started.elapsed() < Duration::from_secs(2),
            "tree kill 未命中时必须回退到直接 kill"
        );
        assert!(reaped, "清理函数必须报告子进程已被 reap");
        assert!(child.try_wait().unwrap().is_some(), "子进程必须已被 reap");
    }

    #[tokio::test]
    async fn approval_waits_for_busy_turn_then_acquires_execution_right() {
        let busy = Arc::new(AtomicBool::new(true));
        let idle_notify = Arc::new(Notify::new());
        let interrupted = Arc::new(AtomicBool::new(false));
        let waiter_busy = busy.clone();
        let waiter_notify = idle_notify.clone();
        let waiter_interrupted = interrupted.clone();
        let waiter = tokio::spawn(async move {
            wait_until_idle_and_begin(&waiter_busy, &waiter_notify, &waiter_interrupted).await
        });

        tokio::task::yield_now().await;
        assert!(
            busy.load(Ordering::Acquire),
            "旧轮次释放前审批不能抢占执行权"
        );

        busy.store(false, Ordering::Release);
        idle_notify.notify_waiters();

        tokio::time::timeout(Duration::from_secs(1), waiter)
            .await
            .expect("审批等待不应丢失 idle 通知")
            .expect("审批等待任务不应崩溃")
            .expect("旧轮次释放后审批应取得执行权");
        assert!(busy.load(Ordering::Acquire));
    }

    #[tokio::test]
    async fn idle_check_does_not_depend_on_a_future_notification() {
        let busy = AtomicBool::new(false);
        let idle_notify = Notify::new();
        let interrupted = AtomicBool::new(false);

        tokio::time::timeout(
            Duration::from_millis(100),
            wait_until_idle_and_begin(&busy, &idle_notify, &interrupted),
        )
        .await
        .expect("已经 idle 时必须立即 CAS 成功，不能等待下一次通知")
        .expect("已经 idle 时应取得执行权");
        assert!(busy.load(Ordering::Acquire));
    }

    #[tokio::test]
    async fn interrupted_approval_never_acquires_or_resumes_after_busy_releases() {
        let busy = Arc::new(AtomicBool::new(true));
        let idle_notify = Arc::new(Notify::new());
        let interrupted = Arc::new(AtomicBool::new(false));
        let waiter = tokio::spawn({
            let busy = busy.clone();
            let idle_notify = idle_notify.clone();
            let interrupted = interrupted.clone();
            async move { wait_until_idle_and_begin(&busy, &idle_notify, &interrupted).await }
        });

        tokio::task::yield_now().await;
        interrupted.store(true, Ordering::Release);
        busy.store(false, Ordering::Release);
        idle_notify.notify_waiters();

        let error = tokio::time::timeout(Duration::from_secs(1), waiter)
            .await
            .expect("中断必须唤醒等待中的审批")
            .expect("等待任务不应崩溃")
            .expect_err("中断后不得启动恢复轮");
        assert!(error.contains("已中断"));
        assert!(!busy.load(Ordering::Acquire));
    }

    #[tokio::test]
    async fn approval_transactions_are_serialized_through_the_commit_window() {
        let lock = Arc::new(Mutex::new(()));
        let release_first = Arc::new(Notify::new());
        let (entered_first, first_entered) = tokio::sync::oneshot::channel();
        let first_lock = lock.clone();
        let first_release = release_first.clone();
        let first = tokio::spawn(async move {
            run_serialized_approval(&first_lock, async move {
                let _ = entered_first.send(());
                first_release.notified().await;
            })
            .await;
        });
        first_entered.await.unwrap();

        let (entered_second, mut second_entered) = tokio::sync::oneshot::channel();
        let second_lock = lock.clone();
        let second = tokio::spawn(async move {
            run_serialized_approval(&second_lock, async move {
                let _ = entered_second.send(());
            })
            .await;
        });

        assert!(
            tokio::time::timeout(Duration::from_millis(50), &mut second_entered)
                .await
                .is_err(),
            "第一笔审批完成提交/回滚前，第二笔不得进入事务窗口"
        );
        release_first.notify_waiters();
        first.await.unwrap();
        tokio::time::timeout(Duration::from_secs(1), second_entered)
            .await
            .expect("第一笔退出后第二笔应进入")
            .unwrap();
        second.await.unwrap();
    }

    #[tokio::test]
    async fn approve_returns_the_actual_manager_error() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let session = AgentSession::Claude(ClaudeSession {
            tx,
            cwd: "D:/repo".to_string(),
            control: None,
        });
        tokio::spawn(async move {
            let Some(SessionCmd::Approve { responder, .. }) = rx.recv().await else {
                panic!("manager 应收到审批命令");
            };
            let _ = responder.send(Err("manager 记录审批失败".to_string()));
        });

        let error = session
            .approve("tool-1".to_string(), ApprovalDecision::Allow)
            .await
            .expect_err("调用方必须收到 manager 的真实失败");

        assert_eq!(error, "manager 记录审批失败");
    }

    #[tokio::test]
    async fn always_decision_without_pending_tool_info_is_rejected() {
        let root = super::unique_temp_dir("helm-approval-missing-pending-test");
        fs::create_dir_all(&root).unwrap();
        let state_path = root.join("approval-state.json");
        fs::write(
            &state_path,
            serde_json::to_string(&super::ApprovalState::default()).unwrap(),
        )
        .unwrap();
        let pending_tools = Mutex::new(std::collections::HashMap::new());

        let error = record_approval_state(
            &state_path,
            &pending_tools,
            "missing-tool",
            ApprovalDecision::Always,
        )
        .await
        .expect_err("缺少 pending tool 信息时不能伪造始终允许成功");

        assert!(error.contains("找不到待审批工具信息"));
        let state = super::read_approval_state(&state_path);
        assert!(!state.decisions.contains_key("missing-tool"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn approval_decision_has_stable_audit_values() {
        assert_eq!(ApprovalDecision::Allow.audit_value(), "allow");
        assert_eq!(ApprovalDecision::Turn.audit_value(), "turn");
        assert_eq!(ApprovalDecision::Session.audit_value(), "session");
        assert_eq!(ApprovalDecision::Project.audit_value(), "project");
        assert_eq!(ApprovalDecision::Deny.audit_value(), "deny");
        assert_eq!(ApprovalDecision::Always.audit_value(), "always");
    }

    #[test]
    fn approval_decision_whitelist_rejects_persistent_scope_without_matcher() {
        let available = super::available_approval_decisions(None);
        assert!(super::validate_approval_decision(ApprovalDecision::Allow, &available).is_ok());
        assert!(super::validate_approval_decision(ApprovalDecision::Deny, &available).is_ok());
        for decision in [
            ApprovalDecision::Turn,
            ApprovalDecision::Session,
            ApprovalDecision::Project,
            ApprovalDecision::Always,
        ] {
            let error = super::validate_approval_decision(
                decision,
                &super::available_approval_decisions(None),
            )
            .unwrap_err();
            assert!(error.starts_with("[approval_decision_unavailable]"));
        }
    }

    #[test]
    fn approval_decision_whitelist_tracks_available_identities() {
        let mut action = crate::permissions::normalize_tool_action(
            "claude-code",
            "session-1",
            "turn-1",
            "tool-1",
            "WebFetch",
            &serde_json::json!({"url":"https://example.com/docs"}),
            Some("D:/repo"),
        );
        assert_eq!(
            super::available_approval_decisions(Some(&action)),
            vec![
                crate::protocol::ApprovalDecisionOption::Allow,
                crate::protocol::ApprovalDecisionOption::Turn,
                crate::protocol::ApprovalDecisionOption::Session,
                crate::protocol::ApprovalDecisionOption::Project,
                crate::protocol::ApprovalDecisionOption::Always,
                crate::protocol::ApprovalDecisionOption::Deny,
            ]
        );

        action.turn_id.clear();
        assert!(!super::available_approval_decisions(Some(&action))
            .contains(&crate::protocol::ApprovalDecisionOption::Turn));
        action.session_id.clear();
        assert!(!super::available_approval_decisions(Some(&action))
            .contains(&crate::protocol::ApprovalDecisionOption::Session));
        action.cwd = None;
        assert!(!super::available_approval_decisions(Some(&action))
            .contains(&crate::protocol::ApprovalDecisionOption::Project));
        assert!(super::available_approval_decisions(Some(&action))
            .contains(&crate::protocol::ApprovalDecisionOption::Always));
    }

    #[test]
    fn failed_grant_commit_removes_the_prepared_hook_decision() {
        let root = std::env::temp_dir().join(format!(
            "helm-prepared-approval-rollback-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let state_path = root.join("state.json");
        write_approval_state(
            &state_path,
            &ApprovalState {
                decisions: HashMap::from([("tool-1".to_string(), "allow".to_string())]),
                denied_targets: Vec::new(),
                turn_mode: "build".to_string(),
            },
        )
        .unwrap();

        rollback_prepared_approval_state(&state_path, "tool-1").unwrap();
        let state: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&state_path).unwrap()).unwrap();
        assert!(state["decisions"].get("tool-1").is_none());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn once_approval_rule_is_bound_to_the_exact_tool_call() {
        let action = build_approval_action(
            "history-1",
            "turn-1",
            "tool-1",
            &PendingToolInfo {
                name: "Bash".to_string(),
                input: serde_json::json!({"command":"ls -la"}),
            },
            "D:/repo",
        );
        let rule = crate::permissions::build_once_rule_from_action(&action, 1_000);

        assert_eq!(rule.scope, crate::permissions::PermissionScope::Once);
        assert_eq!(rule.scope_binding.tool_call_id.as_deref(), Some("tool-1"));
        assert_eq!(rule.scope_binding.turn_id.as_deref(), Some("turn-1"));
        assert_eq!(rule.scope_binding.session_id.as_deref(), Some("history-1"));
        assert_eq!(rule.max_uses, Some(1));
        assert_eq!(rule.operation.as_deref(), Some("ls"));
    }

    #[test]
    fn runtime_claude_settings_registers_the_same_turn_permission_bridge() {
        let files = create_runtime_approval_hook_files().unwrap();
        let settings: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(files.settings_path).unwrap()).unwrap();
        assert!(settings["hooks"]["PreToolUse"].is_array());
        assert!(settings["hooks"]["PreToolUse"][0]["hooks"][0]["command"]
            .as_str()
            .is_some_and(|command| command.contains("--helm-runtime-hook")));
    }

    #[test]
    fn serialize_history_prompt_rebuilds_truncated_context() {
        use crate::protocol::Role;
        use crate::sessions::SessionMessage;

        // 无历史时原样透传
        assert_eq!(super::serialize_history_prompt(&[], "当前问题"), "当前问题");

        // 有历史时序列化成开场上下文（Codex 续聊与 Claude 回溯重建共用，P2-5）
        let history = vec![
            SessionMessage {
                role: Role::User,
                text: "第一轮提问".to_string(),
                ts: 1,
                reverted: false,
                turn_id: None,
                attachments: Vec::new(),
            },
            SessionMessage {
                role: Role::Assistant,
                text: "第一轮回复".to_string(),
                ts: 2,
                reverted: false,
                turn_id: None,
                attachments: Vec::new(),
            },
        ];
        let prompt = super::serialize_history_prompt(&history, "当前问题");
        assert!(prompt.starts_with("之前的对话历史："));
        assert!(prompt.contains("用户: 第一轮提问"));
        assert!(prompt.contains("助手: 第一轮回复"));
        assert!(prompt.ends_with("用户: 当前问题"));
    }

    #[test]
    fn codex_app_server_thread_plan_resumes_normally_and_restarts_after_rewind() {
        assert_eq!(
            super::codex_app_server_thread_plan(Some("thread-1"), false),
            super::CodexAppServerThreadPlan::Resume("thread-1".to_string())
        );
        assert_eq!(
            super::codex_app_server_thread_plan(Some("thread-1"), true),
            super::CodexAppServerThreadPlan::Start
        );
        assert_eq!(
            super::codex_app_server_thread_plan(None, false),
            super::CodexAppServerThreadPlan::Start
        );
        let history = vec![crate::sessions::SessionMessage {
            role: crate::protocol::Role::Assistant,
            text: "回溯后保留的回答".to_string(),
            ts: 1,
            reverted: false,
            turn_id: None,
            attachments: Vec::new(),
        }];
        assert_eq!(
            super::codex_app_server_prompt(false, &history, "当前问题"),
            "当前问题"
        );
        let rebuilt = super::codex_app_server_prompt(true, &history, "当前问题");
        assert!(rebuilt.contains("回溯后保留的回答"));
        assert!(rebuilt.contains("当前问题"));
    }

    #[test]
    fn codex_resume_fallback_only_accepts_explicit_missing_thread_errors() {
        assert!(super::is_codex_thread_missing_error(
            "thread thread-1 not found"
        ));
        assert!(super::is_codex_thread_missing_error(
            "No rollout found for thread id thread-1"
        ));
        assert!(!super::is_codex_thread_missing_error(
            "connection reset while resuming thread"
        ));
        assert!(!super::is_codex_thread_missing_error(
            "authentication failed"
        ));
    }

    #[test]
    fn codex_auth_home_lives_until_the_session_owned_arc_is_dropped() {
        let path = super::unique_temp_dir("helm-codex-home-lifetime-test");
        std::fs::create_dir_all(&path).expect("create test auth home");
        let owner = std::sync::Arc::new(std::sync::Mutex::new(Some(super::CodexAuthHome {
            path: path.clone(),
        })));
        let turn = owner.clone();

        let auth_path = |arc: &std::sync::Arc<std::sync::Mutex<Option<super::CodexAuthHome>>>| {
            arc.lock()
                .ok()
                .and_then(|guard| guard.as_ref().map(|home| home.path.clone()))
        };
        assert_eq!(auth_path(&owner), Some(path.clone()));
        assert_eq!(auth_path(&turn), Some(path.clone()));
        drop(turn);
        assert!(
            path.exists(),
            "dropping a turn clone must not delete session auth"
        );
        drop(owner);
        assert!(
            !path.exists(),
            "dropping the session owner deletes temp auth"
        );
    }

    #[test]
    fn codex_rewind_clears_native_thread_and_forces_one_history_rebuild() {
        let thread_id = std::sync::Arc::new(std::sync::Mutex::new(Some(
            "thread-before-rewind".to_string(),
        )));
        let force_history_rebuild = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let history = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let truncated = vec![crate::sessions::SessionMessage {
            role: crate::protocol::Role::Assistant,
            text: "保留到检查点的回复".to_string(),
            ts: 1,
            reverted: false,
            turn_id: None,
            attachments: Vec::new(),
        }];

        super::reset_codex_context_state(
            &thread_id,
            &force_history_rebuild,
            &history,
            truncated.clone(),
        )
        .expect("reset codex context");

        assert_eq!(*thread_id.lock().unwrap(), None);
        assert!(force_history_rebuild.load(std::sync::atomic::Ordering::Acquire));
        assert_eq!(*history.lock().unwrap(), truncated);
    }

    #[test]
    fn engine_bin_rejects_windows_shell_control_characters() {
        for bin in [
            "claude & calc",
            "codex | more",
            "claude < input",
            "codex > output",
            "claude ^& calc",
            "claude\r\ncalc",
            "claude %PATH%",
            "claude !VAR!",
            "\"claude\"",
        ] {
            assert!(
                validate_engine_bin(bin).is_err(),
                "应拒绝危险引擎路径：{bin}"
            );
        }

        validate_engine_bin(r"C:\Program Files (x86)\Claude\claude.cmd")
            .expect("合法 Windows 路径必须保留支持");
    }

    #[test]
    fn inherited_agent_environment_uses_allowlist_and_excludes_secrets() {
        let filtered = filter_inherited_agent_environment(vec![
            ("PATH".to_string(), "C:\\bin".to_string()),
            ("USERPROFILE".to_string(), "C:\\Users\\tester".to_string()),
            ("LANG".to_string(), "zh_CN.UTF-8".to_string()),
            ("OPENAI_API_KEY".to_string(), "must-not-leak".to_string()),
            (
                "ANTHROPIC_AUTH_TOKEN".to_string(),
                "must-not-leak".to_string(),
            ),
            (
                "AWS_SECRET_ACCESS_KEY".to_string(),
                "must-not-leak".to_string(),
            ),
            (
                "HELM_INTERNAL_TOKEN".to_string(),
                "must-not-leak".to_string(),
            ),
        ]);

        assert!(filtered.iter().any(|(key, _)| key == "PATH"));
        assert!(filtered.iter().any(|(key, _)| key == "USERPROFILE"));
        assert!(filtered.iter().any(|(key, _)| key == "LANG"));
        assert!(!filtered.iter().any(|(key, _)| key.contains("KEY")));
        assert!(!filtered.iter().any(|(key, _)| key.contains("TOKEN")));
    }

    #[test]
    fn turn_mode_parses_wire_values_and_defaults_to_build() {
        assert_eq!(TurnMode::parse(Some("plan")), TurnMode::Plan);
        assert_eq!(TurnMode::parse(Some("ask")), TurnMode::Ask);
        assert_eq!(TurnMode::parse(Some("build")), TurnMode::Build);
        assert_eq!(TurnMode::parse(Some("unknown")), TurnMode::Build);
        assert_eq!(TurnMode::parse(None), TurnMode::Build);
    }

    #[test]
    fn session_turn_lease_rejects_concurrent_send_before_ledger_commit() {
        let busy = AtomicBool::new(false);
        reserve_turn_flag(&busy).unwrap();
        assert!(reserve_turn_flag(&busy).is_err());
        busy.store(false, Ordering::Release);
        reserve_turn_flag(&busy).unwrap();
    }

    #[test]
    fn runtime_permission_profiles_map_to_the_declared_engine_contracts() {
        assert_eq!(
            claude_permission_mode(TurnMode::Build, PermissionProfile::Standard),
            "manual"
        );
        assert_eq!(
            claude_permission_mode(TurnMode::Build, PermissionProfile::Auto),
            "auto"
        );
        assert_eq!(
            claude_permission_mode(TurnMode::Build, PermissionProfile::FullAccess),
            "bypassPermissions"
        );
        assert_eq!(
            claude_permission_mode(TurnMode::Plan, PermissionProfile::FullAccess),
            "plan"
        );
        assert_eq!(
            claude_permission_mode_for_capability(
                TurnMode::Build,
                PermissionProfile::Auto,
                crate::capability_registry::CapabilitySupport::Degraded,
            ),
            "acceptEdits"
        );
        assert_eq!(
            claude_permission_mode_for_capability(
                TurnMode::Build,
                PermissionProfile::Auto,
                crate::capability_registry::CapabilitySupport::Unknown,
            ),
            "auto"
        );

        assert_eq!(
            codex_runtime_profile_policy(TurnMode::Build, PermissionProfile::Standard),
            ("workspace-write", false, CodexApprovalPolicy::Untrusted)
        );
        assert_eq!(
            codex_runtime_profile_policy(TurnMode::Build, PermissionProfile::Auto),
            ("workspace-write", true, CodexApprovalPolicy::OnRequest)
        );
        assert_eq!(
            codex_runtime_profile_policy(TurnMode::Build, PermissionProfile::FullAccess),
            ("danger-full-access", true, CodexApprovalPolicy::Never)
        );
        assert_eq!(
            codex_runtime_profile_policy(TurnMode::Ask, PermissionProfile::FullAccess),
            ("read-only", false, CodexApprovalPolicy::Untrusted)
        );
    }

    #[test]
    fn auto_fallback_retries_once_only_before_any_tool_execution() {
        use crate::protocol::ToolOutcomeKind;

        assert_eq!(
            auto_fallback_decision(
                PermissionProfile::Auto,
                Some(ToolOutcomeKind::AutoReviewUnavailable),
                false,
                false,
            ),
            AutoFallbackDecision::RetryCompatible
        );
        assert_eq!(
            auto_fallback_decision(
                PermissionProfile::Auto,
                Some(ToolOutcomeKind::AutoReviewParseError),
                true,
                false,
            ),
            AutoFallbackDecision::Fuse
        );
        assert_eq!(
            auto_fallback_decision(
                PermissionProfile::Auto,
                Some(ToolOutcomeKind::AutoReviewUnavailable),
                false,
                true,
            ),
            AutoFallbackDecision::Fuse
        );
        assert_eq!(
            auto_fallback_decision(
                PermissionProfile::Auto,
                Some(ToolOutcomeKind::AutoReviewBlocked),
                false,
                false,
            ),
            AutoFallbackDecision::None
        );
    }

    #[test]
    fn full_access_lease_is_bound_to_session_engine_cwd_and_app_instance() {
        let lease = full_access_lease("session-1", "codex", "D:/repo");
        assert!(lease_is_valid(&lease, "session-1", "codex", "d:/repo"));
        assert!(!lease_is_valid(&lease, "session-2", "codex", "D:/repo"));
        assert!(!lease_is_valid(
            &lease,
            "session-1",
            "claude-code",
            "D:/repo"
        ));
        assert!(!lease_is_valid(&lease, "session-1", "codex", "D:/other"));
    }

    #[tokio::test]
    async fn terminal_turn_ledger_survives_waiter_reads_for_notification_deduplication() {
        let terminals = Mutex::new(HashMap::from([("turn-1".to_string(), Ok(()))]));
        assert_eq!(
            terminal_turn_outcome(&terminals, "turn-1").await,
            Some(Ok(()))
        );
        assert_eq!(
            terminal_turn_outcome(&terminals, "turn-1").await,
            Some(Ok(()))
        );
    }

    #[tokio::test]
    async fn codex_interrupt_notifies_waiter_after_terminal_event() {
        let terminals = Arc::new(Mutex::new(HashMap::new()));
        let notify = Arc::new(Notify::new());
        let terminal_event_emitted = Arc::new(AtomicBool::new(false));
        let waiter = tokio::spawn({
            let terminals = terminals.clone();
            let notify = notify.clone();
            let terminal_event_emitted = terminal_event_emitted.clone();
            async move {
                loop {
                    let notified = notify.notified();
                    if terminal_turn_outcome(&terminals, "turn-1").await.is_some() {
                        assert!(terminal_event_emitted.load(Ordering::Acquire));
                        return;
                    }
                    notified.await;
                }
            }
        });

        tokio::task::yield_now().await;
        finish_codex_interrupt_terminal(&terminals, &notify, Some("turn-1".to_string()), || {
            terminal_event_emitted.store(true, Ordering::Release);
        })
        .await;

        tokio::time::timeout(Duration::from_secs(1), waiter)
            .await
            .expect("Codex Stop 必须唤醒当前 Turn 等待器")
            .expect("Codex Turn 等待任务不应崩溃");
    }

    #[test]
    fn codex_sandbox_for_mode_forces_read_only_on_plan_and_ask() {
        // 计划/询问强制只读（取更严值）；构建沿用设置映射，包括显式 full
        assert_eq!(
            codex_sandbox_for_mode("workspace-write", TurnMode::Plan).unwrap(),
            "read-only"
        );
        assert_eq!(
            codex_sandbox_for_mode("danger-full-access", TurnMode::Ask).unwrap(),
            "read-only"
        );
        assert_eq!(
            codex_sandbox_for_mode("workspace-write", TurnMode::Build).unwrap(),
            "workspace-write"
        );
        assert!(
            codex_sandbox_for_mode("danger-full-access", TurnMode::Build)
                .unwrap_err()
                .contains("unsupported")
        );
    }

    #[test]
    fn agent_environment_reflects_anonymous_analytics_preference() {
        let mut settings = AppSettings::default();
        settings.general.anonymous_analytics = false;
        let env = agent_environment_from_settings(&settings);
        assert!(env.contains(&("DO_NOT_TRACK".to_string(), "1".to_string())));
        assert!(env.contains(&("HELM_ANONYMOUS_ANALYTICS".to_string(), "0".to_string())));

        settings.general.anonymous_analytics = true;
        let env = agent_environment_from_settings(&settings);
        assert!(!env.iter().any(|(key, _)| key == "DO_NOT_TRACK"));
        assert!(env.contains(&("HELM_ANONYMOUS_ANALYTICS".to_string(), "1".to_string())));
    }

    #[test]
    fn extracts_file_target_from_write_tools() {
        let input = json!({ "file_path": "secret.txt", "content": "confidential" });
        assert_eq!(
            extract_tool_target("Write", &input),
            Some("secret.txt".to_string())
        );
        assert_eq!(
            extract_tool_target("Edit", &input),
            Some("secret.txt".to_string())
        );
    }

    #[test]
    fn extracts_target_from_dangerous_bash_redirection() {
        let input = json!({ "command": "echo \"confidential\" > secret.txt" });
        assert_eq!(
            extract_tool_target("Bash", &input),
            Some("secret.txt".to_string())
        );
    }

    #[test]
    fn extracts_target_from_dangerous_bash_delete_with_flags() {
        let input = json!({ "command": "rm -f secret.txt" });
        assert_eq!(
            extract_tool_target("Bash", &input),
            Some("secret.txt".to_string())
        );
    }

    #[test]
    fn extracts_target_from_append_redirection_and_notebook_edit() {
        // 变更-07：`>>` 追加重定向不再把第二个 > 捕进路径；NotebookEdit 认 notebook_path
        let append = json!({ "command": "echo hi >> out.txt" });
        assert_eq!(
            extract_tool_target("Bash", &append),
            Some("out.txt".to_string())
        );
        let notebook = json!({ "notebook_path": "analysis.ipynb" });
        assert_eq!(
            extract_tool_target("NotebookEdit", &notebook),
            Some("analysis.ipynb".to_string())
        );
    }

    #[test]
    fn merges_consecutive_deltas_and_flushes_on_other_events() {
        // 变更-09：连续同类 delta 并入缓冲；异类事件不并入（调用方先冲刷再处理）
        let mut pending = Some(AgentEvent::MessageDelta {
            session_id: "s1".to_string(),
            role: Role::Assistant,
            text: "你".to_string(),
        });
        assert!(merge_pending_delta(
            &mut pending,
            &AgentEvent::MessageDelta {
                session_id: "s1".to_string(),
                role: Role::Assistant,
                text: "好".to_string(),
            }
        ));
        match &pending {
            Some(AgentEvent::MessageDelta { text, .. }) => assert_eq!(text, "你好"),
            other => panic!("缓冲应为合并后的 MessageDelta：{other:?}"),
        }

        // 不同会话不合并
        assert!(!merge_pending_delta(
            &mut pending,
            &AgentEvent::MessageDelta {
                session_id: "s2".to_string(),
                role: Role::Assistant,
                text: "x".to_string(),
            }
        ));
        // thinking 与 message 互不合并
        assert!(!merge_pending_delta(
            &mut pending,
            &AgentEvent::ThinkingDelta {
                session_id: "s1".to_string(),
                text: "思考".to_string(),
            }
        ));
        // 非 delta 事件不合并
        assert!(!merge_pending_delta(
            &mut pending,
            &AgentEvent::TurnComplete {
                session_id: "s1".to_string(),
                stop_reason: StopReason::End,
            }
        ));
    }

    #[test]
    fn checkpoint_target_resolves_write_paths_and_ignores_read_tools() {
        let cwd = std::env::temp_dir().join("helm-checkpoint-cwd");
        let _ = fs::remove_dir_all(&cwd);
        fs::create_dir_all(cwd.join("src")).unwrap();
        let input = json!({ "file_path": "src/main.rs" });

        assert_eq!(
            checkpoint_target_path("Write", &input, &cwd),
            Some(cwd.canonicalize().unwrap().join("src/main.rs"))
        );
        assert_eq!(checkpoint_target_path("Read", &input, &cwd), None);
        assert_eq!(
            checkpoint_target_path("Bash", &json!({ "command": "which wm 2>/dev/null" }), &cwd),
            None
        );
        assert_eq!(
            checkpoint_target_path("Bash", &json!({ "command": "echo x > file" }), &cwd),
            None
        );
        assert_eq!(
            checkpoint_target_path("Write", &json!({ "file_path": "../outside.txt" }), &cwd),
            None
        );
        assert_eq!(
            checkpoint_target_path("Write", &json!({ "file_path": "NUL" }), &cwd),
            None
        );
        let _ = fs::remove_dir_all(&cwd);
    }

    #[test]
    fn create_auto_checkpoint_for_tool_saves_snapshot_and_event() {
        let root =
            std::env::temp_dir().join(format!("helm-auto-checkpoint-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let cwd = root.join("workspace");
        fs::create_dir_all(cwd.join("src")).unwrap();
        let file_path = cwd.join("src/main.rs");
        fs::write(&file_path, "old content").unwrap();

        let history_store = SessionHistoryStore::new(root.join("sessions.sqlite"));
        history_store
            .create_session(NewSessionRecord {
                id: "history-1".to_string(),
                engine: EngineId::ClaudeCode,
                model: "claude-sonnet".to_string(),
                cwd: cwd.to_string_lossy().to_string(),
                created_at: 1,
            })
            .unwrap();

        let event = create_auto_checkpoint_for_tool(
            &history_store,
            &root.join("snapshots"),
            "history-1",
            "cli-1",
            &cwd,
            "tool-1",
            "Edit",
            &json!({ "file_path": "src/main.rs" }),
            "turn-1",
        )
        .unwrap()
        .expect("Edit should create checkpoint");

        let AgentEvent::Checkpoint { id, session_id, .. } = event else {
            panic!("expected checkpoint event");
        };
        assert_eq!(session_id, "cli-1");

        let checkpoint = history_store.get_checkpoint(&id).unwrap().unwrap();
        assert_eq!(checkpoint.snapshot_ref, id);
        assert_eq!(checkpoint.turn_id.as_deref(), Some("turn-1"));
        assert!(checkpoint.restorable);
        assert_eq!(checkpoint.file_count, 1);

        fs::write(&file_path, "new content").unwrap();
        let snapshot_store = crate::snapshots::SnapshotStore::new(root.join("snapshots"));
        let snapshot = snapshot_store.load(&checkpoint.snapshot_ref).unwrap();
        snapshot_store.restore_files(&snapshot).unwrap();
        assert_eq!(fs::read_to_string(&file_path).unwrap(), "old content");

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn prompt_with_attachments_lists_paths_for_cli_context() {
        let prompt = prompt_with_attachments(
            "请审查这些内容",
            &["D:\\work\\app.ts".to_string(), "D:\\work\\docs".to_string()],
        );

        assert!(prompt.contains("请审查这些内容"));
        assert!(prompt.contains("已挂载上下文"));
        assert!(prompt.contains("D:\\work\\app.ts"));
        assert!(prompt.contains("D:\\work\\docs"));
    }

    #[test]
    fn claude_session_context_uses_native_add_dir_arguments() {
        let mut command = tokio::process::Command::new("claude");
        super::apply_claude_session_context_args(
            &mut command,
            "D:/repo",
            &[
                crate::turn_start::FrozenSessionContext {
                    id: "file".into(),
                    kind: "file".into(),
                    canonical_path: "D:/repo/docs/guide.md".into(),
                    canonical_path_digest: "sha256:file".into(),
                    identity_digest: "sha256:file-id".into(),
                },
                crate::turn_start::FrozenSessionContext {
                    id: "directory".into(),
                    kind: "directory".into(),
                    canonical_path: "D:/repo/examples".into(),
                    canonical_path_digest: "sha256:directory".into(),
                    identity_digest: "sha256:directory-id".into(),
                },
            ],
        );
        let args = command
            .as_std()
            .get_args()
            .map(|arg| arg.to_string_lossy().to_string())
            .collect::<Vec<_>>();
        assert_eq!(
            args,
            vec!["--add-dir", "D:/repo/docs", "--add-dir", "D:/repo/examples"]
        );
    }

    #[test]
    fn claude_model_only_command_disables_every_extension_surface() {
        let cwd = std::env::temp_dir();
        let command = super::build_claude_model_only_command(
            "claude",
            "claude-fixture",
            &[],
            &cwd,
            "只生成标题",
            crate::reasoning::ReasoningEffort::Auto,
        )
        .unwrap();
        let args = command
            .as_std()
            .get_args()
            .map(|arg| arg.to_string_lossy().to_string())
            .collect::<Vec<_>>();
        for pair in [
            ["--tools", ""],
            ["--mcp-config", "{}"],
            ["--permission-mode", "plan"],
            ["--setting-sources", ""],
        ] {
            assert!(args.windows(2).any(|args| args == pair));
        }
        for flag in [
            "--print",
            "--disable-slash-commands",
            "--strict-mcp-config",
            "--no-session-persistence",
            "--safe-mode",
        ] {
            assert!(args.iter().any(|arg| arg == flag), "缺少 {flag}");
        }
    }

    #[test]
    fn claude_model_only_parser_records_usage_and_rejects_runtime_events() {
        let output = super::parse_claude_model_only_output(
            r#"{"result":"标题\n摘要","model":"claude-fixture","total_cost_usd":0.02,"permission_denials":[],"usage":{"input_tokens":11,"cache_read_input_tokens":3,"cache_creation_input_tokens":2,"output_tokens":5,"service_tier":"standard"}}"#.as_bytes(),
        )
        .unwrap();
        assert_eq!(output.text, "标题\n摘要");
        assert_eq!(output.input_tokens, 11);
        assert_eq!(output.cached_input_tokens, 3);
        assert_eq!(output.cache_write_input_tokens, 2);
        assert_eq!(output.output_tokens, 5);
        assert_eq!(output.reported_cost_usd, Some(0.02));
        assert_eq!(output.observed_model_id.as_deref(), Some("claude-fixture"));

        for forbidden in [
            r#"{"result":"不应接受","permission_denials":[{"tool_name":"Bash"}]}"#.as_bytes(),
            r#"{"result":"不应接受","messages":[{"type":"tool_use","name":"Read"}]}"#.as_bytes(),
            r#"{"result":"不应接受","approval_request":{"id":"approval-1"}}"#.as_bytes(),
        ] {
            let error = super::parse_claude_model_only_output(forbidden).unwrap_err();
            assert!(error.starts_with("[operation_tool_not_allowed]"));
        }
    }

    #[test]
    fn parses_codex_jsonl_into_unified_process_events() {
        let lines = [
            r#"{"type":"item.completed","item":{"id":"rs_1","type":"reasoning","summary":[{"text":"先检查文件。"}]}}"#,
            r#"{"type":"item.started","item":{"id":"call_1","type":"tool_call","name":"Bash","arguments":{"command":"git diff"}}}"#,
            r#"{"type":"item.completed","item":{"id":"out_1","type":"tool_call_output","call_id":"call_1","output":"diff --git a/demo.ts b/demo.ts\n--- a/demo.ts\n+++ b/demo.ts\n@@ -1 +1 @@\n-old\n+new\n"}}"#,
            r#"{"type":"plan_update","steps":[{"step":"检查改动","status":"completed"},{"step":"运行测试","status":"in_progress"}]}"#,
            r#"{"type":"item.completed","item":{"id":"msg_1","type":"agent_message","text":"完成"}}"#,
            r#"{"type":"turn.completed","usage":{"input_tokens":10,"output_tokens":5},"total_cost_usd":0.001}"#,
        ];

        let events = lines
            .iter()
            .flat_map(|line| parse_codex_line("codex-s1", line))
            .collect::<Vec<_>>();

        assert!(matches!(
            events.first(),
            Some(AgentEvent::ThinkingComplete { text, .. }) if text == "先检查文件。"
        ));
        assert!(events.iter().any(|event| matches!(
            event,
            AgentEvent::ToolCall { id, name, .. } if id == "call_1" && name == "Bash"
        )));
        assert!(events.iter().any(|event| matches!(
            event,
            AgentEvent::ToolResult { id, diff: Some(diff), .. }
                if id == "call_1" && diff.path == "demo.ts"
        )));
        assert!(events.iter().any(|event| matches!(
            event,
            AgentEvent::PlanUpdate { steps, .. }
                if steps.len() == 2 && steps[0].text == "检查改动"
        )));
        assert!(events.iter().any(|event| matches!(
            event,
            AgentEvent::MessageComplete { text, .. } if text == "完成"
        )));
        assert!(matches!(
            events.last(),
            Some(AgentEvent::TurnComplete {
                stop_reason: StopReason::End,
                ..
            })
        ));
    }

    #[test]
    fn builds_codex_provider_overrides_from_openai_binding_env() {
        let args = codex_provider_config_args(&[
            (
                "OPENAI_BASE_URL".to_string(),
                "https://api.example.com/v1".to_string(),
            ),
            ("OPENAI_API_KEY".to_string(), "secret".to_string()),
            ("HELM_CODEX_WIRE_API".to_string(), "chat".to_string()),
        ]);

        assert_eq!(
            args,
            vec![
                "model_provider=helm".to_string(),
                "model_providers.helm.name=helm".to_string(),
                "model_providers.helm.base_url=https://api.example.com/v1".to_string(),
                "model_providers.helm.wire_api=chat".to_string(),
                "model_providers.helm.requires_openai_auth=true".to_string(),
            ]
        );
    }

    #[test]
    fn codex_subscription_overrides_stale_global_custom_provider_routing() {
        assert_eq!(
            codex_provider_config_args(&[]),
            vec!["model_provider=openai".to_string()]
        );
    }

    #[test]
    fn background_task_spawn_does_not_require_current_tokio_reactor() {
        let (tx, rx) = std::sync::mpsc::channel();

        let result = std::panic::catch_unwind(move || {
            spawn_agent_task(async move {
                let _ = tx.send(());
            });
        });

        assert!(result.is_ok());
        assert!(rx.recv_timeout(std::time::Duration::from_secs(2)).is_ok());
    }

    #[test]
    fn codex_auth_home_uses_launch_env_api_key_and_cleans_up() {
        let home = create_codex_auth_home(
            &[("OPENAI_API_KEY".to_string(), "helm-runtime-key".to_string())],
            &[],
        )
        .unwrap()
        .unwrap();
        let path = home.path.clone();
        let auth_text = std::fs::read_to_string(path.join("auth.json")).unwrap();
        let auth: serde_json::Value = serde_json::from_str(&auth_text).unwrap();

        assert_eq!(auth["OPENAI_API_KEY"], "helm-runtime-key");
        drop(home);
        assert!(!path.exists());
    }

    #[test]
    fn codex_auth_home_inherits_config_prompts_and_complete_skills_from_source() {
        let source =
            std::env::temp_dir().join(format!("helm-test-codex-source-{}", std::process::id()));
        let _ = fs::remove_dir_all(&source);
        fs::create_dir_all(source.join("prompts")).unwrap();
        fs::create_dir_all(source.join("skills/demo/references")).unwrap();
        fs::create_dir_all(source.join("skills/demo/scripts")).unwrap();
        fs::create_dir_all(source.join("skills/demo/assets")).unwrap();
        fs::write(
            source.join("config.toml"),
            "[mcp_servers.demo]\ncommand = \"demo\"\n",
        )
        .unwrap();
        fs::write(source.join("prompts").join("demo.md"), "demo prompt").unwrap();
        fs::write(source.join("prompts").join("notes.txt"), "非 md 不继承").unwrap();
        fs::write(source.join("skills/demo/SKILL.md"), "# Demo skill").unwrap();
        fs::write(source.join("skills/demo/references/guide.md"), "guide").unwrap();
        fs::write(
            source.join("skills/demo/scripts/run.ps1"),
            "Write-Output demo",
        )
        .unwrap();
        fs::write(source.join("skills/demo/assets/example.txt"), "asset").unwrap();
        fs::write(
            source.join("auth.json"),
            r#"{"OPENAI_API_KEY":"stale-key"}"#,
        )
        .unwrap();

        let home = create_codex_auth_home_with_source(
            &[("OPENAI_API_KEY".to_string(), "helm-runtime-key".to_string())],
            Some(&source),
            &[],
        )
        .unwrap()
        .unwrap();

        let config = fs::read_to_string(home.path.join("config.toml")).unwrap();
        assert!(config.contains("mcp_servers.demo"));
        assert_eq!(
            fs::read_to_string(home.path.join("prompts").join("demo.md")).unwrap(),
            "demo prompt"
        );
        assert!(!home.path.join("prompts").join("notes.txt").exists());
        assert_eq!(
            fs::read_to_string(home.path.join("skills/demo/SKILL.md")).unwrap(),
            "# Demo skill"
        );
        assert!(home.path.join("skills/demo/references/guide.md").is_file());
        assert!(home.path.join("skills/demo/scripts/run.ps1").is_file());
        assert!(home.path.join("skills/demo/assets/example.txt").is_file());
        let auth: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(home.path.join("auth.json")).unwrap())
                .unwrap();
        assert_eq!(auth["OPENAI_API_KEY"], "helm-runtime-key");

        let path = home.path.clone();
        drop(home);
        assert!(!path.exists());
        let _ = fs::remove_dir_all(&source);
    }

    #[test]
    fn codex_auth_home_without_source_dir_still_writes_auth() {
        let home = create_codex_auth_home_with_source(
            &[("OPENAI_API_KEY".to_string(), "helm-runtime-key".to_string())],
            None,
            &[],
        )
        .unwrap()
        .unwrap();
        assert!(home.path.join("auth.json").is_file());
        assert!(!home.path.join("config.toml").exists());
    }

    #[test]
    fn codex_auth_home_filters_disabled_mcp_servers() {
        // 变更-11：会话级停用的 MCP 在继承的 config.toml 里被剔除，其余配置保留
        let source =
            std::env::temp_dir().join(format!("helm-test-codex-mcp-{}", std::process::id()));
        let _ = fs::remove_dir_all(&source);
        fs::create_dir_all(&source).unwrap();
        fs::write(
            source.join("config.toml"),
            "model = \"gpt-5-codex\"\n\n[mcp_servers.github]\ncommand = \"gh-mcp\"\n\n[mcp_servers.postgres]\ncommand = \"pg-mcp\"\n",
        )
        .unwrap();

        let home = create_codex_auth_home_with_source(
            &[("OPENAI_API_KEY".to_string(), "helm-runtime-key".to_string())],
            Some(&source),
            &["postgres".to_string()],
        )
        .unwrap()
        .unwrap();

        let config = fs::read_to_string(home.path.join("config.toml")).unwrap();
        assert!(config.contains("github"), "未停用的服务器应保留");
        assert!(!config.contains("postgres"), "停用的服务器应被剔除");
        assert!(config.contains("gpt-5-codex"), "其余配置项应原样保留");
        let _ = fs::remove_dir_all(&source);
    }

    #[test]
    fn claude_mcp_config_file_filters_disabled_servers() {
        // 变更-11：--strict-mcp-config 的过滤配置来自真实 settings.json 减去停用项。
        // 不构造真实 ~/.claude（读取失败时回退空集合），这里验证过滤与写盘契约。
        let out_dir =
            std::env::temp_dir().join(format!("helm-test-mcp-out-{}", std::process::id()));
        let _ = fs::remove_dir_all(&out_dir);
        fs::create_dir_all(&out_dir).unwrap();
        let path = super::build_claude_mcp_config_file(&["github".to_string()], &out_dir).unwrap();
        let config: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert!(
            config.get("mcpServers").is_some(),
            "输出必须含 mcpServers 键"
        );
        assert!(
            config["mcpServers"].get("github").is_none(),
            "停用的服务器不得出现在过滤后配置中"
        );
        let _ = fs::remove_dir_all(&out_dir);
    }

    #[test]
    fn claude_runtime_managed_api_binding_does_not_load_external_setting_sources() {
        let mut api_command = super::build_command("claude");
        super::apply_claude_setting_source_policy(&mut api_command, false);
        let api_args = api_command
            .as_std()
            .get_args()
            .map(|value| value.to_string_lossy().to_string())
            .collect::<Vec<_>>();
        assert!(api_args.ends_with(&["--setting-sources".to_string(), "".to_string()]));

        let mut subscription_command = super::build_command("claude");
        super::apply_claude_setting_source_policy(&mut subscription_command, true);
        assert_eq!(subscription_command.as_std().get_args().count(), 0);
    }

    #[cfg(windows)]
    #[test]
    fn claude_command_launches_the_resolved_native_binary_without_cmd_wrapper() {
        let root = std::env::temp_dir().join(format!(
            "helm-claude-native-command-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        ));
        let native = root
            .join("node_modules")
            .join("@anthropic-ai")
            .join("claude-code")
            .join("bin")
            .join("claude.exe");
        std::fs::create_dir_all(native.parent().unwrap()).unwrap();
        std::fs::write(&native, b"native").unwrap();
        let wrapper = root.join("claude.cmd");
        std::fs::write(&wrapper, "@echo off\r\n").unwrap();

        let command = super::build_command(wrapper.to_str().unwrap());
        assert_eq!(
            std::path::PathBuf::from(command.as_std().get_program()),
            native.canonicalize().unwrap()
        );

        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn codex_command_launches_the_resolved_native_binary_without_cmd_wrapper() {
        let root = std::env::temp_dir().join(format!(
            "helm-codex-native-command-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        ));
        let native = root
            .join("node_modules/@openai/codex/node_modules/@openai/codex-win32-x64")
            .join("vendor/x86_64-pc-windows-msvc/bin/codex.exe");
        std::fs::create_dir_all(native.parent().unwrap()).unwrap();
        std::fs::write(&native, b"native").unwrap();
        let wrapper = root.join("codex.cmd");
        std::fs::write(&wrapper, "@echo off\r\n").unwrap();

        let command = super::build_codex_command(wrapper.to_str().unwrap());
        assert_eq!(
            std::path::PathBuf::from(command.as_std().get_program()),
            native.canonicalize().unwrap()
        );

        std::fs::remove_dir_all(root).unwrap();
    }
}

/// 从错误文案（含透传的 CLI stderr）推断错误分类，供前端渲染修复动作。
/// 顺序敏感：具体原因（未安装/未登录/目录无效）要排在笼统的"进程异常退出"之前。
pub(crate) fn classify_error(message: &str) -> Option<String> {
    let lower = message.to_lowercase();
    let kind = if lower.contains("there's an issue with the selected model")
        || (lower.contains("model") && lower.contains("may not exist or you may not have access"))
        || lower.contains("模型不存在")
        || lower.contains("模型不可用")
        || lower.contains("模型授权")
    {
        "model_unavailable"
    } else if lower.contains("未设置工作目录") || lower.contains("工作目录不存在") {
        "cwd_invalid"
    } else if lower.contains("还没有配置生效绑定") {
        "no_binding"
    } else if lower.contains("is not recognized")
        || lower.contains("command not found")
        || lower.contains("not found in path")
        || lower.contains("no such file")
        || lower.contains("无法启动 claude 进程")
        || lower.contains("无法启动 codex 进程")
    {
        "not_installed"
    } else if lower.contains("401")
        || lower.contains("unauthorized")
        || lower.contains("invalid api key")
        || lower.contains("authentication")
        || lower.contains("please run /login")
        || lower.contains("钥匙串中没有找到")
        || lower.contains("服务商认证失败")
    {
        "auth_missing"
    } else if lower.contains("[codex_probe_tool_surface_")
        || lower.contains("unknown option")
        || lower.contains("unrecognized option")
        || lower.contains("unexpected argument")
        || lower.contains("版本不兼容")
    {
        "version_incompatible"
    } else if lower.contains("没有任何输出") || lower.contains("超时") {
        "timeout"
    } else if lower.contains("econnrefused")
        || lower.contains("getaddrinfo")
        || lower.contains("fetch failed")
        || lower.contains("network")
        || lower.contains("connection refused")
        || lower.contains("连接服务商失败")
    {
        "network"
    } else if lower.contains("进程异常退出") {
        "process_crash"
    } else {
        return None;
    };
    Some(kind.to_string())
}

async fn emit_error(runtime: &SessionRuntime, message: String, recoverable: bool) {
    let session_id = runtime.session_id.lock().await.clone();
    let kind = classify_error(&message);
    emit_agent_event(
        &runtime.app,
        &runtime.history_session_id,
        &AgentEvent::Error {
            session_id,
            message,
            recoverable,
            kind,
        },
    );
}

async fn emit_interrupted(runtime: &SessionRuntime) {
    if let Some(sid) = runtime.session_id.lock().await.clone() {
        emit_agent_event(
            &runtime.app,
            &runtime.history_session_id,
            &AgentEvent::TurnComplete {
                session_id: sid,
                stop_reason: StopReason::Interrupted,
            },
        );
    }
}

async fn emit_denied_turn(runtime: &SessionRuntime, request_id: String) {
    if let Some(sid) = runtime.session_id.lock().await.clone() {
        emit_agent_event(
            &runtime.app,
            &runtime.history_session_id,
            &AgentEvent::ToolResult {
                session_id: sid.clone(),
                id: request_id,
                status: ToolStatus::Error,
                output: Some("用户已拒绝，操作已终止。".to_string()),
                diff: None,
                outcome: Some(crate::protocol::ToolOutcomeKind::RuntimeDenied),
                started: Some(false),
                has_output: Some(true),
                retryable: Some(false),
                denial_source: Some(crate::protocol::ToolDenialSource::Runtime),
                native_denial_code: Some("user_denied".to_string()),
            },
        );
        emit_agent_event(
            &runtime.app,
            &runtime.history_session_id,
            &AgentEvent::TurnComplete {
                session_id: sid,
                stop_reason: StopReason::Interrupted,
            },
        );
    }
}

#[derive(Debug, PartialEq, Eq)]
enum ClaudeExitDisposition {
    Return,
    ProcessError,
    EmitCandidate,
    ApprovalDeferred,
    MissingResult,
}

fn should_process_claude_event(
    session_started: bool,
    current_approved_tool_result: bool,
    event: &AgentEvent,
) -> bool {
    session_started
        || (current_approved_tool_result && matches!(event, AgentEvent::ToolResult { .. }))
        || matches!(
            event,
            AgentEvent::SessionStarted { .. } | AgentEvent::Error { .. }
        )
}

fn claude_exit_disposition(
    interrupted: bool,
    auto_fallback_requested: bool,
    parser_error_terminal: bool,
    exit_succeeded: bool,
    has_terminal_candidate: bool,
    saw_approval: bool,
) -> ClaudeExitDisposition {
    if interrupted || auto_fallback_requested || parser_error_terminal {
        ClaudeExitDisposition::Return
    } else if !exit_succeeded && has_terminal_candidate {
        // CLI 以非零码退出但已产出 result（如 API 403 错误）：优先 emit 候选事件，
        // 让用户看到真实 API 错误，而非泛化 "进程异常退出"。
        ClaudeExitDisposition::EmitCandidate
    } else if !exit_succeeded {
        ClaudeExitDisposition::ProcessError
    } else if saw_approval {
        ClaudeExitDisposition::ApprovalDeferred
    } else if has_terminal_candidate {
        ClaudeExitDisposition::EmitCandidate
    } else {
        ClaudeExitDisposition::MissingResult
    }
}

fn build_approval_action(
    history_session_id: &str,
    turn_id: &str,
    tool_call_id: &str,
    tool: &PendingToolInfo,
    cwd: &str,
) -> crate::permissions::ActionDescriptor {
    crate::permissions::normalize_tool_action(
        "claude-code",
        history_session_id,
        turn_id,
        tool_call_id,
        &tool.name,
        &tool.input,
        Some(cwd),
    )
}

async fn record_approval(
    runtime: &SessionRuntime,
    request_id: &str,
    decision: ApprovalDecision,
) -> Result<PreparedApproval, String> {
    record_approval_state(
        &runtime.state_path,
        &runtime.pending_tools,
        request_id,
        decision,
    )
    .await
}

async fn record_approval_state(
    state_path: &Path,
    pending_tools: &Mutex<HashMap<String, PendingToolInfo>>,
    request_id: &str,
    decision: ApprovalDecision,
) -> Result<PreparedApproval, String> {
    let pending_info = pending_tools.lock().await.get(request_id).cloned();
    if !matches!(decision, ApprovalDecision::Deny) && pending_info.is_none() {
        return Err(format!("找不到待审批工具信息：{request_id}"));
    }

    let mut state = read_approval_state(state_path);
    let hook_decision = decision.hook_decision().to_string();
    state
        .decisions
        .insert(request_id.to_string(), hook_decision);

    // 拒绝时记录被拒绝的目标，防止换工具重试
    if matches!(decision, ApprovalDecision::Deny) {
        if let Some(info) = &pending_info {
            if let Some(target) = extract_tool_target(&info.name, &info.input) {
                if !state.denied_targets.iter().any(|item| item == &target) {
                    state.denied_targets.push(target);
                }
            }
        }
    }

    write_approval_state(state_path, &state)?;
    Ok(PreparedApproval {
        pending_tool: pending_info,
    })
}

fn rollback_prepared_approval_state(state_path: &Path, request_id: &str) -> Result<(), String> {
    let mut state = read_approval_state(state_path);
    state.decisions.remove(request_id);
    write_approval_state(state_path, &state)
}

async fn commit_approval_grants(
    runtime: &SessionRuntime,
    request_id: &str,
    decision: ApprovalDecision,
    prepared: &PreparedApproval,
) -> Result<CommittedApprovalGrants, String> {
    let history_store = runtime
        .app
        .try_state::<SessionHistoryStore>()
        .ok_or_else(|| "会话历史存储不可用，无法提交审批授权".to_string())?;
    let mut grants = CommittedApprovalGrants::default();
    if !matches!(decision, ApprovalDecision::Deny) {
        let tool = prepared
            .pending_tool
            .as_ref()
            .ok_or_else(|| format!("找不到待审批工具信息：{request_id}"))?;
        let turn_id = runtime.current_turn_id.lock().await.clone();
        let action = build_approval_action(
            &runtime.history_session_id,
            &turn_id,
            request_id,
            tool,
            &runtime.cwd,
        );
        let created_at = now_millis();
        let rule = match decision {
            ApprovalDecision::Allow => {
                crate::permissions::build_once_rule_from_action(&action, created_at)
            }
            ApprovalDecision::Turn => {
                crate::permissions::build_turn_rule_from_action(&action, created_at)
            }
            ApprovalDecision::Session => {
                crate::permissions::build_session_rule_from_action(&action, created_at)
            }
            ApprovalDecision::Project => {
                crate::permissions::build_project_rule_from_action(&action, created_at)?
            }
            ApprovalDecision::Always => {
                crate::permissions::build_always_rule_from_action(&action, created_at)
            }
            ApprovalDecision::Deny => unreachable!(),
        };
        let existed = history_store
            .list_permission_rules()?
            .iter()
            .any(|existing| existing.id == rule.id);
        if matches!(decision, ApprovalDecision::Allow) {
            history_store.save_consumed_permission_rule(&rule)?;
        } else {
            history_store.save_permission_rule(&rule)?;
        }
        if matches!(
            decision,
            ApprovalDecision::Project | ApprovalDecision::Always
        ) {
            if let Err(error) = history_store.save_runtime_grant_for_action(&rule, &action) {
                if !existed {
                    let _ = history_store.remove_permission_rule(&rule.id);
                }
                return Err(format!("无法保存 Runtime 永久授权：{error}"));
            }
        }
        if !existed {
            grants.rule_ids.push(rule.id);
        }
    }
    Ok(grants)
}

fn rollback_approval_grants(
    runtime: &SessionRuntime,
    grants: CommittedApprovalGrants,
) -> Result<(), String> {
    let history_store = runtime
        .app
        .try_state::<SessionHistoryStore>()
        .ok_or_else(|| "会话历史存储不可用，无法回滚审批授权".to_string())?;
    let mut errors = Vec::new();
    for rule_id in grants.rule_ids {
        if let Err(error) = history_store.remove_permission_rule(&rule_id) {
            errors.push(error);
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("；"))
    }
}

async fn wait_until_idle_and_begin(
    busy: &AtomicBool,
    idle_notify: &Notify,
    interrupted: &AtomicBool,
) -> Result<(), String> {
    loop {
        let notified = idle_notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if interrupted.load(Ordering::Acquire) {
            return Err("审批等待期间会话已中断，未启动恢复轮".to_string());
        }
        if busy
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            if interrupted.load(Ordering::Acquire) {
                busy.store(false, Ordering::Release);
                idle_notify.notify_waiters();
                return Err("审批等待期间会话已中断，未启动恢复轮".to_string());
            }
            return Ok(());
        }
        notified.as_mut().await;
    }
}

async fn run_serialized_approval<T, F>(lock: &Mutex<()>, operation: F) -> T
where
    F: Future<Output = T>,
{
    let _guard = lock.lock().await;
    operation.await
}

struct TurnBusyGuard {
    runtime: Arc<SessionRuntime>,
}

impl Drop for TurnBusyGuard {
    fn drop(&mut self) {
        self.runtime.busy.store(false, Ordering::Release);
        self.runtime.idle_notify.notify_waiters();
    }
}

async fn run_claude_turn(
    runtime: Arc<SessionRuntime>,
    prompt: Option<String>,
    attachments: Vec<String>,
    resume: bool,
    mut start_ack: Option<oneshot::Sender<Result<(), String>>>,
) {
    let _busy_guard = TurnBusyGuard {
        runtime: runtime.clone(),
    };

    // 同一个 Claude session 不能并发跑多个 `claude -p`。审批恢复期间如果又发送新消息，
    // 必须等恢复轮次完整结束，否则 stdout 事件会交叉，UI 状态会卡在 working。
    let _turn_guard = runtime.turn_lock.lock().await;

    {
        let running = runtime.running_pid.lock().await;
        if running.is_some() {
            let error = "已有轮次正在运行，请先等待或停止当前任务".to_string();
            if let Some(ack) = start_ack.take() {
                let _ = ack.send(Err(error.clone()));
            }
            emit_error(&runtime, error, true).await;
            return;
        }
    }

    let resume_id = runtime.session_id.lock().await.clone();
    if resume && resume_id.is_none() {
        let error = "无法继续审批：Claude sessionId 尚未建立".to_string();
        if let Some(ack) = start_ack.take() {
            let _ = ack.send(Err(error.clone()));
        }
        emit_error(&runtime, error, false).await;
        return;
    }

    // 工作目录守卫：绝不静默继承 Helm 进程自身目录（Agent 会在错误的地方读写文件）。
    if let Err(message) = validate_cwd(&runtime.cwd) {
        if let Some(ack) = start_ack.take() {
            let _ = ack.send(Err(message.clone()));
        }
        emit_error(&runtime, message, false).await;
        return;
    }
    if let Err(message) = validate_engine_bin(&runtime.bin) {
        if let Some(ack) = start_ack.take() {
            let _ = ack.send(Err(message.clone()));
        }
        emit_error(&runtime, message, false).await;
        return;
    }

    // 本轮会话模式（变更-04）：Send 时已写入 runtime；审批恢复轮沿用发起轮的值
    let mode = *runtime.turn_mode.lock().await;
    let reasoning_effort = *runtime.reasoning_effort.lock().await;
    let configured_profile = *runtime.permission_profile.lock().await;
    let auto_support = runtime
        .capability_snapshot
        .lock()
        .await
        .capabilities
        .auto_approval
        .support;
    let current_turn_id = runtime.current_turn_id.lock().await.clone();

    let _workspace_lease = if mode == TurnMode::Build {
        let coordinator = runtime
            .app
            .try_state::<crate::workspace_execution::WorkspaceExecutionCoordinator>();
        let result = coordinator
            .ok_or_else(|| "工作目录执行协调器未启动".to_string())
            .and_then(|coordinator| coordinator.acquire(&runtime.history_session_id, &runtime.cwd));
        match result {
            Ok(lease) => Some(lease),
            Err(message) => {
                if let Some(ack) = start_ack.take() {
                    let _ = ack.send(Err(message.clone()));
                }
                emit_error(&runtime, message, false).await;
                return;
            }
        }
    } else {
        None
    };

    // 新轮次开始时清空被拒绝目标列表（允许重新尝试），并把本轮模式同步给 hook
    if !resume {
        let mut state = read_approval_state(&runtime.state_path);
        state.denied_targets.clear();
        state.turn_mode = mode.as_state_str().to_string();
        let _ = write_approval_state(&runtime.state_path, &state);
    }

    let mut cmd = build_command(&runtime.bin);
    apply_inherited_agent_environment(&mut cmd);
    apply_claude_setting_source_policy(&mut cmd, runtime.use_user_setting_source);
    cmd.arg("-p").args([
        "--output-format",
        "stream-json",
        "--verbose",
        "--include-partial-messages",
        "--include-hook-events",
    ]);
    let session_context = runtime.current_session_context.lock().await.clone();
    apply_claude_session_context_args(&mut cmd, &runtime.cwd, &session_context);
    if mode != TurnMode::Build
        || configured_profile == PermissionProfile::Standard
        || (configured_profile == PermissionProfile::Auto
            && auto_support == crate::capability_registry::CapabilitySupport::Degraded)
    {
        cmd.arg("--settings").arg(&runtime.settings_path);
    }

    let disabled_mcp = runtime
        .disabled_mcp
        .lock()
        .map(|guard| guard.clone())
        .unwrap_or_default();
    if !disabled_mcp.is_empty() {
        let out_dir = runtime
            .settings_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(std::env::temp_dir);
        match build_claude_mcp_config_file(&disabled_mcp, &out_dir) {
            Ok(config_path) => {
                cmd.arg("--strict-mcp-config")
                    .arg("--mcp-config")
                    .arg(config_path);
            }
            Err(err) => {
                let message = format!("Claude MCP 配置失败，已阻止本轮启动：{err}");
                if let Some(ack) = start_ack.take() {
                    let _ = ack.send(Err(message.clone()));
                }
                emit_error(&runtime, message, false).await;
                return;
            }
        }
    }

    let model = runtime.model.lock().await.clone();
    if !model.is_empty() {
        cmd.args(["--model", &model]);
    }
    // Claude 的环境变量优先级高于 CLI flag，因此 Helm 同时覆盖继承环境；auto 通过
    // 环境变量明确恢复模型默认，非 auto 再带 --effort 形成可审计的 CLI 契约。
    cmd.args(crate::reasoning::claude_cli_effort_args(reasoning_effort));
    let system_prompt = if mode == TurnMode::Ask {
        ASK_MODE_APPEND_PROMPT.to_string()
    } else {
        String::new()
    };
    if !system_prompt.is_empty() {
        cmd.args(["--append-system-prompt", &system_prompt]);
    }
    // 模式 + Session 权限档位 → CLI 参数（普通 RuntimeManaged 由 Runtime 托管）：
    // 计划 = 原生 plan 权限模式（CLI 自带只读约束 + 计划指令）；
    // 询问 = 软约束走 --append-system-prompt，硬约束在审批 hook 的 turnMode 判定；
    // 构建 = 不加参数（现状默认行为）。
    let profile = *runtime.permission_profile.lock().await;
    let profile = if profile == PermissionProfile::FullAccess {
        let valid = runtime
            .full_access_lease
            .lock()
            .await
            .as_ref()
            .is_some_and(|lease| {
                lease_is_valid(
                    lease,
                    &runtime.history_session_id,
                    "claude-code",
                    &runtime.cwd,
                )
            });
        if valid {
            profile
        } else {
            *runtime.permission_profile.lock().await = PermissionProfile::Standard;
            PermissionProfile::Standard
        }
    } else {
        profile
    };
    let permission_mode = claude_permission_mode_for_capability(mode, profile, auto_support);
    cmd.args(["--permission-mode", permission_mode]);
    if let Some(sid) = resume_id {
        cmd.args(["--resume", &sid]);
    }
    if let Some(text) = prompt {
        let current_prompt = prompt_with_attachments(&text, &attachments);
        // 回溯/无 CLI 会话可续时（P2-5）：首轮把截断历史序列化进 prompt 重新开场，用后即清
        let rebuild = std::mem::take(&mut *runtime.rebuild_history.lock().await);
        cmd.arg(serialize_history_prompt(&rebuild, &current_prompt));
    }
    cmd.current_dir(&runtime.cwd);
    for (key, value) in &runtime.env {
        cmd.env(key, value);
    }
    cmd.env("CLAUDE_CODE_EFFORT_LEVEL", reasoning_effort.as_str());
    cmd.env("HELM_PERMISSION_ENDPOINT", &runtime.permission_endpoint)
        .env("HELM_PERMISSION_TOKEN", &runtime.permission_token)
        .env("HELM_HISTORY_SESSION_ID", &runtime.history_session_id)
        .env("HELM_TURN_ID", current_turn_id)
        .env("HELM_SESSION_CWD", &runtime.cwd);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => {
            let error = format!("无法启动 claude 进程：{e}");
            if let Some(ack) = start_ack.take() {
                let _ = ack.send(Err(error.clone()));
            }
            emit_error(&runtime, error, false).await;
            return;
        }
    };

    set_running_pid(&runtime.running_pid, child.id()).await;
    let stdout = match child.stdout.take() {
        Some(stdout) => stdout,
        None => {
            let error = "无法获取 claude stdout".to_string();
            kill_tree(child.id()).await;
            let _ = child.wait().await;
            set_running_pid(&runtime.running_pid, None).await;
            if let Some(ack) = start_ack.take() {
                let _ = ack.send(Err(error.clone()));
            }
            emit_error(&runtime, error, false).await;
            return;
        }
    };
    let stderr = match child.stderr.take() {
        Some(stderr) => stderr,
        None => {
            let error = "无法获取 claude stderr".to_string();
            kill_tree(child.id()).await;
            let _ = child.wait().await;
            set_running_pid(&runtime.running_pid, None).await;
            if let Some(ack) = start_ack.take() {
                let _ = ack.send(Err(error.clone()));
            }
            emit_error(&runtime, error, false).await;
            return;
        }
    };

    if runtime.interrupted.load(Ordering::Acquire) {
        let error = "审批等待期间会话已中断，未启动恢复轮".to_string();
        kill_tree(child.id()).await;
        let _ = child.wait().await;
        set_running_pid(&runtime.running_pid, None).await;
        if let Some(ack) = start_ack.take() {
            let _ = ack.send(Err(error));
        }
        return;
    }
    if let Some(ack) = start_ack.take() {
        let _ = ack.send(Ok(()));
    }

    // Claude 的 result 只是候选终态，必须等子进程退出成功后才能提交；否则 CLI 可能先输出
    // subtype=success，随后以非零状态退出，导致 Supervisor 把真正的错误当作迟到事件丢弃。
    // saw_turn_complete 用于兜底"退出码 0 但无 result"；
    // saw_approval 豁免审批 defer 场景（此时进程退出等待用户决定是正常流程）；
    // last_activity_ms 供看门狗判断进程是否挂起。
    let saw_turn_complete = Arc::new(AtomicBool::new(false));
    let saw_approval = Arc::new(AtomicBool::new(false));
    let saw_model_error = Arc::new(AtomicBool::new(false));
    let saw_session_started = Arc::new(AtomicBool::new(false));
    let auto_fallback_requested = Arc::new(AtomicBool::new(false));
    let saw_executed_tool = Arc::new(AtomicBool::new(false));
    let terminal_candidate = Arc::new(Mutex::new(None::<AgentEvent>));
    let last_activity_ms = Arc::new(AtomicU64::new(now_millis() as u64));

    let stdout_runtime = runtime.clone();
    let stdout_saw_turn_complete = saw_turn_complete.clone();
    let stdout_saw_approval = saw_approval.clone();
    let stdout_saw_model_error = saw_model_error.clone();
    let stdout_saw_session_started = saw_session_started.clone();
    let stdout_auto_fallback_requested = auto_fallback_requested.clone();
    let stdout_saw_executed_tool = saw_executed_tool.clone();
    let stdout_terminal_candidate = terminal_candidate.clone();
    let stdout_last_activity = last_activity_ms.clone();
    let stdout_task = tokio::spawn(async move {
        let mut lines = BufReader::new(stdout).lines();
        // delta 合批（变更-09）：逐 chunk emit 会造成每字符级 IPC + 前端全量重渲染。
        // 连续同类 delta 在 ~33ms 窗口内合并成一条再发；任何其他事件先冲刷缓冲保证顺序。
        let mut pending_delta: Option<AgentEvent> = None;
        loop {
            let next = if pending_delta.is_some() {
                match tokio::time::timeout(std::time::Duration::from_millis(33), lines.next_line())
                    .await
                {
                    Ok(result) => result,
                    Err(_elapsed) => {
                        if let Some(event) = pending_delta.take() {
                            emit_agent_event(
                                &stdout_runtime.app,
                                &stdout_runtime.history_session_id,
                                &event,
                            );
                        }
                        continue;
                    }
                }
            } else {
                lines.next_line().await
            };
            let Ok(Some(line)) = next else {
                break;
            };
            stdout_last_activity.store(now_millis() as u64, Ordering::Release);
            let events = parse_claude_line(&line);

            for mut event in events {
                let session_started = stdout_saw_session_started.load(Ordering::Acquire);
                let current_approved_tool_result = match &event {
                    AgentEvent::ToolResult { id, .. } => {
                        stdout_runtime.user_approved_tools.lock().await.contains(id)
                    }
                    _ => false,
                };
                if !should_process_claude_event(
                    session_started,
                    current_approved_tool_result,
                    &event,
                ) {
                    continue;
                }
                if matches!(&event, AgentEvent::SessionStarted { .. }) {
                    stdout_saw_session_started.store(true, Ordering::Release);
                }
                if merge_pending_delta(&mut pending_delta, &event) {
                    continue;
                }
                if let Some(buffered) = pending_delta.take() {
                    emit_agent_event(
                        &stdout_runtime.app,
                        &stdout_runtime.history_session_id,
                        &buffered,
                    );
                }
                if matches!(
                    event,
                    AgentEvent::MessageDelta { .. } | AgentEvent::ThinkingDelta { .. }
                ) {
                    pending_delta = Some(event);
                    continue;
                }
                let mut suppress_event = false;
                let mut post_event_error = None;
                match &mut event {
                    AgentEvent::SessionStarted {
                        session_id,
                        capabilities,
                        ..
                    } => {
                        *stdout_runtime.session_id.lock().await = Some(session_id.clone());
                        let capability_snapshot =
                            stdout_runtime.capability_snapshot.lock().await.clone();
                        if let Some(observed) = capabilities.as_mut() {
                            observed.capability_snapshot_id = Some(capability_snapshot.id.clone());
                        } else {
                            *capabilities = Some(capability_snapshot.runtime_projection());
                        }
                    }
                    AgentEvent::ToolCall {
                        session_id,
                        id,
                        name,
                        input,
                        ..
                    } => {
                        // ✅ 在 ToolCall 时就记录，供后续 stderr 的 APPROVAL_NEEDED 使用
                        stdout_runtime.pending_tools.lock().await.insert(
                            id.clone(),
                            PendingToolInfo {
                                name: name.clone(),
                                input: input.clone(),
                            },
                        );
                        if let Some(history_store) =
                            stdout_runtime.app.try_state::<SessionHistoryStore>()
                        {
                            let cwd = PathBuf::from(&stdout_runtime.cwd);
                            match stdout_runtime.app.path().app_data_dir() {
                                Ok(app_data_dir) => match create_auto_checkpoint_for_tool(
                                    &history_store,
                                    &app_data_dir.join("snapshots"),
                                    &stdout_runtime.history_session_id,
                                    session_id,
                                    &cwd,
                                    id,
                                    name,
                                    input,
                                    &stdout_runtime.current_turn_id.lock().await,
                                ) {
                                    Ok(Some(checkpoint)) => emit_agent_event(
                                        &stdout_runtime.app,
                                        &stdout_runtime.history_session_id,
                                        &checkpoint,
                                    ),
                                    Ok(None) => {}
                                    Err(err) => {
                                        emit_agent_event(
                                            &stdout_runtime.app,
                                            &stdout_runtime.history_session_id,
                                            &AgentEvent::Error {
                                                session_id: Some(session_id.clone()),
                                                message: format!(
                                                    "自动创建检查点失败，已终止本轮：{err}"
                                                ),
                                                recoverable: false,
                                                kind: Some("checkpoint_failed".to_string()),
                                            },
                                        );
                                        let pid = *stdout_runtime.running_pid.lock().await;
                                        kill_tree(pid).await;
                                    }
                                },
                                Err(err) => {
                                    emit_agent_event(
                                        &stdout_runtime.app,
                                        &stdout_runtime.history_session_id,
                                        &AgentEvent::Error {
                                            session_id: Some(session_id.clone()),
                                            message: format!(
                                                "获取检查点目录失败，已终止本轮：{err}"
                                            ),
                                            recoverable: false,
                                            kind: Some("checkpoint_failed".to_string()),
                                        },
                                    );
                                    let pid = *stdout_runtime.running_pid.lock().await;
                                    kill_tree(pid).await;
                                }
                            }
                        }
                    }
                    AgentEvent::ApprovalRequest {
                        id,
                        action,
                        input,
                        persistent_label,
                        matcher_summary,
                        available_decisions,
                        ..
                    } => {
                        stdout_saw_approval.store(true, Ordering::Release);
                        let pending = PendingToolInfo {
                            name: action.clone(),
                            input: input.clone().unwrap_or(serde_json::Value::Null),
                        };
                        stdout_runtime
                            .pending_tools
                            .lock()
                            .await
                            .entry(id.clone())
                            .or_insert_with(|| pending.clone());
                        let turn_id = stdout_runtime.current_turn_id.lock().await.clone();
                        let descriptor = build_approval_action(
                            &stdout_runtime.history_session_id,
                            &turn_id,
                            id,
                            &pending,
                            &stdout_runtime.cwd,
                        );
                        if let Some(display) =
                            crate::permissions::runtime_grant_display(&descriptor)
                        {
                            *persistent_label = Some(display.persistent_label);
                            *matcher_summary = Some(display.matcher_summary);
                        }
                        *available_decisions = available_approval_decisions(Some(&descriptor));
                    }
                    AgentEvent::ToolResult {
                        id,
                        status,
                        outcome,
                        started,
                        output,
                        native_denial_code,
                        ..
                    } => {
                        if started == &Some(true) {
                            stdout_saw_executed_tool.store(true, Ordering::Release);
                        }
                        if matches!(
                            outcome,
                            Some(
                                crate::protocol::ToolOutcomeKind::AutoReviewUnavailable
                                    | crate::protocol::ToolOutcomeKind::AutoReviewParseError
                            )
                        ) {
                            let decision = auto_fallback_decision(
                                configured_profile,
                                *outcome,
                                stdout_runtime.auto_compat_attempted.load(Ordering::Acquire),
                                stdout_saw_executed_tool.load(Ordering::Acquire),
                            );
                            if let Some(code) = native_denial_code.as_deref() {
                                if let Some(registry) = stdout_runtime
                                    .app
                                    .try_state::<crate::capability_registry::EngineCapabilityRegistry>()
                                {
                                    let current =
                                        stdout_runtime.capability_snapshot.lock().await.clone();
                                    if let Ok(updated) =
                                        registry.record_auto_review_degraded(&current, code)
                                    {
                                        *stdout_runtime.capability_snapshot.lock().await = updated;
                                    }
                                }
                            }
                            if decision == AutoFallbackDecision::RetryCompatible {
                                *output = Some(
                                    "自动审查暂时不可用，工具尚未执行。Helm 正在切换兼容执行方式。"
                                        .to_string(),
                                );
                                stdout_auto_fallback_requested.store(true, Ordering::Release);
                            } else {
                                *output = Some(
                                    "自动审查不可用，工具未执行。本轮已停止，以避免重复尝试。"
                                        .to_string(),
                                );
                            }
                        }
                        let user_approved =
                            stdout_runtime.user_approved_tools.lock().await.remove(id);
                        if user_approved && started == &Some(true) {
                            let audit_result =
                                match stdout_runtime.pending_tools.lock().await.get(id).cloned() {
                                    Some(tool) => {
                                        let turn_id =
                                            stdout_runtime.current_turn_id.lock().await.clone();
                                        let action = build_approval_action(
                                            &stdout_runtime.history_session_id,
                                            &turn_id,
                                            id,
                                            &tool,
                                            &stdout_runtime.cwd,
                                        );
                                        match stdout_runtime.app.try_state::<SessionHistoryStore>()
                                        {
                                            Some(history_store) => history_store
                                                .mark_user_approved_execution_started(&action)
                                                .and_then(|()| {
                                                    history_store.finish_permission_execution(
                                                        &action,
                                                        status == &ToolStatus::Success,
                                                    )
                                                }),
                                            None => {
                                                Err("会话历史存储不可用，无法记录已批准工具执行"
                                                    .to_string())
                                            }
                                        }
                                    }
                                    None => Err(format!("找不到已批准工具的执行审计上下文：{id}")),
                                };
                            if let Err(error) = audit_result {
                                post_event_error =
                                    Some(format!("已批准工具执行审计失败，已终止本轮：{error}"));
                            }
                        }
                    }
                    AgentEvent::Error { kind, .. }
                        if kind.as_deref() == Some("model_unavailable") =>
                    {
                        stdout_saw_model_error.store(true, Ordering::Release);
                    }
                    AgentEvent::TurnComplete { stop_reason, .. } => {
                        if stdout_saw_model_error.load(Ordering::Acquire) {
                            *stop_reason = StopReason::Error;
                        }
                        stdout_saw_turn_complete.store(true, Ordering::Release);
                        suppress_event = true;
                        if !stdout_auto_fallback_requested.load(Ordering::Acquire)
                            && !stdout_saw_model_error.load(Ordering::Acquire)
                        {
                            let mut candidate = stdout_terminal_candidate.lock().await;
                            if candidate.is_none() {
                                *candidate = Some(event.clone());
                            }
                        }
                    }
                    _ => {}
                }
                if suppress_event {
                    continue;
                }
                emit_agent_event(
                    &stdout_runtime.app,
                    &stdout_runtime.history_session_id,
                    &event,
                );
                if let Some(message) = post_event_error {
                    emit_agent_event(
                        &stdout_runtime.app,
                        &stdout_runtime.history_session_id,
                        &AgentEvent::Error {
                            session_id: stdout_runtime.session_id.lock().await.clone(),
                            message,
                            recoverable: false,
                            kind: Some("permission_audit_failed".to_string()),
                        },
                    );
                }
            }
        }
        // 流结束：冲刷残留缓冲
        if let Some(event) = pending_delta.take() {
            emit_agent_event(
                &stdout_runtime.app,
                &stdout_runtime.history_session_id,
                &event,
            );
        }
    });

    let stderr_runtime = runtime.clone();
    let stderr_saw_approval = saw_approval.clone();
    let stderr_saw_session_started = saw_session_started.clone();
    let stderr_last_activity = last_activity_ms.clone();
    let stderr_task = tokio::spawn(async move {
        let mut buf = String::new();
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            stderr_last_activity.store(now_millis() as u64, Ordering::Release);
            // 检测 Hook 的审批通知（兜底通道：常规链路走 stdout 的 deferred_tool_use）
            if line.starts_with("APPROVAL_NEEDED:")
                && stderr_saw_session_started.load(Ordering::Acquire)
            {
                stderr_saw_approval.store(true, Ordering::Release);
                if let Some(request_id) = line.strip_prefix("APPROVAL_NEEDED:") {
                    let pending_info = stderr_runtime
                        .pending_tools
                        .lock()
                        .await
                        .get(request_id)
                        .cloned();
                    // pending_tools 查不到也必须发卡片（变更-07 S7）：否则既无审批卡
                    // 也无终态事件，saw_approval 又豁免了兜底错误，UI 永久卡 working
                    let (name, input) = pending_info
                        .map(|info| (info.name, info.input))
                        .unwrap_or_else(|| ("未知工具".to_string(), serde_json::Value::Null));
                    let detail =
                        serde_json::to_string_pretty(&input).unwrap_or_else(|_| input.to_string());
                    let session_id = stderr_runtime
                        .session_id
                        .lock()
                        .await
                        .clone()
                        .unwrap_or_default();
                    emit_agent_event(
                        &stderr_runtime.app,
                        &stderr_runtime.history_session_id,
                        &AgentEvent::ApprovalRequest {
                            session_id,
                            id: request_id.to_string(),
                            action: name,
                            detail,
                            input: Some(input),
                            available_decisions: available_approval_decisions(None),
                            persistent_label: None,
                            matcher_summary: None,
                        },
                    );
                }
            }
            buf.push_str(&line);
            buf.push('\n');
        }
        buf
    });

    // 看门狗：进程长时间无任何输出时提示用户（不强杀，用户可自行停止）。
    let watchdog_runtime = runtime.clone();
    let watchdog_activity = last_activity_ms.clone();
    let watchdog = tokio::spawn(async move {
        const IDLE_WARN_MS: u64 = 300_000;
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            let idle =
                (now_millis() as u64).saturating_sub(watchdog_activity.load(Ordering::Acquire));
            if idle >= IDLE_WARN_MS {
                let session_id = watchdog_runtime
                    .session_id
                    .lock()
                    .await
                    .clone()
                    .unwrap_or_else(|| watchdog_runtime.history_session_id.clone());
                emit_agent_event(
                    &watchdog_runtime.app,
                    &watchdog_runtime.history_session_id,
                    &AgentEvent::TurnStage {
                        session_id,
                        stage: TurnStage::Stalled,
                        ts: now_millis(),
                        engine_reported_ttft_ms: None,
                        retry_attempt: None,
                    },
                );
                return;
            }
        }
    });

    let status = child.wait().await;
    watchdog.abort();
    let _ = stdout_task.await;
    let detail = stderr_task.await.unwrap_or_default().trim().to_string();
    set_running_pid(&runtime.running_pid, None).await;

    // 退出码判定：wait 出错或被信号杀死（无退出码）一律视为异常，绝不能默认成功——
    // 否则既无报错也无 turn_complete，UI 会永远停在"思考中"。
    let code = match &status {
        Ok(s) if s.success() => 0,
        Ok(s) => s.code().unwrap_or(-1),
        Err(_) => -1,
    };
    let mut terminal_candidate = terminal_candidate.lock().await.take();
    let disposition = claude_exit_disposition(
        runtime.interrupted.load(Ordering::Acquire),
        auto_fallback_requested.load(Ordering::Acquire),
        saw_model_error.load(Ordering::Acquire),
        code == 0,
        terminal_candidate.is_some(),
        saw_approval.load(Ordering::Acquire),
    );
    if disposition == ClaudeExitDisposition::Return && runtime.interrupted.load(Ordering::Acquire) {
        return;
    }
    if auto_fallback_requested.load(Ordering::Acquire) {
        runtime.auto_compat_attempted.store(true, Ordering::Release);
        drop(_workspace_lease);
        drop(_turn_guard);
        drop(_busy_guard);
        let retry_spec = runtime.current_turn_spec.lock().await.clone();
        let retry_result = match retry_spec.as_ref() {
            Some(spec) => match runtime
                .app
                .try_state::<crate::runtime_registry::RuntimeRegistry>()
            {
                Some(registry) => {
                    registry
                        .begin_compatibility_retry(
                            &crate::runtime_registry::RuntimeOwnerRef::Session(
                                runtime.history_session_id.clone(),
                            ),
                            spec,
                            "[auto_review_compatibility_retry] 原生自动审查在工具执行前不可用",
                        )
                        .await
                }
                None => Err("RuntimeRegistry 未启动，无法登记兼容恢复 Attempt".to_string()),
            },
            None => Err("当前 TurnExecutionSpec 缺失，拒绝兼容恢复".to_string()),
        };
        if let Err(error) = retry_result {
            emit_error(&runtime, format!("自动审查兼容恢复未启动：{error}"), false).await;
            return;
        }
        if wait_until_idle_and_begin(&runtime.busy, &runtime.idle_notify, &runtime.interrupted)
            .await
            .is_ok()
        {
            Box::pin(run_claude_turn(
                runtime,
                Some(
                    "继续当前轮次。上一项工具请求在执行前被自动审查拒绝，尚未产生副作用；请仅重新发起该未执行动作，不要重复已完成动作。"
                        .to_string(),
                ),
                Vec::new(),
                true,
                None,
            ))
            .await;
        }
        return;
    }

    if permission_mode == "auto"
        && code == 0
        && saw_turn_complete.load(Ordering::Acquire)
        && configured_profile == PermissionProfile::Auto
    {
        if let Some(registry) = runtime
            .app
            .try_state::<crate::capability_registry::EngineCapabilityRegistry>()
        {
            let current = runtime.capability_snapshot.lock().await.clone();
            if let Ok(updated) = registry.record_auto_review_native(&current) {
                *runtime.capability_snapshot.lock().await = updated;
            }
        }
    }

    if disposition == ClaudeExitDisposition::ProcessError {
        let current_model = runtime.model.lock().await.clone();
        let resume_sid = runtime.session_id.lock().await.clone();
        eprintln!(
            "[helm] claude ProcessError: code={code}, model={current_model}, resume={resume_sid:?}, stderr_len={}, stderr_preview={}",
            detail.len(),
            if detail.is_empty() { "(empty)" } else { &detail[..detail.len().min(500)] }
        );
        let cause = match &status {
            Err(e) => format!("（无法获取退出状态：{e}）"),
            _ => String::new(),
        };
        let suffix = if detail.is_empty() {
            String::new()
        } else {
            format!("：{detail}")
        };
        emit_error(
            &runtime,
            format!("claude 进程异常退出（code={code}）{cause}{suffix}"),
            false,
        )
        .await;
    } else if disposition == ClaudeExitDisposition::EmitCandidate {
        if let Some(event) = terminal_candidate.take() {
            emit_agent_event(&runtime.app, &runtime.history_session_id, &event);
        }
    } else if disposition == ClaudeExitDisposition::MissingResult {
        // 进程正常退出但没有输出 result 行（且不是审批 defer 场景）：
        // 同样必须给出终态事件，否则 UI 悬空。
        emit_error(
            &runtime,
            "claude 进程已退出，但没有返回本轮结果（可能是 CLI 版本不兼容或输出被截断）"
                .to_string(),
            false,
        )
        .await;
    }
}

async fn interrupt_running(runtime: Arc<SessionRuntime>) {
    runtime.interrupted.store(true, Ordering::Release);
    runtime.idle_notify.notify_waiters();
    let pid = *runtime.running_pid.lock().await;
    kill_tree(pid).await;
    set_running_pid(&runtime.running_pid, None).await;
    emit_interrupted(&runtime).await;
}

fn select_effective_codex_home(
    session_home: Option<&PathBuf>,
    subscription_home: Option<&PathBuf>,
) -> Option<PathBuf> {
    session_home.cloned().or_else(|| subscription_home.cloned())
}

async fn update_permission_context(
    runtime: &SessionRuntime,
    turn_id: String,
    _turn_mode: TurnMode,
) -> Result<(), String> {
    let service = runtime
        .app
        .try_state::<PermissionService>()
        .ok_or_else(|| "权限服务未启动".to_string())?;
    service.self_check(&runtime.permission_token).await?;
    service
        .update_context(
            &runtime.permission_token,
            PermissionSessionContext {
                engine: "claude-code".to_string(),
                history_session_id: runtime.history_session_id.clone(),
                turn_id: turn_id.clone(),
                cwd: runtime.policy_cwd.clone(),
                permission_profile: runtime.permission_profile.lock().await.as_str().to_string(),
            },
        )
        .await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub async fn start_claude_with_resume_and_reasoning(
    app: AppHandle,
    history_session_id: String,
    bin: String,
    model: String,
    cwd: String,
    env: Vec<(String, String)>,
    reasoning_effort: ReasoningEffort,
    resume_id: Option<String>,
    history_messages: Vec<crate::sessions::SessionMessage>,
    capability_snapshot: crate::capability_registry::EngineCapabilitySnapshot,
) -> Result<AgentSession, String> {
    let use_user_setting_source = claude_uses_user_setting_source(&env);
    let hook_files = create_runtime_approval_hook_files()?;
    validate_cwd(&cwd)?;
    let canonical_cwd = std::path::Path::new(&cwd)
        .canonicalize()
        .map_err(|error| format!("工作目录不可用：{error}"))?
        .to_string_lossy()
        .to_string();
    let execution_cwd = canonical_cwd.clone();
    let policy_cwd = canonical_cwd.clone();
    let permission_service = app
        .try_state::<PermissionService>()
        .ok_or_else(|| "权限服务未启动，无法创建 Claude 会话".to_string())?;
    let initial_turn_id = "turn-unassigned".to_string();
    let registration = permission_service
        .register(PermissionSessionContext {
            engine: "claude-code".to_string(),
            history_session_id: history_session_id.clone(),
            turn_id: initial_turn_id.clone(),
            cwd: policy_cwd.clone(),
            permission_profile: PermissionProfile::Standard.as_str().to_string(),
        })
        .await;
    let runtime = Arc::new(SessionRuntime {
        app,
        history_session_id,
        bin,
        model: Mutex::new(model),
        cwd: execution_cwd,
        policy_cwd,
        env,
        use_user_setting_source,
        capability_snapshot: Mutex::new(capability_snapshot),
        turn_mode: Mutex::new(TurnMode::Build),
        reasoning_effort: Mutex::new(reasoning_effort),
        settings_path: hook_files.settings_path,
        state_path: hook_files.state_path,
        // 有 resume_id 时优先 --resume；否则首轮用序列化历史重建上下文
        rebuild_history: Mutex::new(if resume_id.is_none() {
            history_messages
        } else {
            Vec::new()
        }),
        session_id: Mutex::new(resume_id),
        running_pid: Mutex::new(None),
        turn_lock: Mutex::new(()),
        approval_lock: Mutex::new(()),
        permission_endpoint: registration.endpoint,
        permission_token: registration.token,
        current_turn_id: Mutex::new(initial_turn_id),
        current_turn_spec: Mutex::new(None),
        current_session_context: Mutex::new(Vec::new()),
        busy: AtomicBool::new(false),
        idle_notify: Notify::new(),
        interrupted: AtomicBool::new(false),
        auto_compat_attempted: AtomicBool::new(false),
        pending_tools: Mutex::new(HashMap::new()),
        user_approved_tools: Mutex::new(HashSet::new()),
        disabled_mcp: std::sync::Mutex::new(Vec::new()),
        permission_profile: Mutex::new(PermissionProfile::Standard),
        full_access_lease: Mutex::new(None),
    });
    let (tx, mut rx) = mpsc::unbounded_channel::<SessionCmd>();
    let manager_runtime = runtime.clone();
    tokio::spawn(async move {
        while let Some(cmd) = rx.recv().await {
            match cmd {
                SessionCmd::Send {
                    text,
                    attachments,
                    spec,
                } => {
                    let mode = TurnMode::parse(Some(&spec.turn_mode));
                    let configured_profile = *manager_runtime.permission_profile.lock().await;
                    if spec.engine_id != "claude-code"
                        || spec.permission_profile != configured_profile.as_str()
                    {
                        manager_runtime.busy.store(false, Ordering::Release);
                        manager_runtime.idle_notify.notify_waiters();
                        emit_error(
                            &manager_runtime,
                            "TurnExecutionSpec 与 Claude Runtime 路由不一致".to_string(),
                            false,
                        )
                        .await;
                        continue;
                    }
                    *manager_runtime.model.lock().await = spec.routed_model_id.clone();
                    manager_runtime.interrupted.store(false, Ordering::Release);
                    manager_runtime
                        .auto_compat_attempted
                        .store(false, Ordering::Release);
                    let turn_id = spec.turn_id.clone();
                    set_event_turn_context(
                        &manager_runtime.history_session_id,
                        &turn_id,
                        spec.turn_epoch,
                    );
                    begin_turn_supervisor(
                        &manager_runtime.app,
                        &manager_runtime.history_session_id,
                        &turn_id,
                        spec.turn_epoch,
                        mode,
                        configured_profile,
                    );
                    manager_runtime.pending_tools.lock().await.clear();
                    manager_runtime.user_approved_tools.lock().await.clear();
                    *manager_runtime.current_turn_id.lock().await = turn_id.clone();
                    *manager_runtime.current_turn_spec.lock().await = Some(spec.clone());
                    *manager_runtime.current_session_context.lock().await =
                        spec.session_context.clone();
                    if let Err(error) =
                        update_permission_context(&manager_runtime, turn_id, mode).await
                    {
                        manager_runtime.busy.store(false, Ordering::Release);
                        manager_runtime.idle_notify.notify_waiters();
                        emit_error(&manager_runtime, error, false).await;
                        continue;
                    }
                    // 模式随发起轮固定（变更-04）：审批恢复轮读到的仍是这里写入的值
                    *manager_runtime.turn_mode.lock().await = mode;
                    *manager_runtime.reasoning_effort.lock().await = spec.routed_reasoning_effort;
                    tokio::spawn(run_claude_turn(
                        manager_runtime.clone(),
                        Some(text),
                        attachments,
                        false,
                        None,
                    ));
                }
                SessionCmd::Approve {
                    request_id,
                    decision,
                    responder,
                } => {
                    let approval_runtime = manager_runtime.clone();
                    tokio::spawn(async move {
                        let result: Result<(), String> =
                            run_serialized_approval(&approval_runtime.approval_lock, async {
                                let prepared =
                                    {
                                        let pending = approval_runtime
                                            .pending_tools
                                            .lock()
                                            .await
                                            .get(&request_id)
                                            .cloned();
                                        let turn_id = approval_runtime.current_turn_id.lock().await.clone();
                                        let available_decisions = pending.as_ref().map_or_else(
                                            || available_approval_decisions(None),
                                            |tool| {
                                            let action = build_approval_action(
                                                &approval_runtime.history_session_id,
                                                &turn_id,
                                                &request_id,
                                                tool,
                                                &approval_runtime.cwd,
                                            );
                                                available_approval_decisions(Some(&action))
                                            },
                                        );
                                        validate_approval_decision(decision, &available_decisions)?;
                                        record_approval(&approval_runtime, &request_id, decision).await?
                                    };
                                if matches!(decision, ApprovalDecision::Deny) {
                                    approval_runtime.interrupted.store(true, Ordering::Release);
                                    approval_runtime.idle_notify.notify_waiters();
                                    let pid = *approval_runtime.running_pid.lock().await;
                                    kill_tree(pid).await;
                                    set_running_pid(&approval_runtime.running_pid, None).await;
                                    emit_denied_turn(&approval_runtime, request_id).await;
                                    return Ok(());
                                }
                                wait_until_idle_and_begin(
                                    &approval_runtime.busy,
                                    &approval_runtime.idle_notify,
                                    &approval_runtime.interrupted,
                                )
                                .await?;
                                let permission_service = approval_runtime
                                    .app
                                    .try_state::<PermissionService>()
                                    .ok_or_else(|| "权限服务未启动，无法恢复审批".to_string())?;
                                permission_service
                                    .self_check(&approval_runtime.permission_token)
                                    .await?;
                                let committed = match commit_approval_grants(
                                    &approval_runtime,
                                    &request_id,
                                    decision,
                                    &prepared,
                                )
                                .await
                                {
                                    Ok(committed) => committed,
                                    Err(error) => {
                                        let state_error = rollback_prepared_approval_state(
                                            &approval_runtime.state_path,
                                            &request_id,
                                        )
                                        .err();
                                        approval_runtime.busy.store(false, Ordering::Release);
                                        approval_runtime.idle_notify.notify_waiters();
                                        return Err(match state_error {
                                            Some(state_error) => {
                                                format!("{error}；同时回滚临时审批状态失败：{state_error}")
                                            }
                                            None => error,
                                        });
                                    }
                                };
                                if approval_runtime.interrupted.load(Ordering::Acquire) {
                                    rollback_approval_grants(&approval_runtime, committed)?;
                                    approval_runtime.busy.store(false, Ordering::Release);
                                    approval_runtime.idle_notify.notify_waiters();
                                    return Err("审批等待期间会话已中断，未启动恢复轮".to_string());
                                }
                                approval_runtime
                                    .user_approved_tools
                                    .lock()
                                    .await
                                    .insert(request_id.clone());
                                let (start_ack, started) = oneshot::channel();
                                tokio::spawn(run_claude_turn(
                                    approval_runtime.clone(),
                                    None,
                                    Vec::new(),
                                    true,
                                    Some(start_ack),
                                ));
                                let start_result = match started.await {
                                    Ok(result) => result,
                                    Err(_) => Err("Claude 恢复任务在启动确认前结束".to_string()),
                                };
                                if let Err(start_error) = start_result {
                                    approval_runtime
                                        .user_approved_tools
                                        .lock()
                                        .await
                                        .remove(&request_id);
                                    rollback_approval_grants(&approval_runtime, committed)?;
                                    return Err(start_error);
                                }
                                Ok(())
                            })
                            .await;
                        if let Err(error) = &result {
                            emit_error(&approval_runtime, error.clone(), false).await;
                        }
                        let _ = responder.send(result);
                    });
                }
                SessionCmd::ResetContext { messages } => {
                    // 回溯重建（P2-5）：作废 CLI 会话 id，下一轮以截断历史重新开场。
                    // 不需要抢 busy——只改状态，不拉进程。
                    *manager_runtime.session_id.lock().await = None;
                    *manager_runtime.rebuild_history.lock().await = messages;
                }
                SessionCmd::SetDisabledMcp {
                    disabled,
                    responder,
                } => {
                    // 会话级 MCP 开关（变更-11）：只改状态，下一轮拉进程时生效
                    let result = if manager_runtime.busy.load(Ordering::Acquire) {
                        Err("轮次进行中，结束后才能更新 MCP 开关".to_string())
                    } else {
                        manager_runtime
                            .disabled_mcp
                            .lock()
                            .map(|mut guard| *guard = disabled)
                            .map_err(|_| "Claude MCP 开关锁中毒".to_string())
                    };
                    let _ = responder.send(result);
                }
                SessionCmd::SetPermissionProfile { profile, responder } => {
                    if manager_runtime.busy.load(Ordering::Acquire) {
                        let _ =
                            responder.send(Err("轮次进行中，结束后才能切换权限档位".to_string()));
                    } else {
                        *manager_runtime.permission_profile.lock().await = profile;
                        *manager_runtime.full_access_lease.lock().await =
                            (profile == PermissionProfile::FullAccess).then(|| {
                                full_access_lease(
                                    &manager_runtime.history_session_id,
                                    "claude-code",
                                    &manager_runtime.cwd,
                                )
                            });
                        let _ = responder.send(Ok(()));
                    }
                }
                SessionCmd::Interrupt => {
                    interrupt_running(manager_runtime.clone()).await;
                }
            }
        }
        if let Some(permission_service) = manager_runtime.app.try_state::<PermissionService>() {
            permission_service
                .unregister(&manager_runtime.permission_token)
                .await;
        }
    });

    Ok(AgentSession::Claude(ClaudeSession {
        tx,
        cwd: runtime.cwd.clone(),
        control: Some(runtime),
    }))
}

fn spawn_codex_app_server_loops(session: CodexSession, process: Arc<CodexAppServerProcess>) {
    let approval_session = session.clone();
    let approval_rpc = process.rpc.clone();
    spawn_agent_task(async move {
        while let Some(request) = approval_rpc.next_approval_request().await {
            let request = match request {
                Ok(request) => request,
                Err(error) => {
                    let _ = approval_session.turn_completions.send(Err(error));
                    break;
                }
            };
            let correlations = approval_session.file_changes_by_item.lock().await.clone();
            let history_store = match approval_session.app.try_state::<SessionHistoryStore>() {
                Some(store) => store,
                None => {
                    let response = denied_approval_response(&request);
                    let _ = approval_rpc.respond(request.request_id(), response).await;
                    let _ = approval_session
                        .turn_completions
                        .send(Err("会话历史存储不可用，Codex 审批已拒绝".to_string()));
                    break;
                }
            };
            let helm_turn_id = approval_session.current_helm_turn_id.lock().await.clone();
            let native_turn_id = approval_session
                .current_app_server_turn_id
                .lock()
                .await
                .clone();
            let actions = match (helm_turn_id, native_turn_id) {
                (Some(helm_turn_id), Some(native_turn_id))
                    if request.native_turn_id() == native_turn_id =>
                {
                    normalize_approval_actions_for_turn(
                        &approval_session.history_session_id,
                        &helm_turn_id,
                        &request,
                        &correlations,
                    )
                }
                _ => Err("Codex 审批不属于当前 Helm Turn，已拒绝".to_string()),
            };
            let decision = actions.as_ref().map_err(Clone::clone).and_then(|actions| {
                evaluate_normalized_actions_with_kernel(&history_store, actions)
            });
            let decision = match decision {
                Ok(decision) => decision,
                Err(error) => {
                    let denied = crate::permissions::PermissionDecision {
                        effect: crate::permissions::PermissionEffect::Deny,
                        reason: error,
                        rule_id: None,
                        policy_version: history_store.permission_policy_version().unwrap_or(1),
                    };
                    if let Some(response) = automatic_approval_response(&request, &denied) {
                        if let Err(error) = approval_rpc
                            .respond(
                                match &request {
                                    crate::codex_app_server::CodexApprovalRequest::Command(
                                        value,
                                    ) => value.request_id.clone(),
                                    crate::codex_app_server::CodexApprovalRequest::FileChange(
                                        value,
                                    ) => value.request_id.clone(),
                                    crate::codex_app_server::CodexApprovalRequest::Permissions(
                                        value,
                                    ) => value.request_id.clone(),
                                },
                                response,
                            )
                            .await
                        {
                            let _ = approval_session.turn_completions.send(Err(error));
                            break;
                        }
                    }
                    continue;
                }
            };
            if let Some(response) = automatic_approval_response(&request, &decision) {
                let request_id = match &request {
                    crate::codex_app_server::CodexApprovalRequest::Command(value) => {
                        value.request_id.clone()
                    }
                    crate::codex_app_server::CodexApprovalRequest::FileChange(value) => {
                        value.request_id.clone()
                    }
                    crate::codex_app_server::CodexApprovalRequest::Permissions(value) => {
                        value.request_id.clone()
                    }
                };
                if let Err(error) = approval_rpc.respond(request_id, response).await {
                    let _ = approval_session.turn_completions.send(Err(error));
                    break;
                }
                continue;
            }
            let Ok(mut actions) = actions else {
                continue;
            };
            if actions.len() != 1 {
                let response = denied_approval_response(&request);
                if let Err(error) = approval_rpc.respond(request.request_id(), response).await {
                    let _ = approval_session.turn_completions.send(Err(error));
                    break;
                }
                let _ = approval_session
                    .turn_completions
                    .send(Err("Codex 审批包含多个不可原子裁决的动作".to_string()));
                break;
            }
            let action = actions.remove(0);
            let grant_display = crate::permissions::runtime_grant_display(&action);
            let approval_id = action.tool_call_id.clone();
            approval_session.pending_approvals.lock().await.insert(
                approval_id.clone(),
                CodexPendingApproval {
                    request,
                    action: action.clone(),
                },
            );
            let session_id = approval_session
                .thread_id
                .lock()
                .ok()
                .and_then(|guard| guard.clone())
                .unwrap_or_else(|| approval_session.history_session_id.clone());
            emit_agent_event(
                &approval_session.app,
                &approval_session.history_session_id,
                &codex_turn_stage(&session_id, TurnStage::WaitingApproval),
            );
            emit_agent_event(
                &approval_session.app,
                &approval_session.history_session_id,
                &AgentEvent::ApprovalRequest {
                    session_id,
                    id: approval_id,
                    action: action.operation.clone(),
                    detail: action.resources.join(", "),
                    available_decisions: available_approval_decisions(Some(&action)),
                    input: Some(action.raw_input),
                    persistent_label: grant_display
                        .as_ref()
                        .map(|display| display.persistent_label.clone()),
                    matcher_summary: grant_display.map(|display| display.matcher_summary),
                },
            );
        }
    });

    let notification_session = session;
    let notification_rpc = process.rpc.clone();
    spawn_agent_task(async move {
        while let Some(notification) = notification_rpc.next_notification().await {
            let notification = match notification {
                Ok(notification) => notification,
                Err(error) => {
                    let _ = notification_session.turn_completions.send(Err(error));
                    fail_codex_active_turn(&notification_session, "Codex app-server 通知协议失败")
                        .await;
                    break;
                }
            };
            if matches!(
                notification
                    .get("method")
                    .and_then(serde_json::Value::as_str),
                Some("item/started" | "item/completed")
            ) {
                if let Some(item) = notification.pointer("/params/item") {
                    let item_type = item.get("type").and_then(serde_json::Value::as_str);
                    let method = notification
                        .get("method")
                        .and_then(serde_json::Value::as_str);
                    if let Some(id) = item.get("id").and_then(serde_json::Value::as_str) {
                        if method == Some("item/started")
                            && matches!(item_type, Some("commandExecution" | "fileChange"))
                        {
                            notification_session
                                .pending_tool_items
                                .lock()
                                .await
                                .insert(id.to_string());
                        } else if method == Some("item/completed") {
                            notification_session
                                .pending_tool_items
                                .lock()
                                .await
                                .remove(id);
                        }
                    }
                    if item_type == Some("fileChange") {
                        if let Some(id) = item.get("id").and_then(serde_json::Value::as_str) {
                            let paths = item
                                .get("changes")
                                .and_then(serde_json::Value::as_array)
                                .into_iter()
                                .flatten()
                                .filter_map(|change| {
                                    change
                                        .get("path")
                                        .and_then(serde_json::Value::as_str)
                                        .map(ToString::to_string)
                                })
                                .collect::<Vec<_>>();
                            if !paths.is_empty() {
                                notification_session
                                    .file_changes_by_item
                                    .lock()
                                    .await
                                    .insert(id.to_string(), paths);
                            }
                        }
                    }
                }
            }
            let pending_tool_count = if notification
                .get("method")
                .and_then(serde_json::Value::as_str)
                == Some("turn/completed")
            {
                notification_session.pending_tool_items.lock().await.len()
            } else {
                0
            };
            let terminal_outcome =
                codex_app_server_terminal_outcome_with_pending(&notification, pending_tool_count);
            if let Some((turn_id, outcome)) = &terminal_outcome {
                let inserted = {
                    let mut terminal_turns = notification_session.terminal_turns.lock().await;
                    record_codex_terminal_once(
                        &mut terminal_turns,
                        turn_id.clone(),
                        outcome.clone(),
                    )
                };
                if !inserted {
                    continue;
                }
                let completion = match outcome {
                    Ok(()) => Ok(turn_id.clone()),
                    Err(error) => Err(error.clone()),
                };
                let _ = notification_session.turn_completions.send(completion);
            }
            let session_id = notification_session
                .thread_id
                .lock()
                .ok()
                .and_then(|guard| guard.clone())
                .unwrap_or_else(|| notification_session.history_session_id.clone());
            let event_notification = codex_notification_for_terminal_outcome(
                &notification,
                terminal_outcome.as_ref().map(|(_, outcome)| outcome),
            );
            let explicit_context =
                if let Some(native_turn_id) = codex_notification_native_turn_id(&notification) {
                    notification_session
                        .native_turn_contexts
                        .lock()
                        .await
                        .resolve(native_turn_id)
                        .or_else(|| {
                            let digest = crate::turn_start::digest_json(native_turn_id)
                                .unwrap_or_else(|_| "sha256:unknown".to_string());
                            Some((format!("unknown-native-turn:{digest}"), 0))
                        })
                } else {
                    None
                };
            for event in parse_codex_app_server_notification(&session_id, &event_notification) {
                if let Some((helm_turn_id, turn_epoch)) = explicit_context.as_ref() {
                    emit_agent_event_in_turn(
                        &notification_session.app,
                        &notification_session.history_session_id,
                        Some(helm_turn_id),
                        Some(*turn_epoch),
                        &event,
                    );
                } else {
                    emit_agent_event(
                        &notification_session.app,
                        &notification_session.history_session_id,
                        &event,
                    );
                }
            }
            if terminal_outcome.is_some() {
                notification_session.terminal_notify.notify_waiters();
            }
        }
        let _ = notification_session.turn_completions.send(Err(
            "Codex app-server notification stream closed".to_string(),
        ));
        fail_codex_active_turn(
            &notification_session,
            "Codex app-server notification stream closed",
        )
        .await;
    });
}

async fn fail_codex_active_turn(session: &CodexSession, error: &str) {
    if let Some(turn_id) = session.current_app_server_turn_id.lock().await.clone() {
        session
            .terminal_turns
            .lock()
            .await
            .entry(turn_id)
            .or_insert_with(|| Err(error.to_string()));
        session.terminal_notify.notify_waiters();
    }
}

impl CodexSession {
    async fn ensure_app_server(&self) -> Result<Arc<CodexAppServerProcess>, String> {
        let mut slot = self.app_server.lock().await;
        if let Some(process) = slot.as_ref() {
            return Ok(process.clone());
        }
        validate_cwd(&self.cwd)?;
        validate_engine_bin(&self.bin)?;
        let canonical_cwd = std::path::Path::new(&self.cwd)
            .canonicalize()
            .map_err(|error| format!("工作目录不可用：{error}"))?;
        *self.execution_cwd.lock().await = Some(canonical_cwd.display().to_string());
        *self.policy_cwd.lock().await = canonical_cwd.display().to_string();
        let configure_command = |command: &mut Command| {
            apply_inherited_agent_environment(command);
            for value in codex_provider_config_args(&self.env) {
                command.arg("-c").arg(value);
            }
            command.current_dir(&canonical_cwd).kill_on_drop(true);
            if let Some(path) = self
                .effective_home
                .lock()
                .ok()
                .and_then(|home| home.clone())
            {
                command.env("CODEX_HOME", path);
            }
            for (key, value) in &self.env {
                if !key.starts_with("HELM_") {
                    command.env(key, value);
                }
            }
        };
        let mut command = build_codex_command(&self.bin);
        configure_command(&mut command);
        let process = Arc::new(spawn_codex_app_server(command).await?);
        *slot = Some(process.clone());
        drop(slot);
        spawn_codex_app_server_loops(self.clone(), process.clone());
        Ok(process)
    }

    async fn run_app_server_turn(
        &self,
        prompt: String,
        attachments: Vec<String>,
        spec: crate::turn_start::TurnExecutionSpec,
    ) -> Result<(), String> {
        let mode = TurnMode::parse(Some(&spec.turn_mode));
        let reasoning_effort = spec.routed_reasoning_effort;
        let helm_turn_id = spec.turn_id.clone();
        *self.current_helm_turn_id.lock().await = Some(helm_turn_id.clone());
        set_event_turn_context(&self.history_session_id, &helm_turn_id, spec.turn_epoch);
        let selected_profile = *self
            .permission_profile
            .lock()
            .map_err(|_| "Codex 权限档位锁中毒".to_string())?;
        if spec.engine_id != "codex" || spec.permission_profile != selected_profile.as_str() {
            return Err("TurnExecutionSpec 与 Codex Runtime 路由不一致".to_string());
        }
        *self.model.lock().await = spec.routed_model_id.clone();
        let routed_model = spec.routed_model_id.clone();
        begin_turn_supervisor(
            &self.app,
            &self.history_session_id,
            &helm_turn_id,
            spec.turn_epoch,
            mode,
            selected_profile,
        );
        let _workspace_lease = if mode == TurnMode::Build {
            Some(
                self.app
                    .try_state::<crate::workspace_execution::WorkspaceExecutionCoordinator>()
                    .ok_or_else(|| "工作目录执行协调器未启动".to_string())?
                    .acquire(&self.history_session_id, &self.cwd)?,
            )
        } else {
            None
        };
        let process = self.ensure_app_server().await?;
        let mut permission_profile = *self
            .permission_profile
            .lock()
            .map_err(|_| "Codex 权限档位锁中毒".to_string())?;
        if permission_profile == PermissionProfile::FullAccess {
            let valid = self
                .full_access_lease
                .lock()
                .map_err(|_| "Codex FullAccessLease 锁中毒".to_string())?
                .as_ref()
                .is_some_and(|lease| {
                    lease_is_valid(lease, &self.history_session_id, "codex", &self.cwd)
                });
            if !valid {
                permission_profile = PermissionProfile::Standard;
                *self
                    .permission_profile
                    .lock()
                    .map_err(|_| "Codex 权限档位锁中毒".to_string())? = permission_profile;
            }
        }
        let (sandbox, native_network_allowed, approval_policy) =
            codex_runtime_profile_policy(mode, permission_profile);
        let prompt = prompt_with_attachments(&prompt, &attachments);
        let prompt = if mode == TurnMode::Plan {
            format!("{CODEX_PLAN_PROMPT_PREFIX}\n\n{prompt}")
        } else {
            prompt
        };
        let existing_thread = self.thread_id.lock().ok().and_then(|guard| guard.clone());
        let force_rebuild = self.force_history_rebuild.load(Ordering::Acquire);
        let history = self
            .history_messages
            .lock()
            .map(|history| history.clone())
            .unwrap_or_default();
        let base_prompt = prompt;
        let mut prompt = codex_app_server_prompt(force_rebuild, &history, &base_prompt);
        let execution_cwd = self
            .execution_cwd
            .lock()
            .await
            .clone()
            .ok_or_else(|| "Codex 工作目录尚未初始化".to_string())?;
        let thread_id = if self.app_server_thread_ready.load(Ordering::Acquire) {
            existing_thread.ok_or_else(|| "Codex app-server thread state is missing".to_string())?
        } else {
            let thread_id =
                match codex_app_server_thread_plan(existing_thread.as_deref(), force_rebuild) {
                    CodexAppServerThreadPlan::Start => {
                        process
                            .rpc
                            .start_thread_with_policy(
                                &execution_cwd,
                                &routed_model,
                                sandbox,
                                approval_policy,
                            )
                            .await?
                    }
                    CodexAppServerThreadPlan::Resume(thread_id) => {
                        match process
                            .rpc
                            .resume_thread_with_policy(
                                &thread_id,
                                &execution_cwd,
                                &routed_model,
                                sandbox,
                                approval_policy,
                            )
                            .await
                        {
                            Ok(thread_id) => thread_id,
                            Err(error) if is_codex_thread_missing_error(&error) => {
                                // app-server reports a missing rollout synchronously from
                                // thread/resume, before the turn stream can trigger the normal
                                // exec-path fallback. Rebuild from Helm's local history here.
                                if let Ok(mut guard) = self.thread_id.lock() {
                                    *guard = None;
                                }
                                self.force_history_rebuild.store(true, Ordering::Release);
                                self.app_server_thread_ready.store(false, Ordering::Release);
                                prompt = codex_app_server_prompt(true, &history, &base_prompt);
                                emit_agent_event(
                                    &self.app,
                                    &self.history_session_id,
                                    &AgentEvent::Error {
                                        session_id: Some(self.history_session_id.clone()),
                                        message: format!(
                                        "Codex 原生 thread 已不存在，将用本地历史重建一次：{error}"
                                    ),
                                        recoverable: true,
                                        kind: Some("thread_missing".to_string()),
                                    },
                                );
                                emit_agent_event(
                                    &self.app,
                                    &self.history_session_id,
                                    &AgentEvent::TurnStage {
                                        session_id: self.history_session_id.clone(),
                                        stage: TurnStage::Retrying,
                                        ts: now_millis(),
                                        engine_reported_ttft_ms: None,
                                        retry_attempt: Some(1),
                                    },
                                );
                                process
                                    .rpc
                                    .start_thread_with_policy(
                                        &execution_cwd,
                                        &routed_model,
                                        sandbox,
                                        approval_policy,
                                    )
                                    .await?
                            }
                            Err(error) => return Err(error),
                        }
                    }
                };
            if let Ok(mut guard) = self.thread_id.lock() {
                *guard = Some(thread_id.clone());
            }
            self.force_history_rebuild.store(false, Ordering::Release);
            self.app_server_thread_ready.store(true, Ordering::Release);
            if let Some(history_store) = self.app.try_state::<SessionHistoryStore>() {
                history_store
                    .attach_native_thread_to_session(&self.history_session_id, &thread_id)?;
            }
            thread_id
        };
        emit_agent_event(
            &self.app,
            &self.history_session_id,
            &AgentEvent::SessionStarted {
                session_id: thread_id.clone(),
                engine: EngineId::Codex,
                model: routed_model.clone(),
                cwd: execution_cwd.clone(),
                ts: now_millis(),
                capabilities: Some(self.capability_snapshot.lock().await.runtime_projection()),
            },
        );
        self.pending_tool_items.lock().await.clear();
        let native_turn_id = process
            .rpc
            .start_turn_with_context_policy(
                &thread_id,
                &prompt,
                &routed_model,
                sandbox,
                &execution_cwd,
                native_network_allowed,
                (!reasoning_effort.is_auto()).then_some(reasoning_effort.as_str()),
                approval_policy,
                &spec.session_context,
            )
            .await?;
        self.native_turn_contexts.lock().await.insert(
            native_turn_id.clone(),
            helm_turn_id,
            spec.turn_epoch,
        );
        *self.current_app_server_turn_id.lock().await = Some(native_turn_id.clone());
        loop {
            let notified = self.terminal_notify.notified();
            if let Some(outcome) =
                terminal_turn_outcome(&self.terminal_turns, &native_turn_id).await
            {
                match outcome {
                    Ok(()) => break,
                    Err(error) if error.starts_with(CODEX_TURN_FAILED_PREFIX) => break,
                    Err(error) => return Err(error),
                }
            }
            if tokio::time::timeout(std::time::Duration::from_secs(300), notified)
                .await
                .is_err()
            {
                emit_agent_event(
                    &self.app,
                    &self.history_session_id,
                    &AgentEvent::TurnStage {
                        session_id: thread_id.clone(),
                        stage: TurnStage::Stalled,
                        ts: now_millis(),
                        engine_reported_ttft_ms: None,
                        retry_attempt: None,
                    },
                );
            }
        }
        *self.current_app_server_turn_id.lock().await = None;
        Ok(())
    }

    async fn approve(&self, request_id: String, decision: ApprovalDecision) -> Result<(), String> {
        let pending = self
            .pending_approvals
            .lock()
            .await
            .get(&request_id)
            .cloned()
            .ok_or_else(|| format!("找不到 Codex 待审批请求：{request_id}"))?;
        let available_decisions = available_approval_decisions(Some(&pending.action));
        validate_approval_decision(decision, &available_decisions)?;
        let process = self
            .app_server
            .lock()
            .await
            .as_ref()
            .cloned()
            .ok_or_else(|| "Codex app-server 未运行，无法应用审批".to_string())?;
        let history_store = self
            .app
            .try_state::<SessionHistoryStore>()
            .ok_or_else(|| "会话历史存储不可用，无法提交 Codex 审批".to_string())?;
        let decision = match decision {
            ApprovalDecision::Allow => CodexUserDecision::Allow,
            ApprovalDecision::Turn => CodexUserDecision::Turn,
            ApprovalDecision::Session => CodexUserDecision::Session,
            ApprovalDecision::Project => CodexUserDecision::Project,
            ApprovalDecision::Deny => CodexUserDecision::Deny,
            ApprovalDecision::Always => CodexUserDecision::Always,
        };
        apply_codex_user_decision(&history_store, &process.rpc, &pending, decision).await?;
        self.pending_approvals.lock().await.remove(&request_id);
        Ok(())
    }

    fn send_reserved(
        &self,
        text: String,
        attachments: Vec<String>,
        spec: crate::turn_start::TurnExecutionSpec,
    ) {
        let session = self.clone();
        let busy = self.busy.clone();
        self.interrupted.store(false, Ordering::Release);
        spawn_agent_task(async move {
            if let Err(error) = session.run_app_server_turn(text, attachments, spec).await {
                emit_agent_event(
                    &session.app,
                    &session.history_session_id,
                    &AgentEvent::Error {
                        session_id: session
                            .thread_id
                            .lock()
                            .ok()
                            .and_then(|guard| guard.clone()),
                        message: error,
                        recoverable: true,
                        kind: Some("process_crash".to_string()),
                    },
                );
                let session_id = session
                    .thread_id
                    .lock()
                    .ok()
                    .and_then(|guard| guard.clone())
                    .unwrap_or_else(|| session.history_session_id.clone());
                emit_agent_event(
                    &session.app,
                    &session.history_session_id,
                    &AgentEvent::TurnComplete {
                        session_id,
                        stop_reason: StopReason::Error,
                    },
                );
            }
            busy.store(false, Ordering::Release);
        });
    }

    fn interrupt(&self) -> Result<(), String> {
        let process = self.app_server.clone();
        let app = self.app.clone();
        let history_session_id = self.history_session_id.clone();
        let thread_id = self.thread_id.clone();
        let turn_id = self.current_app_server_turn_id.clone();
        let terminal_turns = self.terminal_turns.clone();
        let terminal_notify = self.terminal_notify.clone();
        let running_pid = self.running_pid.clone();
        // 先立标志再杀进程：app-server 轮次收尾时据此改发 TurnComplete{Interrupted}
        self.interrupted.store(true, Ordering::Release);
        spawn_agent_task(async move {
            if let (Some(process), Some(thread_id), Some(turn_id)) = (
                process.lock().await.as_ref().cloned(),
                thread_id.lock().ok().and_then(|guard| guard.clone()),
                turn_id.lock().await.clone(),
            ) {
                let _ = process
                    .rpc
                    .request(
                        "turn/interrupt",
                        serde_json::json!({"threadId":thread_id,"turnId":turn_id}),
                    )
                    .await;
            }
            let pid = *running_pid.lock().await;
            kill_tree(pid).await;
            set_running_pid(&running_pid, None).await;
            let active_turn_id = turn_id.lock().await.clone();
            finish_codex_interrupt_terminal(
                &terminal_turns,
                &terminal_notify,
                active_turn_id,
                || {
                    let session_id = thread_id
                        .lock()
                        .ok()
                        .and_then(|guard| guard.clone())
                        .unwrap_or_else(|| history_session_id.clone());
                    emit_agent_event(
                        &app,
                        &history_session_id,
                        &AgentEvent::TurnComplete {
                            session_id,
                            stop_reason: StopReason::Interrupted,
                        },
                    );
                },
            )
            .await;
        });
        Ok(())
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn start_codex_with_reasoning(
    app: AppHandle,
    history_session_id: String,
    bin: String,
    model: String,
    cwd: String,
    env: Vec<(String, String)>,
    history_messages: Vec<crate::sessions::SessionMessage>,
    native_thread_id: Option<String>,
    auth_home: Option<CodexAuthHome>,
    subscription_home: Option<PathBuf>,
    capability_snapshot: crate::capability_registry::EngineCapabilitySnapshot,
    _reasoning_effort: ReasoningEffort,
) -> Result<AgentSession, String> {
    let canonical_cwd = std::path::Path::new(&cwd)
        .canonicalize()
        .map_err(|error| format!("工作目录不可用：{error}"))?
        .to_string_lossy()
        .to_string();
    let (turn_completions, _) = broadcast::channel(32);
    let session_home = auth_home.as_ref().map(|home| home.path.clone());
    let effective_home =
        select_effective_codex_home(session_home.as_ref(), subscription_home.as_ref());
    Ok(AgentSession::Codex(CodexSession {
        app,
        history_session_id,
        bin,
        model: Arc::new(Mutex::new(model)),
        cwd: canonical_cwd.clone(),
        execution_cwd: Arc::new(Mutex::new(Some(canonical_cwd.clone()))),
        policy_cwd: Arc::new(Mutex::new(canonical_cwd)),
        env,
        running_pid: Arc::new(Mutex::new(None)),
        history_messages: Arc::new(std::sync::Mutex::new(history_messages)),
        busy: Arc::new(AtomicBool::new(false)),
        interrupted: Arc::new(AtomicBool::new(false)),
        disabled_mcp: Arc::new(std::sync::Mutex::new(Vec::new())),
        thread_id: Arc::new(std::sync::Mutex::new(native_thread_id)),
        auth_home: Arc::new(std::sync::Mutex::new(auth_home)),
        effective_home: Arc::new(std::sync::Mutex::new(effective_home)),
        force_history_rebuild: Arc::new(AtomicBool::new(false)),
        app_server: Arc::new(Mutex::new(None)),
        app_server_thread_ready: Arc::new(AtomicBool::new(false)),
        pending_approvals: Arc::new(Mutex::new(HashMap::new())),
        file_changes_by_item: Arc::new(Mutex::new(HashMap::new())),
        pending_tool_items: Arc::new(Mutex::new(HashSet::new())),
        turn_completions,
        terminal_turns: Arc::new(Mutex::new(HashMap::new())),
        terminal_notify: Arc::new(Notify::new()),
        current_helm_turn_id: Arc::new(Mutex::new(None)),
        current_app_server_turn_id: Arc::new(Mutex::new(None)),
        native_turn_contexts: Arc::new(Mutex::new(CodexTurnContextIndex::default())),
        permission_profile: Arc::new(std::sync::Mutex::new(PermissionProfile::Standard)),
        full_access_lease: Arc::new(std::sync::Mutex::new(None)),
        capability_snapshot: Arc::new(Mutex::new(capability_snapshot)),
    }))
}

const CODEX_TURN_FAILED_PREFIX: &str = "[codex_turn_failed]";

fn codex_app_server_terminal_outcome(
    notification: &serde_json::Value,
) -> Option<(String, Result<(), String>)> {
    if notification
        .get("method")
        .and_then(serde_json::Value::as_str)
        != Some("turn/completed")
    {
        return None;
    }
    let turn_id = notification
        .pointer("/params/turn/id")
        .and_then(serde_json::Value::as_str)?
        .to_string();
    let status = notification
        .pointer("/params/turn/status")
        .and_then(serde_json::Value::as_str);
    let outcome = if status == Some("failed") {
        Err(format!(
            "{CODEX_TURN_FAILED_PREFIX} {}",
            codex_app_server_failure_message(notification)
        ))
    } else {
        Ok(())
    };
    Some((turn_id, outcome))
}

fn codex_app_server_terminal_outcome_with_pending(
    notification: &serde_json::Value,
    pending_tool_count: usize,
) -> Option<(String, Result<(), String>)> {
    let (turn_id, outcome) = codex_app_server_terminal_outcome(notification)?;
    if outcome.is_ok() && pending_tool_count > 0 {
        return Some((
            turn_id,
            Err(format!(
                "{CODEX_TURN_FAILED_PREFIX} Codex turn completed with {pending_tool_count} unfinished tool item(s)"
            )),
        ));
    }
    Some((turn_id, outcome))
}

fn codex_notification_for_terminal_outcome(
    notification: &serde_json::Value,
    outcome: Option<&Result<(), String>>,
) -> serde_json::Value {
    let Some(Err(error)) = outcome else {
        return notification.clone();
    };
    if notification
        .pointer("/params/turn/status")
        .and_then(serde_json::Value::as_str)
        == Some("failed")
    {
        return notification.clone();
    }
    let mut adjusted = notification.clone();
    adjusted["params"]["turn"]["status"] = serde_json::json!("failed");
    adjusted["params"]["turn"]["error"] = serde_json::json!({"message": error});
    adjusted
}

fn codex_notification_native_turn_id(notification: &serde_json::Value) -> Option<&str> {
    notification
        .pointer("/params/turnId")
        .and_then(serde_json::Value::as_str)
        .or_else(|| {
            notification
                .pointer("/params/turn/id")
                .and_then(serde_json::Value::as_str)
        })
}

fn record_codex_terminal_once(
    terminal_turns: &mut HashMap<String, Result<(), String>>,
    turn_id: String,
    outcome: Result<(), String>,
) -> bool {
    if terminal_turns.contains_key(&turn_id) {
        return false;
    }
    if terminal_turns.len() >= 64 {
        if let Some(oldest) = terminal_turns.keys().next().cloned() {
            terminal_turns.remove(&oldest);
        }
    }
    terminal_turns.insert(turn_id, outcome);
    true
}

async fn finish_codex_interrupt_terminal<F>(
    terminal_turns: &Mutex<HashMap<String, Result<(), String>>>,
    terminal_notify: &Notify,
    active_turn_id: Option<String>,
    emit_terminal: F,
) where
    F: FnOnce(),
{
    let should_emit = if let Some(active_turn_id) = active_turn_id.as_ref() {
        let mut terminal_turns = terminal_turns.lock().await;
        record_codex_terminal_once(&mut terminal_turns, active_turn_id.clone(), Ok(()))
    } else {
        true
    };
    if should_emit {
        emit_terminal();
    }
    if active_turn_id.is_some() {
        terminal_notify.notify_waiters();
    }
}

pub(crate) fn codex_app_server_failure_message(notification: &serde_json::Value) -> String {
    let message = notification
        .pointer("/params/turn/error/message")
        .and_then(serde_json::Value::as_str)
        .or_else(|| {
            notification
                .pointer("/params/error/message")
                .and_then(serde_json::Value::as_str)
        })
        .or_else(|| {
            notification
                .pointer("/params/turn/error")
                .and_then(serde_json::Value::as_str)
        })
        .unwrap_or_default();
    // v2 的结构化详情仅参与分类，不能原样展示，避免泄露 URL、请求 ID 或 header。
    let additional_details = notification
        .pointer("/params/turn/error/additionalDetails")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    let error_info = notification
        .pointer("/params/turn/error/codexErrorInfo")
        .map(serde_json::Value::to_string)
        .unwrap_or_default();
    let raw = format!("{message}\n{additional_details}\n{error_info}").to_ascii_lowercase();
    if raw.contains("unfinished tool") || raw.contains("missing tool result") {
        "Codex 工具执行未返回结果，已阻止本轮伪成功；请检查当前 Codex 版本的命令执行能力。"
            .to_string()
    } else if raw.contains("401") || raw.contains("unauthorized") || raw.contains("authentication")
    {
        "Codex 服务商认证失败，请重新验证当前 API 接入。".to_string()
    } else if raw.contains("403") || raw.contains("forbidden") {
        "Codex 服务商拒绝了当前请求，请检查账号权限与模型授权。".to_string()
    } else if raw.contains("model")
        && (raw.contains("not found")
            || raw.contains("does not exist")
            || raw.contains("access")
            || raw.contains("unavailable"))
    {
        "Codex 当前绑定的模型不存在或账号无权访问，请重新同步模型并调整生效绑定。".to_string()
    } else if raw.contains("429") || raw.contains("rate limit") {
        "Codex 服务商触发了请求频率限制，请稍后重试。".to_string()
    } else if raw.contains("usagelimitexceeded") || raw.contains("sessionbudgetexceeded") {
        "Codex 订阅账号当前用量已达上限，请在官方账号页面确认额度后重试。".to_string()
    } else if raw.contains("contextwindowexceeded") {
        "Codex 当前对话超过模型上下文上限，请新建会话或减少输入内容。".to_string()
    } else if raw.contains("serveroverloaded") || raw.contains("internalservererror") {
        "Codex 服务暂时繁忙，请稍后重试。".to_string()
    } else if raw.contains("timeout")
        || raw.contains("timed out")
        || raw.contains("connection")
        || raw.contains("network")
        || raw.contains("httpconnectionfailed")
        || raw.contains("responsestreamconnectionfailed")
        || raw.contains("responsestreamdisconnected")
        || raw.contains("responsetoomanyfailedattempts")
    {
        "Codex 连接服务商失败，请检查网络、代理与服务商可达性。".to_string()
    } else {
        "Codex 轮次失败；服务商未返回可安全展示的详细原因。".to_string()
    }
}

fn parse_codex_app_server_notification(
    session_id: &str,
    notification: &serde_json::Value,
) -> Vec<AgentEvent> {
    let Some(method) = notification
        .get("method")
        .and_then(serde_json::Value::as_str)
    else {
        return Vec::new();
    };
    let params = notification
        .get("params")
        .unwrap_or(&serde_json::Value::Null);
    match method {
        "thread/started" | "turn/started" => {
            vec![codex_turn_stage(session_id, TurnStage::WaitingModel)]
        }
        "item/agentMessage/delta" => params
            .get("delta")
            .and_then(serde_json::Value::as_str)
            .map(|text| {
                vec![AgentEvent::MessageDelta {
                    session_id: session_id.to_string(),
                    role: Role::Assistant,
                    text: text.to_string(),
                }]
            })
            .unwrap_or_default(),
        "item/reasoning/textDelta" | "item/reasoning/summaryTextDelta" => params
            .get("delta")
            .and_then(serde_json::Value::as_str)
            .map(|text| {
                vec![AgentEvent::ThinkingDelta {
                    session_id: session_id.to_string(),
                    text: text.to_string(),
                }]
            })
            .unwrap_or_default(),
        "item/started" => params
            .get("item")
            .map(|item| codex_app_server_started_item(session_id, item))
            .unwrap_or_default(),
        "item/completed" => params
            .get("item")
            .map(|item| codex_app_server_completed_item(session_id, item))
            .unwrap_or_default(),
        "thread/tokenUsage/updated" => {
            let usage = params.pointer("/tokenUsage/last");
            let input_tokens = usage
                .and_then(|value| value.get("inputTokens"))
                .and_then(serde_json::Value::as_u64);
            let output_tokens = usage
                .and_then(|value| value.get("outputTokens"))
                .and_then(serde_json::Value::as_u64);
            let cached_input_tokens = usage
                .and_then(|value| value.get("cachedInputTokens"))
                .and_then(serde_json::Value::as_u64);
            match (input_tokens, output_tokens) {
                (Some(input_tokens), Some(output_tokens)) => {
                    let context_window = params
                        .pointer("/tokenUsage/modelContextWindow")
                        .and_then(serde_json::Value::as_u64);
                    vec![
                        AgentEvent::ContextUsage {
                            session_id: session_id.to_string(),
                            // Codex app-server 的 inputTokens 已包含 cachedInputTokens。
                            context_tokens: input_tokens,
                            context_window,
                        },
                        AgentEvent::TokenUsage {
                            session_id: session_id.to_string(),
                            input_tokens,
                            cached_input_tokens,
                            cache_write_input_tokens: None,
                            output_tokens,
                            cost_usd: 0.0,
                            service_tier: None,
                            context_window,
                        },
                    ]
                }
                _ => Vec::new(),
            }
        }
        "turn/completed" => {
            let status = params
                .pointer("/turn/status")
                .and_then(serde_json::Value::as_str);
            let stop_reason = match status {
                Some("interrupted") => StopReason::Interrupted,
                Some("failed") => StopReason::Error,
                _ => StopReason::End,
            };
            let mut events = Vec::new();
            if status == Some("failed") {
                let message = codex_app_server_failure_message(notification);
                events.push(AgentEvent::Error {
                    session_id: Some(session_id.to_string()),
                    kind: classify_error(&message),
                    message,
                    recoverable: true,
                });
            }
            events.push(AgentEvent::TurnComplete {
                session_id: session_id.to_string(),
                stop_reason,
            });
            events
        }
        _ => Vec::new(),
    }
}

fn codex_app_server_started_item(session_id: &str, item: &serde_json::Value) -> Vec<AgentEvent> {
    match item.get("type").and_then(serde_json::Value::as_str) {
        Some("reasoning") => vec![codex_turn_stage(session_id, TurnStage::Reasoning)],
        Some("agentMessage") => vec![codex_turn_stage(session_id, TurnStage::Responding)],
        Some("commandExecution") => {
            let mut events = vec![codex_turn_stage(session_id, TurnStage::UsingTool)];
            if let Some(id) = codex_item_id(item) {
                events.push(AgentEvent::ToolCall {
                    session_id: session_id.to_string(),
                    id,
                    name: "Bash".to_string(),
                    input: serde_json::json!({
                        "command": item.get("command").cloned().unwrap_or_default(),
                        "cwd": item.get("cwd").cloned().unwrap_or_default(),
                    }),
                    status: CallStatus::Pending,
                });
            }
            events
        }
        Some("fileChange") => {
            let mut events = vec![codex_turn_stage(session_id, TurnStage::UsingTool)];
            if let Some(id) = codex_item_id(item) {
                let paths = item
                    .get("changes")
                    .and_then(serde_json::Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|change| change.get("path").and_then(serde_json::Value::as_str))
                    .collect::<Vec<_>>();
                events.push(AgentEvent::ToolCall {
                    session_id: session_id.to_string(),
                    id,
                    name: "Write".to_string(),
                    input: serde_json::json!({"paths": paths}),
                    status: CallStatus::Pending,
                });
            }
            events
        }
        _ => Vec::new(),
    }
}

fn codex_app_server_completed_item(session_id: &str, item: &serde_json::Value) -> Vec<AgentEvent> {
    match item.get("type").and_then(serde_json::Value::as_str) {
        Some("agentMessage") => item
            .get("text")
            .and_then(serde_json::Value::as_str)
            .map(|text| {
                vec![AgentEvent::MessageComplete {
                    session_id: session_id.to_string(),
                    role: Role::Assistant,
                    text: text.to_string(),
                }]
            })
            .unwrap_or_default(),
        Some("reasoning") => {
            let text = item
                .get("summary")
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(serde_json::Value::as_str)
                .chain(
                    item.get("content")
                        .and_then(serde_json::Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter_map(serde_json::Value::as_str),
                )
                .collect::<Vec<_>>()
                .join("\n");
            (!text.is_empty())
                .then(|| {
                    vec![AgentEvent::ThinkingComplete {
                        session_id: session_id.to_string(),
                        text,
                    }]
                })
                .unwrap_or_default()
        }
        Some("commandExecution") => {
            if item
                .get("aggregatedOutput")
                .is_none_or(serde_json::Value::is_null)
                && item.get("exitCode").is_none_or(serde_json::Value::is_null)
                && item
                    .get("durationMs")
                    .is_none_or(serde_json::Value::is_null)
            {
                return Vec::new();
            }
            let Some(id) = codex_item_id(item) else {
                return Vec::new();
            };
            let failed = matches!(
                item.get("status").and_then(serde_json::Value::as_str),
                Some("failed" | "declined")
            );
            let output = item
                .get("aggregatedOutput")
                .and_then(serde_json::Value::as_str)
                .map(ToString::to_string);
            vec![AgentEvent::ToolResult {
                session_id: session_id.to_string(),
                id,
                status: if failed {
                    ToolStatus::Error
                } else {
                    ToolStatus::Success
                },
                output: output.clone(),
                diff: None,
                outcome: Some(if failed {
                    crate::protocol::ToolOutcomeKind::ToolFailed
                } else {
                    crate::protocol::ToolOutcomeKind::ToolSucceeded
                }),
                started: Some(true),
                has_output: Some(output.as_deref().is_some_and(|value| !value.is_empty())),
                retryable: Some(false),
                denial_source: if failed {
                    Some(crate::protocol::ToolDenialSource::Tool)
                } else {
                    None
                },
                native_denial_code: None,
            }]
        }
        Some("fileChange") => {
            let Some(id) = codex_item_id(item) else {
                return Vec::new();
            };
            let failed = matches!(
                item.get("status").and_then(serde_json::Value::as_str),
                Some("failed" | "declined")
            );
            vec![AgentEvent::ToolResult {
                session_id: session_id.to_string(),
                id,
                status: if failed {
                    ToolStatus::Error
                } else {
                    ToolStatus::Success
                },
                output: item
                    .get("changes")
                    .and_then(|changes| serde_json::to_string(changes).ok()),
                diff: None,
                outcome: Some(if failed {
                    crate::protocol::ToolOutcomeKind::ToolFailed
                } else {
                    crate::protocol::ToolOutcomeKind::ToolSucceeded
                }),
                started: Some(true),
                has_output: Some(item.get("changes").is_some()),
                retryable: Some(false),
                denial_source: if failed {
                    Some(crate::protocol::ToolDenialSource::Tool)
                } else {
                    None
                },
                native_denial_code: None,
            }]
        }
        _ => Vec::new(),
    }
}

fn parse_codex_line(session_id: &str, raw: &str) -> Vec<AgentEvent> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(raw.trim()) else {
        return Vec::new();
    };
    let Some(kind) = value.get("type").and_then(serde_json::Value::as_str) else {
        return Vec::new();
    };

    match kind {
        "thread.started" | "turn.started" => {
            vec![codex_turn_stage(session_id, TurnStage::WaitingModel)]
        }
        "item.started" => value
            .get("item")
            .map(|item| codex_events_from_started_item(session_id, item))
            .unwrap_or_default(),
        "item.completed" => value
            .get("item")
            .map(|item| codex_events_from_completed_item(session_id, item))
            .unwrap_or_default(),
        "plan_update" => codex_plan_update_from_value(session_id, &value)
            .into_iter()
            .collect(),
        "turn.completed" => {
            let mut events = Vec::new();
            if let Some((input_tokens, output_tokens, cost_usd)) = codex_usage_from_value(&value) {
                events.push(AgentEvent::TokenUsage {
                    session_id: session_id.to_string(),
                    input_tokens,
                    cached_input_tokens: None,
                    cache_write_input_tokens: None,
                    output_tokens,
                    cost_usd,
                    service_tier: None,
                    context_window: None,
                });
            }
            events.push(AgentEvent::TurnComplete {
                session_id: session_id.to_string(),
                stop_reason: StopReason::End,
            });
            events
        }
        "turn.failed" => {
            let message = value
                .get("error")
                .and_then(|error| error.get("message"))
                .and_then(serde_json::Value::as_str)
                .or_else(|| value.get("message").and_then(serde_json::Value::as_str))
                .unwrap_or("codex 轮次失败")
                .to_string();
            vec![
                AgentEvent::Error {
                    session_id: Some(session_id.to_string()),
                    kind: classify_error(&message),
                    message,
                    recoverable: false,
                },
                AgentEvent::TurnComplete {
                    session_id: session_id.to_string(),
                    stop_reason: StopReason::Error,
                },
            ]
        }
        _ => Vec::new(),
    }
}

/// 仅供 debug/test 构建中的跨 crate 契约测试调用；release 不暴露解析入口。
#[cfg(debug_assertions)]
#[doc(hidden)]
pub fn parse_codex_line_for_contract(session_id: &str, raw: &str) -> Vec<AgentEvent> {
    parse_codex_line(session_id, raw)
}

fn codex_events_from_completed_item(session_id: &str, item: &serde_json::Value) -> Vec<AgentEvent> {
    match item.get("type").and_then(serde_json::Value::as_str) {
        Some("agent_message") | Some("message") => codex_text_from_item(item)
            .map(|text| {
                vec![AgentEvent::MessageComplete {
                    session_id: session_id.to_string(),
                    role: Role::Assistant,
                    text,
                }]
            })
            .unwrap_or_default(),
        Some("reasoning") | Some("reasoning_summary") => codex_reasoning_text(item)
            .map(|text| {
                vec![AgentEvent::ThinkingComplete {
                    session_id: session_id.to_string(),
                    text,
                }]
            })
            .unwrap_or_default(),
        Some("tool_call") | Some("function_call") => Vec::new(),
        Some("tool_call_output") | Some("function_call_output") => {
            codex_tool_result_from_item(session_id, item)
                .into_iter()
                .collect()
        }
        Some("plan") => codex_plan_update_from_value(session_id, item)
            .into_iter()
            .collect(),
        _ => Vec::new(),
    }
}

fn codex_events_from_started_item(session_id: &str, item: &serde_json::Value) -> Vec<AgentEvent> {
    match item.get("type").and_then(serde_json::Value::as_str) {
        Some("reasoning") | Some("reasoning_summary") => {
            vec![codex_turn_stage(session_id, TurnStage::Reasoning)]
        }
        Some("agent_message") | Some("message") => {
            vec![codex_turn_stage(session_id, TurnStage::Responding)]
        }
        Some("tool_call") | Some("function_call") => codex_tool_call_from_item(session_id, item)
            .into_iter()
            .collect(),
        _ => Vec::new(),
    }
}

fn codex_turn_stage(session_id: &str, stage: TurnStage) -> AgentEvent {
    AgentEvent::TurnStage {
        session_id: session_id.to_string(),
        stage,
        ts: now_millis(),
        engine_reported_ttft_ms: None,
        retry_attempt: None,
    }
}

fn codex_tool_call_from_item(session_id: &str, item: &serde_json::Value) -> Option<AgentEvent> {
    let id = codex_item_id(item)?;
    let name = item
        .get("name")
        .or_else(|| item.get("tool_name"))
        .or_else(|| {
            item.get("function")
                .and_then(|function| function.get("name"))
        })
        .and_then(serde_json::Value::as_str)
        .unwrap_or("Tool");
    let input = item
        .get("arguments")
        .or_else(|| item.get("input"))
        .or_else(|| item.get("params"))
        .cloned()
        .unwrap_or(serde_json::Value::Object(Default::default()));
    let input = if let Some(text) = input.as_str() {
        serde_json::from_str(text).unwrap_or_else(|_| serde_json::Value::String(text.to_string()))
    } else {
        input
    };

    Some(AgentEvent::ToolCall {
        session_id: session_id.to_string(),
        id,
        name: normalize_codex_tool_name(name).to_string(),
        input,
        status: CallStatus::Pending,
    })
}

fn codex_tool_result_from_item(session_id: &str, item: &serde_json::Value) -> Option<AgentEvent> {
    let id = item
        .get("call_id")
        .or_else(|| item.get("tool_call_id"))
        .and_then(serde_json::Value::as_str)
        .map(ToString::to_string)
        .or_else(|| codex_item_id(item))?;
    let output = item
        .get("output")
        .or_else(|| item.get("content"))
        .or_else(|| item.get("text"))
        .and_then(|value| {
            value
                .as_str()
                .map(ToString::to_string)
                .or_else(|| serde_json::to_string(value).ok())
        });
    let diff = item
        .get("diff")
        .and_then(|value| serde_json::from_value::<Diff>(value.clone()).ok())
        .or_else(|| output.as_deref().and_then(parse_unified_diff));

    Some(AgentEvent::ToolResult {
        session_id: session_id.to_string(),
        id,
        status: if item.get("is_error").and_then(serde_json::Value::as_bool) == Some(true) {
            ToolStatus::Error
        } else {
            ToolStatus::Success
        },
        output: output.clone(),
        diff,
        outcome: Some(
            if item.get("is_error").and_then(serde_json::Value::as_bool) == Some(true) {
                crate::protocol::ToolOutcomeKind::ToolFailed
            } else {
                crate::protocol::ToolOutcomeKind::ToolSucceeded
            },
        ),
        started: Some(true),
        has_output: Some(output.as_deref().is_some_and(|value| !value.is_empty())),
        retryable: Some(false),
        denial_source: if item.get("is_error").and_then(serde_json::Value::as_bool) == Some(true) {
            Some(crate::protocol::ToolDenialSource::Tool)
        } else {
            None
        },
        native_denial_code: None,
    })
}

fn codex_plan_update_from_value(session_id: &str, value: &serde_json::Value) -> Option<AgentEvent> {
    let steps_value = value
        .get("steps")
        .or_else(|| value.get("plan"))
        .and_then(serde_json::Value::as_array)?;
    let steps = steps_value
        .iter()
        .filter_map(|step| {
            let text = step
                .get("text")
                .or_else(|| step.get("step"))
                .and_then(serde_json::Value::as_str)?;
            let status = step
                .get("status")
                .and_then(serde_json::Value::as_str)
                .map(codex_plan_status)
                .unwrap_or(PlanStatus::Pending);
            Some(PlanStep {
                text: text.to_string(),
                status,
            })
        })
        .collect::<Vec<_>>();
    (!steps.is_empty()).then(|| AgentEvent::PlanUpdate {
        session_id: session_id.to_string(),
        steps,
    })
}

fn codex_plan_status(status: &str) -> PlanStatus {
    match status {
        "done" | "completed" | "complete" => PlanStatus::Done,
        "active" | "in_progress" | "doing" => PlanStatus::Active,
        _ => PlanStatus::Pending,
    }
}

fn codex_item_id(item: &serde_json::Value) -> Option<String> {
    item.get("call_id")
        .or_else(|| item.get("id"))
        .and_then(serde_json::Value::as_str)
        .map(ToString::to_string)
}

fn normalize_codex_tool_name(name: &str) -> &str {
    match name {
        "shell" | "exec" | "exec_command" => "Bash",
        other => other,
    }
}

fn codex_reasoning_text(item: &serde_json::Value) -> Option<String> {
    item.get("text")
        .and_then(serde_json::Value::as_str)
        .map(ToString::to_string)
        .or_else(|| {
            item.get("summary")
                .and_then(serde_json::Value::as_array)
                .map(|parts| {
                    parts
                        .iter()
                        .filter_map(|part| {
                            part.get("text")
                                .and_then(serde_json::Value::as_str)
                                .or_else(|| part.as_str())
                        })
                        .collect::<Vec<_>>()
                        .join("")
                })
                .filter(|text| !text.is_empty())
        })
}

fn parse_unified_diff(text: &str) -> Option<Diff> {
    let mut path = String::new();
    let mut hunks: Vec<DiffHunk> = Vec::new();
    let mut current: Option<DiffHunk> = None;

    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("+++ ") {
            path = rest.strip_prefix("b/").unwrap_or(rest).trim().to_string();
            continue;
        }
        if line.starts_with("@@ ") {
            if let Some(hunk) = current.take() {
                if !hunk.lines.is_empty() {
                    hunks.push(hunk);
                }
            }
            let (old_start, new_start) = parse_unified_hunk_header(line)?;
            current = Some(DiffHunk {
                old_start,
                new_start,
                lines: Vec::new(),
            });
            continue;
        }
        let Some(hunk) = current.as_mut() else {
            continue;
        };
        if line.starts_with("--- ") || line.starts_with("+++ ") {
            continue;
        }
        if let Some(text) = line.strip_prefix('+') {
            hunk.lines.push(DiffLine {
                kind: DiffKind::Add,
                text: text.to_string(),
            });
        } else if let Some(text) = line.strip_prefix('-') {
            hunk.lines.push(DiffLine {
                kind: DiffKind::Del,
                text: text.to_string(),
            });
        } else if let Some(text) = line.strip_prefix(' ') {
            hunk.lines.push(DiffLine {
                kind: DiffKind::Ctx,
                text: text.to_string(),
            });
        }
    }

    if let Some(hunk) = current.take() {
        if !hunk.lines.is_empty() {
            hunks.push(hunk);
        }
    }

    if path.is_empty() || hunks.is_empty() {
        return None;
    }
    Some(Diff { path, hunks })
}

fn parse_unified_hunk_header(header: &str) -> Option<(u32, u32)> {
    let mut parts = header.split_whitespace();
    parts.next()?;
    let old = parts.next()?.trim_start_matches('-');
    let new = parts.next()?.trim_start_matches('+');
    Some((
        old.split(',').next()?.parse().ok()?,
        new.split(',').next()?.parse().ok()?,
    ))
}

fn codex_usage_from_value(value: &serde_json::Value) -> Option<(u64, u64, f64)> {
    let usage = value
        .get("usage")
        .or_else(|| value.get("token_usage"))
        .or_else(|| value.get("tokenUsage"))?;
    let input_tokens = usage_u64(usage, &["input_tokens", "inputTokens", "prompt_tokens"])?;
    let output_tokens = usage_u64(
        usage,
        &["output_tokens", "outputTokens", "completion_tokens"],
    )?;
    let cost_usd = usage_f64(usage, &["cost_usd", "costUsd"])
        .or_else(|| usage_f64(value, &["total_cost_usd", "cost_usd", "costUsd"]))
        .unwrap_or(0.0);
    Some((input_tokens, output_tokens, cost_usd))
}

fn usage_u64(value: &serde_json::Value, keys: &[&str]) -> Option<u64> {
    keys.iter()
        .find_map(|key| value.get(key).and_then(serde_json::Value::as_u64))
}

fn usage_f64(value: &serde_json::Value, keys: &[&str]) -> Option<f64> {
    keys.iter()
        .find_map(|key| value.get(key).and_then(serde_json::Value::as_f64))
}

fn codex_text_from_item(item: &serde_json::Value) -> Option<String> {
    match item.get("type").and_then(|kind| kind.as_str()) {
        Some("agent_message") => item
            .get("text")
            .and_then(|message| message.as_str())
            .map(ToString::to_string),
        Some("message") => {
            let content = item.get("content")?.as_array()?;
            let parts = content
                .iter()
                .filter_map(|part| part.get("text").and_then(|text| text.as_str()))
                .collect::<Vec<_>>();
            (!parts.is_empty()).then(|| parts.join(""))
        }
        _ => None,
    }
}

#[cfg(test)]
mod turn_stage_tests {
    use super::{
        codex_app_server_terminal_outcome, codex_notification_native_turn_id,
        parse_codex_app_server_notification, parse_codex_line, record_codex_terminal_once,
        CodexTurnContextIndex,
    };
    use std::collections::HashMap;

    fn serialized_events(raw: serde_json::Value) -> Vec<serde_json::Value> {
        parse_codex_line("codex-session", &raw.to_string())
            .into_iter()
            .map(|event| serde_json::to_value(event).unwrap())
            .collect()
    }

    #[test]
    fn native_codex_turn_ids_resolve_to_the_frozen_helm_turn_context() {
        let mut contexts = CodexTurnContextIndex::default();
        contexts.insert("native-1".into(), "helm-1".into(), 7);
        contexts.insert("native-2".into(), "helm-2".into(), 8);
        assert_eq!(contexts.resolve("native-1"), Some(("helm-1".into(), 7)));
        assert_eq!(contexts.resolve("native-2"), Some(("helm-2".into(), 8)));
        assert_eq!(contexts.resolve("unknown"), None);
        assert_eq!(
            codex_notification_native_turn_id(&serde_json::json!({
                "method": "item/started",
                "params": {"turnId": "native-1"}
            })),
            Some("native-1")
        );
        assert_eq!(
            codex_notification_native_turn_id(&serde_json::json!({
                "method": "turn/completed",
                "params": {"turn": {"id": "native-2"}}
            })),
            Some("native-2")
        );
    }

    #[test]
    fn codex_failed_turn_classifies_structured_details_without_exposing_them() {
        let notification = serde_json::json!({"method":"turn/completed","params":{"turn":{
        "id":"turn-2","status":"failed","error":{
            "message":"request rejected",
            "additionalDetails":"model gpt-private is unavailable at https://secret.example",
            "codexErrorInfo":"badRequest"
        }}}});
        let events = parse_codex_app_server_notification("codex-session", &notification);
        let error = serde_json::to_value(&events[0]).unwrap();
        assert_eq!(error["kind"], "model_unavailable");
        assert!(error["message"].as_str().unwrap().contains("模型不存在"));
        assert!(!error["message"]
            .as_str()
            .unwrap()
            .contains("secret.example"));
        assert!(!error["message"].as_str().unwrap().contains("gpt-private"));
    }

    #[test]
    fn codex_failed_turn_classifies_structured_usage_limit() {
        let notification = serde_json::json!({"method":"turn/completed","params":{"turn":{
        "id":"turn-3","status":"failed","error":{
            "message":"request rejected","codexErrorInfo":"usageLimitExceeded"
        }}}});
        let events = parse_codex_app_server_notification("codex-session", &notification);
        let error = serde_json::to_value(&events[0]).unwrap();
        assert!(error["message"].as_str().unwrap().contains("用量已达上限"));
    }

    #[test]
    fn codex_thread_and_turn_started_map_to_waiting_model() {
        for raw in [
            serde_json::json!({ "type": "thread.started", "thread_id": "thread-1" }),
            serde_json::json!({ "type": "turn.started" }),
        ] {
            let events = serialized_events(raw);
            assert_eq!(events.len(), 1);
            assert_eq!(events[0]["type"], "turn_stage");
            assert_eq!(events[0]["sessionId"], "codex-session");
            assert_eq!(events[0]["stage"], "waiting_model");
        }
    }

    #[test]
    fn codex_failed_app_server_turn_emits_safe_error_and_records_one_failed_terminal() {
        let notification = serde_json::json!({
            "method":"turn/completed",
            "params":{
                "threadId":"thread-1",
                "turn":{
                    "id":"turn-1",
                    "status":"failed",
                    "error":{
                        "message":"model unavailable at https://secret.example/v1 request_id=req-secret Authorization: Bearer secret"
                    },
                    "items":[]
                }
            }
        });
        let events = parse_codex_app_server_notification("codex-session", &notification);
        assert_eq!(events.len(), 2);
        let serialized = events
            .iter()
            .map(|event| serde_json::to_value(event).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(serialized[0]["type"], "error");
        assert_eq!(serialized[0]["kind"], "model_unavailable");
        assert_eq!(serialized[1]["type"], "turn_complete");
        assert_eq!(serialized[1]["stopReason"], "error");
        let safe_message = serialized[0]["message"].as_str().unwrap();
        assert!(!safe_message.contains("secret.example"));
        assert!(!safe_message.contains("req-secret"));
        assert!(!safe_message.contains("Bearer"));

        let (turn_id, outcome) = codex_app_server_terminal_outcome(&notification).unwrap();
        assert!(outcome.is_err());
        let mut terminal_turns = HashMap::new();
        assert!(record_codex_terminal_once(
            &mut terminal_turns,
            turn_id.clone(),
            outcome.clone()
        ));
        assert!(!record_codex_terminal_once(
            &mut terminal_turns,
            turn_id,
            outcome
        ));
        assert_eq!(terminal_turns.len(), 1);
        assert!(terminal_turns["turn-1"].is_err());
    }

    #[test]
    fn codex_successful_turn_with_unfinished_tool_is_not_accepted() {
        let notification = serde_json::json!({
            "method": "turn/completed",
            "params": {"turn": {"id": "turn-1", "status": "completed"}}
        });
        let (_, outcome) = super::codex_app_server_terminal_outcome_with_pending(&notification, 1)
            .expect("turn completion should be recognized");
        let error = outcome.expect_err("unfinished tool must fail closed");
        assert!(error.starts_with(super::CODEX_TURN_FAILED_PREFIX));
        assert!(error.contains("unfinished tool"));
        let adjusted =
            super::codex_notification_for_terminal_outcome(&notification, Some(&Err(error)));
        let events = parse_codex_app_server_notification("codex-session", &adjusted)
            .into_iter()
            .map(|event| serde_json::to_value(event).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(events[0]["type"], "error");
        assert!(events[0]["message"]
            .as_str()
            .unwrap()
            .contains("工具执行未返回结果"));
        assert_eq!(events[1]["type"], "turn_complete");
        assert_eq!(events[1]["stopReason"], "error");
    }

    #[test]
    fn codex_reasoning_and_message_item_started_map_to_truthful_stages() {
        let reasoning = serialized_events(serde_json::json!({
            "type": "item.started",
            "item": { "id": "reasoning-1", "type": "reasoning" }
        }));
        assert_eq!(reasoning.len(), 1);
        assert_eq!(reasoning[0]["type"], "turn_stage");
        assert_eq!(reasoning[0]["stage"], "reasoning");

        let responding = serialized_events(serde_json::json!({
            "type": "item.started",
            "item": { "id": "message-1", "type": "agent_message" }
        }));
        assert_eq!(responding.len(), 1);
        assert_eq!(responding[0]["type"], "turn_stage");
        assert_eq!(responding[0]["stage"], "responding");
    }

    #[test]
    fn codex_completed_tool_item_does_not_duplicate_started_tool_call() {
        let item = serde_json::json!({
            "id": "call-1",
            "type": "tool_call",
            "name": "shell",
            "arguments": { "command": "pwd" }
        });
        let mut events = serialized_events(serde_json::json!({
            "type": "item.started",
            "item": item.clone()
        }));
        events.extend(serialized_events(serde_json::json!({
            "type": "item.completed",
            "item": item
        })));

        assert_eq!(
            events
                .iter()
                .filter(|event| event["type"] == "tool_call")
                .count(),
            1
        );
    }

    #[test]
    fn app_server_notifications_map_messages_tools_files_and_turn_terminal_state() {
        let delta = parse_codex_app_server_notification(
            "codex-session",
            &serde_json::json!({
                "method":"item/agentMessage/delta",
                "params":{"threadId":"thread-1","turnId":"turn-1","itemId":"msg-1","delta":"你好"}
            }),
        );
        assert!(matches!(
            &delta[0],
            super::AgentEvent::MessageDelta { text, .. } if text == "你好"
        ));

        let command = parse_codex_app_server_notification(
            "codex-session",
            &serde_json::json!({
                "method":"item/started",
                "params":{"threadId":"thread-1","turnId":"turn-1","startedAtMs":1,"item":{
                    "id":"cmd-1","type":"commandExecution","command":"cargo test","cwd":"D:/repo",
                    "commandActions":[],"status":"inProgress"
                }}
            }),
        );
        assert!(command.iter().any(|event| matches!(
            event,
            super::AgentEvent::ToolCall { id, name, .. } if id == "cmd-1" && name == "Bash"
        )));

        let file = parse_codex_app_server_notification(
            "codex-session",
            &serde_json::json!({
                "method":"item/started",
                "params":{"threadId":"thread-1","turnId":"turn-1","startedAtMs":1,"item":{
                    "id":"file-1","type":"fileChange","status":"inProgress","changes":[
                        {"path":"src/main.rs","diff":"@@ -1 +1 @@\n-old\n+new","kind":{"type":"update"}}
                    ]
                }}
            }),
        );
        assert!(file.iter().any(|event| matches!(
            event,
            super::AgentEvent::ToolCall { id, name, input, .. }
                if id == "file-1" && name == "Write" && input["paths"][0] == "src/main.rs"
        )));

        let completed = parse_codex_app_server_notification(
            "codex-session",
            &serde_json::json!({
                "method":"turn/completed",
                "params":{"threadId":"thread-1","turn":{"id":"turn-1","status":"interrupted","items":[]}}
            }),
        );
        assert!(matches!(
            completed.last().unwrap(),
            super::AgentEvent::TurnComplete {
                stop_reason: super::StopReason::Interrupted,
                ..
            }
        ));
    }

    #[test]
    fn app_server_token_usage_uses_last_increment_instead_of_cumulative_total() {
        let events = parse_codex_app_server_notification(
            "codex-session",
            &serde_json::json!({
                "method":"thread/tokenUsage/updated",
                "params":{
                    "threadId":"thread-1",
                    "turnId":"turn-1",
                    "tokenUsage":{
                        "total":{
                            "totalTokens":29051,
                            "inputTokens":28324,
                            "cachedInputTokens":19456,
                            "outputTokens":727,
                            "reasoningOutputTokens":563
                        },
                        "last":{
                            "totalTokens":14645,
                            "inputTokens":14456,
                            "cachedInputTokens":12800,
                            "outputTokens":189,
                            "reasoningOutputTokens":153
                        },
                        "modelContextWindow":353400
                    }
                }
            }),
        );

        assert!(matches!(
            events.as_slice(),
            [
            super::AgentEvent::ContextUsage {
                session_id: context_session_id,
                context_tokens: 14456,
                context_window: Some(353400),
            },
            super::AgentEvent::TokenUsage {
                session_id,
                input_tokens: 14456,
                cached_input_tokens: Some(12800),
                output_tokens: 189,
                cost_usd,
                context_window: Some(353400),
                ..
            }] if context_session_id == "codex-session" && session_id == "codex-session" && *cost_usd == 0.0
        ));
    }

    #[test]
    fn app_server_ignores_provisional_command_completion_without_terminal_payload() {
        let provisional = parse_codex_app_server_notification(
            "codex-session",
            &serde_json::json!({
                "method":"item/completed",
                "params":{"item":{
                    "type":"commandExecution",
                    "id":"cmd-1",
                    "status":"declined",
                    "aggregatedOutput":null,
                    "exitCode":null,
                    "durationMs":null
                }}
            }),
        );
        let terminal = parse_codex_app_server_notification(
            "codex-session",
            &serde_json::json!({
                "method":"item/completed",
                "params":{"item":{
                    "type":"commandExecution",
                    "id":"cmd-1",
                    "status":"declined",
                    "aggregatedOutput":"exec command rejected by user",
                    "exitCode":-1,
                    "durationMs":0
                }}
            }),
        );

        assert!(provisional.is_empty());
        assert!(matches!(
            terminal.as_slice(),
            [super::AgentEvent::ToolResult {
                id,
                status: super::ToolStatus::Error,
                output: Some(output),
                ..
            }] if id == "cmd-1" && output == "exec command rejected by user"
        ));
    }
}
