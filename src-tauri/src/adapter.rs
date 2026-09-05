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
    AgentEvent, ApprovalDecisionOption, CallStatus, ContextCompactionStatus, Diff, DiffHunk,
    DiffKind, DiffLine, EngineId, PlanStatus, PlanStep, Role, RuntimeCapabilityAvailability,
    StopReason, ToolStatus, TurnStage,
};
use crate::reasoning::ReasoningEffort;
use crate::sessions::SessionHistoryStore;
use crate::settings::AppSettings;
use crate::util::{now_millis, sha256_hex};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::fs;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::{broadcast, mpsc, oneshot, Mutex, Notify};

const EVENT_NAME: &str = "agent-event";
const CODEX_INTERRUPT_RPC_GRACE: Duration = Duration::from_millis(500);
const CODEX_INTERRUPT_TASK_DRAIN_TIMEOUT: Duration = Duration::from_secs(1);
const RUNTIME_TOOL_STALLED_AFTER: Duration = Duration::from_secs(60);
const CODEX_SEARCH_CATALOG_FILE: &str = ".helm-model-catalog.json";
/// 落盘 Profile 目录的「静态文件全量校验通过」标记（内容=当时 revision）。
/// A 方案第二道省：标记命中即跳过 prompts/skills 455 文件的读回比对（实测 10s+）。
/// 信任边界：skills/prompts 镜像自用户自己的 ~/.codex（能改写 APPDATA 者同样能改
/// ~/.codex，不构成额外攻击面）；真正的安全敏感文件——engine-config.toml（脱敏
/// 路由）、config.toml（Runtime 校验）、清单与 auth.json（密钥检测）——仍然每次强校验。
const CODEX_PROFILE_STATIC_VERIFIED_FILE: &str = ".helm-static-verified";
const CODEX_SEARCH_CATALOG_JSON_ENV: &str = "HELM_CODEX_MODEL_CATALOG_JSON";
const CODEX_SEARCH_CATALOG_DIGEST_ENV: &str = "HELM_CODEX_MODEL_CATALOG_DIGEST";
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
    Interrupt {
        responder: Option<oneshot::Sender<Result<(), String>>>,
    },
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
    // 弹窗只提供当次允许 / 本会话(总是)允许 / 拒绝；项目与全局持久范围不再从可用集下发。
    // 底层枚举与规则构成保留，以支持既有 project/global 记录在设置页展示与撤销。
    let mut decisions = vec![ApprovalDecisionOption::Allow];
    if let Some(action) =
        action.filter(|action| crate::permissions::runtime_grant_display(action).is_some())
    {
        if !action.session_id.is_empty() {
            decisions.push(ApprovalDecisionOption::Session);
        }
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

pub fn agent_environment_from_settings(_settings: &AppSettings) -> Vec<(String, String)> {
    // 匿名分析开关已按原型移除（决策记录 §8.2.7「不提供匿名使用分析」）。
    // 始终为 CLI 注入 DO_NOT_TRACK=1 与遥测关闭，默认保护隐私。
    vec![
        ("HELM_ANONYMOUS_ANALYTICS".to_string(), "0".to_string()),
        ("DO_NOT_TRACK".to_string(), "1".to_string()),
        ("HELM_TELEMETRY_DISABLED".to_string(), "1".to_string()),
    ]
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
    /// 同引擎无损分支（十次反馈）：首轮 `--resume <源> --fork-session` 复制完整历史；
    /// CLI init 事件回报新 session id 时置回 false，此后轮次回归普通 resume。
    /// 首轮失败保持 true——用户重发即重试分支语义。
    pending_native_branch: Mutex<bool>,
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
    /// 同引擎无损分支待 fork 的源线程 id（分支首轮专用）；首轮 fork 成功后清空，后续轮回到普通 resume。
    fork_source_thread_id: Arc<std::sync::Mutex<Option<String>>>,
    /// 切点分叉：thread/fork 的 lastTurnId（被点回答所属的 Codex 原生轮 id）；
    /// 首轮 fork 成功后与源线程 id 一同清空。None 表示整段分叉。
    fork_last_turn_id: Arc<std::sync::Mutex<Option<String>>>,
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
    tool_item_facts: Arc<Mutex<HashMap<String, PendingCodexTool>>>,
    /// 通知循环向当前 Turn 等待者广播完成或协议错误。
    turn_completions: broadcast::Sender<Result<String, String>>,
    /// 当前 Turn 的任务句柄。Stop 在 Runtime 无法及时排空时必须取消它，
    /// 否则任务内持有的 RAII 资源不会释放、busy 也不会清除。
    turn_task: Arc<std::sync::Mutex<Option<tauri::async_runtime::JoinHandle<()>>>>,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PendingCodexToolStage {
    WaitingApproval,
    Executing,
    WaitingResult,
}

impl PendingCodexToolStage {
    fn as_str(self) -> &'static str {
        match self {
            Self::WaitingApproval => "waiting_approval",
            Self::Executing => "executing",
            Self::WaitingResult => "waiting_result",
        }
    }
}

#[derive(Debug, Clone)]
struct PendingCodexTool {
    queued_at: i64,
    started_at: i64,
    last_progress_at: i64,
    ended_at: Option<i64>,
    stage: PendingCodexToolStage,
}

fn pending_codex_tool_is_stalled(
    tool: &PendingCodexTool,
    now: i64,
    stalled_after: Duration,
) -> bool {
    tool.ended_at.is_none()
        && now.saturating_sub(tool.last_progress_at) >= stalled_after.as_millis() as i64
}

fn stalled_codex_tool_kind(tools: &HashMap<String, PendingCodexTool>) -> Option<&'static str> {
    let stalled: Vec<_> = tools
        .values()
        .filter(|tool| {
            tool.ended_at.is_none() && tool.stage == PendingCodexToolStage::WaitingApproval
        })
        .collect();
    if stalled.is_empty() {
        return None;
    }
    Some("waiting_approval")
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct CodexRuntimeProfile {
    path: PathBuf,
    revision: String,
}

async fn abort_codex_turn_task(
    turn_task: &std::sync::Mutex<Option<tauri::async_runtime::JoinHandle<()>>>,
    busy: &AtomicBool,
    timeout: Duration,
) -> Result<(), String> {
    let task = turn_task
        .lock()
        .map_err(|_| "Codex Turn 任务锁中毒".to_string())?
        .take()
        .ok_or_else(|| {
            "[codex_interrupt_cleanup_timeout] Codex Stop 后轮次任务仍 busy，但任务句柄缺失"
                .to_string()
        })?;
    task.abort();
    let _ = tokio::time::timeout(timeout, task).await.map_err(|_| {
        "[codex_interrupt_cleanup_timeout] Codex Stop 取消轮次任务后仍未完成回收".to_string()
    })?;
    busy.store(false, Ordering::Release);
    Ok(())
}

/// 源 EngineProfile 文件树的 stat 指纹（路径+大小+mtime_ns 聚合哈希），
/// 命中即代表 `~/.codex` 下的 config/prompts/skills 内容未变，可直接复用上次
/// 读出的文件集合，跳过全量读+逐文件 SHA-256（15MB / 439 文件，实测约 5~10s）。
#[derive(Clone)]
struct ProfileFilesCacheEntry {
    fingerprint: String,
    files: Vec<(PathBuf, Vec<u8>)>,
}

/// Helm-owned Codex API EngineProfile。目录只保存经过过滤的配置、扩展快照和
/// Codex 自己生成的 sandbox 状态；Provider 凭据始终只通过子进程环境传入。
#[derive(Clone)]
pub struct CodexRuntimeProfileStore {
    root: PathBuf,
    history: SessionHistoryStore,
    profile_lock: Arc<std::sync::Mutex<()>>,
    authorized_workspaces: Arc<std::sync::Mutex<HashMap<String, BTreeSet<String>>>>,
    /// 源文件树指纹缓存，键为「source 目录 + disabled_mcp」。
    files_cache: Arc<std::sync::Mutex<HashMap<String, ProfileFilesCacheEntry>>>,
    /// 本进程内正被活跃 Runtime 使用的 Profile 目录，GC 时绝不回收。
    active_profiles: Arc<std::sync::Mutex<HashSet<PathBuf>>>,
}

impl CodexRuntimeProfileStore {
    pub fn new(app_config_dir: PathBuf, history: SessionHistoryStore) -> Self {
        Self {
            root: app_config_dir.join("cli-profiles").join("codex-runtime"),
            history,
            profile_lock: Arc::new(std::sync::Mutex::new(())),
            authorized_workspaces: Arc::new(std::sync::Mutex::new(HashMap::new())),
            files_cache: Arc::new(std::sync::Mutex::new(HashMap::new())),
            active_profiles: Arc::new(std::sync::Mutex::new(HashSet::new())),
        }
    }

    fn api_profile(
        &self,
        env: &[(String, String)],
        disabled_mcp: &[String],
        workspace_root: &Path,
    ) -> Result<Option<CodexRuntimeProfile>, String> {
        if !env
            .iter()
            .any(|(key, value)| key == "OPENAI_API_KEY" && !value.trim().is_empty())
        {
            return Ok(None);
        }
        let source = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .ok()
            .map(|home| PathBuf::from(home).join(".codex"));
        let search_catalog = codex_search_catalog_bytes(env)?;
        self.api_profile_with_source_and_catalog(
            source.as_deref(),
            disabled_mcp,
            Some(workspace_root),
            search_catalog.as_deref(),
        )
        .map(Some)
    }

    #[cfg(test)]
    fn api_profile_with_source(
        &self,
        source: Option<&Path>,
        disabled_mcp: &[String],
        workspace_root: Option<&Path>,
    ) -> Result<CodexRuntimeProfile, String> {
        self.api_profile_with_source_and_catalog(source, disabled_mcp, workspace_root, None)
    }

    fn api_profile_with_source_and_catalog(
        &self,
        source: Option<&Path>,
        disabled_mcp: &[String],
        workspace_root: Option<&Path>,
        search_catalog: Option<&[u8]>,
    ) -> Result<CodexRuntimeProfile, String> {
        // A 方案第一道省：源树（~/.codex 的 config/prompts/skills）指纹命中即复用上次
        // 读出的文件集合，跳过 439 文件全量读+逐文件 SHA-256（实测 5~10s）。
        // 指纹只 stat 元数据（毫秒级）；任一文件增删改名或改内容都会翻转指纹，
        // 因此缓存失效即回落到全量读，不会读到陈旧内容。
        let mut cache_key = source
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        cache_key.push('\u{0}');
        cache_key.push_str(&disabled_mcp.join("\u{1}"));
        // stat 指纹（毫秒级）命中即复用上次读出的文件集合（零内容读）；
        // 源树任一文件增删改都会翻转指纹，回落全量读，不会用陈旧内容算 revision。
        let fingerprint = source.map(profile_tree_fingerprint);
        let mut files = match fingerprint.as_deref() {
            Some(fp) => {
                let cached = self
                    .files_cache
                    .lock()
                    .map_err(|_| "Codex Profile 文件缓存锁中毒".to_string())?
                    .get(&cache_key)
                    .filter(|entry| entry.fingerprint == fp)
                    .map(|entry| entry.files.clone());
                match cached {
                    Some(files) => files,
                    None => {
                        let files = codex_engine_profile_files(source, disabled_mcp)?;
                        self.files_cache
                            .lock()
                            .map_err(|_| "Codex Profile 文件缓存锁中毒".to_string())?
                            .insert(
                                cache_key.clone(),
                                ProfileFilesCacheEntry {
                                    fingerprint: fp.to_string(),
                                    files: files.clone(),
                                },
                            );
                        files
                    }
                }
            }
            None => codex_engine_profile_files(source, disabled_mcp)?,
        };
        if let Some(search_catalog) = search_catalog {
            files.push((
                PathBuf::from(CODEX_SEARCH_CATALOG_FILE),
                search_catalog.to_vec(),
            ));
        }
        let engine_config = files
            .iter()
            .find(|(path, _)| path == Path::new("config.toml"))
            .map(|(_, bytes)| bytes.clone())
            .unwrap_or_default();
        let inventory = files
            .iter()
            .map(|(path, bytes)| {
                serde_json::json!({
                    "path": path.to_string_lossy().replace('\\', "/"),
                    "sha256": sha256_hex(bytes),
                })
            })
            .collect::<Vec<_>>();
        let revision = crate::turn_start::digest_json(&serde_json::json!({
            "schema": 3,
            "files": inventory,
            "disabledMcp": disabled_mcp,
        }))?;
        let directory_name = revision.trim_start_matches("sha256:");
        let canonical_path = self.root.join(directory_name);
        let _guard = self
            .profile_lock
            .lock()
            .map_err(|_| "Codex Runtime Profile 锁中毒".to_string())?;
        let (path, migrating_legacy_profile, static_trusted) = select_codex_runtime_profile_path(
            &self.root,
            &canonical_path,
            &revision,
            &engine_config,
            &files,
            disabled_mcp,
        )?;
        let workspace_key = workspace_root.map(codex_project_key).transpose()?;
        let persisted_workspaces = self
            .history
            .list_sessions()?
            .into_iter()
            .filter_map(|session| {
                let path = Path::new(&session.cwd);
                path.is_absolute()
                    .then(|| normalize_codex_project_key(path))
            })
            .collect::<BTreeSet<_>>();
        let authorized_workspaces = {
            let mut by_revision = self
                .authorized_workspaces
                .lock()
                .map_err(|_| "Codex Runtime Profile 工作区锁中毒".to_string())?;
            let workspaces = by_revision.entry(revision.clone()).or_default();
            workspaces.extend(persisted_workspaces);
            if let Some(workspace_key) = workspace_key {
                workspaces.insert(workspace_key);
            }
            workspaces.clone()
        };
        fs::create_dir_all(&path)
            .map_err(|error| format!("创建 Codex Runtime Profile 失败：{error}"))?;
        // static_trusted：候选目录 manifest revision 精确命中且静态标记一致，
        // select 已确认 engine-config.toml 脱敏路由等价——skills/prompts/目录文件
        // 此前按同一 revision 全量校验落盘，跳过 455 文件读回比对（实测省 10s+）。
        if !static_trusted {
            for (relative, bytes) in &files {
                let destination = if relative == Path::new("config.toml") {
                    path.join("engine-config.toml")
                } else {
                    path.join(relative)
                };
                if let Some(parent) = destination.parent() {
                    fs::create_dir_all(parent)
                        .map_err(|error| format!("创建 Codex Profile 继承目录失败：{error}"))?;
                }
                if destination.is_file() {
                    let existing = fs::read(&destination)
                        .map_err(|error| format!("读取既有 Codex Runtime Profile 失败：{error}"))?;
                    if existing != *bytes {
                        if migrating_legacy_profile && relative == Path::new("config.toml") {
                            fs::write(&destination, bytes).map_err(|error| {
                                format!("净化旧 Codex EngineProfile 路由失败：{error}")
                            })?;
                        } else {
                            return Err(format!(
                                "[codex_runtime_profile_tampered] Codex Runtime Profile 文件与 revision 不一致：{}",
                                relative.to_string_lossy()
                            ));
                        }
                    }
                } else {
                    fs::write(&destination, bytes)
                        .map_err(|error| format!("写入 Codex Runtime Profile 失败：{error}"))?;
                }
            }
        }
        let engine_config_path = path.join("engine-config.toml");
        if !engine_config_path.is_file() {
            fs::write(&engine_config_path, &engine_config)
                .map_err(|error| format!("写入 Codex EngineProfile 基线失败：{error}"))?;
        }
        let live_config = codex_runtime_config(&engine_config, &authorized_workspaces)?;
        let live_config_path = path.join("config.toml");
        if live_config_path.is_file() {
            let existing = fs::read(&live_config_path)
                .map_err(|error| format!("读取 Codex Runtime 配置失败：{error}"))?;
            if existing != live_config {
                if !migrating_legacy_profile {
                    validate_codex_runtime_config(&engine_config, &existing)?;
                }
                fs::write(&live_config_path, &live_config)
                    .map_err(|error| format!("刷新 Codex Runtime 工作区配置失败：{error}"))?;
            }
        } else {
            fs::write(&live_config_path, &live_config)
                .map_err(|error| format!("写入 Codex Runtime 配置失败：{error}"))?;
        }
        let manifest = path.join(".helm-runtime-profile.json");
        let manifest_bytes = serde_json::to_vec_pretty(&serde_json::json!({
            "schemaVersion": 3,
            "revision": revision,
            "containsCredentials": false,
        }))
        .map_err(|error| format!("序列化 Codex Profile 清单失败：{error}"))?;
        if manifest.is_file() {
            if fs::read(&manifest)
                .map_err(|error| format!("读取 Codex Profile 清单失败：{error}"))?
                != manifest_bytes
            {
                if migrating_legacy_profile {
                    fs::write(&manifest, &manifest_bytes)
                        .map_err(|error| format!("更新 Codex Profile 清单失败：{error}"))?;
                } else {
                    return Err(
                        "[codex_runtime_profile_tampered] Codex Runtime Profile 清单与 revision 不一致"
                            .to_string(),
                    );
                }
            }
        } else {
            fs::write(&manifest, manifest_bytes)
                .map_err(|error| format!("写入 Codex Profile 清单失败：{error}"))?;
        }
        if path.join("auth.json").exists() {
            return Err("[codex_runtime_profile_secret_detected] 持久 Runtime Profile 中出现认证文件，已阻止启动".to_string());
        }
        // 走到这里：要么本次做了全量读回校验/写入（static_trusted=false），要么
        // 命中已有标记。前者补写标记，让下次恢复免读 455 文件；后者不重复写盘。
        if !static_trusted {
            let marker = path.join(CODEX_PROFILE_STATIC_VERIFIED_FILE);
            if fs::read_to_string(&marker).ok().as_deref() != Some(revision.as_str()) {
                let _ = fs::write(&marker, revision.as_bytes());
            }
        }
        // 登记为活跃 Profile，GC 时绝不回收正在被 app-server 使用的目录。
        if let Ok(mut active) = self.active_profiles.lock() {
            active.insert(path.clone());
        }
        Ok(CodexRuntimeProfile { path, revision })
    }

    /// C 方案：回收 `codex-runtime/` 下陈旧 revision 目录（实测会堆到 656MB / 15 份）。
    /// 保留策略：本进程活跃集合永不回收；其余按目录 mtime 保留最新 `keep` 个，
    /// 超出且 mtime 早于 24 小时的才删除——新鲜目录可能是强杀残留的孤儿 app-server
    /// 正在使用，宁可多留一天也不冒险。启动后台延迟调用，不阻塞首屏。
    pub fn gc_stale_profiles(&self, keep: usize) {
        let _guard = match self.profile_lock.lock() {
            Ok(guard) => guard,
            Err(_) => return,
        };
        let Ok(entries) = fs::read_dir(&self.root) else {
            return;
        };
        let active = self
            .active_profiles
            .lock()
            .map(|set| set.clone())
            .unwrap_or_default();
        let mut dirs: Vec<(PathBuf, std::time::SystemTime)> = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            // 只认带清单的 Helm Profile 目录，跳过遗留/外来目录。
            if !path.join(".helm-runtime-profile.json").is_file() {
                continue;
            }
            if active.contains(&path) {
                continue;
            }
            let mtime = fs::metadata(&path)
                .and_then(|meta| meta.modified())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            dirs.push((path, mtime));
        }
        if dirs.len() <= keep {
            return;
        }
        dirs.sort_by_key(|(_, mtime)| std::cmp::Reverse(*mtime));
        let cutoff = std::time::SystemTime::now()
            - std::time::Duration::from_secs(24 * 60 * 60);
        for (path, mtime) in dirs.into_iter().skip(keep) {
            if mtime > cutoff {
                continue;
            }
            let _ = fs::remove_dir_all(&path);
        }
    }
}

/// 返回 `(path, migrating_legacy_profile, static_trusted)`。
/// `static_trusted = true` 表示候选目录的 manifest revision 命中且静态文件校验标记
/// 与 revision 一致——prompts/skills 落盘内容此前已按同一 revision 全量校验过，
/// 主循环可跳过 455 文件的读回比对（engine-config.toml/config.toml 仍强校验）。
fn select_codex_runtime_profile_path(
    profile_root: &Path,
    canonical_path: &Path,
    revision: &str,
    engine_config: &[u8],
    files: &[(PathBuf, Vec<u8>)],
    disabled_mcp: &[String],
) -> Result<(PathBuf, bool, bool), String> {
    if !profile_root.is_dir() {
        return Ok((canonical_path.to_path_buf(), false, false));
    }
    let mut candidates = fs::read_dir(profile_root)
        .map_err(|error| format!("读取 Codex Runtime Profile 目录失败：{error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("读取 Codex Runtime Profile 条目失败：{error}"))?;
    candidates.sort_by_key(|entry| entry.file_name());
    let mut current_candidate = None;

    for candidate in candidates {
        let candidate = candidate.path();
        if !candidate.is_dir() {
            continue;
        }
        let Some(directory_name) = candidate.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let Some(manifest) = fs::read(candidate.join(".helm-runtime-profile.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        else {
            continue;
        };
        let manifest_revision = manifest.get("revision").and_then(serde_json::Value::as_str);
        let trusted_manifest = matches!(
            manifest
                .get("schemaVersion")
                .and_then(serde_json::Value::as_u64),
            Some(2 | 3)
        ) && manifest
            .get("containsCredentials")
            .and_then(serde_json::Value::as_bool)
            == Some(false)
            && manifest_revision.is_some_and(|candidate_revision| {
                candidate_revision == revision
                    || candidate_revision == format!("sha256:{directory_name}")
            });
        if !trusted_manifest {
            continue;
        }
        let candidate_engine_config_raw =
            match fs::read_to_string(candidate.join("engine-config.toml")) {
                Ok(raw) => raw,
                Err(_) => continue,
            };
        let candidate_engine_config =
            match filter_codex_engine_config(&candidate_engine_config_raw, disabled_mcp) {
                Ok(config) => config.into_bytes(),
                Err(_) => continue,
            };
        if candidate_engine_config != engine_config {
            continue;
        }
        // 快路径：manifest revision 精确命中 + 静态文件标记命中 → 免读 455 文件。
        // 标记信任仅到「路径+大小」粒度（stat，毫秒级）：文件被删/被换尺寸仍会
        // 回落到全量校验，只有同路径同大小的篡改（攻击者需已有 APPDATA 写权限，
        // 同等权限也能改 ~/.codex 源树本身）不构成额外攻击面。
        let static_marker_ok = manifest_revision == Some(revision)
            && fs::read_to_string(candidate.join(CODEX_PROFILE_STATIC_VERIFIED_FILE))
                .is_ok_and(|marker| marker == revision)
            && codex_profile_static_files_sizes_match(&candidate, files);
        if !static_marker_ok && !codex_profile_static_files_match(&candidate, files)? {
            continue;
        }
        let needs_migration = manifest_revision != Some(revision)
            || candidate_engine_config_raw.as_bytes() != engine_config;
        if manifest_revision == Some(revision) {
            current_candidate = Some((
                candidate.clone(),
                needs_migration,
                static_marker_ok && !needs_migration,
            ));
        }
    }
    Ok(current_candidate.unwrap_or_else(|| {
        (canonical_path.to_path_buf(), false, false)
    }))
}

/// 信任快路径的廉价守卫：只遍历候选目录 prompts/skills/目录文件的
/// 「相对路径+大小」（stat，不读内容），与期望集合比对。
/// 任一文件缺失、多出或改尺寸都会回落全量校验；与 revision 指纹同源，
/// 内容级篡改（改内容不改大小）由「攻击者需已有 APPDATA 写权限，同等权限
/// 可直接改 ~/.codex 源」的信任边界吸收，不另设防线。
fn codex_profile_static_files_sizes_match(candidate: &Path, files: &[(PathBuf, Vec<u8>)]) -> bool {
    let mut expected = files
        .iter()
        .filter(|(path, _)| path != Path::new("config.toml"))
        .map(|(path, bytes)| (path.to_string_lossy().replace('\\', "/"), bytes.len()))
        .collect::<Vec<_>>();
    expected.sort();
    let mut actual: Vec<(String, usize)> = Vec::new();
    let mut stack = vec![
        (candidate.join("prompts"), String::from("prompts")),
        (candidate.join("skills"), String::from("skills")),
    ];
    if let Some(bytes) = files
        .iter()
        .find(|(path, _)| path == Path::new(CODEX_SEARCH_CATALOG_FILE))
        .map(|(_, bytes)| bytes.len())
    {
        if fs::metadata(candidate.join(CODEX_SEARCH_CATALOG_FILE))
            .ok()
            .filter(|meta| meta.is_file())
            .is_some_and(|meta| meta.len() as usize == bytes)
        {
            actual.push((CODEX_SEARCH_CATALOG_FILE.to_string(), bytes));
        } else {
            return false;
        }
    } else if candidate.join(CODEX_SEARCH_CATALOG_FILE).exists() {
        return false;
    }
    while let Some((dir, prefix)) = stack.pop() {
        let Ok(read) = fs::read_dir(&dir) else {
            if prefix == "prompts" && !dir.is_dir() {
                continue; // 空 prompts 目录合法：期望集合里也不会有条目
            }
            return false;
        };
        for item in read.flatten() {
            let name = item.file_name().to_string_lossy().replace('\\', "/");
            let relative = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            };
            let Ok(meta) = item.metadata() else {
                return false;
            };
            if meta.is_dir() {
                stack.push((item.path(), relative));
            } else {
                actual.push((relative, meta.len() as usize));
            }
        }
    }
    actual.sort();
    actual == expected
}

fn codex_profile_static_files_match(
    candidate: &Path,
    files: &[(PathBuf, Vec<u8>)],
) -> Result<bool, String> {
    let expected = files
        .iter()
        .filter(|(path, _)| path != Path::new("config.toml"))
        .cloned()
        .collect::<Vec<_>>();
    let mut actual = Vec::new();
    for directory in ["prompts", "skills"] {
        let root = candidate.join(directory);
        if root.is_dir() {
            collect_profile_tree(&root, Path::new(directory), &mut actual)?;
        }
    }
    actual.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(actual == expected)
}

fn codex_project_key(workspace_root: &Path) -> Result<String, String> {
    let canonical = workspace_root
        .canonicalize()
        .map_err(|error| format!("Codex Runtime 工作目录不可用：{error}"))?;
    Ok(normalize_codex_project_key(&canonical))
}

fn normalize_codex_project_key(workspace_root: &Path) -> String {
    let path = workspace_root.to_string_lossy();
    #[cfg(target_os = "windows")]
    {
        path.strip_prefix(r"\\?\")
            .unwrap_or(&path)
            .replace('/', r"\")
            .to_lowercase()
    }
    #[cfg(not(target_os = "windows"))]
    {
        path.into_owned()
    }
}

fn codex_runtime_config(
    engine_config: &[u8],
    authorized_workspaces: &BTreeSet<String>,
) -> Result<Vec<u8>, String> {
    let mut config = if engine_config.is_empty() {
        toml::Value::Table(toml::map::Map::new())
    } else {
        toml::from_str::<toml::Value>(
            std::str::from_utf8(engine_config)
                .map_err(|error| format!("Codex EngineProfile 配置不是 UTF-8：{error}"))?,
        )
        .map_err(|error| format!("解析 Codex EngineProfile 配置失败：{error}"))?
    };
    let root = config
        .as_table_mut()
        .ok_or_else(|| "Codex EngineProfile config.toml 顶层不是表".to_string())?;
    let projects = root
        .entry("projects")
        .or_insert_with(|| toml::Value::Table(toml::map::Map::new()))
        .as_table_mut()
        .ok_or_else(|| "Codex EngineProfile projects 配置不是表".to_string())?;
    for workspace in authorized_workspaces {
        projects.entry(workspace.clone()).or_insert_with(|| {
            toml::Value::Table(toml::map::Map::from_iter([(
                "trust_level".to_string(),
                toml::Value::String("trusted".to_string()),
            )]))
        });
    }
    toml::to_string_pretty(&config)
        .map(|value| value.into_bytes())
        .map_err(|error| format!("序列化 Codex Runtime 配置失败：{error}"))
}

fn validate_codex_runtime_config(engine_config: &[u8], existing: &[u8]) -> Result<(), String> {
    let baseline = codex_runtime_config(engine_config, &BTreeSet::new())?;
    let mut baseline = toml::from_str::<toml::Value>(
        std::str::from_utf8(&baseline)
            .map_err(|error| format!("Codex Runtime 基线不是 UTF-8：{error}"))?,
    )
    .map_err(|error| format!("解析 Codex Runtime 基线失败：{error}"))?;
    let mut actual = toml::from_str::<toml::Value>(
        std::str::from_utf8(existing)
            .map_err(|error| format!("Codex Runtime 配置不是 UTF-8：{error}"))?,
    )
    .map_err(|error| {
        format!("[codex_runtime_profile_tampered] Codex Runtime config.toml 无法解析：{error}")
    })?;
    strip_codex_runtime_ui_state(&mut baseline);
    strip_codex_runtime_ui_state(&mut actual);
    let baseline_projects = baseline
        .get("projects")
        .and_then(toml::Value::as_table)
        .cloned()
        .unwrap_or_default();
    let actual_projects = actual
        .get_mut("projects")
        .and_then(toml::Value::as_table_mut)
        .ok_or_else(|| {
            "[codex_runtime_profile_tampered] Codex Runtime projects 配置不是表".to_string()
        })?;
    let trusted = toml::Value::Table(toml::map::Map::from_iter([(
        "trust_level".to_string(),
        toml::Value::String("trusted".to_string()),
    )]));
    // Codex 会补记发现过的 project；该提示状态不扩大 Helm 的 cwd/批准根。
    // 只忽略精确 trusted 新项，已有基线项和其他值仍由最终比较拒绝。
    let runtime_projects = actual_projects
        .keys()
        .filter(|workspace| !baseline_projects.contains_key(*workspace))
        .cloned()
        .collect::<Vec<_>>();
    for workspace in runtime_projects {
        if actual_projects.remove(&workspace).as_ref() != Some(&trusted) {
            return Err(format!(
                "[codex_runtime_profile_tampered] Codex Runtime 工作区信任状态非法：{}",
                sha256_hex(workspace.as_bytes())
            ));
        }
    }
    if actual != baseline {
        return Err(
            "[codex_runtime_profile_tampered] Codex Runtime config.toml 含未授权改动".to_string(),
        );
    }
    Ok(())
}

fn strip_codex_runtime_ui_state(config: &mut toml::Value) {
    let Some(root) = config.as_table_mut() else {
        return;
    };
    for (section, key) in [
        ("notice", "model_migrations"),
        ("tui", "model_availability_nux"),
    ] {
        let remove_section = root
            .get_mut(section)
            .and_then(toml::Value::as_table_mut)
            .is_some_and(|table| {
                table.remove(key);
                table.is_empty()
            });
        if remove_section {
            root.remove(section);
        }
    }
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
                .send(SessionCmd::Interrupt { responder: None })
                .map_err(|_| "会话已结束，无法中断".to_string()),
            AgentSession::Codex(session) => {
                let session = session.clone();
                spawn_agent_task(async move {
                    let _ = session.interrupt_and_wait().await;
                });
                Ok(())
            }
        }
    }

    pub async fn interrupt_and_wait(&self) -> Result<(), String> {
        match self {
            AgentSession::Claude(session) => {
                let (responder, response) = oneshot::channel();
                session
                    .tx
                    .send(SessionCmd::Interrupt {
                        responder: Some(responder),
                    })
                    .map_err(|_| "会话已结束，无法中断".to_string())?;
                response
                    .await
                    .map_err(|_| "Claude 中断协调器已结束，无法确认终态".to_string())?
            }
            AgentSession::Codex(session) => session.interrupt_and_wait().await,
        }
    }

    /// 触发引擎原生上下文压缩（变更-34/35 · B4）。
    /// 只有 Codex 提供真实 headless 契约（app-server `thread/compact/start`，2026-08-12 更正）；
    /// Claude `-p` 无 `/compact` 注入契约，返回明确错误而非伪造按钮。
    pub async fn compact_context(&self) -> Result<(), String> {
        match self {
            AgentSession::Claude(_) => Err(
                "Claude Code 当前驱动方式（claude -p）无 /compact 注入契约，无法手动压缩"
                    .to_string(),
            ),
            AgentSession::Codex(session) => session.compact_context().await,
        }
    }

    pub fn close(&self) {
        let _ = self.interrupt();
    }

    pub async fn shutdown(&self) {
        let _ = self.interrupt_and_wait().await;
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
                // API Key Session 切换到对应不可变 EngineProfile snapshot；Key 仍只在 env。
                // subscription 没有 API Runtime Profile，继续复用 Helm-owned 登录目录。
                let runtime_profile = if session
                    .env
                    .iter()
                    .any(|(key, value)| key == "OPENAI_API_KEY" && !value.trim().is_empty())
                {
                    Some(
                        session
                            .app
                            .try_state::<CodexRuntimeProfileStore>()
                            .ok_or_else(|| "Codex Runtime Profile Store 未启动".to_string())?
                            .api_profile(&session.env, &disabled, Path::new(&session.cwd))?
                            .ok_or_else(|| "Codex API Runtime Profile 未创建".to_string())?,
                    )
                } else {
                    None
                };
                let replacement_path = runtime_profile.as_ref().map(|profile| profile.path.clone());
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
                    .map_err(|_| "Codex CODEX_HOME 锁中毒".to_string())? = None;
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
            let mut probe = std::process::Command::new("where.exe");
            probe.arg(configured_bin);
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt as _;
                probe.creation_flags(CREATE_NO_WINDOW);
            }
            let output = probe
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

/// Model-only 调用（分叉摘要/旁路提问/自审）的 Claude 命令构造。
///
/// prompt 不进 argv：Windows `CreateProcess` 命令行总长约 32K 字符，长 Ledger 的
/// 交接 prompt 曾撑爆命令行导致 `[operation_spawn_failed] os error 206`。改为由
/// 调用方在 spawn 后经 [`write_model_only_prompt`] 把 prompt 全量写入 stdin——
/// `claude -p` 在没有位置 prompt 参数时从标准输入读取完整输入。该契约的依据是
/// CLI 自身的报错文案「Input must be provided through stdin or --prompt」（官方
/// headless 文档同样给出管道用法 `... | claude -p`；仓库内先例见 docs/技术方案.md
/// 「用户消息以 JSONL 写入 stdin」）。全部隔离/禁用 flag 保持不变。
pub(crate) fn build_claude_model_only_command(
    bin: &str,
    model: &str,
    env: &[(String, String)],
    cwd: &std::path::Path,
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
        .stdin(std::process::Stdio::piped())
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
            "{\"mcpServers\": {}}",
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
    // 不追加任何位置 prompt 参数：prompt 由调用方在 spawn 后经 stdin 写入（见函数注释）。
    Ok(command)
}

/// spawn 之后把 model-only prompt 全量写入子进程 stdin 并立即关闭。
///
/// 必须在 spawn 后调用（stdin 已由 `build_claude_model_only_command` 建立管道）；
/// 写入完成后立刻 drop 关闭 stdin，让 `claude -p` 读到 EOF 开始推理，避免子进程
/// 等待剩余输入而挂起。失败时由调用方回收子进程并沿用各自既有错误语义
/// （`error_tag_prefix` 取 `operation` / `side_query`，与现有错误 tag 分类一致）。
pub(crate) async fn write_model_only_prompt(
    child: &mut tokio::process::Child,
    prompt: &str,
    error_tag_prefix: &str,
) -> Result<(), String> {
    use tokio::io::AsyncWriteExt;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| format!("[{error_tag_prefix}_stdin_missing] Claude CLI stdin 未建立管道"))?;
    let written = async {
        stdin.write_all(prompt.as_bytes()).await?;
        stdin.flush().await?;
        Ok::<(), std::io::Error>(())
    }
    .await;
    // 无论写入成败都立即关闭 stdin：成功时交付 EOF；失败时不给子进程留悬挂管道。
    drop(stdin);
    written.map_err(|error| format!("[{error_tag_prefix}_stdin_write_failed] {error}"))
}

/// Windows `CreateProcess` 命令行总长硬上限约 32768 字符，超限 spawn 直接以
/// os error 206（"文件名或扩展名太长"）失败——用户看到的是无法定位的 OS 哑错。
/// 模型专用路径的 prompt 已走 stdin（见 build_claude_model_only_command 的契约注释），
/// 正常 argv 远低于上限；本预检在 spawn 前兜底：一旦未来改动把长载荷塞回 argv，
/// 立即给出带错误 tag 的可定位失败，而不是等 OS 报 206。
pub(crate) fn ensure_command_line_within_limit(
    command: &tokio::process::Command,
    error_tag_prefix: &str,
) -> Result<(), String> {
    const MAX_COMMAND_LINE_CHARS: usize = 30_000;
    let std_command = command.as_std();
    let total: usize = std_command.get_program().to_string_lossy().chars().count()
        + std_command
            .get_args()
            .map(|arg| arg.to_string_lossy().chars().count() + 1)
            .sum::<usize>();
    if total > MAX_COMMAND_LINE_CHARS {
        return Err(format!(
            "[{error_tag_prefix}_command_line_too_long] 命令行长度约 {total} 字符，超过 Windows 上限；prompt 必须经 stdin 交付，禁止回填 argv"
        ));
    }
    Ok(())
}

/// spawn 失败取证（七次反馈）：os error 206 除命令行超限外还可能来自环境块超限、
/// 程序路径或 cwd 异常——这些输入的实际大小取决于用户机器，远端无法凭代码推断，
/// 用户侧又只看得到 OS 哑错。本函数把 spawn 全部输入的规模压缩成一行有界诊断串，
/// 由调用方拼进 spawn 失败消息落库并在 UI 展示：数字直接指认超限来源；
/// `diag=v2` 标记同时证明运行中的二进制包含本轮诊断代码（旧构建不会出现该标记）。
///
/// 仅适用于先 `env_clear()` 再显式注入变量的构造路径（本项目全部 agent spawn 均
/// 经 `apply_inherited_agent_environment` 如此构造）：此时 `get_envs()` 就是子进程
/// 完整环境，stable API 也无法读回「父进程继承集减去清除」这一隐式状态。
pub(crate) fn command_spawn_forensics(command: &tokio::process::Command) -> String {
    const MAX_LISTED_VARS: usize = 3;
    const MAX_CWD_CHARS: usize = 160;
    let std_command = command.as_std();
    let program = std_command.get_program().to_string_lossy();
    let args: Vec<String> = std_command
        .get_args()
        .map(|arg| arg.to_string_lossy().to_string())
        .collect();
    let argv_chars: usize = program.chars().count()
        + args
            .iter()
            .map(|arg| arg.chars().count() + 1)
            .sum::<usize>();
    let mut env_entries: Vec<(String, usize)> = std_command
        .get_envs()
        .filter_map(|(key, value)| {
            value.map(|value| {
                (
                    key.to_string_lossy().to_string(),
                    key.to_string_lossy().chars().count()
                        + value.to_string_lossy().chars().count()
                        + 2,
                )
            })
        })
        .collect();
    env_entries.sort_by(|left, right| right.1.cmp(&left.1).then(left.0.cmp(&right.0)));
    let env_chars: usize = env_entries.iter().map(|(_, size)| size).sum();
    let top_env = env_entries
        .iter()
        .take(MAX_LISTED_VARS)
        .map(|(key, size)| format!("{key}({size})"))
        .collect::<Vec<_>>()
        .join(",");
    let cwd = std_command
        .get_current_dir()
        .map(|path| {
            let text = path.to_string_lossy();
            if text.chars().count() > MAX_CWD_CHARS {
                let clipped: String = text.chars().take(MAX_CWD_CHARS - 1).collect();
                format!("{clipped}…")
            } else {
                text.to_string()
            }
        })
        .unwrap_or_else(|| "<inherit>".to_string());
    format!(
        "diag=v2 program_len={} args={} argv_chars={} env_vars={} env_chars={} top_env=[{top_env}] cwd={cwd}",
        program.chars().count(),
        args.len(),
        argv_chars,
        env_entries.len(),
        env_chars,
    )
}

/// spawn 前环境块预检：Windows `CreateProcess` 的环境块上限同为约 32K 字符，超限
/// 同样以 os error 206 失败且不携带任何可定位信息。继承面已被
/// `apply_inherited_agent_environment` 收敛为固定白名单，但白名单值（如 PATH）与
/// 设置页注入的 agent 环境变量仍取决于用户机器；超限时带 tag 并指认最大变量，
/// 替代 OS 哑错（阈值留出命令行与引号转义的安全余量）。
pub(crate) fn ensure_env_block_within_limit(
    command: &tokio::process::Command,
    error_tag_prefix: &str,
) -> Result<(), String> {
    const MAX_ENV_BLOCK_CHARS: usize = 24_000;
    let entries: Vec<(String, usize)> = command
        .as_std()
        .get_envs()
        .filter_map(|(key, value)| {
            value.map(|value| {
                (
                    key.to_string_lossy().to_string(),
                    key.to_string_lossy().chars().count()
                        + value.to_string_lossy().chars().count()
                        + 2,
                )
            })
        })
        .collect();
    let total: usize = entries.iter().map(|(_, size)| size).sum();
    if total <= MAX_ENV_BLOCK_CHARS {
        return Ok(());
    }
    let largest = entries.iter().max_by_key(|(_, size)| *size).map_or_else(
        || "unknown".to_string(),
        |(key, size)| format!("{key}（{size} 字符）"),
    );
    Err(format!(
        "[{error_tag_prefix}_env_block_too_large] 子进程环境块约 {total} 字符，超过 Windows 约 32K 上限的安全阈值；最大变量：{largest}"
    ))
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
    /// 无损分支首轮：在服务端按源线程派生新线程，再在新线程上开启本轮（与 Start 对称）。
    /// 第二参数为切点分叉的原生轮次 id（Some 时 thread/fork 截断到该轮含，None 整段）。
    Fork(String, Option<String>),
}

fn codex_app_server_thread_plan(
    native_thread_id: Option<&str>,
    fork_source_thread_id: Option<&str>,
    fork_last_turn_id: Option<&str>,
    force_history_rebuild: bool,
) -> CodexAppServerThreadPlan {
    if force_history_rebuild {
        CodexAppServerThreadPlan::Start
    } else if let Some(source) = fork_source_thread_id.filter(|id| !id.is_empty()) {
        CodexAppServerThreadPlan::Fork(
            source.to_string(),
            fork_last_turn_id
                .filter(|id| !id.is_empty())
                .map(str::to_string),
        )
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
        "model_providers.helm.env_key=OPENAI_API_KEY".to_string(),
        "model_providers.helm.requires_openai_auth=false".to_string(),
    ]
}

/// Codex 的原生 WebSearch 只由 Responses wire API 暴露。
/// 该开关必须与 ProviderLaunchProfile 一致，不能由模型名称猜测。
pub(crate) fn codex_native_search_enabled(env: &[(String, String)]) -> bool {
    if env.iter().any(|(key, value)| {
        key == "HELM_CODEX_SEARCH_TRANSPORT" && value.eq_ignore_ascii_case("unavailable")
    }) {
        return false;
    }
    env.iter()
        .find(|(key, _)| key == "HELM_CODEX_WIRE_API")
        .map(|(_, value)| value.trim().eq_ignore_ascii_case("responses"))
        .unwrap_or(true)
}

pub(crate) fn codex_search_catalog_bytes(
    env: &[(String, String)],
) -> Result<Option<Vec<u8>>, String> {
    let Some((_, encoded)) = env
        .iter()
        .find(|(key, _)| key == CODEX_SEARCH_CATALOG_JSON_ENV)
    else {
        return Ok(None);
    };
    let bytes = encoded.as_bytes().to_vec();
    let expected = env
        .iter()
        .find(|(key, _)| key == CODEX_SEARCH_CATALOG_DIGEST_ENV)
        .map(|(_, value)| value.as_str())
        .ok_or_else(|| {
            "[codex_search_catalog_invalid] 兼容模型目录缺少摘要，已阻止启动".to_string()
        })?;
    let actual = format!("sha256:{}", sha256_hex(&bytes));
    if expected != actual {
        return Err(
            "[codex_search_catalog_tampered] 兼容模型目录与启动摘要不一致，已阻止启动".to_string(),
        );
    }
    serde_json::from_slice::<serde_json::Value>(&bytes).map_err(|error| {
        format!("[codex_search_catalog_invalid] 兼容模型目录不是 JSON：{error}")
    })?;
    Ok(Some(bytes))
}

/// 将已协商的无秘密模型目录绑定到本次 app-server。目录内容来自同一 Codex
/// binary 的 `debug models --bundled`，这里仅负责摘要复核和不可变落盘。
pub(crate) fn apply_codex_search_catalog(
    command: &mut Command,
    env: &[(String, String)],
    codex_home: Option<&Path>,
) -> Result<Option<String>, String> {
    let Some(bytes) = codex_search_catalog_bytes(env)? else {
        return Ok(None);
    };
    let home = codex_home.ok_or_else(|| {
        "[codex_search_catalog_home_missing] 搜索兼容目录没有隔离 CODEX_HOME，已阻止启动"
            .to_string()
    })?;
    fs::create_dir_all(home).map_err(|error| format!("创建 Codex 搜索目录失败：{error}"))?;
    let path = home.join(CODEX_SEARCH_CATALOG_FILE);
    if path.is_file() {
        let existing =
            fs::read(&path).map_err(|error| format!("读取 Codex 搜索目录失败：{error}"))?;
        if existing != bytes {
            return Err(
                "[codex_search_catalog_tampered] CODEX_HOME 中的兼容模型目录已被修改".to_string(),
            );
        }
    } else {
        fs::write(&path, bytes).map_err(|error| format!("写入 Codex 搜索目录失败：{error}"))?;
    }
    let value = path
        .canonicalize()
        .unwrap_or(path)
        .to_string_lossy()
        .to_string();
    let quoted = serde_json::to_string(&value)
        .map_err(|error| format!("编码 Codex 搜索目录路径失败：{error}"))?;
    command
        .arg("-c")
        .arg(format!("model_catalog_json={quoted}"));
    Ok(Some(value))
}

pub(crate) fn apply_codex_native_search(command: &mut Command, env: &[(String, String)]) -> bool {
    let enabled = codex_native_search_enabled(env);
    if enabled {
        // app-server 不继承 TUI 的交互工具装配；除官方快捷参数外同时固定真实配置值，
        // 防止 CLI 接受 --search 但 thread/start 工具面仍保持 cached/disabled。
        command.arg("-c").arg("web_search=\"live\"");
        command.arg("--search");
    }
    enabled
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

static RUNTIME_LOG_PATH: OnceLock<PathBuf> = OnceLock::new();

/// 运行时排查日志：同时打到 stderr（dev 可见）并追加到
/// `<app_config_dir>/helm-runtime.log`（打包后也持久，便于事后还原现场）。
/// 不引第三方日志库，保持最小依赖。用于记录每轮 turn 的生死关键节点，
/// 解决「进程卡死/漏发终态时无法还原现场」的问题。
pub(crate) fn log_runtime_event(app: &AppHandle, tag: &str, detail: &str) {
    if let Ok(dir) = app.path().app_config_dir() {
        let _ = RUNTIME_LOG_PATH.set(dir.join("helm-runtime.log"));
    }
    log_runtime_line(tag, detail);
}

pub(crate) fn log_runtime_line(tag: &str, detail: &str) {
    let line = format!("[helm-{}] ts={} {}", tag, now_millis(), detail);
    eprintln!("{}", line);
    let path = RUNTIME_LOG_PATH.get().cloned().or_else(|| {
        std::env::var_os("APPDATA").map(|dir| {
            PathBuf::from(dir)
                .join("com.helm.desktop")
                .join("helm-runtime.log")
        })
    });
    if let Some(path) = path {
        let _ = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .and_then(|mut f| std::io::Write::write_all(&mut f, format!("{}\n", line).as_bytes()));
    }
}

/// 只记录对诊断「一直进行中/卡死」关键、且非高频流式的事件类型；
/// message_delta/thinking_delta/tool_progress 等逐块流式事件跳过，避免日志爆炸。
fn loggable_event_type(event: &AgentEvent) -> Option<&'static str> {
    match event {
        AgentEvent::SessionStarted { .. } => Some("session_started"),
        AgentEvent::MessageDelta { .. } => None,
        AgentEvent::MessageComplete { .. } => Some("message_complete"),
        AgentEvent::ThinkingDelta { .. } => None,
        AgentEvent::ThinkingComplete { .. } => Some("thinking_complete"),
        AgentEvent::TurnStage { .. } => Some("turn_stage"),
        AgentEvent::ToolCall { .. } => Some("tool_call"),
        AgentEvent::ToolProgress { .. } => None,
        AgentEvent::ToolResult { .. } => Some("tool_result"),
        AgentEvent::ApprovalRequest { .. } => Some("approval_request"),
        AgentEvent::PlanUpdate { .. } => Some("plan_update"),
        AgentEvent::Checkpoint { .. } => Some("checkpoint"),
        AgentEvent::TokenUsage { .. } => Some("token_usage"),
        AgentEvent::ContextUsage { .. } => Some("context_usage"),
        AgentEvent::ContextCompaction { .. } => Some("context_compaction"),
        AgentEvent::TurnComplete { .. } => Some("turn_complete"),
        AgentEvent::Error { .. } => Some("error"),
    }
}

fn emit_agent_event(app: &AppHandle, history_session_id: &str, event: &AgentEvent) {
    if let Some(t) = loggable_event_type(event) {
        let mut detail = format!("history={} type={}", history_session_id, t);
        if let AgentEvent::SessionStarted { engine, .. } = event {
            detail.push_str(&format!(" engine={:?}", engine));
        }
        log_runtime_event(app, "event-emit", &detail);
    }
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
    // 性能：脱敏收敛到两处——TurnSupervisor 入口（覆盖 adapter 与 runtime_registry
    // 两条提交路径）与落库前。同一条事件此前在同步链上被全量正则扫描 3 次。
    let event = normalize_runtime_search_tool_result(event);
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

fn collect_profile_tree(
    source: &Path,
    relative_root: &Path,
    files: &mut Vec<(PathBuf, Vec<u8>)>,
) -> Result<(), String> {
    let mut entries = fs::read_dir(source)
        .map_err(|error| format!("读取 Codex Profile 继承目录失败：{error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("读取 Codex Profile 继承目录项失败：{error}"))?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let from = entry.path();
        let relative = relative_root.join(entry.file_name());
        let file_type = entry
            .file_type()
            .map_err(|error| format!("读取 Codex Profile 继承类型失败：{error}"))?;
        let resolved = if file_type.is_symlink() {
            fs::canonicalize(&from).map_err(|error| {
                format!(
                    "解析 Codex Profile 继承链接失败（{}）：{error}",
                    from.display()
                )
            })?
        } else {
            from
        };
        if resolved.is_dir() {
            collect_profile_tree(&resolved, &relative, files)?;
        } else if resolved.is_file() {
            files.push((
                relative,
                fs::read(&resolved).map_err(|error| {
                    format!(
                        "读取 Codex Profile 继承文件失败（{}）：{error}",
                        resolved.display()
                    )
                })?,
            ));
        }
    }
    Ok(())
}

/// 只遍历目录项的元数据（路径+大小+mtime_ns），不读文件内容，聚合出稳定指纹。
/// 覆盖范围与 `codex_engine_profile_files` 的读取集严格一致：config.toml +
/// prompts/ + skills/。`~/.codex` 下其它高频变化文件（sqlite/日志）不进指纹，
/// 否则每次恢复都会失效。指纹命中即代表这三处的内容与上次读出时一致（含 mtime
/// 粒度）；任一文件增删改名都会翻转指纹，回落全量读，不会用陈旧内容算 revision。
fn profile_tree_fingerprint(source: &Path) -> String {
    use sha2::Digest as _;
    let mut entries: Vec<(String, u64, i128)> = Vec::new();
    fn mtime_ns(meta: &std::fs::Metadata) -> i128 {
        meta.modified()
            .ok()
            .and_then(|v| v.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|v| v.as_nanos() as i128)
            .unwrap_or(0)
    }
    if let Ok(meta) = fs::metadata(source.join("config.toml")) {
        if meta.is_file() {
            entries.push(("config.toml".to_string(), meta.len(), mtime_ns(&meta)));
        }
    }
    let mut stack = vec![
        (source.join("prompts"), "prompts".to_string()),
        (source.join("skills"), "skills".to_string()),
    ];
    while let Some((dir, prefix)) = stack.pop() {
        let Ok(read) = fs::read_dir(&dir) else {
            continue;
        };
        for item in read.flatten() {
            let path = item.path();
            let name = item.file_name().to_string_lossy().replace('\\', "/");
            let relative = format!("{prefix}/{name}");
            let Ok(meta) = item.metadata() else {
                continue;
            };
            if meta.is_dir() {
                stack.push((path, relative));
            } else {
                entries.push((relative, meta.len(), mtime_ns(&meta)));
            }
        }
    }
    entries.sort();
    let mut hasher = <sha2::Sha256 as sha2::Digest>::new();
    for (relative, len, mtime) in &entries {
        hasher.update(relative.as_bytes());
        hasher.update(len.to_le_bytes());
        hasher.update(mtime.to_le_bytes());
    }
    format!("sha256:{:x}:{}", hasher.finalize(), entries.len())
}

fn codex_engine_profile_files(
    source: Option<&Path>,
    disabled_mcp: &[String],
) -> Result<Vec<(PathBuf, Vec<u8>)>, String> {
    let mut files = Vec::new();
    let Some(source) = source else {
        return Ok(files);
    };
    let config = source.join("config.toml");
    if config.is_file() {
        let raw =
            fs::read_to_string(&config).map_err(|error| format!("读取 Codex 配置失败：{error}"))?;
        let filtered = filter_codex_engine_config(&raw, disabled_mcp)?;
        files.push((PathBuf::from("config.toml"), filtered.into_bytes()));
    }
    let prompts = source.join("prompts");
    if prompts.is_dir() {
        let mut prompt_files = fs::read_dir(&prompts)
            .map_err(|error| format!("读取 Codex prompts 失败：{error}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("读取 Codex prompt 失败：{error}"))?;
        prompt_files.sort_by_key(|entry| entry.file_name());
        for entry in prompt_files {
            let from = entry.path();
            if from.extension().and_then(|value| value.to_str()) == Some("md") && from.is_file() {
                files.push((
                    PathBuf::from("prompts").join(entry.file_name()),
                    fs::read(&from).map_err(|error| format!("读取 Codex prompt 失败：{error}"))?,
                ));
            }
        }
    }
    let skills = source.join("skills");
    if skills.is_dir() {
        collect_profile_tree(&skills, Path::new("skills"), &mut files)?;
    }
    files.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(files)
}

fn filter_codex_engine_config(raw: &str, disabled_mcp: &[String]) -> Result<String, String> {
    let mut value: toml::Value =
        toml::from_str(raw).map_err(|error| format!("解析 Codex 配置失败：{error}"))?;
    let root = value
        .as_table_mut()
        .ok_or_else(|| "Codex config.toml 顶层不是表".to_string())?;

    // Provider、模型和 Base URL 由冻结的 ProviderLaunchProfile 在启动时覆盖，
    // 不能进入持久 EngineProfile，也不能让它们改变 sandbox revision。
    root.remove("model");
    root.remove("model_provider");
    root.remove("model_providers");
    root.remove("model_catalog_json");
    if let Some(profiles) = root.get_mut("profiles").and_then(toml::Value::as_table_mut) {
        for profile in profiles
            .iter_mut()
            .filter_map(|(_, value)| value.as_table_mut())
        {
            profile.remove("model");
            profile.remove("model_provider");
        }
    }

    if let Some(servers) = root
        .get_mut("mcp_servers")
        .and_then(toml::Value::as_table_mut)
    {
        for name in disabled_mcp {
            servers.remove(name);
        }
    }
    toml::to_string_pretty(&value).map_err(|error| format!("序列化 Codex 配置失败：{error}"))
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
    // 自定义 Provider 通过 env_key 从子进程环境读取 API Key。临时探测 Profile 也不得
    // 落 auth.json，避免能力发现或崩溃路径留下凭据。
    let _ = api_key;
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
        // 标准档开网络：原生 WebSearch 走 Runtime 通道，审批仍保持 Untrusted（Ask）。
        // 变更-31：standard / auto 都应遵循 Runtime 原生搜索；关网络会把标准档搜索掐死。
        PermissionProfile::Standard => ("workspace-write", true, CodexApprovalPolicy::Untrusted),
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

/// 单附件注入 prompt 的文本上限（字符）；超出截断并注明。
const ATTACHMENT_TEXT_CAP: usize = 24_000;
/// 全部附件累计注入 prompt 的文本上限（字符）；超出后其余附件降级为纯路径列表。
const ATTACHMENT_TEXT_TOTAL_CAP: usize = 80_000;

/// 把附件内容注入 prompt（2026-08-12 增强）：此前只列出路径让 agent 自行读取，
/// 二进制 office/pdf 格式 agent 的 Read 工具读不了，内容永远出不来。
/// 现在文本类直接读入，docx/pptx/xlsx 走 zip+XML 提取文本，pdf 走 pdf-extract；
/// 图片等无法提取的格式保留路径（视觉能力可读），提取失败回退纯路径列表。
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
    let mut injected_chars = 0usize;
    for path in &mounted_paths {
        let path_obj = Path::new(path);
        let mut line = format!("- {path}");
        if path_obj.is_dir() {
            // 目录只列路径（agent 自行探查）
        } else {
            let extraction = if injected_chars < ATTACHMENT_TEXT_TOTAL_CAP {
                attachment_extract_text(path_obj)
            } else {
                None
            };
            if let Some((label, content)) = extraction {
                // 先按单文件上限截断，再按累计上限收口
                let original_chars = content.chars().count();
                let mut shown = if original_chars > ATTACHMENT_TEXT_CAP {
                    content
                        .chars()
                        .take(ATTACHMENT_TEXT_CAP)
                        .collect::<String>()
                } else {
                    content
                };
                let mut truncated =
                    shown.chars().count() > ATTACHMENT_TEXT_TOTAL_CAP - injected_chars;
                if truncated {
                    shown = shown
                        .chars()
                        .take(ATTACHMENT_TEXT_TOTAL_CAP - injected_chars)
                        .collect();
                }
                truncated = truncated || shown.chars().count() < original_chars;
                line.push_str(&format!(
                    "\n  {label}{}",
                    if truncated {
                        "（内容过长已截断）"
                    } else {
                        ""
                    }
                ));
                line.push_str("\n  ");
                line.push_str(&shown.replace('\n', "\n  "));
                injected_chars += shown.chars().count();
            } else {
                line.push_str("（附件：路径供 agent 读取）");
            }
        }
        prompt.push_str(&line);
        prompt.push('\n');
    }
    prompt.push_str("\n请优先依据以上挂载内容回答；二进制/图片等未提取内容请自行读取。");
    prompt
}

/// 尽力从附件路径提取可读文本；失败返回 None（调用方回退路径列表）。
/// 返回 (展示标签, 提取文本)。
fn attachment_extract_text(path: &Path) -> Option<(String, String)> {
    let ext = path
        .extension()
        .map(|value| value.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "docx" => {
            let text = extract_zip_office_text(path, "word/document.xml", "w:p", "docx")?;
            Some((
                format!("已提取文本（docx，{} 字符）", text.chars().count()),
                text,
            ))
        }
        "pptx" => {
            let text = extract_pptx_text(path)?;
            Some((
                format!("已提取文本（pptx，{} 字符）", text.chars().count()),
                text,
            ))
        }
        "xlsx" => {
            let text = extract_zip_office_text(path, "xl/sharedStrings.xml", "si", "xlsx")?;
            Some((
                format!(
                    "已提取文本（xlsx 文本单元格，{} 字符）",
                    text.chars().count()
                ),
                text,
            ))
        }
        "pdf" => {
            let text = pdf_extract::extract_text(path).ok()?;
            if text.trim().is_empty() {
                return None;
            }
            Some((
                format!("已提取文本（pdf，{} 字符）", text.chars().count()),
                text,
            ))
        }
        _ => {
            // 文本/代码类：直接读入；前 1KB 出现 NUL 视为二进制，不注入
            let bytes = fs::read(path).ok()?;
            if bytes.iter().take(1024).any(|byte| *byte == 0) {
                return None;
            }
            let text = String::from_utf8_lossy(&bytes).into_owned();
            if text.trim().is_empty() {
                return None;
            }
            Some((format!("已读入内容（{} 字节）", bytes.len()), text))
        }
    }
}

/// pptx 幻灯片文本：遍历 ppt/slides/slide*.xml，段落分隔后去标签。
fn extract_pptx_text(path: &Path) -> Option<String> {
    let bytes = fs::read(path).ok()?;
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).ok()?;
    let mut names = (0..archive.len())
        .filter_map(|index| {
            archive
                .by_index(index)
                .ok()
                .map(|entry| entry.name().to_string())
        })
        .collect::<Vec<_>>();
    names.sort();
    let mut pages = Vec::new();
    for name in names {
        if name.starts_with("ppt/slides/slide") && name.ends_with(".xml") {
            let mut entry = archive.by_name(&name).ok()?;
            let mut xml = String::new();
            std::io::Read::read_to_string(&mut entry, &mut xml).ok()?;
            let page = strip_xml_text(&xml, "a:p");
            if !page.trim().is_empty() {
                pages.push(page.trim().to_string());
            }
        }
    }
    if pages.is_empty() {
        return None;
    }
    Some(pages.join("\n\n"))
}

/// 从 zip 内指定 XML 提取文本：段落/条目结束标签换行，剥标签 + 反转义实体。
fn extract_zip_office_text(
    path: &Path,
    entry_name: &str,
    item_tag: &str,
    _kind: &str,
) -> Option<String> {
    let bytes = fs::read(path).ok()?;
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).ok()?;
    let mut entry = archive.by_name(entry_name).ok()?;
    let mut xml = String::new();
    std::io::Read::read_to_string(&mut entry, &mut xml).ok()?;
    let text = strip_xml_text(&xml, item_tag);
    if text.trim().is_empty() {
        return None;
    }
    Some(text)
}

/// 剥 XML 标签提取可见文本：item_tag 的结束标签转为换行，实体反转义。
fn strip_xml_text(xml: &str, item_tag: &str) -> String {
    let paragraph_ends = format!("</{item_tag}>");
    let with_newlines = xml.replace(&paragraph_ends, "\n");
    let stripped = regex::Regex::new(r"<[^>]*>")
        .unwrap()
        .replace_all(&with_newlines, "")
        .into_owned();
    let mut result = String::with_capacity(stripped.len());
    let mut chars = stripped.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '&' {
            result.push(ch);
            continue;
        }
        let mut entity = String::new();
        for next in chars.by_ref() {
            if next == ';' {
                break;
            }
            entity.push(next);
        }
        result.push(match entity.as_str() {
            "amp" => '&',
            "lt" => '<',
            "gt" => '>',
            "quot" => '"',
            "apos" => '\'',
            "nbsp" => ' ',
            _ => {
                if let Some(decoded) = decode_numeric_entity(&entity) {
                    decoded
                } else {
                    '&'
                }
            }
        });
    }
    let no_blank_lines = result
        .lines()
        .map(|line| line.trim())
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    no_blank_lines
}

/// 解码 &#nnn; / &#xhh; 数字实体；失败返回 None 保留原样。
fn decode_numeric_entity(entity: &str) -> Option<char> {
    let value = entity
        .strip_prefix("#x")
        .or_else(|| entity.strip_prefix("#X"))
        .and_then(|hex| u32::from_str_radix(hex, 16).ok())
        .or_else(|| {
            entity
                .strip_prefix('#')
                .and_then(|dec| dec.parse::<u32>().ok())
        })?;
    char::from_u32(value)
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
        abort_codex_turn_task, agent_environment_from_settings, apply_codex_native_search,
        apply_codex_search_catalog, auto_fallback_decision, build_approval_action,
        checkpoint_target_path, claude_exit_disposition, claude_permission_mode,
        claude_permission_mode_for_capability, codex_native_search_enabled,
        codex_provider_config_args, codex_runtime_profile_policy, codex_sandbox_for_mode,
        create_auto_checkpoint_for_tool, create_codex_auth_home,
        create_codex_auth_home_with_source, create_runtime_approval_hook_files,
        extract_tool_target, filter_inherited_agent_environment, finish_codex_interrupt_terminal,
        full_access_lease, lease_is_valid, merge_pending_delta,
        normalize_runtime_search_tool_result, parse_codex_line, prompt_with_attachments,
        record_approval_state, reserve_turn_flag, rollback_prepared_approval_state,
        run_serialized_approval, should_process_claude_event, spawn_agent_task,
        terminal_turn_outcome, validate_engine_bin, wait_until_idle_and_begin,
        write_approval_state, AgentSession, ApprovalDecision, ApprovalState, AutoFallbackDecision,
        ClaudeExitDisposition, ClaudeSession, CodexRuntimeProfileStore, PendingToolInfo,
        PermissionProfile, SessionCmd, TurnMode, CODEX_SEARCH_CATALOG_DIGEST_ENV,
        CODEX_SEARCH_CATALOG_JSON_ENV,
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
                stalled_kind: None,
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
    async fn claude_compact_context_rejects_without_contract() {
        // Claude 由 `claude -p` 驱动，无 /compact 注入契约（2026-08-12 更正 Codex 后有，
        // 但 Claude 仍无）。Helm 必须 fail-closed，不能伪造压缩成功。
        let (tx, _rx) = mpsc::unbounded_channel();
        let session = AgentSession::Claude(ClaudeSession {
            tx,
            cwd: "D:/repo".to_string(),
            control: None,
        });
        let error = session
            .compact_context()
            .await
            .expect_err("Claude 无压缩契约时必须返回错误");
        assert!(
            error.contains("无 /compact 注入契约"),
            "unexpected error: {error}"
        );
    }

    #[tokio::test]
    async fn claude_interrupt_waits_for_manager_terminal_acknowledgement() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let session = AgentSession::Claude(ClaudeSession {
            tx,
            cwd: "D:/repo".to_string(),
            control: None,
        });
        let interrupt = tokio::spawn(async move { session.interrupt_and_wait().await });

        let responder = match rx.recv().await {
            Some(SessionCmd::Interrupt {
                responder: Some(responder),
            }) => responder,
            _ => panic!("manager 应收到带终态回执的中断命令"),
        };
        assert!(
            !interrupt.is_finished(),
            "收到 manager 回执前不得报告 Stop 完成"
        );
        responder.send(Ok(())).unwrap();

        tokio::time::timeout(Duration::from_secs(1), interrupt)
            .await
            .expect("manager 回执后中断应立即完成")
            .expect("中断等待任务不应崩溃")
            .expect("中断回执应成功");
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
                crate::protocol::ApprovalDecisionOption::Session,
                crate::protocol::ApprovalDecisionOption::Deny,
            ]
        );

        action.session_id.clear();
        assert!(!super::available_approval_decisions(Some(&action))
            .contains(&crate::protocol::ApprovalDecisionOption::Session));
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
            super::codex_app_server_thread_plan(Some("thread-1"), None, None, false),
            super::CodexAppServerThreadPlan::Resume("thread-1".to_string())
        );
        assert_eq!(
            super::codex_app_server_thread_plan(Some("thread-1"), None, None, true),
            super::CodexAppServerThreadPlan::Start
        );
        assert_eq!(
            super::codex_app_server_thread_plan(None, None, None, false),
            super::CodexAppServerThreadPlan::Start
        );
        // 无损分支：fork 计划区分整段（lastTurn None）与切点截断（Some 原生轮 id）。
        assert_eq!(
            super::codex_app_server_thread_plan(None, Some("thread-src"), None, false),
            super::CodexAppServerThreadPlan::Fork("thread-src".to_string(), None)
        );
        assert_eq!(
            super::codex_app_server_thread_plan(
                None,
                Some("thread-src"),
                Some("turn-native-9"),
                false
            ),
            super::CodexAppServerThreadPlan::Fork(
                "thread-src".to_string(),
                Some("turn-native-9".to_string())
            )
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
            ("workspace-write", true, CodexApprovalPolicy::Untrusted)
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

    #[tokio::test]
    async fn codex_interrupt_replaces_silent_stream_close_with_interrupted_terminal() {
        let terminals = Mutex::new(HashMap::from([(
            "turn-1".to_string(),
            Err("Codex app-server notification stream closed".to_string()),
        )]));
        let emitted = AtomicBool::new(false);
        finish_codex_interrupt_terminal(
            &terminals,
            &Notify::new(),
            Some("turn-1".to_string()),
            || {
                emitted.store(true, Ordering::Release);
            },
        )
        .await;
        assert!(emitted.load(Ordering::Acquire));
        assert_eq!(
            terminal_turn_outcome(&terminals, "turn-1").await,
            Some(Ok(()))
        );
    }

    #[tokio::test]
    async fn codex_interrupt_abort_clears_busy_and_awaits_task() {
        let busy = Arc::new(AtomicBool::new(true));
        let task = tauri::async_runtime::spawn(async move {
            std::future::pending::<()>().await;
        });
        let task = std::sync::Mutex::new(Some(task));

        abort_codex_turn_task(&task, &busy, Duration::from_secs(1))
            .await
            .expect("Stop 兜底取消必须完成任务回收");

        assert!(!busy.load(Ordering::Acquire));
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
    fn agent_environment_always_opt_out_of_analytics() {
        // 匿名分析开关移除后，CLI 子进程始终带 DO_NOT_TRACK=1 / HELM_TELEMETRY_DISABLED=1。
        let env = agent_environment_from_settings(&AppSettings::default());
        assert!(env.contains(&("DO_NOT_TRACK".to_string(), "1".to_string())));
        assert!(env.contains(&("HELM_TELEMETRY_DISABLED".to_string(), "1".to_string())));
        assert!(env.contains(&("HELM_ANONYMOUS_ANALYTICS".to_string(), "0".to_string())));
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

    /// 构造最小 docx/zip：word/document.xml 含两段文本。
    fn write_mini_docx(path: &std::path::Path) {
        use std::io::Write;
        let file = std::fs::File::create(path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        writer.start_file("word/document.xml", options).unwrap();
        writer
            .write_all(
                "<w:document><w:body>\
                  <w:p><w:r><w:t>你好 &amp; 再见</w:t></w:r></w:p>\
                  <w:p><w:r><w:t>第二段</w:t></w:r></w:p>\
                  </w:body></w:document>"
                    .as_bytes(),
            )
            .unwrap();
        writer.finish().unwrap();
    }

    #[test]
    fn prompt_with_attachments_injects_text_file_content() {
        let root = std::env::temp_dir().join(format!(
            "helm-adapter-attach-text-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let file = root.join("说明.md");
        std::fs::write(&file, "这是文档正文。\n第二行。").unwrap();
        let prompt = prompt_with_attachments("按文档回答", &[file.to_string_lossy().to_string()]);

        assert!(prompt.contains("已读入内容（"));
        assert!(
            prompt.contains("这是文档正文。\n  第二行。"),
            "正文应缩进注入：{prompt}"
        );
        assert!(prompt.contains("请优先依据以上挂载内容回答"));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn prompt_with_attachments_extracts_docx_and_xlsx_text() {
        let root = std::env::temp_dir().join(format!(
            "helm-adapter-attach-office-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        ));
        std::fs::create_dir_all(&root).unwrap();

        // docx：zip 内文本提取
        let docx = root.join("报告.docx");
        write_mini_docx(&docx);
        let prompt = prompt_with_attachments("总结", &[docx.to_string_lossy().to_string()]);
        assert!(prompt.contains("已提取文本（docx，"), "{prompt}");
        assert!(prompt.contains("你好 & 再见"), "实体应反转义：{prompt}");
        assert!(prompt.contains("第二段"), "段落应换行保留：{prompt}");

        // xlsx：sharedStrings 文本提取
        use std::io::Write as _;
        let xlsx = root.join("台账.xlsx");
        let file = std::fs::File::create(&xlsx).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        writer.start_file("xl/sharedStrings.xml", options).unwrap();
        writer
            .write_all("<sst><si><t>项目名称</t></si><si><t>预算 100 万</t></si></sst>".as_bytes())
            .unwrap();
        writer.finish().unwrap();
        let prompt = prompt_with_attachments("看台账", &[xlsx.to_string_lossy().to_string()]);
        assert!(prompt.contains("已提取文本（xlsx"), "{prompt}");
        assert!(prompt.contains("项目名称"));
        assert!(prompt.contains("预算 100 万"));

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn prompt_with_attachments_falls_back_to_path_for_binary() {
        let root = std::env::temp_dir().join(format!(
            "helm-adapter-attach-binary-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let file = root.join("logo.png");
        std::fs::write(&file, [0x89_u8, 0x50, 0x4E, 0x47, 0x00, 0x01, 0x02]).unwrap();
        let prompt = prompt_with_attachments("看看图", &[file.to_string_lossy().to_string()]);

        assert!(
            prompt.contains("路径供 agent 读取"),
            "NUL 二进制不应注入内容：{prompt}"
        );
        assert!(!prompt.contains("已读入内容"));
        std::fs::remove_dir_all(&root).ok();
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
            ["--mcp-config", "{\"mcpServers\": {}}"],
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
        // prompt 不再作为位置参数进入 argv（os error 206 回归），改由 stdin 交付：
        assert!(
            !args.iter().any(|arg| arg.contains("只生成标题")),
            "prompt 不得出现在命令行参数中"
        );
        assert_eq!(
            args.last().map(String::as_str),
            Some(""),
            "argv 末尾必须是 --setting-sources 的空值，而非 prompt"
        );
    }

    /// 预检守卫（六次反馈加固）：超限 argv 在 spawn 前被拦截并带错误 tag；
    /// 常规短命令不受影响。防止未来改动把长 prompt 塞回 argv 后退化成 OS 哑错。
    #[test]
    fn command_line_guard_rejects_overlong_argv_with_tagged_error() {
        let mut overlong = tokio::process::Command::new("claude");
        overlong.arg("--print").arg("字".repeat(31_000));
        let error = super::ensure_command_line_within_limit(&overlong, "operation")
            .expect_err("超限命令行必须被预检拦截");
        assert!(
            error.starts_with("[operation_command_line_too_long]"),
            "错误必须带 tag 便于前端定位，实际：{error}"
        );

        let short = tokio::process::Command::new("claude");
        super::ensure_command_line_within_limit(&short, "operation").expect("常规短命令不应被拦截");
        let mut normal = tokio::process::Command::new("claude");
        normal.arg("--print").arg("--model").arg("claude-fixture");
        super::ensure_command_line_within_limit(&normal, "side_query")
            .expect("常规模型专用命令不应被拦截");
    }

    /// 环境块预检（七次反馈加固）：os error 206 也可能来自环境块超限；超限 env 在
    /// spawn 前被拦截、带错误 tag 并指认最大变量，常规白名单环境不受影响。
    #[test]
    fn env_block_guard_rejects_oversized_environment_with_tagged_error() {
        let mut bloated = tokio::process::Command::new("claude");
        bloated.env_clear();
        bloated.env("PATH", "C:\\Windows\\system32");
        bloated.env("HELM_BLOAT_TEST", "字".repeat(25_000));
        bloated.env("HELM_SMALL_TEST", "ok");
        let error = super::ensure_env_block_within_limit(&bloated, "operation")
            .expect_err("超限环境块必须被预检拦截");
        assert!(
            error.starts_with("[operation_env_block_too_large]"),
            "错误必须带 tag 便于前端定位，实际：{error}"
        );
        assert!(
            error.contains("HELM_BLOAT_TEST"),
            "错误必须指认最大的环境变量，实际：{error}"
        );

        let mut normal = tokio::process::Command::new("claude");
        normal.env_clear();
        normal.env("PATH", "C:\\Windows\\system32;C:\\Windows");
        super::ensure_env_block_within_limit(&normal, "side_query")
            .expect("常规白名单环境不应被拦截");
    }

    /// 取证串（七次反馈）：spawn 失败消息携带 diag=v2 标记与各输入规模，
    /// 足以在用户侧一次性定位 206 的真实来源并证明二进制新旧。
    #[test]
    fn spawn_forensics_reports_sizes_and_marker() {
        let mut command = tokio::process::Command::new("claude");
        command.arg("--print").arg("--model").arg("claude-fixture");
        command.env_clear();
        command.env("PATH", "C:\\Windows\\system32;C:\\Windows");
        command.env(
            "CLAUDE_CONFIG_DIR",
            "C:\\Users\\demo\\.helm\\cli-profiles\\claude-subscription",
        );
        command.current_dir(std::env::temp_dir());
        let diag = super::command_spawn_forensics(&command);
        assert!(diag.starts_with("diag=v2 "), "取证串必须带版本标记：{diag}");
        assert!(diag.contains("args=3"), "参数计数错误：{diag}");
        assert!(diag.contains("env_vars=2"), "环境变量计数错误：{diag}");
        assert!(diag.contains("cwd="), "取证串必须包含 cwd：{diag}");
        // top_env 按字符数降序：值更长的 CLAUDE_CONFIG_DIR 必须排在 PATH 前。
        let config_pos = diag.find("top_env=[CLAUDE_CONFIG_DIR(");
        assert!(config_pos.is_some(), "top_env 必须包含最大变量：{diag}");
        let path_pos = diag.find("PATH(").expect("top_env 应列出 PATH");
        assert!(
            path_pos > config_pos.unwrap(),
            "top_env 必须按大小降序排列：{diag}"
        );
    }

    /// 回归测试（Windows os error 206）：`CreateProcess` 命令行总长约 32767 字符，
    /// 分叉摘要曾把整份 Ledger prompt 作为 argv 传递导致 spawn 失败。用真实子进程
    /// 证明：>32K 的 prompt 全文经 `write_model_only_prompt` 从 stdin 到达子进程、
    /// stdin 正确关闭（子进程能读到 EOF 退出），且命令行参数不含 prompt。
    #[tokio::test]
    async fn model_only_prompt_over_32k_is_delivered_via_stdin_never_argv() {
        let cwd = std::env::temp_dir();
        let marker = "HELM-FORK-LONG-PROMPT-MARKER";
        let long_prompt = format!("{marker}{}", "字".repeat(40_000));
        assert!(long_prompt.chars().count() > 32 * 1024);

        // argv 契约：构造出的 model-only 命令行不包含 prompt，总长度有界；
        // stdin 显式为管道，供 spawn 后写入。
        let command = super::build_claude_model_only_command(
            "claude",
            "claude-fixture",
            &[],
            &cwd,
            crate::reasoning::ReasoningEffort::Auto,
        )
        .unwrap();
        let joined_args = command
            .as_std()
            .get_args()
            .map(|arg| arg.to_string_lossy())
            .collect::<Vec<_>>()
            .join("\u{0}");
        assert!(
            !joined_args.contains(marker),
            "prompt（含其片段）不得出现在命令行参数中"
        );
        assert!(
            joined_args.chars().count() < 32 * 1024,
            "model-only 固定 argv 必须远小于 CreateProcess 约 32K 上限"
        );
        // builder 必须显式把 stdin 设为管道（stable 工具链没有 Stdio getter，用行为级
        // 断言）：spawn 后 Child.stdin 应为 Some，否则 prompt 无法经 stdin 交付。
        {
            #[cfg(windows)]
            let probe_bin = "cmd";
            #[cfg(not(windows))]
            let probe_bin = "sh";
            let probe = super::build_claude_model_only_command(
                probe_bin,
                "claude-fixture",
                &[],
                &cwd,
                crate::reasoning::ReasoningEffort::Auto,
            )
            .unwrap()
            .spawn()
            .expect("探针子进程必须能启动");
            assert!(
                probe.stdin.is_some(),
                "model-only 命令的 stdin 必须是管道，否则 prompt 无法经 stdin 交付"
            );
        }

        // stdin 契约：spawn 真实子进程按字节计数回显，验证 >32K prompt 全量到达
        // 且写入后 stdin 正确关闭（否则子进程等不到 EOF，外层超时兜底失败）。
        #[cfg(windows)]
        let mut echo = {
            let mut c = tokio::process::Command::new("powershell");
            c.args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "$stdin=[Console]::OpenStandardInput();$ms=New-Object System.IO.MemoryStream;$buf=New-Object byte[] 65536;while(($n=$stdin.Read($buf,0,$buf.Length)) -gt 0){$ms.Write($buf,0,$n)};Write-Output $ms.ToArray().Length",
            ]);
            c.creation_flags(super::CREATE_NO_WINDOW);
            c
        };
        #[cfg(not(windows))]
        let mut echo = {
            let mut c = tokio::process::Command::new("sh");
            c.args(["-c", "exec wc -c"]);
            c
        };
        echo.stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let run = async {
            let mut child = echo.spawn().expect("回显子进程必须能启动");
            super::write_model_only_prompt(&mut child, &long_prompt, "test")
                .await
                .expect("prompt 必须经 stdin 全量写入");
            child.wait_with_output().await.expect("等待回显子进程失败")
        };
        let output = tokio::time::timeout(std::time::Duration::from_secs(60), run)
            .await
            .expect("stdin 未正确关闭会导致子进程等不到 EOF，超时即回归");
        assert!(
            output.status.success(),
            "回显子进程退出码异常：{:?}",
            output.status
        );
        let echoed = String::from_utf8_lossy(&output.stdout);
        let received_bytes: usize = echoed
            .trim()
            .parse()
            .unwrap_or_else(|_| panic!("回显子进程应只输出字节数，实际输出：{echoed}"));
        assert_eq!(
            received_bytes,
            long_prompt.len(),
            ">32K prompt 必须完整经 stdin 到达子进程，无截断无多余"
        );
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
    fn codex_file_change_add_output_produces_diff() {
        let output = "[{\"diff\":\"test1234\\n\",\"kind\":{\"type\":\"add\"},\"path\":\"D:\\\\repo\\\\test.txt\"}]";
        let diff = super::parse_codex_file_change_output(output).unwrap();
        assert_eq!(diff.path, "D:\\repo\\test.txt");
        assert_eq!(diff.hunks.len(), 1);
        assert_eq!(
            diff.hunks[0].lines[0],
            super::DiffLine {
                kind: super::DiffKind::Add,
                text: "test1234".to_string(),
            }
        );
    }

    #[test]
    fn codex_file_change_update_output_produces_diff() {
        let output =
            "[{\"diff\":\"@@ -1 +1 @@\\n-test\\n+test123\\n\",\"kind\":{\"move_path\":null,\"type\":\"update\"},\"path\":\"D:\\\\repo\\\\test.txt\"}]";
        let diff = super::parse_codex_file_change_output(output).unwrap();
        assert_eq!(diff.path, "D:\\repo\\test.txt");
        assert_eq!(diff.hunks.len(), 1);
        let kinds: Vec<_> = diff.hunks[0].lines.iter().map(|l| l.kind).collect();
        assert!(kinds.contains(&super::DiffKind::Del));
        assert!(kinds.contains(&super::DiffKind::Add));
    }

    #[test]
    fn codex_file_change_non_json_output_returns_none() {
        assert!(
            super::parse_codex_file_change_output("File created successfully at: C:\\x.txt")
                .is_none()
        );
        assert!(super::parse_codex_file_change_output("plain text").is_none());
    }

    #[test]
    fn codex_tool_result_from_item_carries_structured_diff() {
        let item = serde_json::json!({
            "type": "tool_call_output",
            "call_id": "call_x",
            "output": r#"[{"diff":"hello\n","kind":{"type":"add"},"path":"C:\\repo\\a.txt"}]"#,
        });
        let events = super::codex_events_from_completed_item("s-1", &item);
        assert!(events.iter().any(|event| matches!(
            event,
            super::AgentEvent::ToolResult { id, diff: Some(diff), .. }
                if id == "call_x" && diff.path == "C:\\repo\\a.txt"
        )));
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
                "model_providers.helm.env_key=OPENAI_API_KEY".to_string(),
                "model_providers.helm.requires_openai_auth=false".to_string(),
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
    fn codex_native_search_only_enables_for_responses_wire_api() {
        assert!(codex_native_search_enabled(&[]));
        assert!(codex_native_search_enabled(&[(
            "HELM_CODEX_WIRE_API".to_string(),
            "responses".to_string(),
        )]));
        assert!(!codex_native_search_enabled(&[(
            "HELM_CODEX_WIRE_API".to_string(),
            "chat".to_string(),
        )]));

        let mut command = tokio::process::Command::new("codex");
        assert!(apply_codex_native_search(&mut command, &[]));
        assert!(command
            .as_std()
            .get_args()
            .any(|argument| argument == "--search"));
        assert!(command
            .as_std()
            .get_args()
            .any(|argument| argument == "web_search=\"live\""));

        let mut chat_command = tokio::process::Command::new("codex");
        assert!(!apply_codex_native_search(
            &mut chat_command,
            &[("HELM_CODEX_WIRE_API".to_string(), "chat".to_string())]
        ));
        assert!(!chat_command
            .as_std()
            .get_args()
            .any(|argument| argument == "--search"));
    }

    #[test]
    fn codex_search_catalog_is_digest_bound_and_tamper_evident() {
        let home = std::env::temp_dir().join(format!(
            "helm-test-codex-search-catalog-{}-{}",
            std::process::id(),
            crate::util::now_millis()
        ));
        let catalog = r#"{"models":[{"slug":"gpt-test","use_responses_lite":false}]}"#;
        let env = vec![
            (
                CODEX_SEARCH_CATALOG_JSON_ENV.to_string(),
                catalog.to_string(),
            ),
            (
                CODEX_SEARCH_CATALOG_DIGEST_ENV.to_string(),
                format!("sha256:{}", crate::util::sha256_hex(catalog.as_bytes())),
            ),
        ];
        let mut command = tokio::process::Command::new("codex");
        let path = apply_codex_search_catalog(&mut command, &env, Some(&home))
            .unwrap()
            .unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), catalog);
        assert!(command.as_std().get_args().any(|argument| argument
            .to_string_lossy()
            .starts_with("model_catalog_json=")));

        fs::write(&path, r#"{"models":[]}"#).unwrap();
        let error = apply_codex_search_catalog(
            &mut tokio::process::Command::new("codex"),
            &env,
            Some(&home),
        )
        .unwrap_err();
        assert!(error.starts_with("[codex_search_catalog_tampered]"));
        let _ = fs::remove_dir_all(home);
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
    fn codex_probe_home_uses_launch_env_without_writing_key_and_cleans_up() {
        let home = create_codex_auth_home(
            &[("OPENAI_API_KEY".to_string(), "helm-runtime-key".to_string())],
            &[],
        )
        .unwrap()
        .unwrap();
        let path = home.path.clone();
        assert!(!path.join("auth.json").exists());
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
        assert!(!home.path.join("auth.json").exists());

        let path = home.path.clone();
        drop(home);
        assert!(!path.exists());
        let _ = fs::remove_dir_all(&source);
    }

    #[test]
    fn codex_probe_home_without_source_dir_still_keeps_key_off_disk() {
        let home = create_codex_auth_home_with_source(
            &[("OPENAI_API_KEY".to_string(), "helm-runtime-key".to_string())],
            None,
            &[],
        )
        .unwrap()
        .unwrap();
        assert!(!home.path.join("auth.json").exists());
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
    fn codex_runtime_profile_is_persistent_and_secret_free() {
        let root = std::env::temp_dir().join(format!(
            "helm-test-codex-runtime-profile-{}-{}",
            std::process::id(),
            crate::util::now_millis()
        ));
        let source = root.join("source");
        let workspace_a = root.join("workspace-a");
        let workspace_b = root.join("workspace-b");
        fs::create_dir_all(source.join("skills/demo")).unwrap();
        fs::create_dir_all(&workspace_a).unwrap();
        fs::create_dir_all(&workspace_b).unwrap();
        fs::write(
            source.join("config.toml"),
            concat!(
                "model = \"stale-model\"\n",
                "model_provider = \"stale-provider\"\n",
                "model_catalog_json = \"C:/stale/catalog.json\"\n",
                "profile = \"work\"\n\n",
                "[profiles.work]\n",
                "model = \"profile-model\"\n",
                "model_provider = \"profile-provider\"\n",
                "sandbox_mode = \"workspace-write\"\n\n",
                "[model_providers.stale-provider]\n",
                "base_url = \"https://provider-secret.example/v1\"\n",
                "wire_api = \"responses\"\n\n",
                "[windows]\n",
                "sandbox = \"elevated\"\n\n",
                "[mcp_servers.demo]\n",
                "command = \"demo\"\n",
            ),
        )
        .unwrap();
        fs::write(source.join("skills/demo/SKILL.md"), "# demo").unwrap();
        fs::write(source.join("auth.json"), "global-secret-sentinel").unwrap();
        let profile_history = SessionHistoryStore::new(root.join("profile.sqlite"));
        for (id, cwd) in [("workspace-a", &workspace_a), ("workspace-b", &workspace_b)] {
            profile_history
                .create_session(NewSessionRecord {
                    id: id.to_string(),
                    engine: EngineId::Codex,
                    model: "gpt-5".to_string(),
                    cwd: cwd.to_string_lossy().to_string(),
                    created_at: 1,
                })
                .unwrap();
        }
        let store = CodexRuntimeProfileStore::new(root.join("app-config"), profile_history.clone());

        let first = store
            .api_profile_with_source(Some(&source), &[], Some(&workspace_a))
            .unwrap();
        assert!(first.path.join("config.toml").is_file());
        assert!(first.path.join("engine-config.toml").is_file());
        assert!(first.path.join("skills/demo/SKILL.md").is_file());
        assert!(!first.path.join("auth.json").exists());
        let engine_config = fs::read_to_string(first.path.join("engine-config.toml")).unwrap();
        let engine_config = toml::from_str::<toml::Value>(&engine_config).unwrap();
        assert!(engine_config.get("model").is_none());
        assert!(engine_config.get("model_provider").is_none());
        assert!(engine_config.get("model_providers").is_none());
        assert!(engine_config.get("model_catalog_json").is_none());
        assert!(engine_config["profiles"]["work"].get("model").is_none());
        assert!(engine_config["profiles"]["work"]
            .get("model_provider")
            .is_none());
        assert_eq!(
            engine_config["windows"]["sandbox"].as_str(),
            Some("elevated")
        );
        assert_eq!(
            engine_config["mcp_servers"]["demo"]["command"].as_str(),
            Some("demo")
        );
        let rerouted = fs::read_to_string(source.join("config.toml"))
            .unwrap()
            .replace("stale-model", "new-model")
            .replace("profile-model", "new-profile-model")
            .replace("stale-provider", "new-provider")
            .replace("profile-provider", "new-profile-provider")
            .replace("provider-secret.example", "provider-new.example");
        fs::write(source.join("config.toml"), rerouted).unwrap();
        let same_engine_profile = store
            .api_profile_with_source(Some(&source), &[], Some(&workspace_a))
            .unwrap();
        assert_eq!(
            same_engine_profile, first,
            "Provider/Model 路由变化不得轮换持久 EngineProfile"
        );

        let warm = store
            .api_profile_with_source(Some(&source), &[], Some(&workspace_a))
            .unwrap();
        assert_eq!(warm, first);
        let second_workspace = store
            .api_profile_with_source(Some(&source), &[], Some(&workspace_b))
            .unwrap();
        assert_eq!(second_workspace, first);
        let runtime_config = fs::read_to_string(first.path.join("config.toml")).unwrap();
        let runtime_config = toml::from_str::<toml::Value>(&runtime_config).unwrap();
        let projects = runtime_config["projects"].as_table().unwrap();
        assert!(projects.contains_key(&super::codex_project_key(&workspace_a).unwrap()));
        assert!(projects.contains_key(&super::codex_project_key(&workspace_b).unwrap()));

        let clean_config = fs::read(first.path.join("config.toml")).unwrap();
        let mut runtime_ui_state =
            toml::from_str::<toml::Value>(std::str::from_utf8(&clean_config).unwrap()).unwrap();
        runtime_ui_state.as_table_mut().unwrap().insert(
            "notice".to_string(),
            toml::Value::Table(toml::map::Map::from_iter([(
                "model_migrations".to_string(),
                toml::Value::Table(toml::map::Map::from_iter([(
                    "gpt-5.3-codex".to_string(),
                    toml::Value::String("gpt-5.4".to_string()),
                )])),
            )])),
        );
        runtime_ui_state.as_table_mut().unwrap().insert(
            "tui".to_string(),
            toml::Value::Table(toml::map::Map::from_iter([(
                "model_availability_nux".to_string(),
                toml::Value::Table(toml::map::Map::from_iter([(
                    "gpt-5.6-sol".to_string(),
                    toml::Value::Integer(4),
                )])),
            )])),
        );
        runtime_ui_state
            .as_table_mut()
            .unwrap()
            .get_mut("projects")
            .and_then(toml::Value::as_table_mut)
            .unwrap()
            .insert(
                "c:\\runtime-discovered".to_string(),
                toml::Value::Table(toml::map::Map::from_iter([(
                    "trust_level".to_string(),
                    toml::Value::String("trusted".to_string()),
                )])),
            );
        fs::write(
            first.path.join("config.toml"),
            toml::to_string_pretty(&runtime_ui_state).unwrap(),
        )
        .unwrap();
        let runtime_ui_state_accepted = store
            .api_profile_with_source(Some(&source), &[], Some(&workspace_a))
            .unwrap();
        assert_eq!(runtime_ui_state_accepted, first);

        let mut invalid_project = runtime_ui_state;
        invalid_project
            .as_table_mut()
            .unwrap()
            .get_mut("projects")
            .and_then(toml::Value::as_table_mut)
            .unwrap()
            .get_mut("c:\\runtime-discovered")
            .and_then(toml::Value::as_table_mut)
            .unwrap()
            .insert(
                "trust_level".to_string(),
                toml::Value::String("untrusted".to_string()),
            );
        fs::write(
            first.path.join("config.toml"),
            toml::to_string_pretty(&invalid_project).unwrap(),
        )
        .unwrap();
        let invalid_project = store
            .api_profile_with_source(Some(&source), &[], Some(&workspace_a))
            .unwrap_err();
        assert!(invalid_project.starts_with("[codex_runtime_profile_tampered]"));

        fs::write(first.path.join("config.toml"), &clean_config).unwrap();
        fs::write(
            first.path.join("config.toml"),
            String::from_utf8(clean_config.clone())
                .unwrap()
                .replace("sandbox = \"elevated\"", "sandbox = \"danger-full-access\""),
        )
        .unwrap();
        let tampered = store
            .api_profile_with_source(Some(&source), &[], Some(&workspace_a))
            .unwrap_err();
        assert!(tampered.starts_with("[codex_runtime_profile_tampered]"));
        fs::write(first.path.join("config.toml"), clean_config).unwrap();
        let restarted_store =
            CodexRuntimeProfileStore::new(root.join("app-config"), profile_history);
        let restarted = restarted_store
            .api_profile_with_source(Some(&source), &[], Some(&workspace_a))
            .unwrap();
        assert_eq!(restarted, first);

        let disabled = store
            .api_profile_with_source(Some(&source), &["demo".to_string()], Some(&workspace_a))
            .unwrap();
        assert_ne!(disabled.revision, first.revision);
        assert!(!fs::read_to_string(disabled.path.join("config.toml"))
            .unwrap()
            .contains("mcp_servers.demo"));

        let _ = fs::remove_dir_all(root);
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
        || lower.contains("model may not exist or you may not have access")
        || lower.contains("model unavailable")
        || (lower.contains("model") && lower.contains("unavailable"))
        || lower.contains("model not found")
        || lower.contains("model does not exist")
        || lower.contains("model access")
        || lower.contains("模型不存在")
        || lower.contains("模型不可用")
        || lower.contains("模型授权")
    {
        "model_unavailable"
    } else if lower.contains("用量已达上限")
        || lower.contains("usagelimitexceeded")
        || lower.contains("usage limit")
        || lower.contains("sessionbudgetexceeded")
        || lower.contains("quota")
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
    } else if lower.contains("[codex_turn_pending_tools]") {
        "tool_result_missing"
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
            stalled_kind: None,
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
    let turn_start_ts = now_millis();
    let _busy_guard = TurnBusyGuard {
        runtime: runtime.clone(),
    };

    // 同一个 Claude session 不能并发跑多个 `claude -p`。审批恢复期间如果又发送新消息，
    // 必须等恢复轮次完整结束，否则 stdout 事件会交叉，UI 状态会卡在 working。
    let _turn_guard = runtime.turn_lock.lock().await;

    log_runtime_event(
        &runtime.app,
        "turn-start",
        &format!(
            "history={} resume={} prompt_len={} attachments={}",
            runtime.history_session_id,
            resume,
            prompt.as_ref().map(|p| p.chars().count()).unwrap_or(0),
            attachments.len(),
        ),
    );

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

    // 执行放行：不再对工作目录做独占 lease，同目录的多个会话可并行
    // （与 claude -p / codex 原生行为一致）；同文件竞争由 diff/检查点展示兜底。

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
        // 同引擎无损分支（十次反馈）：首轮带 --fork-session 把完整历史复制进新 CLI
        // 会话；成功后 init 事件回报新 session id 并清除标记，后续轮次回归普通 resume。
        if *runtime.pending_native_branch.lock().await {
            cmd.args(["--resume", &sid, "--fork-session"]);
        } else {
            cmd.args(["--resume", &sid]);
        }
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
                        // 将 CLI 回报的原生 session id 落库为 cli_session_id，使 `--fork-session`
                        // 无损分支在 start_session_branch 中可达（此前该字段从未落库，claude 分叉
                        // 被静默降级为摘要）；分支首轮 --fork-session 后会回报新 id 覆盖之。
                        if let Some(history_store) =
                            stdout_runtime.app.try_state::<SessionHistoryStore>()
                        {
                            let _ = history_store.attach_native_thread_to_session(
                                &stdout_runtime.history_session_id,
                                session_id,
                            );
                        }
                        // 无损分支首轮成功：CLI 已回报分支出的新 session id，
                        // 此后轮次回归普通 resume；失败时标记保留供用户重发重试。
                        if *stdout_runtime.pending_native_branch.lock().await {
                            *stdout_runtime.pending_native_branch.lock().await = false;
                        }
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
                                                stalled_kind: None,
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
                                            stalled_kind: None,
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
                            stalled_kind: None,
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
    // 认证/配额类失败（欠费、Key 失效、403）在 stream-json 模式下 CLI 可能长期
    // 挂住不退出，仅 stderr 有错误行；首次命中立即浮出错误，不等进程退出。
    let stderr_task = tokio::spawn(async move {
        let mut buf = String::new();
        let mut lines = BufReader::new(stderr).lines();
        let mut auth_error_reported = false;
        while let Ok(Some(line)) = lines.next_line().await {
            stderr_last_activity.store(now_millis() as u64, Ordering::Release);
            if !auth_error_reported
                && (line.contains("Failed to authenticate")
                    || line.contains("AccessDenied")
                    || line.contains("Current user is in debt")
                    || line.contains("API Error: 4"))
            {
                auth_error_reported = true;
                eprintln!(
                    "[helm] claude stderr auth/api failure surfaced early: {}",
                    &line[..line.len().min(200)]
                );
                emit_error(
                    &stderr_runtime,
                    format!("模型调用被拒绝（认证或账户问题）：{line}"),
                    false,
                )
                .await;
            }
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
        // 卡死兜底（修复「进程卡死时 UI 永远显示进行中」）：
        // 60s 无活动先标记 Stalled 提示用户；若持续 5 分钟仍无任何活动，
        // 判定为 CLI 进程卡死，主动杀掉进程并发 interrupted 终态，让前端正常收尾、
        // 用户可重新发送，而不是无限转圈。正常长轮次（生成长代码/跑测试等）不会触发。
        const IDLE_WARN_MS: u64 = 60_000;
        const IDLE_KILL_MS: u64 = 300_000;
        let mut stalled_emitted = false;
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            let idle =
                (now_millis() as u64).saturating_sub(watchdog_activity.load(Ordering::Acquire));
            if idle >= IDLE_WARN_MS && !stalled_emitted {
                stalled_emitted = true;
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
                log_runtime_event(
                    &watchdog_runtime.app,
                    "watchdog",
                    &format!(
                        "stalled: history={} 已 {}s 无活动，先提示「卡住」（未强杀，等待 {}s 仍无活动将强制终结）",
                        watchdog_runtime.history_session_id,
                        idle / 1000,
                        IDLE_KILL_MS / 1000,
                    ),
                );
            }
            if idle >= IDLE_KILL_MS {
                log_runtime_event(
                    &watchdog_runtime.app,
                    "watchdog",
                    &format!(
                        "force-kill: history={} 已 {}s 无活动，判定进程卡死，强制终结并发送 interrupted 终态",
                        watchdog_runtime.history_session_id,
                        idle / 1000,
                    ),
                );
                interrupt_running(watchdog_runtime.clone()).await;
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
    // 排查日志：进程退出这一刻是「案发现场」——记录退出码与各标志位、最终 disposition，
    // 日后「一直转圈/卡进行中」可直接从 helm-runtime.log 还原进程究竟怎么退出的。
    let current_turn = runtime.current_turn_id.lock().await.clone();
    log_runtime_event(
        &runtime.app,
        "turn-end",
        &format!(
            "history={} turn={} code={} interrupted={} saw_turn_complete={} has_candidate={} saw_approval={} disposition={:?} elapsed_ms={} stderr_len={} stderr_tail=<{}>",
            runtime.history_session_id,
            current_turn,
            code,
            runtime.interrupted.load(Ordering::Acquire),
            saw_turn_complete.load(Ordering::Acquire),
            terminal_candidate.is_some(),
            saw_approval.load(Ordering::Acquire),
            disposition,
            now_millis().saturating_sub(turn_start_ts),
            detail.len(),
            detail.chars().skip(detail.chars().count().saturating_sub(200)).collect::<String>(),
        ),
    );
    if disposition == ClaudeExitDisposition::Return && runtime.interrupted.load(Ordering::Acquire) {
        log_runtime_event(
            &runtime.app,
            "turn-end",
            "disposition=Return(interrupted)，终态由 interrupt_running 另行发出",
        );
        return;
    }
    if auto_fallback_requested.load(Ordering::Acquire) {
        runtime.auto_compat_attempted.store(true, Ordering::Release);
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
        log_runtime_event(
            &runtime.app,
            "emit",
            &format!(
                "emit=error(ProcessError) code={code} stderr_len={}",
                detail.len()
            ),
        );
        emit_error(
            &runtime,
            format!("claude 进程异常退出（code={code}）{cause}{suffix}"),
            false,
        )
        .await;
    } else if disposition == ClaudeExitDisposition::EmitCandidate {
        if let Some(event) = terminal_candidate.take() {
            log_runtime_event(&runtime.app, "emit", "emit=turn_complete(candidate)");
            emit_agent_event(&runtime.app, &runtime.history_session_id, &event);
        } else {
            log_runtime_event(
                &runtime.app,
                "emit",
                "emit=NONE(candidate 为 None，未发 turn_complete)",
            );
        }
    } else if disposition == ClaudeExitDisposition::MissingResult {
        // 进程正常退出但没有输出 result 行（且不是审批 defer 场景）：
        // 同样必须给出终态事件，否则 UI 悬空。
        log_runtime_event(&runtime.app, "emit", "emit=error(MissingResult)");
        emit_error(
            &runtime,
            "claude 进程已退出，但没有返回本轮结果（可能是 CLI 版本不兼容或输出被截断）"
                .to_string(),
            false,
        )
        .await;
    } else if disposition == ClaudeExitDisposition::ApprovalDeferred {
        // 进程正常退出但存在未决审批且无 result 候选：此前既不发 turn_complete 也不发
        // error，前端永远停在「进行中」。补一个终态，让 UI 收尾并提示用户重发。
        log_runtime_event(&runtime.app, "emit", "emit=error(ApprovalDeferred)");
        emit_error(
            &runtime,
            "claude 进程已退出，存在未决审批且未返回本轮结果，本轮已终止。请重新发送这条消息。"
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
    pending_native_branch: bool,
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
        pending_native_branch: Mutex::new(pending_native_branch),
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
                SessionCmd::Interrupt { responder } => {
                    interrupt_running(manager_runtime.clone()).await;
                    if let Some(responder) = responder {
                        let _ = responder.send(Ok(()));
                    }
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
            let native_item_id = request.item_id().to_string();
            approval_session.pending_approvals.lock().await.insert(
                approval_id.clone(),
                CodexPendingApproval {
                    request,
                    action: action.clone(),
                },
            );
            if let Some(tool) = approval_session
                .tool_item_facts
                .lock()
                .await
                .get_mut(&native_item_id)
            {
                tool.stage = PendingCodexToolStage::WaitingApproval;
                tool.last_progress_at = now_millis();
            }
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
    // 本轮是否出现过任何可见产出（助手文本 / 工具结果 / 文件变更）。空回复据此判定为失败。
    let mut saw_content = false;
    let mut last_output_tokens: Option<u64> = None;
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
                            && matches!(
                                item_type,
                                Some(
                                    "commandExecution"
                                        | "fileChange"
                                        | "webSearch"
                                        | "mcpToolCall"
                                        | "dynamicToolCall"
                                )
                            )
                        {
                            let now = now_millis();
                            notification_session.tool_item_facts.lock().await.insert(
                                id.to_string(),
                                PendingCodexTool {
                                    queued_at: now,
                                    started_at: now,
                                    last_progress_at: now,
                                    ended_at: None,
                                    stage: PendingCodexToolStage::Executing,
                                },
                            );
                        } else if method == Some("item/completed") {
                            if let Some(tool) = notification_session
                                .tool_item_facts
                                .lock()
                                .await
                                .get_mut(id)
                            {
                                let now = now_millis();
                                tool.last_progress_at = now;
                                tool.ended_at = Some(now);
                                tool.stage = PendingCodexToolStage::WaitingResult;
                            }
                        }
                    }
                    if item_type == Some("webSearch") && method == Some("item/completed") {
                        if let Some(registry) = notification_session
                            .app
                            .try_state::<crate::capability_registry::EngineCapabilityRegistry>(
                        ) {
                            let current = notification_session
                                .capability_snapshot
                                .lock()
                                .await
                                .clone();
                            let updated = if codex_web_search_item_unavailable(item) {
                                registry.record_web_search_unavailable(
                                    &current,
                                    "runtime_web_search_unavailable",
                                )
                            } else {
                                registry.record_web_search_native(&current)
                            };
                            if let Ok(updated) = updated {
                                let projection = updated.runtime_projection();
                                if let Err(error) = notification_session
                                    .app
                                    .try_state::<SessionHistoryStore>()
                                    .ok_or_else(|| "SessionHistoryStore 未启动".to_string())
                                    .and_then(|history| {
                                        history.persist_runtime_capabilities(
                                            &notification_session.history_session_id,
                                            Some(&projection),
                                        )
                                    })
                                {
                                    eprintln!(
                                        "[helm] 持久化 Codex WebSearch 能力观察失败：{error}"
                                    );
                                }
                                *notification_session.capability_snapshot.lock().await = updated;
                            }
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
            {
                let note_method = notification
                    .get("method")
                    .and_then(serde_json::Value::as_str);
                match note_method {
                    Some("turn/started") => {
                        saw_content = false;
                        last_output_tokens = None;
                    }
                    Some("thread/tokenUsage/updated") => {
                        last_output_tokens = notification
                            .pointer("/params/tokenUsage/last/outputTokens")
                            .and_then(serde_json::Value::as_u64);
                    }
                    Some("item/agentMessage/delta") => {
                        if notification
                            .pointer("/params/delta")
                            .and_then(serde_json::Value::as_str)
                            .map(|text| !text.is_empty())
                            .unwrap_or(false)
                        {
                            saw_content = true;
                        }
                    }
                    Some("item/completed") => {
                        let item_type = notification
                            .pointer("/params/item/type")
                            .and_then(serde_json::Value::as_str);
                        if matches!(
                            item_type,
                            Some(
                                "agentMessage"
                                    | "agent_message"
                                    | "commandExecution"
                                    | "fileChange"
                                    | "webSearch"
                                    | "mcpToolCall"
                                    | "dynamicToolCall"
                            )
                        ) {
                            saw_content = true;
                        }
                    }
                    _ => {}
                }
            }
            if let Some(item_id) = notification
                .pointer("/params/itemId")
                .and_then(serde_json::Value::as_str)
                .or_else(|| {
                    notification
                        .pointer("/params/item/id")
                        .and_then(serde_json::Value::as_str)
                })
            {
                if let Some(tool) = notification_session
                    .tool_item_facts
                    .lock()
                    .await
                    .get_mut(item_id)
                {
                    tool.last_progress_at = now_millis();
                    if notification
                        .get("method")
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|method| method.contains("output") || method.contains("delta"))
                    {
                        tool.stage = PendingCodexToolStage::WaitingResult;
                    }
                }
            }
            let pending_tool_summary = if notification
                .get("method")
                .and_then(serde_json::Value::as_str)
                == Some("turn/completed")
            {
                let tools = notification_session.tool_item_facts.lock().await;
                summarize_pending_codex_tools(&tools)
            } else {
                None
            };
            let terminal_outcome = codex_app_server_terminal_outcome_with_pending(
                &notification,
                pending_tool_summary.as_deref(),
                saw_content,
                last_output_tokens,
            );
            // `turn/interrupt` 会让 app-server 先回 `turn/completed`，此时等待审批或
            // 执行中的工具仍是 pending。用户 Stop 已先立下 interrupted 标志，不能再
            // 把这个原生完成通知改写成 pending-tools 失败；唯一终态由 Stop 路径提交。
            if codex_terminal_is_masked_by_interrupt(
                &notification,
                notification_session.interrupted.load(Ordering::Acquire),
            ) {
                continue;
            }
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
        log_runtime_event(
            &session.app,
            "emit",
            &format!("emit=codex_fail turn={} error=<{}>", turn_id, error),
        );
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
    fn emit_stage(&self, stage: TurnStage) {
        let session_id = self
            .thread_id
            .lock()
            .ok()
            .and_then(|guard| guard.clone())
            .unwrap_or_else(|| self.history_session_id.clone());
        emit_agent_event(
            &self.app,
            &self.history_session_id,
            &AgentEvent::TurnStage {
                session_id,
                stage,
                ts: now_millis(),
                engine_reported_ttft_ms: None,
                retry_attempt: None,
            },
        );
    }

    async fn emit_stage_without_blocking_runtime(&self, stage: TurnStage) -> Result<(), String> {
        let app = self.app.clone();
        let history_session_id = self.history_session_id.clone();
        let session_id = self
            .thread_id
            .lock()
            .ok()
            .and_then(|guard| guard.clone())
            .unwrap_or_else(|| history_session_id.clone());
        tokio::task::spawn_blocking(move || {
            emit_agent_event(
                &app,
                &history_session_id,
                &AgentEvent::TurnStage {
                    session_id,
                    stage,
                    ts: now_millis(),
                    engine_reported_ttft_ms: None,
                    retry_attempt: None,
                },
            );
        })
        .await
        .map_err(|error| format!("Codex 阶段事件持久化任务失败：{error}"))
    }

    async fn ensure_app_server(&self) -> Result<Arc<CodexAppServerProcess>, String> {
        let mut slot = self.app_server.lock().await;
        if let Some(process) = slot.as_ref() {
            return Ok(process.clone());
        }
        validate_cwd(&self.cwd)?;
        validate_engine_bin(&self.bin)?;
        let canonical_cwd = crate::sessions::strip_extended_path_prefix(
            &std::path::Path::new(&self.cwd)
                .canonicalize()
                .map_err(|error| format!("工作目录不可用：{error}"))?
                .to_string_lossy(),
        );
        *self.execution_cwd.lock().await = Some(canonical_cwd.clone());
        *self.policy_cwd.lock().await = canonical_cwd.clone();
        let configure_command = |command: &mut Command| {
            apply_inherited_agent_environment(command);
            for value in codex_provider_config_args(&self.env) {
                command.arg("-c").arg(value);
            }
            apply_codex_native_search(command, &self.env);
            command.current_dir(&canonical_cwd).kill_on_drop(true);
            let effective_home = self
                .effective_home
                .lock()
                .ok()
                .and_then(|home| home.clone());
            if let Some(path) = effective_home.as_ref() {
                command.env("CODEX_HOME", path);
            }
            apply_codex_search_catalog(command, &self.env, effective_home.as_deref())?;
            for (key, value) in &self.env {
                if !key.starts_with("HELM_") {
                    command.env(key, value);
                }
            }
            Ok::<(), String>(())
        };
        let mut command = build_codex_command(&self.bin);
        configure_command(&mut command)?;
        let process = Arc::new(spawn_codex_app_server(command).await?);
        *slot = Some(process.clone());
        drop(slot);
        spawn_codex_app_server_loops(self.clone(), process.clone());
        Ok(process)
    }

    /// 触发 Codex 原生上下文压缩（app-server `thread/compact/start`，2026-08-12 更正）。
    /// 只在无运行中 Turn 时允许（app-server 压缩期间 thread 保持 busy，避免与 Turn 并发）。
    /// 返回即表示已提交；进度由 app-server 以 `contextCompaction` item 生命周期通知上报，
    /// Helm 侧不伪造进度事件。
    async fn compact_context(&self) -> Result<(), String> {
        if self.busy.load(Ordering::Acquire) {
            return Err("当前轮次运行中，请先停止再压缩".to_string());
        }
        let thread_id = self
            .thread_id
            .lock()
            .ok()
            .and_then(|guard| guard.clone())
            .ok_or_else(|| "Codex 会话尚无原生 thread，无法压缩".to_string())?;
        let process = self.ensure_app_server().await?;
        process.rpc.compact_thread(&thread_id).await
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
        log_runtime_event(
            &self.app,
            "turn-start",
            &format!(
                "engine=codex helm_turn={} epoch={}",
                helm_turn_id, spec.turn_epoch
            ),
        );
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
        // 执行放行：不再对工作目录做独占 lease，同目录会话可并行。
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
        self.emit_stage(TurnStage::PreparingRuntime);
        let process = self.ensure_app_server().await?;
        if self.interrupted.load(Ordering::Acquire) {
            return Err("[turn_interrupted] Codex Runtime 准备已停止".to_string());
        }
        self.emit_stage_without_blocking_runtime(TurnStage::WaitingModel)
            .await?;
        let prompt = prompt_with_attachments(&prompt, &attachments);
        let prompt = if mode == TurnMode::Plan {
            format!("{CODEX_PLAN_PROMPT_PREFIX}\n\n{prompt}")
        } else {
            prompt
        };
        let runtime_capabilities = self.capability_snapshot.lock().await.runtime_projection();
        let native_search_enabled = codex_native_search_enabled(&self.env);
        log_runtime_line(
            "codex-turn-search",
            &format!(
                "native_search_enabled={} runtime_web_search={:?} prompt_branch_native={}",
                native_search_enabled,
                runtime_capabilities.web_search,
                matches!(
                    (native_search_enabled, runtime_capabilities.web_search),
                    (true, _)
                )
            ),
        );
        let prompt = match (native_search_enabled, runtime_capabilities.web_search) {
            (true, RuntimeCapabilityAvailability::Available) => prompt,
            (true, RuntimeCapabilityAvailability::Unknown) => format!(
                "[Helm Runtime 能力约束]\n当前 Codex Runtime 已启用原生 WebSearch，但本 Provider/Model 尚无成功观察证据。需要联网时必须直接调用一次原生 WebSearch；不得读取 Skill、使用 shell、Get-Location 或其他探针代替。只有 Runtime 明确返回缺工具或不支持时，才返回 [runtime_web_search_unavailable]。\n\n{prompt}"
            ),
            (_, RuntimeCapabilityAvailability::Unavailable) | (false, _) => format!(
                "[Helm Runtime 能力约束]\n当前 Runtime 已证明不提供联网搜索。不得用 shell、Get-Location 或 Skill 读取探测网络；需要联网的请求直接返回 [runtime_web_search_unavailable]。\n\n{prompt}"
            ),
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
            let fork_source = self
                .fork_source_thread_id
                .lock()
                .map(|g| g.clone())
                .unwrap_or_default();
            let fork_last_turn = self
                .fork_last_turn_id
                .lock()
                .map(|g| g.clone())
                .unwrap_or_default();
            let thread_id = match codex_app_server_thread_plan(
                existing_thread.as_deref(),
                fork_source.as_deref(),
                fork_last_turn.as_deref(),
                force_rebuild,
            ) {
                CodexAppServerThreadPlan::Fork(source, last_turn) => {
                    let forked = process
                        .rpc
                        .fork_thread(&source, last_turn.as_deref())
                        .await?;
                    // 分支已拥有独立线程：消费来源行，后续轮次回归普通 resume(forked)。
                    if let Some(history_store) = self.app.try_state::<SessionHistoryStore>() {
                        let _ = history_store.clear_session_native_branch(&self.history_session_id);
                        let _ = history_store
                            .attach_native_thread_to_session(&self.history_session_id, &forked);
                    }
                    if let Ok(mut guard) = self.fork_source_thread_id.lock() {
                        *guard = None;
                    }
                    if let Ok(mut guard) = self.fork_last_turn_id.lock() {
                        *guard = None;
                    }
                    forked
                }
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
                                    stalled_kind: None,
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
        self.tool_item_facts.lock().await.clear();
        log_runtime_event(
            &self.app,
            "codex-rpc",
            &format!("method=turn/start thread={thread_id} model={routed_model}"),
        );
        let native_turn_id = match process
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
            .await
        {
            Ok(turn_id) => {
                log_runtime_event(
                    &self.app,
                    "codex-rpc",
                    &format!("method=turn/start ok native_turn={turn_id}"),
                );
                turn_id
            }
            Err(error) => {
                log_runtime_event(
                    &self.app,
                    "codex-rpc",
                    &format!("method=turn/start fail error=<{error}>"),
                );
                return Err(error);
            }
        };
        // 切点分叉依赖：把原生轮次 id 落库到本轮（turn.native_turn_id），后续从
        // 对话内分叉时可解析「被点回答 → 原生轮」并作为 thread/fork 的 lastTurnId。
        // 落库失败不阻断轮次（切点分叉会安全回退为整段分叉）。
        if let Some(history_store) = self.app.try_state::<SessionHistoryStore>() {
            let _ = history_store.set_turn_native_id(
                &self.history_session_id,
                &spec.turn_id,
                &native_turn_id,
            );
        }
        self.native_turn_contexts.lock().await.insert(
            native_turn_id.clone(),
            helm_turn_id,
            spec.turn_epoch,
        );
        *self.current_app_server_turn_id.lock().await = Some(native_turn_id.clone());
        let terminal_wait_started = tokio::time::Instant::now();
        let mut tool_stall_reported = false;
        let mut turn_stall_reported = false;
        let mut turn_terminal_reported = false;
        let mut pending_terminal_failure = false;
        loop {
            let notified = self.terminal_notify.notified();
            if let Some(outcome) =
                terminal_turn_outcome(&self.terminal_turns, &native_turn_id).await
            {
                match outcome {
                    Ok(()) => break,
                    Err(error) if error.starts_with(CODEX_TURN_FAILED_PREFIX) => {
                        pending_terminal_failure = error.contains("[codex_turn_pending_tools]");
                        break;
                    }
                    Err(error) => return Err(error),
                }
            }
            if tokio::time::timeout(std::time::Duration::from_secs(1), notified)
                .await
                .is_err()
            {
                let now = now_millis();
                let stalled_tools = {
                    let tools = self.tool_item_facts.lock().await;
                    tools
                        .values()
                        .any(|tool| {
                            pending_codex_tool_is_stalled(tool, now, RUNTIME_TOOL_STALLED_AFTER)
                        })
                        .then(|| summarize_pending_codex_tools(&tools))
                        .flatten()
                };
                if !tool_stall_reported && stalled_tools.is_some() {
                    tool_stall_reported = true;
                    self.emit_stage(TurnStage::Stalled);
                    let stalled_kind = {
                        let tools = self.tool_item_facts.lock().await;
                        stalled_codex_tool_kind(&tools)
                    };
                    emit_agent_event(
                        &self.app,
                        &self.history_session_id,
                        &AgentEvent::Error {
                            session_id: Some(thread_id.clone()),
                            message: if stalled_kind == Some("waiting_approval") {
                                "[runtime_tool_stalled] 有一项操作正在等待你的确认".to_string()
                            } else {
                                format!(
                                    "[runtime_tool_stalled] Codex 工具 60 秒没有新进展：{}",
                                    stalled_tools.unwrap_or_default()
                                )
                            },
                            recoverable: true,
                            kind: Some("tool_stalled".to_string()),
                            stalled_kind: stalled_kind.map(str::to_string),
                        },
                    );
                } else if !turn_stall_reported
                    && terminal_wait_started.elapsed() >= Duration::from_secs(60)
                {
                    turn_stall_reported = true;
                    self.emit_stage(TurnStage::Stalled);
                    log_runtime_event(
                        &self.app,
                        "turn-stall",
                        &format!("engine=codex turn={} elapsed_s=60", native_turn_id),
                    );
                } else if !turn_terminal_reported
                    && terminal_wait_started.elapsed() >= Duration::from_secs(300)
                {
                    turn_terminal_reported = true;
                    log_runtime_event(
                        &self.app,
                        "turn-force-kill",
                        &format!(
                            "engine=codex turn={} elapsed_s=300 reason=no_terminal_in_5min",
                            native_turn_id
                        ),
                    );
                    if let Some(stale_process) = self.app_server.lock().await.take() {
                        stale_process.shutdown().await;
                    }
                    self.app_server_thread_ready.store(false, Ordering::Release);
                    self.pending_approvals.lock().await.clear();
                    self.tool_item_facts.lock().await.clear();
                    fail_codex_active_turn(self, "Codex 本轮超过 5 分钟无任何终态事件，已强制终止")
                        .await;
                }
            }
        }
        *self.current_app_server_turn_id.lock().await = None;
        if pending_terminal_failure {
            if let Some(stale_process) = self.app_server.lock().await.take() {
                stale_process.shutdown().await;
            }
            self.app_server_thread_ready.store(false, Ordering::Release);
            self.pending_approvals.lock().await.clear();
            self.tool_item_facts.lock().await.clear();
        }
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
        if let Some(tool) = self
            .tool_item_facts
            .lock()
            .await
            .get_mut(pending.request.item_id())
        {
            tool.stage = PendingCodexToolStage::Executing;
            tool.last_progress_at = now_millis();
        }
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
        let turn_task = self.turn_task.clone();
        self.interrupted.store(false, Ordering::Release);
        let task = tauri::async_runtime::spawn(async move {
            if let Err(error) = session.run_app_server_turn(text, attachments, spec).await {
                if session.interrupted.load(Ordering::Acquire) {
                    busy.store(false, Ordering::Release);
                    return;
                }
                let error_kind = codex_error_kind(&error).to_string();
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
                        kind: Some(error_kind),
                        stalled_kind: None,
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
            if let Ok(mut slot) = turn_task.lock() {
                slot.take();
            }
        });
        if let Ok(mut slot) = self.turn_task.lock() {
            *slot = Some(task);
        }
    }

    async fn interrupt_and_wait(&self) -> Result<(), String> {
        let process = self.app_server.clone();
        let app = self.app.clone();
        let history_session_id = self.history_session_id.clone();
        let thread_id = self.thread_id.clone();
        let turn_id = self.current_app_server_turn_id.clone();
        let terminal_turns = self.terminal_turns.clone();
        let terminal_notify = self.terminal_notify.clone();
        let running_pid = self.running_pid.clone();
        let app_server_thread_ready = self.app_server_thread_ready.clone();
        // 先立标志再杀进程：app-server 轮次收尾时据此改发 TurnComplete{Interrupted}
        self.interrupted.store(true, Ordering::Release);
        let live_process = process.lock().await.as_ref().cloned();
        if let (Some(process), Some(thread_id), Some(turn_id)) = (
            live_process.as_ref(),
            thread_id.lock().ok().and_then(|guard| guard.clone()),
            turn_id.lock().await.clone(),
        ) {
            let _ = tokio::time::timeout(
                CODEX_INTERRUPT_RPC_GRACE,
                process.rpc.request(
                    "turn/interrupt",
                    serde_json::json!({"threadId":thread_id,"turnId":turn_id}),
                ),
            )
            .await;
        }
        let pid = *running_pid.lock().await;
        kill_tree(pid).await;
        set_running_pid(&running_pid, None).await;
        if let Some(stale_process) = process.lock().await.take() {
            stale_process.shutdown().await;
        }
        app_server_thread_ready.store(false, Ordering::Release);
        let active_turn_id = turn_id.lock().await.clone();
        finish_codex_interrupt_terminal(&terminal_turns, &terminal_notify, active_turn_id, || {
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
        })
        .await;
        let drained = tokio::time::timeout(CODEX_INTERRUPT_TASK_DRAIN_TIMEOUT, async {
            while self.busy.load(Ordering::Acquire) {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .is_ok();
        if !drained {
            // Runtime 已经被终止但 app-server RPC 仍可能卡住；取消 Turn 任务本身，
            // 确保任务内持有的 RAII 资源确定释放。
            abort_codex_turn_task(
                &self.turn_task,
                &self.busy,
                CODEX_INTERRUPT_TASK_DRAIN_TIMEOUT,
            )
            .await?;
        }
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
    fork_source_thread_id: Option<String>,
    fork_last_turn_id: Option<String>,
    auth_home: Option<CodexAuthHome>,
    subscription_home: Option<PathBuf>,
    capability_snapshot: crate::capability_registry::EngineCapabilitySnapshot,
    _reasoning_effort: ReasoningEffort,
) -> Result<AgentSession, String> {
    let canonical_cwd = crate::sessions::strip_extended_path_prefix(
        &std::path::Path::new(&cwd)
            .canonicalize()
            .map_err(|error| format!("工作目录不可用：{error}"))?
            .to_string_lossy(),
    );
    let (turn_completions, _) = broadcast::channel(32);
    let runtime_profile = if env
        .iter()
        .any(|(key, value)| key == "OPENAI_API_KEY" && !value.trim().is_empty())
    {
        app.try_state::<CodexRuntimeProfileStore>()
            .map(|store| store.api_profile(&env, &[], Path::new(&canonical_cwd)))
            .transpose()?
            .flatten()
    } else {
        None
    };
    if runtime_profile.is_some() && auth_home.is_some() {
        return Err(
            "Codex API Runtime 不得同时持有临时认证 Profile 与持久 EngineProfile".to_string(),
        );
    }
    let session_home = runtime_profile
        .as_ref()
        .map(|profile| profile.path.clone())
        .or_else(|| auth_home.as_ref().map(|home| home.path.clone()));
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
        fork_source_thread_id: Arc::new(std::sync::Mutex::new(fork_source_thread_id)),
        fork_last_turn_id: Arc::new(std::sync::Mutex::new(fork_last_turn_id)),
        auth_home: Arc::new(std::sync::Mutex::new(auth_home)),
        effective_home: Arc::new(std::sync::Mutex::new(effective_home)),
        force_history_rebuild: Arc::new(AtomicBool::new(false)),
        app_server: Arc::new(Mutex::new(None)),
        app_server_thread_ready: Arc::new(AtomicBool::new(false)),
        pending_approvals: Arc::new(Mutex::new(HashMap::new())),
        file_changes_by_item: Arc::new(Mutex::new(HashMap::new())),
        tool_item_facts: Arc::new(Mutex::new(HashMap::new())),
        turn_completions,
        turn_task: Arc::new(std::sync::Mutex::new(None)),
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

fn codex_error_kind(error: &str) -> &'static str {
    if error.starts_with("[codex_turn_pending_tools]") {
        "tool_result_missing"
    } else {
        "process_crash"
    }
}

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

fn codex_empty_turn_error(output_tokens: Option<u64>) -> String {
    // 服务端以「完成」结束但没有任何可见产出（空回复）。最常见于中转对 Responses API
    // 返回了空流，Codex 把它当成成功。这里直接把事实亮出来，不做猜测式包装。
    match output_tokens {
        Some(0) => "[codex_turn_empty] 模型本轮未返回任何内容（outputTokens=0）。若服务方对 Responses API 返回了空流，会表现为「成功但无输出」，请检查模型与中转配置。".to_string(),
        Some(n) => format!("[codex_turn_empty] 本轮以「完成」结束但未包含可见回复（outputTokens={n}）。"),
        None => "[codex_turn_empty] 本轮以「完成」结束，但没有产生任何助手回复或工具结果。".to_string(),
    }
}

fn codex_app_server_terminal_outcome_with_pending(
    notification: &serde_json::Value,
    pending_tool_summary: Option<&str>,
    saw_content: bool,
    output_tokens: Option<u64>,
) -> Option<(String, Result<(), String>)> {
    let (turn_id, outcome) = codex_app_server_terminal_outcome(notification)?;
    if outcome.is_ok() {
        if let Some(summary) = pending_tool_summary {
            return Some((
                turn_id,
                Err(format!(
                    "{CODEX_TURN_FAILED_PREFIX} [codex_turn_pending_tools] Codex Turn 已结束，但仍有未完成工具：{summary}"
                )),
            ));
        }
        if !saw_content {
            return Some((turn_id, Err(codex_empty_turn_error(output_tokens))));
        }
    }
    Some((turn_id, outcome))
}

fn codex_terminal_is_masked_by_interrupt(
    notification: &serde_json::Value,
    interrupted: bool,
) -> bool {
    interrupted && codex_app_server_terminal_outcome(notification).is_some()
}

fn summarize_pending_codex_tools(tools: &HashMap<String, PendingCodexTool>) -> Option<String> {
    let pending = tools
        .values()
        .filter(|tool| tool.ended_at.is_none())
        .collect::<Vec<_>>();
    if pending.is_empty() {
        return None;
    }
    let mut stages = HashMap::<&'static str, usize>::new();
    let mut oldest_queued_at = i64::MAX;
    let mut oldest_started_at = i64::MAX;
    for tool in &pending {
        *stages.entry(tool.stage.as_str()).or_default() += 1;
        oldest_queued_at = oldest_queued_at.min(tool.queued_at);
        oldest_started_at = oldest_started_at.min(tool.started_at);
    }
    let mut stage_counts = stages.into_iter().collect::<Vec<_>>();
    stage_counts.sort_by_key(|(stage, _)| *stage);
    let stages = stage_counts
        .into_iter()
        .map(|(stage, count)| format!("{stage}={count}"))
        .collect::<Vec<_>>()
        .join(",");
    Some(format!(
        "count={}, stages={}, oldestQueuedMs={}, oldestStartedMs={}",
        pending.len(),
        stages,
        now_millis().saturating_sub(oldest_queued_at),
        now_millis().saturating_sub(oldest_started_at),
    ))
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
        match terminal_turns.get(active_turn_id) {
            Some(Err(error)) if error == "Codex app-server notification stream closed" => {
                terminal_turns.insert(active_turn_id.clone(), Ok(()));
                true
            }
            Some(_) => false,
            None => record_codex_terminal_once(&mut terminal_turns, active_turn_id.clone(), Ok(())),
        }
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
    // 直接把模型/Codex 返回的原始报错（脱敏后）展示给用户，不再逐场景包装成中文套话。
    let error = notification
        .pointer("/params/turn/error")
        .or_else(|| notification.pointer("/params/error"));
    let mut parts: Vec<String> = Vec::new();
    if let Some(message) = error
        .and_then(|e| e.get("message"))
        .and_then(serde_json::Value::as_str)
    {
        if !message.trim().is_empty() {
            parts.push(message.trim().to_string());
        }
    }
    if let Some(details) = error
        .and_then(|e| e.get("additionalDetails"))
        .and_then(serde_json::Value::as_str)
    {
        if !details.trim().is_empty() {
            parts.push(details.trim().to_string());
        }
    }
    if let Some(info) = error.and_then(|e| e.get("codexErrorInfo")) {
        if !info.is_null() {
            parts.push(serde_json::to_string_pretty(info).unwrap_or_else(|_| info.to_string()));
        }
    }
    let combined = parts.join("\n\n");
    if combined.trim().is_empty() {
        "Codex 以失败状态结束，但没有返回可展示的错误详情。".to_string()
    } else {
        crate::redaction::redact_text(&combined)
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
                let mut message = codex_app_server_failure_message(notification);
                if let Some(stripped) = message.strip_prefix(CODEX_TURN_FAILED_PREFIX) {
                    message = stripped.trim_start().to_string();
                }
                events.push(AgentEvent::Error {
                    session_id: Some(session_id.to_string()),
                    kind: classify_error(&message),
                    message,
                    recoverable: true,
                    stalled_kind: None,
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
        // P0-04：Codex contextCompaction item 开始 → running。
        // app-server 不发独立的 item/started for contextCompaction（某些版本只发 completed），
        // 但若收到 started，即标记 running。
        Some("contextCompaction") => {
            let id = codex_item_id(item)
                .or_else(|| {
                    item.get("itemId")
                        .and_then(serde_json::Value::as_str)
                        .map(ToString::to_string)
                })
                .unwrap_or_else(|| format!("compact-{}", now_millis()));
            vec![AgentEvent::ContextCompaction {
                session_id: session_id.to_string(),
                id,
                status: ContextCompactionStatus::Running,
                ts: now_millis(),
                summary: None,
                error: None,
            }]
        }
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
        Some("webSearch") => {
            let mut events = vec![codex_turn_stage(session_id, TurnStage::UsingTool)];
            if let Some(id) = codex_item_id(item) {
                events.push(AgentEvent::ToolCall {
                    session_id: session_id.to_string(),
                    id,
                    name: "WebSearch".to_string(),
                    input: serde_json::json!({
                        "query": item.get("query").cloned().unwrap_or_default(),
                        "action": item.get("action").cloned().unwrap_or_default(),
                    }),
                    status: CallStatus::Pending,
                });
            }
            events
        }
        Some("mcpToolCall") => {
            let mut events = vec![codex_turn_stage(session_id, TurnStage::UsingTool)];
            if let Some(id) = codex_item_id(item) {
                let server = item
                    .get("server")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("MCP");
                let tool = item
                    .get("tool")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("tool");
                events.push(AgentEvent::ToolCall {
                    session_id: session_id.to_string(),
                    id,
                    name: format!("{server}/{tool}"),
                    input: item.get("arguments").cloned().unwrap_or_default(),
                    status: CallStatus::Pending,
                });
            }
            events
        }
        Some("dynamicToolCall") => {
            let mut events = vec![codex_turn_stage(session_id, TurnStage::UsingTool)];
            if let Some(id) = codex_item_id(item) {
                events.push(AgentEvent::ToolCall {
                    session_id: session_id.to_string(),
                    id,
                    name: item
                        .get("tool")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("DynamicTool")
                        .to_string(),
                    input: item.get("arguments").cloned().unwrap_or_default(),
                    status: CallStatus::Pending,
                });
            }
            events
        }
        _ => Vec::new(),
    }
}

fn codex_web_search_item_unavailable(item: &serde_json::Value) -> bool {
    let failed_status = matches!(
        item.get("status").and_then(serde_json::Value::as_str),
        Some("failed" | "declined")
    );
    let error = item
        .get("error")
        .filter(|value| !value.is_null())
        .map(ToString::to_string)
        .unwrap_or_default()
        .to_ascii_lowercase();
    failed_status
        && (error.contains("unsupported")
            || error.contains("unavailable")
            || error.contains("unknown tool")
            || error.contains("not supported"))
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
            let output = item
                .get("changes")
                .and_then(|changes| serde_json::to_string(changes).ok());
            let diff = output.as_deref().and_then(parse_codex_file_change_output);
            vec![AgentEvent::ToolResult {
                session_id: session_id.to_string(),
                id,
                status: if failed {
                    ToolStatus::Error
                } else {
                    ToolStatus::Success
                },
                output,
                diff,
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
        Some("webSearch") => {
            let Some(id) = codex_item_id(item) else {
                return Vec::new();
            };
            let failed = matches!(
                item.get("status").and_then(serde_json::Value::as_str),
                Some("failed" | "declined")
            ) || item.get("error").is_some_and(|value| !value.is_null());
            let unavailable = codex_web_search_item_unavailable(item);
            let output = item
                .get("results")
                .filter(|value| !value.is_null())
                .or_else(|| item.get("error").filter(|value| !value.is_null()))
                .and_then(|value| serde_json::to_string(value).ok());
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
                has_output: Some(output.is_some()),
                retryable: Some(false),
                denial_source: failed.then_some(crate::protocol::ToolDenialSource::Tool),
                native_denial_code: unavailable
                    .then_some("runtime_web_search_unavailable".to_string()),
            }]
        }
        Some("mcpToolCall" | "dynamicToolCall") => {
            let Some(id) = codex_item_id(item) else {
                return Vec::new();
            };
            let failed = item.get("status").and_then(serde_json::Value::as_str) == Some("failed")
                || item.get("success").and_then(serde_json::Value::as_bool) == Some(false)
                || item.get("error").is_some_and(|value| !value.is_null());
            let output_value = item
                .get("result")
                .filter(|value| !value.is_null())
                .or_else(|| item.get("contentItems").filter(|value| !value.is_null()))
                .or_else(|| item.get("error").filter(|value| !value.is_null()));
            let output = output_value.and_then(|value| serde_json::to_string(value).ok());
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
                has_output: Some(output.is_some()),
                retryable: Some(false),
                denial_source: failed.then_some(crate::protocol::ToolDenialSource::Tool),
                native_denial_code: None,
            }]
        }
        // P0-04：Codex contextCompaction item 完成 → succeeded/failed。
        // 只使用 app-server 真实上报的字段；summary 缺省不补写虚构内容。
        Some("contextCompaction") => {
            let id = codex_item_id(item)
                .or_else(|| {
                    item.get("itemId")
                        .and_then(serde_json::Value::as_str)
                        .map(ToString::to_string)
                })
                .unwrap_or_else(|| format!("compact-{}", now_millis()));
            let status_str = item.get("status").and_then(serde_json::Value::as_str);
            let failed = matches!(status_str, Some("failed" | "declined"));
            let summary = item
                .get("summary")
                .and_then(serde_json::Value::as_str)
                .map(ToString::to_string)
                .or_else(|| {
                    // 某些版本把摘要放在 output/lastSummary 字段
                    item.get("lastSummary")
                        .and_then(serde_json::Value::as_str)
                        .map(ToString::to_string)
                });
            let error = item
                .get("error")
                .filter(|value| !value.is_null())
                .map(|value| value.to_string());
            vec![AgentEvent::ContextCompaction {
                session_id: session_id.to_string(),
                id,
                status: if failed {
                    ContextCompactionStatus::Failed
                } else {
                    ContextCompactionStatus::Succeeded
                },
                ts: now_millis(),
                summary,
                error,
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
                    stalled_kind: None,
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
        .or_else(|| output.as_deref().and_then(parse_codex_file_change_output))
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

/// 解析 Codex Write/Edit 工具的 fileChange 结构化输出为 Diff。
///
/// Codex 对 `write_file`/`edit_file` 的工具结果 output 是 JSON 数组，例如：
/// ```json
/// [{"diff":"test1234\n","kind":{"type":"add"},"path":"D:\\repo\\test.txt"}]
/// [{"diff":"@@ -1 +1 @@\n-old\n+new","kind":{"type":"update"},"path":"D:\\repo\\test.txt"}]
/// ```
/// - `type=add`：diff 是文件全量新内容（无 hunk 头），按新增整文件构造 hunk。
/// - `type=update`：diff 是 unified diff 文本，交给 `parse_unified_diff` 但路径取自外层 `path` 字段。
fn parse_codex_file_change_output(output: &str) -> Option<Diff> {
    let value: serde_json::Value = serde_json::from_str(output).ok()?;
    let entries = match value {
        serde_json::Value::Array(items) => items,
        serde_json::Value::Object(_) => vec![value],
        _ => return None,
    };
    let mut result: Option<Diff> = None;
    for entry in entries {
        let Some(path) = entry.get("path").and_then(serde_json::Value::as_str) else {
            continue;
        };
        let diff_text = entry
            .get("diff")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        if diff_text.is_empty() {
            continue;
        }
        let kind = entry
            .get("kind")
            .and_then(|kind| kind.get("type"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("update");
        if kind == "add" {
            let mut lines: Vec<DiffLine> = Vec::new();
            if diff_text.is_empty() {
                continue;
            }
            for line in diff_text.lines() {
                lines.push(DiffLine {
                    kind: DiffKind::Add,
                    text: line.to_string(),
                });
            }
            if lines.is_empty() {
                lines.push(DiffLine {
                    kind: DiffKind::Add,
                    text: String::new(),
                });
            }
            result = Some(Diff {
                path: path.to_string(),
                hunks: vec![DiffHunk {
                    old_start: 0,
                    new_start: 1,
                    lines,
                }],
            });
        } else {
            if let Some(parsed) = parse_unified_diff_with_fallback(diff_text, Some(path)) {
                result = Some(parsed);
            }
        }
    }
    result
}

fn parse_unified_diff(text: &str) -> Option<Diff> {
    parse_unified_diff_with_fallback(text, None)
}

/// 解析 unified diff 文本；若文本缺 `+++` 头（Codex update 输出格式不含文件头），
/// 使用 `fallback_path` 作为 path。
fn parse_unified_diff_with_fallback(text: &str, fallback_path: Option<&str>) -> Option<Diff> {
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

    if path.is_empty() {
        path = fallback_path.unwrap_or("").trim().to_string();
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
    fn codex_failed_turn_classifies_structured_details_and_shows_raw() {
        let notification = serde_json::json!({"method":"turn/completed","params":{"turn":{
        "id":"turn-2","status":"failed","error":{
            "message":"request rejected",
            "additionalDetails":"model gpt-private is unavailable at https://example.com",
            "codexErrorInfo":"badRequest"
        }}}});
        let events = parse_codex_app_server_notification("codex-session", &notification);
        let error = serde_json::to_value(&events[0]).unwrap();
        assert_eq!(error["kind"], "model_unavailable");
        let message = error["message"].as_str().unwrap();
        // 原始报错（含结构化详情）原样展示，便于用户直接定位。
        assert!(message.contains("request rejected"));
        assert!(message.contains("model gpt-private is unavailable"));
        assert!(message.contains("badRequest"));
    }

    #[test]
    fn codex_failed_turn_classifies_structured_usage_limit() {
        let notification = serde_json::json!({"method":"turn/completed","params":{"turn":{
        "id":"turn-3","status":"failed","error":{
            "message":"request rejected","codexErrorInfo":"usageLimitExceeded"
        }}}});
        let events = parse_codex_app_server_notification("codex-session", &notification);
        let error = serde_json::to_value(&events[0]).unwrap();
        let message = error["message"].as_str().unwrap();
        assert!(message.contains("request rejected"));
        assert!(message.contains("usageLimitExceeded"));
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
                        "message":"model unavailable at https://example.com/v1 api_key=sk-HELM_TEST_SECRET_123456 Authorization: Bearer sklAbCdEfGhIjKlMnOpQrStUvWxYz0123456789"
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
        assert!(safe_message.contains("model unavailable"));
        assert!(!safe_message.contains("sk-HELM_TEST_SECRET_123456"));
        assert!(!safe_message.contains("sklAbCdEfGhIjKlMnOpQrStUvWxYz0123456789"));
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
        let (_, outcome) = super::codex_app_server_terminal_outcome_with_pending(
            &notification,
            Some("count=1, stages=executing=1"),
            true,
            None,
        )
        .expect("turn completion should be recognized");
        let error = outcome.expect_err("unfinished tool must fail closed");
        assert!(error.starts_with(super::CODEX_TURN_FAILED_PREFIX));
        assert!(error.contains("[codex_turn_pending_tools]"));
        let adjusted =
            super::codex_notification_for_terminal_outcome(&notification, Some(&Err(error)));
        let events = parse_codex_app_server_notification("codex-session", &adjusted)
            .into_iter()
            .map(|event| serde_json::to_value(event).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(events[0]["type"], "error");
        let message = events[0]["message"].as_str().unwrap();
        assert!(message.contains("[codex_turn_pending_tools]"));
        assert!(message.contains("stages=executing=1"));
        assert_eq!(events[0]["kind"], "tool_result_missing");
        assert_eq!(events[1]["type"], "turn_complete");
        assert_eq!(events[1]["stopReason"], "error");
    }

    #[test]
    fn codex_interrupt_masks_native_completion_with_pending_tools() {
        let notification = serde_json::json!({
            "method": "turn/completed",
            "params": {"turn": {"id": "turn-1", "status": "completed"}}
        });

        assert!(super::codex_terminal_is_masked_by_interrupt(
            &notification,
            true
        ));
        assert!(!super::codex_terminal_is_masked_by_interrupt(
            &notification,
            false
        ));
        assert!(!super::codex_terminal_is_masked_by_interrupt(
            &serde_json::json!({"method": "item/completed"}),
            true
        ));
    }

    #[test]
    fn codex_tool_fact_summary_only_counts_items_without_ended_at() {
        let now = crate::util::now_millis();
        let mut tools = HashMap::new();
        tools.insert(
            "done".to_string(),
            super::PendingCodexTool {
                queued_at: now - 20,
                started_at: now - 10,
                last_progress_at: now,
                ended_at: Some(now),
                stage: super::PendingCodexToolStage::WaitingResult,
            },
        );
        assert_eq!(super::summarize_pending_codex_tools(&tools), None);
        tools.insert(
            "pending".to_string(),
            super::PendingCodexTool {
                queued_at: now - 30,
                started_at: now - 25,
                last_progress_at: now - 5,
                ended_at: None,
                stage: super::PendingCodexToolStage::WaitingApproval,
            },
        );
        let summary = super::summarize_pending_codex_tools(&tools).unwrap();
        assert!(summary.contains("count=1"));
        assert!(summary.contains("waiting_approval=1"));
        let pending = tools.get("pending").unwrap();
        assert!(!super::pending_codex_tool_is_stalled(
            pending,
            now,
            std::time::Duration::from_millis(10)
        ));
        assert!(super::pending_codex_tool_is_stalled(
            pending,
            now,
            std::time::Duration::from_millis(4)
        ));
        assert!(!super::pending_codex_tool_is_stalled(
            tools.get("done").unwrap(),
            now + 100,
            std::time::Duration::from_millis(1)
        ));
    }

    #[test]
    fn stalled_codex_tool_kind_reports_waiting_approval_when_an_approval_is_pending() {
        let now = crate::util::now_millis();
        let mut tools = HashMap::new();
        tools.insert(
            "done".to_string(),
            super::PendingCodexTool {
                queued_at: now - 20,
                started_at: now - 10,
                last_progress_at: now,
                ended_at: Some(now),
                stage: super::PendingCodexToolStage::WaitingResult,
            },
        );
        assert_eq!(super::stalled_codex_tool_kind(&tools), None);
        tools.insert(
            "waiting".to_string(),
            super::PendingCodexTool {
                queued_at: now - 30,
                started_at: now - 25,
                last_progress_at: now - 5,
                ended_at: None,
                stage: super::PendingCodexToolStage::WaitingApproval,
            },
        );
        assert_eq!(
            super::stalled_codex_tool_kind(&tools),
            Some("waiting_approval")
        );
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
    fn codex_native_network_and_extension_tools_have_visible_lifecycles() {
        for (item, expected_name) in [
            (
                serde_json::json!({
                    "id":"search-1","type":"webSearch","query":"上海天气",
                    "action":{"type":"search","query":"上海天气"},"results":null
                }),
                "WebSearch",
            ),
            (
                serde_json::json!({
                    "id":"mcp-1","type":"mcpToolCall","server":"weather","tool":"lookup",
                    "status":"inProgress","arguments":{"city":"上海"},"result":null,"error":null
                }),
                "weather/lookup",
            ),
            (
                serde_json::json!({
                    "id":"dynamic-1","type":"dynamicToolCall","namespace":"demo","tool":"fetch",
                    "status":"inProgress","arguments":{"url":"https://example.invalid"},
                    "contentItems":null,"success":null
                }),
                "fetch",
            ),
        ] {
            let events = parse_codex_app_server_notification(
                "codex-session",
                &serde_json::json!({
                    "method":"item/started",
                    "params":{"threadId":"thread-1","turnId":"turn-1","item":item}
                }),
            );
            assert!(events.iter().any(|event| matches!(
                event,
                super::AgentEvent::ToolCall { name, .. } if name == expected_name
            )));
        }

        let completed = parse_codex_app_server_notification(
            "codex-session",
            &serde_json::json!({
                "method":"item/completed",
                "params":{"threadId":"thread-1","turnId":"turn-1","item":{
                    "id":"search-1","type":"webSearch","query":"上海天气",
                    "action":{"type":"search","query":"上海天气"},
                    "results":[{"title":"上海天气","url":"https://example.invalid/weather"}]
                }}
            }),
        );
        assert!(completed.iter().any(|event| matches!(
            event,
            super::AgentEvent::ToolResult { id, status: crate::protocol::ToolStatus::Success, output: Some(output), .. }
                if id == "search-1" && output.contains("上海天气")
        )));

        let unavailable = parse_codex_app_server_notification(
            "codex-session",
            &serde_json::json!({
                "method":"item/completed",
                "params":{"threadId":"thread-1","turnId":"turn-1","item":{
                    "id":"search-2","type":"webSearch","status":"failed",
                    "query":"上海天气","error":{"message":"web search is unsupported"}
                }}
            }),
        );
        assert!(unavailable.iter().any(|event| matches!(
            event,
            super::AgentEvent::ToolResult {
                id,
                status: crate::protocol::ToolStatus::Error,
                native_denial_code: Some(code),
                ..
            } if id == "search-2" && code == "runtime_web_search_unavailable"
        )));
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
                "method":"item/completed",
                "params":{"threadId":"thread-1","turnId":"turn-1","item":{
                    "id":"file-1","type":"fileChange","status":"completed","changes":[
                        {"path":"src/main.rs","diff":"@@ -1 +1 @@\n-old\n+new","kind":{"type":"update"}}
                    ]
                }}
            }),
        );
        assert!(completed.iter().any(|event| matches!(
            event,
            super::AgentEvent::ToolResult { id, status: super::ToolStatus::Success, diff: Some(diff), .. }
                if id == "file-1" && diff.path == "src/main.rs"
                    && diff.hunks.iter().any(|h| h.lines.iter().any(|l| l.kind == super::DiffKind::Del))
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

    #[test]
    fn codex_context_compaction_started_emits_running_lifecycle_event() {
        let events = parse_codex_app_server_notification(
            "codex-session",
            &serde_json::json!({
                "method": "item/started",
                "params": {
                    "threadId": "thread-1",
                    "turnId": "turn-1",
                    "item": {
                        "id": "compact-1",
                        "type": "contextCompaction",
                        "status": "inProgress"
                    }
                }
            }),
        );
        assert!(matches!(
            events.as_slice(),
            [super::AgentEvent::ContextCompaction {
                status: crate::protocol::ContextCompactionStatus::Running,
                id,
                ..
            }] if id == "compact-1"
        ));
    }

    #[test]
    fn codex_context_compaction_completed_emits_succeeded_with_real_summary() {
        let events = parse_codex_app_server_notification(
            "codex-session",
            &serde_json::json!({
                "method": "item/completed",
                "params": {
                    "threadId": "thread-1",
                    "turnId": "turn-1",
                    "item": {
                        "id": "compact-1",
                        "type": "contextCompaction",
                        "status": "completed",
                        "summary": "保留了最近 3 轮对话和文件变更"
                    }
                }
            }),
        );
        assert!(matches!(
            events.as_slice(),
            [super::AgentEvent::ContextCompaction {
                status: crate::protocol::ContextCompactionStatus::Succeeded,
                id,
                summary: Some(summary),
                error: None,
                ..
            }] if id == "compact-1" && summary.contains("最近 3 轮")
        ));
    }

    #[test]
    fn codex_context_compaction_failed_emits_failed_with_error() {
        let events = parse_codex_app_server_notification(
            "codex-session",
            &serde_json::json!({
                "method": "item/completed",
                "params": {
                    "threadId": "thread-1",
                    "turnId": "turn-1",
                    "item": {
                        "id": "compact-2",
                        "type": "contextCompaction",
                        "status": "failed",
                        "error": "context window exceeded hard limit"
                    }
                }
            }),
        );
        assert!(matches!(
            events.as_slice(),
            [super::AgentEvent::ContextCompaction {
                status: crate::protocol::ContextCompactionStatus::Failed,
                id,
                summary: None,
                error: Some(error),
                ..
            }] if id == "compact-2" && error.contains("hard limit")
        ));
    }

    #[test]
    fn codex_failure_message_shows_raw_error_after_redaction() {
        // 不再包成中文套话，而是把模型/Codex 原始报错（脱敏后）直接展示。
        let notification = serde_json::json!({
            "method": "turn/completed",
            "params": {
                "turn": {
                    "status": "failed",
                    "error": {
                        "message": "unexpected status 404 Not Found",
                        "additionalDetails": "<html><head><title>404</title></head><body>nginx</body></html>"
                    }
                }
            }
        });
        let message = super::codex_app_server_failure_message(&notification);
        assert!(
            message.contains("unexpected status 404 Not Found"),
            "raw message must be shown: {message}"
        );
        assert!(
            message.contains("nginx"),
            "additionalDetails must be shown: {message}"
        );
    }

    #[test]
    fn codex_failure_message_redacts_secrets_in_raw_error() {
        let notification = serde_json::json!({
            "method": "turn/completed",
            "params": {
                "turn": {
                    "status": "failed",
                    "error": {
                        "message": "auth failed: Authorization: Bearer sklAbCdEfGhIjKlMnOpQrStUvWxYz0123456789",
                        "additionalDetails": "sk-HELM_TEST_SECRET_123456"
                    }
                }
            }
        });
        let message = super::codex_app_server_failure_message(&notification);
        assert!(
            message.contains("auth failed"),
            "raw context kept: {message}"
        );
        assert!(
            !message.contains("sk-HELM_TEST_SECRET_123456"),
            "sk- secret redacted: {message}"
        );
        assert!(
            !message.contains("sklAbCdEfGhIjKlMnOpQrStUvWxYz0123456789"),
            "bearer token redacted: {message}"
        );
    }

    #[test]
    fn codex_empty_turn_is_reported_as_failure() {
        let notification = serde_json::json!({
            "method": "turn/completed",
            "params": {"threadId":"thread-1","turn":{"id":"turn-1","status":"completed"}}
        });
        let (_, outcome) = super::codex_app_server_terminal_outcome_with_pending(
            &notification,
            None,
            false,
            Some(0),
        )
        .expect("turn completion should be recognized");
        let error = outcome.expect_err("empty successful turn must fail closed");
        assert!(error.contains("[codex_turn_empty]"));
        assert!(error.contains("outputTokens=0"));
    }
}
