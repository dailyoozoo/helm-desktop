//! Claude Code 引擎适配器（Rust/tokio 版）：拉起真实 `claude` 子进程，以 stream-json
//! 模式读取输出，逐行归一化为 `AgentEvent` 并推给前端。
//!
//! Slice 2 的审批不再依赖不可见的内置 TTY prompt：Claude Code 2.1.x 在 headless
//! 模式支持 `PreToolUse` hook 返回 `permissionDecision:"defer"`，随后可用
//! `claude -p --resume <sessionId>` 重新评估同一个工具调用。Helm 用这个真实 CLI
//! 能力实现审批卡：先 defer → UI 批准/拒绝 → 写入 hook 状态 → resume 继续原会话。

use crate::parse::parse_claude_line;
use crate::protocol::{
    AgentEvent, CallStatus, Diff, DiffHunk, DiffKind, DiffLine, EngineId, PlanStatus, PlanStep,
    Role, StopReason, ToolStatus, TurnStage,
};
use crate::sessions::SessionHistoryStore;
use crate::settings::AppSettings;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::fs;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, Manager};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::{mpsc, Mutex};

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
        mode: TurnMode,
    },
    Approve {
        request_id: String,
        decision: ApprovalDecision,
    },
    /// 检查点回溯后重建上下文（P2-5）：作废 CLI 会话 id，下一轮用截断历史重新开场
    ResetContext {
        messages: Vec<crate::sessions::SessionMessage>,
    },
    /// 会话级 MCP 开关（变更-11）：设置停用名单，下一轮生效
    SetDisabledMcp {
        disabled: Vec<String>,
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

impl TurnMode {
    pub fn parse(value: Option<&str>) -> Self {
        match value {
            Some("plan") => TurnMode::Plan,
            Some("ask") => TurnMode::Ask,
            _ => TurnMode::Build,
        }
    }

    fn as_state_str(self) -> &'static str {
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
    Deny,
    Always,
}

impl ApprovalDecision {
    fn hook_decision(self) -> &'static str {
        match self {
            ApprovalDecision::Allow | ApprovalDecision::Always => "allow",
            ApprovalDecision::Deny => "deny",
        }
    }
}

/// hook 状态文件。hook 子进程只读它；Helm 后端在用户点击审批按钮后写入。
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApprovalState {
    #[serde(default)]
    policy: ApprovalPolicy,
    decisions: HashMap<String, String>,
    always_allow: Vec<String>,
    /// 当前轮次内被拒绝的操作目标（文件路径等），防止换工具重试
    denied_targets: Vec<String>,
    /// 当前轮次的会话模式（变更-04）：ask 时 hook 以最高优先级拒绝写操作
    #[serde(default)]
    turn_mode: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalPolicy {
    pub confirm_before_command: bool,
    pub read_files: String,
    pub edit_files: String,
    pub run_commands: String,
    pub fetch_urls: String,
    pub mcp_tools: String,
    pub command_allowlist: Vec<String>,
    /// 跨会话持久化的「始终允许」工具名（P2-4）；会话启动时播种进 hook state
    #[serde(default)]
    pub always_allow_tools: Vec<String>,
}

impl Default for ApprovalPolicy {
    fn default() -> Self {
        Self {
            confirm_before_command: true,
            read_files: "allow".to_string(),
            edit_files: "ask".to_string(),
            run_commands: "ask".to_string(),
            fetch_urls: "deny".to_string(),
            mcp_tools: "ask".to_string(),
            command_allowlist: Vec::new(),
            always_allow_tools: Vec::new(),
        }
    }
}

pub fn approval_policy_from_settings(settings: &AppSettings) -> ApprovalPolicy {
    ApprovalPolicy {
        confirm_before_command: settings.general.confirm_before_command,
        read_files: settings.permissions.read_files.clone(),
        edit_files: settings.permissions.edit_files.clone(),
        run_commands: settings.permissions.run_commands.clone(),
        fetch_urls: settings.permissions.fetch_urls.clone(),
        mcp_tools: settings.permissions.mcp_tools.clone(),
        command_allowlist: settings.permissions.command_allowlist.clone(),
        always_allow_tools: Vec::new(),
    }
}

pub fn codex_sandbox_from_settings(settings: &AppSettings) -> &'static str {
    if matches!(settings.engines.codex.sandbox.as_deref(), Some("full")) {
        return "danger-full-access";
    }
    if settings.permissions.edit_files == "deny" {
        return "read-only";
    }
    match settings.engines.codex.sandbox.as_deref() {
        Some("readonly") => "read-only",
        _ => "workspace-write",
    }
}

/// 逐轮解析 Codex sandbox（变更-04）：计划/询问强制只读（取更严值），构建沿用设置映射。
pub fn codex_sandbox_for_mode(settings_sandbox: &str, mode: TurnMode) -> String {
    match mode {
        TurnMode::Plan | TurnMode::Ask => "read-only".to_string(),
        TurnMode::Build => settings_sandbox.to_string(),
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

struct SessionRuntime {
    app: AppHandle,
    history_session_id: String,
    bin: String,
    model: String,
    cwd: String,
    env: Vec<(String, String)>,
    /// 当前轮次的会话模式（变更-04）：Send 时写入；审批恢复轮沿用发起轮的值
    turn_mode: Mutex<TurnMode>,
    settings_path: PathBuf,
    state_path: PathBuf,
    session_id: Mutex<Option<String>>,
    running_pid: Mutex<Option<u32>>,
    turn_lock: Mutex<()>,
    busy: AtomicBool,
    interrupted: AtomicBool,
    pending_tools: Mutex<HashMap<String, PendingToolInfo>>,
    /// 回溯/恢复时的重建历史（P2-5）：没有 CLI 会话可 --resume 时，
    /// 下一轮把这份截断历史序列化进 prompt 重新开场，用后即清。
    rebuild_history: Mutex<Vec<crate::sessions::SessionMessage>>,
    /// 会话级停用的 MCP 服务器名单（变更-11）：非空时下一轮以
    /// `--strict-mcp-config --mcp-config <过滤后配置>` 启动，真实生效。
    disabled_mcp: std::sync::Mutex<Vec<String>>,
}

/// 一个 Claude 会话句柄。每个用户轮次会拉起一次真实 `claude -p`；会话连续性通过
/// Claude Code 的 sessionId + `--resume` 保持。
pub struct ClaudeSession {
    tx: mpsc::UnboundedSender<SessionCmd>,
}

pub struct CodexSession {
    app: AppHandle,
    history_session_id: String,
    bin: String,
    model: String,
    cwd: String,
    env: Vec<(String, String)>,
    sandbox_mode: String,
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
    /// 回溯后下一轮必须丢弃旧 thread，并用截断历史新建一次 thread。
    force_history_rebuild: Arc<AtomicBool>,
}

struct CodexAuthHome {
    path: PathBuf,
}

fn codex_auth_home_path(
    auth_home: &Arc<std::sync::Mutex<Option<CodexAuthHome>>>,
) -> Option<PathBuf> {
    auth_home
        .lock()
        .ok()
        .and_then(|guard| guard.as_ref().map(|home| home.path.clone()))
}

impl Drop for CodexAuthHome {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

pub enum AgentSession {
    Claude(ClaudeSession),
    Codex(CodexSession),
}

impl AgentSession {
    pub fn send(
        &self,
        text: String,
        attachments: Vec<String>,
        mode: TurnMode,
    ) -> Result<(), String> {
        match self {
            AgentSession::Claude(session) => session
                .tx
                .send(SessionCmd::Send {
                    text,
                    attachments,
                    mode,
                })
                .map_err(|_| "会话已结束，无法发送".to_string()),
            AgentSession::Codex(session) => session.send(text, attachments, mode),
        }
    }

    pub fn approve(&self, request_id: String, decision: ApprovalDecision) -> Result<(), String> {
        match self {
            AgentSession::Claude(session) => session
                .tx
                .send(SessionCmd::Approve {
                    request_id,
                    decision,
                })
                .map_err(|_| "会话已结束，无法审批".to_string()),
            AgentSession::Codex(_) => {
                let _ = (request_id, decision);
                Err("Codex 当前没有待审批操作".to_string())
            }
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
            AgentSession::Codex(session) => reset_codex_context_state(
                &session.thread_id,
                &session.force_history_rebuild,
                &session.history_messages,
                messages,
            ),
        }
    }

    /// 会话级 MCP 开关（变更-11）：设置停用名单，下一轮真实生效。
    /// Claude：下一轮 `--strict-mcp-config --mcp-config <过滤后配置>`；
    /// Codex：API Key 模式下过滤临时 CODEX_HOME 的 config.toml（订阅登录暂不支持）。
    pub fn set_disabled_mcp(&self, disabled: Vec<String>) -> Result<(), String> {
        match self {
            AgentSession::Claude(session) => session
                .tx
                .send(SessionCmd::SetDisabledMcp { disabled })
                .map_err(|_| "会话已结束，无法更新 MCP 开关".to_string()),
            AgentSession::Codex(session) => {
                // CODEX_HOME 仍归 Session 所有；只有用户显式变更 MCP 开关时才重建一次，
                // 普通 Turn 之间始终复用同一路径。
                let replacement = create_codex_auth_home(&session.env, &disabled)?;
                *session
                    .auth_home
                    .lock()
                    .map_err(|_| "Codex CODEX_HOME 锁中毒".to_string())? = replacement;
                *session
                    .disabled_mcp
                    .lock()
                    .map_err(|_| "Codex MCP 开关锁中毒".to_string())? = disabled;
                Ok(())
            }
        }
    }
}

/// 按平台构造 `claude` 命令。Windows 上 npm 全局安装的 `claude` 是 `.cmd` 包装器，
/// 必须经 `cmd /C` 走 PATH 解析（对应 TS 适配器里的 `shell: true`）。
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

fn apply_inherited_agent_environment(cmd: &mut Command) {
    cmd.env_clear();
    cmd.envs(filter_inherited_agent_environment(std::env::vars()));
}

fn build_command(bin: &str) -> Command {
    #[cfg(target_os = "windows")]
    {
        let mut c = Command::new("cmd");
        c.arg("/C").arg(bin).creation_flags(CREATE_NO_WINDOW);
        c
    }
    #[cfg(not(target_os = "windows"))]
    {
        Command::new(bin)
    }
}

fn build_codex_command(bin: &str) -> Command {
    #[cfg(target_os = "windows")]
    {
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
enum CodexExecCommand {
    Start,
    Resume { thread_id: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CodexExecPlan {
    command: CodexExecCommand,
    prompt: String,
}

fn codex_exec_plan(
    thread_id: Option<&str>,
    force_history_rebuild: bool,
    history: &[crate::sessions::SessionMessage],
    current_prompt: &str,
) -> CodexExecPlan {
    if !force_history_rebuild {
        if let Some(thread_id) = thread_id.filter(|id| !id.trim().is_empty()) {
            return CodexExecPlan {
                command: CodexExecCommand::Resume {
                    thread_id: thread_id.to_string(),
                },
                prompt: current_prompt.to_string(),
            };
        }
    }
    CodexExecPlan {
        command: CodexExecCommand::Start,
        prompt: serialize_history_prompt(history, current_prompt),
    }
}

fn codex_exec_args(plan: &CodexExecPlan, model: &str, sandbox_mode: &str) -> Vec<String> {
    let mut args = vec![
        "exec".to_string(),
        "--sandbox".to_string(),
        sandbox_mode.to_string(),
    ];
    if let CodexExecCommand::Resume { .. } = &plan.command {
        args.push("resume".to_string());
    }
    args.extend([
        "--json".to_string(),
        "--model".to_string(),
        model.to_string(),
        "--skip-git-repo-check".to_string(),
    ]);
    if let CodexExecCommand::Resume { thread_id } = &plan.command {
        args.push(thread_id.clone());
    }
    args.push(plan.prompt.clone());
    args
}

fn codex_thread_id_from_line(raw: &str) -> Option<String> {
    let value = serde_json::from_str::<serde_json::Value>(raw.trim()).ok()?;
    (value.get("type").and_then(serde_json::Value::as_str) == Some("thread.started"))
        .then(|| value.get("thread_id").and_then(serde_json::Value::as_str))
        .flatten()
        .filter(|id| !id.trim().is_empty())
        .map(ToString::to_string)
}

fn is_codex_thread_missing_error(message: &str) -> bool {
    let normalized = message.to_ascii_lowercase();
    normalized.contains("no rollout found for thread id")
        || ((normalized.contains("thread") || normalized.contains("session"))
            && (normalized.contains("not found") || normalized.contains("does not exist")))
}

fn codex_thread_missing_message_from_line(raw: &str) -> Option<String> {
    let value = serde_json::from_str::<serde_json::Value>(raw.trim()).ok()?;
    if value.get("type").and_then(serde_json::Value::as_str) != Some("turn.failed") {
        return None;
    }
    let message = value
        .get("error")
        .and_then(|error| error.get("message"))
        .and_then(serde_json::Value::as_str)
        .or_else(|| value.get("message").and_then(serde_json::Value::as_str))?;
    is_codex_thread_missing_error(message).then(|| message.to_string())
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

fn append_codex_turn_history(
    history: &mut Vec<crate::sessions::SessionMessage>,
    user_text: &str,
    assistant_text: &str,
    ts: i64,
) {
    history.push(crate::sessions::SessionMessage {
        role: Role::User,
        text: user_text.to_string(),
        ts,
        reverted: false,
    });
    history.push(crate::sessions::SessionMessage {
        role: Role::Assistant,
        text: assistant_text.to_string(),
        ts: ts.saturating_add(1),
        reverted: false,
    });
}

fn codex_provider_config_args(env: &[(String, String)]) -> Vec<String> {
    let Some((_, base_url)) = env.iter().find(|(key, _)| key == "OPENAI_BASE_URL") else {
        return Vec::new();
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

/// 事件信封（变更-06）：`agent-event` 的实际载荷。
/// `history_id` 是稳定的线程身份（历史会话 id；新会话即句柄 id），
/// 前端一律按它路由事件——CLI 侧 session_id 每轮可能变化（Codex 每轮新 id、
/// Claude resume 后可能换发），不能作为路由键。
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct AgentEventEnvelope<'a> {
    history_id: &'a str,
    event: &'a AgentEvent,
}

fn emit_agent_event(app: &AppHandle, history_session_id: &str, event: &AgentEvent) {
    if let Some(store) = app.try_state::<SessionHistoryStore>() {
        if let Err(err) = store.record_event_for_session(history_session_id, event) {
            eprintln!("[helm] 会话历史写入失败：{err}");
            let _ = app.emit(
                EVENT_NAME,
                &AgentEventEnvelope {
                    history_id: history_session_id,
                    event: &AgentEvent::Error {
                        session_id: event_session_id(event),
                        message: format!("会话历史写入失败：{err}"),
                        recoverable: true,
                        kind: None,
                    },
                },
            );
        }
    }
    // 用量托盘 + 阈值通知（P3-2）：真实 token 用量落库后刷新
    if matches!(event, AgentEvent::TokenUsage { .. }) {
        crate::tray::refresh_usage(app);
    }
    // 审批等待系统通知（变更-12）：用户可能切走了会话/页面/窗口，
    // CLI 挂起等决定的状态必须主动可感知
    if let AgentEvent::ApprovalRequest { action, .. } = event {
        use tauri_plugin_notification::NotificationExt;
        let _ = app
            .notification()
            .builder()
            .title("Helm 等待审批")
            .body(format!("Agent 请求执行「{action}」，请回到会话处理。"))
            .show();
    }
    // fast model 自动起标题（P3-5）：首轮完成后后台生成，不阻塞事件流
    if matches!(event, AgentEvent::TurnComplete { .. }) {
        crate::titler::maybe_generate_title(app, history_session_id);
    }
    let _ = app.emit(
        EVENT_NAME,
        &AgentEventEnvelope {
            history_id: history_session_id,
            event,
        },
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

fn event_session_id(event: &AgentEvent) -> Option<String> {
    match event {
        AgentEvent::SessionStarted { session_id, .. }
        | AgentEvent::MessageDelta { session_id, .. }
        | AgentEvent::MessageComplete { session_id, .. }
        | AgentEvent::ThinkingDelta { session_id, .. }
        | AgentEvent::ThinkingComplete { session_id, .. }
        | AgentEvent::TurnStage { session_id, .. }
        | AgentEvent::ToolCall { session_id, .. }
        | AgentEvent::ToolProgress { session_id, .. }
        | AgentEvent::ToolResult { session_id, .. }
        | AgentEvent::ApprovalRequest { session_id, .. }
        | AgentEvent::PlanUpdate { session_id, .. }
        | AgentEvent::Checkpoint { session_id, .. }
        | AgentEvent::TokenUsage { session_id, .. }
        | AgentEvent::TurnComplete { session_id, .. } => Some(session_id.clone()),
        AgentEvent::Error { session_id, .. } => session_id.clone(),
    }
}

fn create_codex_auth_home(
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

/// 会话级 MCP 开关（变更-11）：从真实 `~/.claude/settings.json` 读出 mcpServers，
/// 剔除停用项后写成一份临时 mcp-config。配合 `--strict-mcp-config` 让本轮
/// 只加载过滤后的集合（CLI 官方支持的完全控制路径）。
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
    let filtered: serde_json::Map<String, serde_json::Value> = servers
        .into_iter()
        .filter(|(name, _)| !disabled.iter().any(|item| item == name))
        .collect();
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

/// 按 pid 杀掉整棵进程树（中断用）。Windows 走 `taskkill /T`，Unix 走 `kill -TERM`。
async fn kill_tree(pid: Option<u32>) {
    let Some(pid) = pid else {
        return;
    };
    #[cfg(target_os = "windows")]
    {
        let _ = Command::new("taskkill")
            .args(["/F", "/T", "/PID", &pid.to_string()])
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .await;
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = Command::new("kill")
            .args(["-TERM", &pid.to_string()])
            .output()
            .await;
    }
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

fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
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

fn quoted(path: &Path) -> String {
    format!("\"{}\"", path.to_string_lossy().replace('"', "\\\""))
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
fn hook_command(script_path: &Path, state_path: &Path) -> String {
    format!(
        "powershell -NoProfile -ExecutionPolicy Bypass -File {} {}",
        quoted(script_path),
        quoted(state_path)
    )
}

#[cfg(not(target_os = "windows"))]
fn hook_command(script_path: &Path, state_path: &Path) -> String {
    format!("python3 {} {}", quoted(script_path), quoted(state_path))
}

#[cfg(target_os = "windows")]
fn hook_script_content() -> &'static str {
    r#"
param([Parameter(Mandatory=$true)][string]$StatePath)

$raw = [Console]::In.ReadToEnd()
try {
  $payload = $raw | ConvertFrom-Json
} catch {
  $payload = $null
}

$toolName = ""
$requestId = ""
$toolInput = $null
if ($payload -and $payload.tool_name) { $toolName = [string]$payload.tool_name }
if ($payload -and $payload.tool_use_id) { $requestId = [string]$payload.tool_use_id }
if ($payload -and $payload.tool_input) { $toolInput = $payload.tool_input }

$decision = "defer"
$reason = ""

function Get-ToolTarget {
  param($ToolName, $ToolInput)
  if (-not $ToolInput) {
    return $null
  }
  if ($ToolInput.file_path) {
    return [string]$ToolInput.file_path
  }
  if ($ToolName -eq "NotebookEdit" -and $ToolInput.notebook_path) {
    return [string]$ToolInput.notebook_path
  }
  if ($ToolName -eq "Bash" -and $ToolInput.command) {
    $cmd = [string]$ToolInput.command
    if ($cmd -match '>{1,2}\s*["'']?([^\s"''>][^\s"'']*)') {
      return $Matches[1]
    }
    if ($cmd -match '\b(?:rm|rmdir|mv|dd|chmod)\b\s+(?:-[^\s]+\s+)*["'']?([^"'']\S*)') {
      return $Matches[1]
    }
  }
  return $null
}

$target = Get-ToolTarget $toolName $toolInput

try {
  $state = Get-Content -LiteralPath $StatePath -Raw | ConvertFrom-Json
} catch {
  $state = $null
}
$policy = $null
if ($state -and $state.policy) { $policy = $state.policy }

# 询问模式（变更-04/07）：白名单化只读约束，最高优先级，优先于 decisions / alwaysAllow / 权限矩阵。
# 原则：识别不了的操作一律拒绝——写文件类工具、全部 MCP 工具（读写不可判读）、
# 以及不在只读白名单里的任何 Bash 命令。
$turnMode = ""
if ($state -and $state.turnMode) { $turnMode = [string]$state.turnMode }
if ($turnMode -eq "ask") {
  $askDeny = $false
  if ($toolName -eq "Write" -or $toolName -eq "Edit" -or $toolName -eq "MultiEdit" -or $toolName -eq "NotebookEdit") {
    $askDeny = $true
  } elseif ($toolName -like "mcp__*") {
    $askDeny = $true
  } elseif ($toolName -eq "Bash") {
    $askDeny = $true
    if ($toolInput -and $toolInput.command) {
      $cmd = ([string]$toolInput.command).Trim()
      if ($cmd -notmatch '[>|;&]') {
        $parts = $cmd -split '\s+'
        $first = $parts[0].ToLowerInvariant()
        $readOnly = @("ls","dir","cat","type","head","tail","grep","rg","findstr","find","pwd","whoami","which","where","wc","du","df","stat","file","tree","env","printenv","date","echo")
        if ($readOnly -contains $first) {
          $askDeny = $false
        } elseif ($first -eq "git" -and $parts.Length -ge 2) {
          $gitReadOnly = @("status","log","diff","show","branch","remote","rev-parse","blame","shortlog","describe","ls-files")
          if ($gitReadOnly -contains $parts[1].ToLowerInvariant()) { $askDeny = $false }
        }
      }
    }
  }
  if ($askDeny) {
    $decision = "deny"
    $reason = "当前为询问模式（只读），本轮只允许只读操作。请只回答问题，不要修改文件、执行写命令或调用 MCP 工具。"
  }
}

function Get-PolicyMode {
  param([string]$Name, [string]$Default)
  if ($policy -and $policy.PSObject.Properties[$Name]) {
    $value = [string]$policy.PSObject.Properties[$Name].Value
    if ($value -eq "allow" -or $value -eq "ask" -or $value -eq "deny") {
      return $value
    }
  }
  return $Default
}

function Test-CommandAllowlisted {
  param([string]$Command)
  if (-not $policy -or -not $policy.commandAllowlist) { return $false }
  foreach ($pattern in @($policy.commandAllowlist)) {
    if ($Command -like [string]$pattern) { return $true }
  }
  return $false
}

function Get-ToolPolicyName {
  param([string]$ToolName)
  if ($ToolName -eq "Bash") { return "runCommands" }
  if ($ToolName -eq "Read" -or $ToolName -eq "Glob" -or $ToolName -eq "Grep" -or $ToolName -eq "LS") { return "readFiles" }
  if ($ToolName -eq "Write" -or $ToolName -eq "Edit" -or $ToolName -eq "MultiEdit" -or $ToolName -eq "NotebookEdit") { return "editFiles" }
  if ($ToolName -eq "WebFetch" -or $ToolName -eq "WebSearch") { return "fetchUrls" }
  if ($ToolName -like "mcp__*") { return "mcpTools" }
  return $null
}

if ($target) {
  if ($state -and $state.deniedTargets) {
    foreach ($denied in @($state.deniedTargets)) {
      if ([string]$denied -eq $target) {
        $decision = "deny"
        $reason = "User denied this operation. Do NOT retry with alternative tools or methods."
        break
      }
    }  
  }
}

if ($decision -eq "defer" -and $state) {
  if ($state.decisions -and $state.decisions.PSObject.Properties[$requestId]) {
    $decision = [string]$state.decisions.PSObject.Properties[$requestId].Value
  }
  elseif ($state.alwaysAllow) {
    # 「始终允许」粒度（变更-07）：Bash 按命令首词记录为 "Bash:npm" 形式，
    # 批准一次 npm 不再放行 rm -rf；其余工具仍按工具名。旧的裸 "Bash" 条目不再匹配。
    $allowKey = $toolName
    if ($toolName -eq "Bash" -and $toolInput -and $toolInput.command) {
      $first = ((([string]$toolInput.command).Trim()) -split '\s+')[0]
      $allowKey = "Bash:" + $first
    }
    foreach ($tool in @($state.alwaysAllow)) {
      if ([string]$tool -eq $allowKey) { $decision = "allow" }
    }
  }
}

if ($decision -eq "defer") {
  $policyName = Get-ToolPolicyName $toolName
  if ($toolName -eq "Bash" -and $toolInput -and $toolInput.command) {
    $cmd = [string]$toolInput.command
    $confirmBeforeCommand = $true
    if ($policy -and $policy.PSObject.Properties["confirmBeforeCommand"]) {
      $confirmBeforeCommand = [bool]$policy.confirmBeforeCommand
    }
    $mode = Get-PolicyMode "runCommands" "ask"
    if ($mode -eq "deny") {
      $decision = "deny"
    } elseif (Test-CommandAllowlisted $cmd) {
      $decision = "allow"
    } elseif ($confirmBeforeCommand) {
      $decision = "defer"
    } else {
      if ($mode -eq "allow") { $decision = "allow" }
    }
  } elseif ($policyName) {
    $mode = Get-PolicyMode $policyName "ask"
    if ($mode -eq "allow") { $decision = "allow" }
    elseif ($mode -eq "deny") { $decision = "deny" }
  }
}

if (-not $reason) {
  if ($decision -eq "deny") {
    $reason = "User denied this operation. Do NOT retry with alternative tools or methods."
  } else {
    $reason = "Helm approval " + $decision
  }
}

$out = @{
  hookSpecificOutput = @{
    hookEventName = "PreToolUse"
    permissionDecision = $decision
    permissionDecisionReason = $reason
  }
}
$out | ConvertTo-Json -Depth 20 -Compress
"#
}

#[cfg(not(target_os = "windows"))]
fn hook_script_content() -> &'static str {
    r#"#!/usr/bin/env python3
import json
import sys
import re

state_path = sys.argv[1]
raw = sys.stdin.read()
try:
    payload = json.loads(raw)
except Exception:
    payload = {}

tool_name = str(payload.get("tool_name") or "")
request_id = str(payload.get("tool_use_id") or "")
tool_input = payload.get("tool_input") or {}
decision = "defer"
reason = ""

# 提取操作目标（文件路径等）
def get_tool_target(tool_name, tool_input):
    if "file_path" in tool_input:
        return str(tool_input["file_path"])
    # NotebookEdit 的目标是 notebook_path（变更-07）
    if tool_name == "NotebookEdit" and "notebook_path" in tool_input:
        return str(tool_input["notebook_path"])
    if tool_name == "Bash" and "command" in tool_input:
        cmd = str(tool_input["command"])
        # >{1,2}：覆盖追加重定向 >>（此前 >> 会把第二个 > 捕进目标）
        match = re.search(r'>{1,2}\s*["\']?([^\s"\'>][^\s"\']*)', cmd)
        if match:
            return match.group(1)
        match = re.search(r'\b(?:rm|rmdir|mv|dd|chmod)\b\s+(?:-[^\s]+\s+)*["\']?([^\s"\']+)', cmd)
        if match:
            return match.group(1)
    return None

target = get_tool_target(tool_name, tool_input)

try:
    with open(state_path, "r", encoding="utf-8") as f:
        state = json.load(f)
except Exception:
    state = {}
policy = state.get("policy") or {}

# 询问模式（变更-04/07）：白名单化只读约束，最高优先级。
# 识别不了的操作一律拒绝——写文件类工具、全部 MCP 工具、不在只读白名单里的 Bash 命令。
_READ_ONLY_BASH = {
    "ls", "dir", "cat", "type", "head", "tail", "grep", "rg", "findstr", "find",
    "pwd", "whoami", "which", "where", "wc", "du", "df", "stat", "file", "tree",
    "env", "printenv", "date", "echo",
}
_READ_ONLY_GIT = {
    "status", "log", "diff", "show", "branch", "remote", "rev-parse",
    "blame", "shortlog", "describe", "ls-files",
}

def bash_is_read_only(command):
    cmd = str(command).strip()
    if re.search(r'[>|;&]', cmd):
        return False
    parts = cmd.split()
    if not parts:
        return False
    first = parts[0].lower()
    if first in _READ_ONLY_BASH:
        return True
    if first == "git" and len(parts) >= 2 and parts[1].lower() in _READ_ONLY_GIT:
        return True
    return False

if str(state.get("turnMode") or "") == "ask":
    ask_deny = False
    if tool_name in ("Write", "Edit", "MultiEdit", "NotebookEdit"):
        ask_deny = True
    elif tool_name.startswith("mcp__"):
        ask_deny = True
    elif tool_name == "Bash":
        ask_deny = not bash_is_read_only(tool_input.get("command", ""))
    if ask_deny:
        decision = "deny"
        reason = "当前为询问模式（只读），本轮只允许只读操作。请只回答问题，不要修改文件、执行写命令或调用 MCP 工具。"

def get_policy_mode(name, default):
    value = str(policy.get(name) or default)
    return value if value in ("allow", "ask", "deny") else default

def command_allowlisted(command):
    import fnmatch
    for pattern in policy.get("commandAllowlist") or []:
        if fnmatch.fnmatch(command, str(pattern)):
            return True
    return False

def tool_policy_name(tool_name):
    if tool_name == "Bash":
        return "runCommands"
    if tool_name in ("Read", "Glob", "Grep", "LS"):
        return "readFiles"
    if tool_name in ("Write", "Edit", "MultiEdit", "NotebookEdit"):
        return "editFiles"
    if tool_name in ("WebFetch", "WebSearch"):
        return "fetchUrls"
    if tool_name.startswith("mcp__"):
        return "mcpTools"
    return None

if target and target in state.get("deniedTargets", []):
    decision = "deny"
    reason = "User denied this operation. Do NOT retry with alternative tools or methods."

if decision == "defer":
    if request_id in state.get("decisions", {}):
        decision = str(state["decisions"][request_id])
    else:
        # 「始终允许」粒度（变更-07）：Bash 按命令首词 "Bash:npm"，其余按工具名
        allow_key = tool_name
        if tool_name == "Bash" and tool_input.get("command"):
            allow_key = "Bash:" + str(tool_input["command"]).strip().split()[0]
        if allow_key in state.get("alwaysAllow", []):
            decision = "allow"

if decision == "defer":
    policy_name = tool_policy_name(tool_name)
    if tool_name == "Bash" and tool_input.get("command"):
        cmd = str(tool_input["command"])
        confirm_before_command = bool(policy.get("confirmBeforeCommand", True))
        mode = get_policy_mode("runCommands", "ask")
        if mode == "deny":
            decision = "deny"
        elif command_allowlisted(cmd):
            decision = "allow"
        elif confirm_before_command:
            decision = "defer"
        else:
            if mode == "allow":
                decision = "allow"
    elif policy_name:
        mode = get_policy_mode(policy_name, "ask")
        if mode == "allow":
            decision = "allow"
        elif mode == "deny":
            decision = "deny"

if not reason:
    if decision == "deny":
        reason = "User denied this operation. Do NOT retry with alternative tools or methods."
    else:
        reason = "Helm approval " + decision

print(json.dumps({
    "hookSpecificOutput": {
        "hookEventName": "PreToolUse",
        "permissionDecision": decision,
        "permissionDecisionReason": reason,
    }
}, separators=(",", ":")))
"#
}

fn create_approval_hook_files(policy: ApprovalPolicy) -> Result<ApprovalHookFiles, String> {
    let root = unique_temp_dir("helm-approval");
    fs::create_dir_all(&root).map_err(|e| format!("创建审批 hook 目录失败：{e}"))?;

    let state_path = root.join("approval-state.json");
    // 持久化的「始终允许」清单播种进初始 state，hook 脚本读 alwaysAllow 顶层字段（P2-4）
    let always_allow = policy.always_allow_tools.clone();
    let initial_state = serde_json::to_string(&ApprovalState {
        policy,
        always_allow,
        ..ApprovalState::default()
    })
    .map_err(|e| format!("序列化审批状态失败：{e}"))?;
    fs::write(&state_path, initial_state).map_err(|e| format!("写入审批状态失败：{e}"))?;

    let script_path = root.join(hook_script_name());
    // Windows PowerShell 5.1 把无 BOM 文件按 ANSI 解析，脚本里的中文（询问模式拒绝文案）
    // 会变乱码并破坏语法——必须带 UTF-8 BOM（变更-04 踩坑）。
    #[cfg(target_os = "windows")]
    let script_content = format!("\u{FEFF}{}", hook_script_content());
    #[cfg(not(target_os = "windows"))]
    let script_content = hook_script_content().to_string();
    fs::write(&script_path, script_content).map_err(|e| format!("写入审批 hook 失败：{e}"))?;

    let settings_path = root.join("claude-settings.json");
    let settings = serde_json::json!({
        "hooks": {
            "PreToolUse": [{
                "matcher": "Read|Glob|Grep|LS|Write|Edit|MultiEdit|NotebookEdit|Bash|WebFetch|WebSearch|mcp__.*",
                "hooks": [{
                    "type": "command",
                    "command": hook_command(&script_path, &state_path),
                    "timeout": 30
                }]
            }]
        }
    });
    let settings_text = serde_json::to_string_pretty(&settings)
        .map_err(|e| format!("序列化 Claude 设置失败：{e}"))?;
    fs::write(&settings_path, settings_text).map_err(|e| format!("写入 Claude 设置失败：{e}"))?;

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
        "Write" | "Edit" | "MultiEdit" | "NotebookEdit" | "Bash" => {
            let target = extract_tool_target(tool_name, input)?;
            let path = PathBuf::from(target);
            Some(if path.is_absolute() {
                path
            } else {
                cwd.join(path)
            })
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
) -> Result<Option<AgentEvent>, String> {
    let Some(target) = checkpoint_target_path(tool_name, input, cwd) else {
        return Ok(None);
    };

    let checkpoint_id = checkpoint_id_for_tool(tool_id);
    let snapshot_store = crate::snapshots::SnapshotStore::new(snapshots_dir.to_path_buf());
    let snapshot = snapshot_store.capture_files(std::slice::from_ref(&target))?;
    snapshot_store.save(&checkpoint_id, &snapshot)?;

    let label_target = target
        .file_name()
        .and_then(|name| name.to_str())
        .map(ToString::to_string)
        .unwrap_or_else(|| target.to_string_lossy().to_string());
    let ts = now_millis() as i64;
    history_store.save_checkpoint(
        &checkpoint_id,
        history_session_id,
        0,
        &format!("改动前：{label_target}"),
        &checkpoint_id,
        ts,
    )?;

    Ok(Some(AgentEvent::Checkpoint {
        session_id: cli_session_id.to_string(),
        id: checkpoint_id,
        label: format!("改动前：{label_target}"),
        ts,
    }))
}

#[cfg(test)]
mod tests {
    use super::{
        agent_environment_from_settings, append_codex_turn_history, approval_policy_from_settings,
        checkpoint_target_path, codex_output_summary, codex_provider_config_args,
        codex_sandbox_for_mode, codex_sandbox_from_settings, create_approval_hook_files,
        create_auto_checkpoint_for_tool, create_codex_auth_home,
        create_codex_auth_home_with_source, extract_tool_target,
        filter_inherited_agent_environment, merge_pending_delta, parse_codex_line,
        prompt_with_attachments, spawn_agent_task, validate_engine_bin, ApprovalPolicy, TurnMode,
    };
    use crate::protocol::{AgentEvent, EngineId, Role, StopReason};
    use crate::sessions::{NewSessionRecord, SessionHistoryStore};
    use crate::settings::AppSettings;
    use serde_json::json;
    use std::fs;

    #[test]
    fn approval_policy_follows_app_settings_permissions() {
        let mut settings = AppSettings::default();
        settings.general.confirm_before_command = false;
        settings.permissions.read_files = "ask".to_string();
        settings.permissions.edit_files = "deny".to_string();
        settings.permissions.run_commands = "allow".to_string();
        settings.permissions.fetch_urls = "deny".to_string();
        settings.permissions.mcp_tools = "ask".to_string();
        settings.permissions.command_allowlist = vec!["git status".to_string()];

        let policy = approval_policy_from_settings(&settings);

        assert!(!policy.confirm_before_command);
        assert_eq!(policy.read_files, "ask");
        assert_eq!(policy.edit_files, "deny");
        assert_eq!(policy.run_commands, "allow");
        assert_eq!(policy.fetch_urls, "deny");
        assert_eq!(policy.mcp_tools, "ask");
        assert_eq!(policy.command_allowlist, vec!["git status"]);
    }

    #[test]
    fn codex_sandbox_uses_app_settings() {
        let mut settings = AppSettings::default();
        settings.engines.codex.sandbox = Some("readonly".to_string());
        assert_eq!(codex_sandbox_from_settings(&settings), "read-only");

        settings.engines.codex.sandbox = Some("workspace".to_string());
        assert_eq!(codex_sandbox_from_settings(&settings), "workspace-write");

        settings.engines.codex.sandbox = Some("full".to_string());
        assert_eq!(codex_sandbox_from_settings(&settings), "danger-full-access");
    }

    #[test]
    fn codex_sandbox_respects_file_edit_permission_matrix() {
        let mut settings = AppSettings::default();
        settings.engines.codex.sandbox = Some("workspace".to_string());
        settings.permissions.edit_files = "deny".to_string();
        assert_eq!(codex_sandbox_from_settings(&settings), "read-only");

        settings.permissions.edit_files = "ask".to_string();
        assert_eq!(codex_sandbox_from_settings(&settings), "workspace-write");
    }

    #[test]
    fn claude_approval_hook_covers_reading_tools() {
        let files = create_approval_hook_files(ApprovalPolicy::default()).unwrap();
        let settings = fs::read_to_string(files.settings_path).unwrap();

        assert!(settings.contains("Read"));
        assert!(settings.contains("Glob"));
        assert!(settings.contains("Grep"));
        assert!(settings.contains("LS"));
    }

    #[test]
    fn approval_hook_state_seeds_persistent_always_allow() {
        // P2-4：持久化的「始终允许」清单必须播种进 hook state 顶层 alwaysAllow，
        // hook 脚本不改就能直接放行
        let policy = ApprovalPolicy {
            always_allow_tools: vec!["Bash".to_string(), "Write".to_string()],
            ..ApprovalPolicy::default()
        };
        let files = create_approval_hook_files(policy).unwrap();
        let state: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(files.state_path).unwrap()).unwrap();

        assert_eq!(
            state["alwaysAllow"],
            serde_json::json!(["Bash", "Write"]),
            "初始 state 顶层 alwaysAllow 必须等于持久化清单"
        );
    }

    /// 真实执行审批 hook 脚本（Windows: powershell / 其他: python3），返回 permissionDecision。
    fn run_hook_script(state_path: &std::path::Path, payload: &serde_json::Value) -> String {
        use std::io::Write as _;
        use std::process::{Command, Stdio};

        let script_path = state_path.parent().unwrap().join(super::hook_script_name());
        #[cfg(target_os = "windows")]
        let mut cmd = {
            let mut cmd = Command::new("powershell");
            cmd.args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
                .arg(&script_path)
                .arg(state_path);
            cmd
        };
        #[cfg(not(target_os = "windows"))]
        let mut cmd = {
            let mut cmd = Command::new("python3");
            cmd.arg(&script_path).arg(state_path);
            cmd
        };
        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("启动 hook 脚本失败");
        child
            .stdin
            .take()
            .unwrap()
            .write_all(payload.to_string().as_bytes())
            .unwrap();
        let output = child.wait_with_output().expect("hook 脚本执行失败");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let parsed: serde_json::Value = serde_json::from_str(stdout.trim())
            .unwrap_or_else(|e| panic!("hook 输出不是 JSON：{e}\n{stdout}"));
        parsed["hookSpecificOutput"]["permissionDecision"]
            .as_str()
            .unwrap_or("")
            .to_string()
    }

    #[test]
    fn ask_mode_hook_denies_writes_above_always_allow() {
        // 变更-04/07：询问模式白名单化只读约束（> alwaysAllow > 权限矩阵），真实执行脚本验证
        let policy = ApprovalPolicy {
            always_allow_tools: vec!["Write".to_string(), "Bash:echo".to_string()],
            edit_files: "allow".to_string(),
            ..ApprovalPolicy::default()
        };
        let files = create_approval_hook_files(policy).unwrap();

        let mut state = super::read_approval_state(&files.state_path);
        state.turn_mode = "ask".to_string();
        super::write_approval_state(&files.state_path, &state).unwrap();

        let write_payload = serde_json::json!({
            "tool_name": "Write",
            "tool_use_id": "t-write",
            "tool_input": {"file_path": "x.txt", "content": "hi"}
        });
        assert_eq!(
            run_hook_script(&files.state_path, &write_payload),
            "deny",
            "询问模式下写文件必须被拒绝，即使工具在 alwaysAllow 里"
        );

        let bash_write_payload = serde_json::json!({
            "tool_name": "Bash",
            "tool_use_id": "t-bash",
            "tool_input": {"command": "echo hi > out.txt"}
        });
        assert_eq!(
            run_hook_script(&files.state_path, &bash_write_payload),
            "deny",
            "询问模式下 Bash 写目标（重定向）必须被拒绝"
        );

        // 变更-07：此前提取不到目标的写命令（cp/sed -i/tee）会漏网，现在一律拒绝
        for cmd in [
            "cp a.txt b.txt",
            "sed -i 's/a/b/' x.txt",
            "tee out.txt",
            "python -c \"open('x','w')\"",
        ] {
            let payload = serde_json::json!({
                "tool_name": "Bash",
                "tool_use_id": "t-bash2",
                "tool_input": {"command": cmd}
            });
            assert_eq!(
                run_hook_script(&files.state_path, &payload),
                "deny",
                "询问模式下未被识别为只读的命令必须拒绝：{cmd}"
            );
        }

        // 变更-07：MCP 工具在询问模式一律拒绝（读写不可判读）
        let mcp_payload = serde_json::json!({
            "tool_name": "mcp__github__create_issue",
            "tool_use_id": "t-mcp",
            "tool_input": {"title": "x"}
        });
        assert_eq!(
            run_hook_script(&files.state_path, &mcp_payload),
            "deny",
            "询问模式下 MCP 工具必须被拒绝"
        );

        let read_payload = serde_json::json!({
            "tool_name": "Read",
            "tool_use_id": "t-read",
            "tool_input": {"file_path": "x.txt"}
        });
        assert_eq!(
            run_hook_script(&files.state_path, &read_payload),
            "allow",
            "询问模式不影响只读工具"
        );

        // 只读 Bash 命令（git status）不被询问模式硬拒，回落常规审批流程
        let git_status = serde_json::json!({
            "tool_name": "Bash",
            "tool_use_id": "t-git",
            "tool_input": {"command": "git status"}
        });
        assert_eq!(
            run_hook_script(&files.state_path, &git_status),
            "defer",
            "询问模式下只读 Bash（git status）不硬拒，走常规审批"
        );

        // 同一 state 切回构建模式：alwaysAllow 恢复生效
        let mut state = super::read_approval_state(&files.state_path);
        state.turn_mode = "build".to_string();
        super::write_approval_state(&files.state_path, &state).unwrap();
        assert_eq!(
            run_hook_script(&files.state_path, &write_payload),
            "allow",
            "构建模式下 alwaysAllow 的 Write 应放行"
        );
    }

    #[test]
    fn always_allow_bash_is_scoped_to_command_first_word() {
        // 变更-07：批准一次 `Bash:echo` 不放行其他 Bash 命令（如 rm -rf）
        let policy = ApprovalPolicy {
            always_allow_tools: vec!["Bash:echo".to_string()],
            run_commands: "ask".to_string(),
            confirm_before_command: true,
            ..ApprovalPolicy::default()
        };
        let files = create_approval_hook_files(policy).unwrap();

        let echo = serde_json::json!({
            "tool_name": "Bash",
            "tool_use_id": "t-echo",
            "tool_input": {"command": "echo hi"}
        });
        assert_eq!(
            run_hook_script(&files.state_path, &echo),
            "allow",
            "已批准的 echo 命令应放行"
        );

        let rm = serde_json::json!({
            "tool_name": "Bash",
            "tool_use_id": "t-rm",
            "tool_input": {"command": "rm -rf /tmp/x"}
        });
        assert_eq!(
            run_hook_script(&files.state_path, &rm),
            "defer",
            "未批准的 rm 命令不应被 Bash:echo 放行，需继续审批"
        );
    }

    #[test]
    fn always_allow_key_scopes_bash_by_first_word() {
        assert_eq!(
            super::always_allow_key("Bash", &serde_json::json!({"command": "npm test auth"})),
            "Bash:npm"
        );
        assert_eq!(
            super::always_allow_key("Write", &serde_json::json!({"file_path": "x"})),
            "Write"
        );
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
            },
            SessionMessage {
                role: Role::Assistant,
                text: "第一轮回复".to_string(),
                ts: 2,
                reverted: false,
            },
        ];
        let prompt = super::serialize_history_prompt(&history, "当前问题");
        assert!(prompt.starts_with("之前的对话历史："));
        assert!(prompt.contains("用户: 第一轮提问"));
        assert!(prompt.contains("助手: 第一轮回复"));
        assert!(prompt.ends_with("用户: 当前问题"));
    }

    #[test]
    fn codex_resume_plan_sends_only_the_current_prompt() {
        let first = super::codex_exec_plan(None, false, &[], "第一轮");
        assert!(matches!(first.command, super::CodexExecCommand::Start));
        assert_eq!(first.prompt, "第一轮");

        let history = vec![crate::sessions::SessionMessage {
            role: crate::protocol::Role::Assistant,
            text: "第一轮回复".to_string(),
            ts: 1,
            reverted: false,
        }];
        let second = super::codex_exec_plan(Some("thread-1"), false, &history, "第二轮");
        assert!(matches!(
            second.command,
            super::CodexExecCommand::Resume { ref thread_id } if thread_id == "thread-1"
        ));
        assert_eq!(second.prompt, "第二轮");
        assert!(!second.prompt.contains("第一轮回复"));
    }

    #[test]
    fn codex_rebuild_plan_discards_native_thread_and_serializes_history() {
        let history = vec![crate::sessions::SessionMessage {
            role: crate::protocol::Role::Assistant,
            text: "截断后的回复".to_string(),
            ts: 1,
            reverted: false,
        }];
        let plan = super::codex_exec_plan(Some("stale-thread"), true, &history, "当前问题");

        assert!(matches!(plan.command, super::CodexExecCommand::Start));
        assert!(plan.prompt.contains("截断后的回复"));
        assert!(plan.prompt.ends_with("用户: 当前问题"));
    }

    #[test]
    fn codex_resume_command_places_resume_before_thread_and_prompt() {
        let plan = super::CodexExecPlan {
            command: super::CodexExecCommand::Resume {
                thread_id: "thread-1".to_string(),
            },
            prompt: "只发送当前问题".to_string(),
        };

        let args = super::codex_exec_args(&plan, "gpt-5", "read-only");

        assert_eq!(
            args,
            vec![
                "exec",
                "--sandbox",
                "read-only",
                "resume",
                "--json",
                "--model",
                "gpt-5",
                "--skip-git-repo-check",
                "thread-1",
                "只发送当前问题",
            ]
        );
    }

    #[test]
    fn codex_thread_started_exposes_native_thread_id() {
        assert_eq!(
            super::codex_thread_id_from_line(
                r#"{"type":"thread.started","thread_id":"019eaa24-be0b"}"#
            ),
            Some("019eaa24-be0b".to_string())
        );
        assert_eq!(
            super::codex_thread_id_from_line(r#"{"type":"turn.started"}"#),
            None
        );
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

        assert_eq!(super::codex_auth_home_path(&owner), Some(path.clone()));
        assert_eq!(super::codex_auth_home_path(&turn), Some(path.clone()));
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
    fn completed_codex_turn_is_appended_to_runtime_history() {
        use crate::sessions::SessionMessage;

        let mut history = vec![SessionMessage {
            role: Role::Assistant,
            text: "已有回复".to_string(),
            ts: 10,
            reverted: false,
        }];

        append_codex_turn_history(&mut history, "本轮问题", "本轮回答", 20);

        assert_eq!(history.len(), 3);
        assert_eq!(history[0].text, "已有回复");
        assert_eq!(history[1].role, Role::User);
        assert_eq!(history[1].text, "本轮问题");
        assert_eq!(history[1].ts, 20);
        assert_eq!(history[2].role, Role::Assistant);
        assert_eq!(history[2].text, "本轮回答");
        assert_eq!(history[2].ts, 21);
        assert!(history.iter().all(|message| !message.reverted));
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
    fn codex_sandbox_for_mode_forces_read_only_on_plan_and_ask() {
        // 计划/询问强制只读（取更严值）；构建沿用设置映射，包括显式 full
        assert_eq!(
            codex_sandbox_for_mode("workspace-write", TurnMode::Plan),
            "read-only"
        );
        assert_eq!(
            codex_sandbox_for_mode("danger-full-access", TurnMode::Ask),
            "read-only"
        );
        assert_eq!(
            codex_sandbox_for_mode("workspace-write", TurnMode::Build),
            "workspace-write"
        );
        assert_eq!(
            codex_sandbox_for_mode("danger-full-access", TurnMode::Build),
            "danger-full-access"
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
        let input = json!({ "file_path": "src/main.rs" });

        assert_eq!(
            checkpoint_target_path("Write", &input, &cwd),
            Some(cwd.join("src/main.rs"))
        );
        assert_eq!(checkpoint_target_path("Read", &input, &cwd), None);
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
        )
        .unwrap()
        .expect("Edit should create checkpoint");

        let AgentEvent::Checkpoint { id, session_id, .. } = event else {
            panic!("expected checkpoint event");
        };
        assert_eq!(session_id, "cli-1");

        let checkpoint = history_store.get_checkpoint(&id).unwrap().unwrap();
        assert_eq!(checkpoint.snapshot_ref, id);

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
    fn parses_codex_failed_jsonl_error_from_stdout() {
        let stdout = r#"{"type":"thread.started","thread_id":"019eaa24-be0b-7e11-9ecc-d9b3dacc4805"}
{"type":"turn.started"}
{"type":"error","message":"Reconnecting... 1/5"}
{"type":"turn.failed","error":{"message":"unexpected status 503 Service Unavailable"}}
"#;

        let summary = codex_output_summary(stdout);

        assert_eq!(summary.final_text, None);
        assert_eq!(
            summary.error_message,
            Some("unexpected status 503 Service Unavailable".to_string())
        );
    }

    #[test]
    fn parses_codex_final_answer_from_message_event() {
        let stdout = r#"{"type":"thread.started","thread_id":"t1"}
{"type":"item.completed","item":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"HELM_CODEX_REAL_CLI_OK"}]}}
{"type":"turn.completed"}
"#;

        let summary = codex_output_summary(stdout);

        assert_eq!(
            summary.final_text,
            Some("HELM_CODEX_REAL_CLI_OK".to_string())
        );
        assert_eq!(summary.error_message, None);
    }

    #[test]
    fn parses_codex_final_answer_from_agent_message_event() {
        let stdout = r#"{"type":"thread.started","thread_id":"t1"}
{"type":"turn.started"}
{"type":"item.completed","item":{"id":"item_0","type":"agent_message","text":"OK"}}
{"type":"turn.completed"}
"#;

        let summary = codex_output_summary(stdout);

        assert_eq!(summary.final_text, Some("OK".to_string()));
        assert_eq!(summary.error_message, None);
    }

    #[test]
    fn parses_codex_usage_from_turn_completed_event() {
        let stdout = r#"{"type":"thread.started","thread_id":"t1"}
{"type":"turn.completed","usage":{"input_tokens":123,"output_tokens":45},"total_cost_usd":0.0123}
"#;

        let summary = codex_output_summary(stdout);

        assert_eq!(summary.input_tokens, 123);
        assert_eq!(summary.output_tokens, 45);
        assert_eq!(summary.cost_usd, 0.0123);
    }

    #[test]
    fn parses_codex_usage_from_camelcase_event() {
        let stdout = r#"{"type":"turn.completed","tokenUsage":{"inputTokens":7,"outputTokens":8,"costUsd":0.0009}}"#;

        let summary = codex_output_summary(stdout);

        assert_eq!(summary.input_tokens, 7);
        assert_eq!(summary.output_tokens, 8);
        assert_eq!(summary.cost_usd, 0.0009);
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
}

/// 从错误文案（含透传的 CLI stderr）推断错误分类，供前端渲染修复动作。
/// 顺序敏感：具体原因（未安装/未登录/目录无效）要排在笼统的"进程异常退出"之前。
pub(crate) fn classify_error(message: &str) -> Option<String> {
    let lower = message.to_lowercase();
    let kind = if lower.contains("未设置工作目录") || lower.contains("工作目录不存在")
    {
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
    {
        "auth_missing"
    } else if lower.contains("unknown option")
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

/// 「始终允许」的匹配键（变更-07）：Bash 细化到命令首词（`Bash:npm`），
/// 批准一次某命令不再放行所有 Bash（含 `rm -rf`）；其余工具仍按工具名。
/// 必须与 hook 脚本里的 allowKey 构造保持一致。
fn always_allow_key(tool_name: &str, input: &serde_json::Value) -> String {
    if tool_name == "Bash" {
        if let Some(cmd) = input.get("command").and_then(|c| c.as_str()) {
            if let Some(first) = cmd.trim().split_whitespace().next() {
                return format!("Bash:{first}");
            }
        }
    }
    tool_name.to_string()
}

async fn record_approval(
    runtime: &SessionRuntime,
    request_id: &str,
    decision: ApprovalDecision,
) -> Result<(), String> {
    let mut state = read_approval_state(&runtime.state_path);
    let hook_decision = decision.hook_decision().to_string();
    state
        .decisions
        .insert(request_id.to_string(), hook_decision);

    // 获取待审批工具的信息
    let pending_info = runtime.pending_tools.lock().await.get(request_id).cloned();

    if matches!(decision, ApprovalDecision::Always) {
        if let Some(info) = &pending_info {
            let allow_key = always_allow_key(&info.name, &info.input);
            if !state.always_allow.iter().any(|item| item == &allow_key) {
                state.always_allow.push(allow_key.clone());
            }
            // 跨会话持久化（P2-4）：写入 SQLite，下个会话启动时播种回 hook state。
            // 持久化失败不阻断本次审批（会话内仍然生效），只留诊断日志。
            if let Some(store) = runtime.app.try_state::<SessionHistoryStore>() {
                if let Err(err) = store.add_always_allow_tool(&allow_key) {
                    eprintln!("[approval] 持久化「始终允许」失败（{allow_key}）：{err}");
                }
            }
        }
    }

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

    write_approval_state(&runtime.state_path, &state)
}

fn try_begin_turn(runtime: &SessionRuntime) -> bool {
    runtime
        .busy
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
}

struct TurnBusyGuard {
    runtime: Arc<SessionRuntime>,
}

impl Drop for TurnBusyGuard {
    fn drop(&mut self) {
        self.runtime.busy.store(false, Ordering::Release);
    }
}

async fn run_claude_turn(
    runtime: Arc<SessionRuntime>,
    prompt: Option<String>,
    attachments: Vec<String>,
    resume: bool,
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
            emit_error(
                &runtime,
                "已有轮次正在运行，请先等待或停止当前任务".to_string(),
                true,
            )
            .await;
            return;
        }
    }

    let resume_id = runtime.session_id.lock().await.clone();
    if resume && resume_id.is_none() {
        emit_error(
            &runtime,
            "无法继续审批：Claude sessionId 尚未建立".to_string(),
            false,
        )
        .await;
        return;
    }

    runtime.interrupted.store(false, Ordering::Release);

    // 工作目录守卫：绝不静默继承 Helm 进程自身目录（Agent 会在错误的地方读写文件）。
    if let Err(message) = validate_cwd(&runtime.cwd) {
        emit_error(&runtime, message, false).await;
        return;
    }
    if let Err(message) = validate_engine_bin(&runtime.bin) {
        emit_error(&runtime, message, false).await;
        return;
    }

    // 本轮会话模式（变更-04）：Send 时已写入 runtime；审批恢复轮沿用发起轮的值
    let mode = *runtime.turn_mode.lock().await;

    // 新轮次开始时清空被拒绝目标列表（允许重新尝试），并把本轮模式同步给 hook
    if !resume {
        let mut state = read_approval_state(&runtime.state_path);
        state.denied_targets.clear();
        state.turn_mode = mode.as_state_str().to_string();
        let _ = write_approval_state(&runtime.state_path, &state);
    }

    let mut cmd = build_command(&runtime.bin);
    apply_inherited_agent_environment(&mut cmd);
    cmd.arg("-p")
        .args([
            "--output-format",
            "stream-json",
            "--verbose",
            "--include-partial-messages",
            "--include-hook-events",
        ])
        .arg("--settings")
        .arg(&runtime.settings_path);

    // 会话级 MCP 开关（变更-11）：停用名单非空时，以过滤后的配置完全接管本轮 MCP 集合
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
                emit_error(&runtime, format!("应用会话级 MCP 开关失败：{err}"), true).await;
            }
        }
    }

    if let Some(model) = (!runtime.model.is_empty()).then_some(runtime.model.as_str()) {
        cmd.args(["--model", model]);
    }
    // 模式 → CLI 参数（变更-04，C.1 实测契约）：
    // 计划 = 原生 plan 权限模式（CLI 自带只读约束 + 计划指令）；
    // 询问 = 软约束走 --append-system-prompt，硬约束在审批 hook 的 turnMode 判定；
    // 构建 = 不加参数（现状默认行为）。
    match mode {
        TurnMode::Plan => {
            cmd.args(["--permission-mode", "plan"]);
        }
        TurnMode::Ask => {
            cmd.args(["--append-system-prompt", ASK_MODE_APPEND_PROMPT]);
        }
        TurnMode::Build => {}
    }
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
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => {
            emit_error(&runtime, format!("无法启动 claude 进程：{e}"), false).await;
            return;
        }
    };

    set_running_pid(&runtime.running_pid, child.id()).await;
    let stdout = match child.stdout.take() {
        Some(stdout) => stdout,
        None => {
            set_running_pid(&runtime.running_pid, None).await;
            emit_error(&runtime, "无法获取 claude stdout".to_string(), false).await;
            return;
        }
    };
    let stderr = match child.stderr.take() {
        Some(stderr) => stderr,
        None => {
            set_running_pid(&runtime.running_pid, None).await;
            emit_error(&runtime, "无法获取 claude stderr".to_string(), false).await;
            return;
        }
    };

    // 终态与活动追踪：saw_turn_complete 用于兜底"退出码 0 但无 result"；
    // saw_approval 豁免审批 defer 场景（此时进程退出等待用户决定是正常流程）；
    // last_activity_ms 供看门狗判断进程是否挂起。
    let saw_turn_complete = Arc::new(AtomicBool::new(false));
    let saw_approval = Arc::new(AtomicBool::new(false));
    let last_activity_ms = Arc::new(AtomicU64::new(now_millis() as u64));

    let stdout_runtime = runtime.clone();
    let stdout_saw_turn_complete = saw_turn_complete.clone();
    let stdout_saw_approval = saw_approval.clone();
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

            for event in events {
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
                match &event {
                    AgentEvent::SessionStarted { session_id, .. } => {
                        *stdout_runtime.session_id.lock().await = Some(session_id.clone());
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
                        id, action, input, ..
                    } => {
                        stdout_saw_approval.store(true, Ordering::Release);
                        stdout_runtime
                            .pending_tools
                            .lock()
                            .await
                            .entry(id.clone())
                            .or_insert_with(|| PendingToolInfo {
                                name: action.clone(),
                                input: input.clone().unwrap_or(serde_json::Value::Null),
                            });
                    }
                    AgentEvent::TurnComplete { .. } => {
                        stdout_saw_turn_complete.store(true, Ordering::Release);
                    }
                    _ => {}
                }
                emit_agent_event(
                    &stdout_runtime.app,
                    &stdout_runtime.history_session_id,
                    &event,
                );
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
    let stderr_last_activity = last_activity_ms.clone();
    let stderr_task = tokio::spawn(async move {
        let mut buf = String::new();
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            stderr_last_activity.store(now_millis() as u64, Ordering::Release);
            // 检测 Hook 的审批通知（兜底通道：常规链路走 stdout 的 deferred_tool_use）
            if line.starts_with("APPROVAL_NEEDED:") {
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
                emit_error(
                    &watchdog_runtime,
                    "claude 已超过 5 分钟没有任何输出，可能已挂起；可点击停止按钮中断本轮"
                        .to_string(),
                    true,
                )
                .await;
                return;
            }
        }
    });

    let status = child.wait().await;
    watchdog.abort();
    let _ = stdout_task.await;
    let detail = stderr_task.await.unwrap_or_default().trim().to_string();
    set_running_pid(&runtime.running_pid, None).await;

    if runtime.interrupted.load(Ordering::Acquire) {
        return;
    }

    // 退出码判定：wait 出错或被信号杀死（无退出码）一律视为异常，绝不能默认成功——
    // 否则既无报错也无 turn_complete，UI 会永远停在"思考中"。
    let code = match &status {
        Ok(s) if s.success() => 0,
        Ok(s) => s.code().unwrap_or(-1),
        Err(_) => -1,
    };
    if code != 0 {
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
    } else if !saw_turn_complete.load(Ordering::Acquire) && !saw_approval.load(Ordering::Acquire) {
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
    let pid = *runtime.running_pid.lock().await;
    kill_tree(pid).await;
    set_running_pid(&runtime.running_pid, None).await;
    emit_interrupted(&runtime).await;
}

/// 创建 Claude 会话运行时。真正的 CLI 进程在每次 send / approve(resume) 时启动。
pub fn start_claude(
    app: AppHandle,
    history_session_id: String,
    bin: String,
    model: String,
    cwd: String,
    env: Vec<(String, String)>,
    approval_policy: ApprovalPolicy,
) -> Result<AgentSession, String> {
    start_claude_with_resume(
        app,
        history_session_id,
        bin,
        model,
        cwd,
        env,
        approval_policy,
        None,
        Vec::new(),
    )
}

#[allow(clippy::too_many_arguments)]
pub fn start_claude_with_resume(
    app: AppHandle,
    history_session_id: String,
    bin: String,
    model: String,
    cwd: String,
    env: Vec<(String, String)>,
    approval_policy: ApprovalPolicy,
    resume_id: Option<String>,
    // 没有可 --resume 的 CLI 会话（如回溯后被作废）时，用这份历史序列化重建上下文（P2-5）
    history_messages: Vec<crate::sessions::SessionMessage>,
) -> Result<AgentSession, String> {
    let hook_files = create_approval_hook_files(approval_policy)?;
    let runtime = Arc::new(SessionRuntime {
        app,
        history_session_id,
        bin,
        model,
        cwd,
        env,
        turn_mode: Mutex::new(TurnMode::Build),
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
        busy: AtomicBool::new(false),
        interrupted: AtomicBool::new(false),
        pending_tools: Mutex::new(HashMap::new()),
        disabled_mcp: std::sync::Mutex::new(Vec::new()),
    });

    let (tx, mut rx) = mpsc::unbounded_channel::<SessionCmd>();
    let manager_runtime = runtime.clone();
    tokio::spawn(async move {
        while let Some(cmd) = rx.recv().await {
            match cmd {
                SessionCmd::Send {
                    text,
                    attachments,
                    mode,
                } => {
                    if !try_begin_turn(&manager_runtime) {
                        emit_error(
                            &manager_runtime,
                            "已有轮次正在运行，请先等待或停止当前任务".to_string(),
                            true,
                        )
                        .await;
                        continue;
                    }
                    // 模式随发起轮固定（变更-04）：审批恢复轮读到的仍是这里写入的值
                    *manager_runtime.turn_mode.lock().await = mode;
                    tokio::spawn(run_claude_turn(
                        manager_runtime.clone(),
                        Some(text),
                        attachments,
                        false,
                    ));
                }
                SessionCmd::Approve {
                    request_id,
                    decision,
                } => {
                    if !try_begin_turn(&manager_runtime) {
                        emit_error(
                            &manager_runtime,
                            "已有轮次正在运行，请先等待或停止当前任务".to_string(),
                            true,
                        )
                        .await;
                        continue;
                    }
                    if let Err(e) = record_approval(&manager_runtime, &request_id, decision).await {
                        manager_runtime.busy.store(false, Ordering::Release);
                        emit_error(&manager_runtime, e, false).await;
                        continue;
                    }
                    if matches!(decision, ApprovalDecision::Deny) {
                        emit_denied_turn(&manager_runtime, request_id).await;
                        manager_runtime.busy.store(false, Ordering::Release);
                        continue;
                    }
                    tokio::spawn(run_claude_turn(
                        manager_runtime.clone(),
                        None,
                        Vec::new(),
                        true,
                    ));
                }
                SessionCmd::ResetContext { messages } => {
                    // 回溯重建（P2-5）：作废 CLI 会话 id，下一轮以截断历史重新开场。
                    // 不需要抢 busy——只改状态，不拉进程。
                    *manager_runtime.session_id.lock().await = None;
                    *manager_runtime.rebuild_history.lock().await = messages;
                }
                SessionCmd::SetDisabledMcp { disabled } => {
                    // 会话级 MCP 开关（变更-11）：只改状态，下一轮拉进程时生效
                    if let Ok(mut guard) = manager_runtime.disabled_mcp.lock() {
                        *guard = disabled;
                    }
                }
                SessionCmd::Interrupt => {
                    interrupt_running(manager_runtime.clone()).await;
                }
            }
        }
    });

    Ok(AgentSession::Claude(ClaudeSession { tx }))
}

impl CodexSession {
    fn send(&self, text: String, attachments: Vec<String>, mode: TurnMode) -> Result<(), String> {
        // 轮次互斥（变更-06）：Codex 每轮独立 spawn，没有 Claude 的 manager 循环，
        // 在 send 入口做 CAS，防止前端状态失真时同一会话双进程并发。
        if self
            .busy
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err("当前会话已有轮次正在运行，请等待完成或先停止".to_string());
        }
        let app = self.app.clone();
        let history_session_id = self.history_session_id.clone();
        let bin = self.bin.clone();
        let model = self.model.clone();
        let cwd = self.cwd.clone();
        let env = self.env.clone();
        // 逐轮解析 sandbox（变更-04）：计划/询问强制只读，构建沿用设置映射值
        let sandbox_mode = codex_sandbox_for_mode(&self.sandbox_mode, mode);
        let running_pid = self.running_pid.clone();
        let history_messages = self.history_messages.clone();
        let busy = self.busy.clone();
        let interrupted = self.interrupted.clone();
        let disabled_mcp = self.disabled_mcp.clone();
        let thread_id = self.thread_id.clone();
        let auth_home = self.auth_home.clone();
        let force_history_rebuild = self.force_history_rebuild.clone();
        interrupted.store(false, Ordering::Release);
        spawn_agent_task(async move {
            run_codex_turn(
                app,
                history_session_id,
                bin,
                model,
                cwd,
                env,
                sandbox_mode,
                running_pid,
                history_messages,
                interrupted,
                disabled_mcp,
                thread_id,
                auth_home,
                force_history_rebuild,
                text,
                attachments,
                mode,
                true,
            )
            .await;
            busy.store(false, Ordering::Release);
        });
        Ok(())
    }

    fn interrupt(&self) -> Result<(), String> {
        let running_pid = self.running_pid.clone();
        // 先立标志再杀进程：run_codex_turn 收尾时据此改发 TurnComplete{Interrupted}
        self.interrupted.store(true, Ordering::Release);
        spawn_agent_task(async move {
            let pid = *running_pid.lock().await;
            kill_tree(pid).await;
            set_running_pid(&running_pid, None).await;
        });
        Ok(())
    }
}

pub fn start_codex(
    app: AppHandle,
    history_session_id: String,
    bin: String,
    model: String,
    cwd: String,
    env: Vec<(String, String)>,
    sandbox_mode: String,
    history_messages: Vec<crate::sessions::SessionMessage>,
    native_thread_id: Option<String>,
) -> Result<AgentSession, String> {
    let auth_home = create_codex_auth_home(&env, &[])?;
    Ok(AgentSession::Codex(CodexSession {
        app,
        history_session_id,
        bin,
        model,
        cwd,
        env,
        sandbox_mode,
        running_pid: Arc::new(Mutex::new(None)),
        history_messages: Arc::new(std::sync::Mutex::new(history_messages)),
        busy: Arc::new(AtomicBool::new(false)),
        interrupted: Arc::new(AtomicBool::new(false)),
        disabled_mcp: Arc::new(std::sync::Mutex::new(Vec::new())),
        thread_id: Arc::new(std::sync::Mutex::new(native_thread_id)),
        auth_home: Arc::new(std::sync::Mutex::new(auth_home)),
        force_history_rebuild: Arc::new(AtomicBool::new(false)),
    }))
}

async fn run_codex_turn(
    app: AppHandle,
    history_session_id: String,
    bin: String,
    model: String,
    cwd: String,
    env: Vec<(String, String)>,
    sandbox_mode: String,
    running_pid: Arc<Mutex<Option<u32>>>,
    history_messages: Arc<std::sync::Mutex<Vec<crate::sessions::SessionMessage>>>,
    interrupted: Arc<AtomicBool>,
    disabled_mcp: Arc<std::sync::Mutex<Vec<String>>>,
    thread_id: Arc<std::sync::Mutex<Option<String>>>,
    auth_home: Arc<std::sync::Mutex<Option<CodexAuthHome>>>,
    force_history_rebuild: Arc<AtomicBool>,
    prompt: String,
    attachments: Vec<String>,
    mode: TurnMode,
    allow_missing_thread_fallback: bool,
) {
    let seeded_thread_id = thread_id.lock().ok().and_then(|guard| guard.clone());
    let session_id = seeded_thread_id
        .clone()
        .unwrap_or_else(|| format!("codex-{}-{}", std::process::id(), now_millis()));
    emit_agent_event(
        &app,
        &history_session_id,
        &AgentEvent::SessionStarted {
            session_id: session_id.clone(),
            engine: EngineId::Codex,
            model: model.clone(),
            cwd: cwd.clone(),
            ts: now_millis() as i64,
        },
    );

    // 工作目录守卫：与 Claude 一致，绝不静默继承 Helm 进程自身目录。
    if let Err(message) = validate_cwd(&cwd) {
        emit_agent_event(
            &app,
            &history_session_id,
            &AgentEvent::Error {
                session_id: Some(session_id),
                kind: Some("cwd_invalid".to_string()),
                message,
                recoverable: false,
            },
        );
        return;
    }
    if let Err(message) = validate_engine_bin(&bin) {
        emit_agent_event(
            &app,
            &history_session_id,
            &AgentEvent::Error {
                session_id: Some(session_id),
                kind: Some("invalid_engine_bin".to_string()),
                message,
                recoverable: false,
            },
        );
        return;
    }

    let history_user_prompt = prompt_with_attachments(&prompt, &attachments);
    // 计划模式软约束（变更-04 A.3）：只注入发给 CLI 的 prompt，Helm 历史存的是用户原文
    let current_prompt = if mode == TurnMode::Plan {
        format!("{CODEX_PLAN_PROMPT_PREFIX}\n\n{history_user_prompt}")
    } else {
        history_user_prompt.clone()
    };
    let history_snapshot = history_messages
        .lock()
        .map(|history| history.clone())
        .unwrap_or_default();
    let rebuild_requested = force_history_rebuild.load(Ordering::Acquire);
    let exec_plan = codex_exec_plan(
        seeded_thread_id.as_deref(),
        rebuild_requested,
        &history_snapshot,
        &current_prompt,
    );
    let is_resume = matches!(exec_plan.command, CodexExecCommand::Resume { .. });

    let mut cmd = build_codex_command(&bin);
    apply_inherited_agent_environment(&mut cmd);
    let exec_args = codex_exec_args(&exec_plan, &model, &sandbox_mode);
    cmd.arg(&exec_args[0]);
    for arg in codex_provider_config_args(&env) {
        cmd.arg("-c").arg(arg);
    }
    cmd.args(&exec_args[1..]);
    cmd.current_dir(&cwd);
    let disabled_mcp_list = disabled_mcp
        .lock()
        .map(|guard| guard.clone())
        .unwrap_or_default();
    if let Some(path) = codex_auth_home_path(&auth_home) {
        cmd.env("CODEX_HOME", path);
    } else if !disabled_mcp_list.is_empty() {
        // 订阅登录（无 API Key）直接用真实 ~/.codex，改不了它的 config.toml——
        // 会话级 MCP 开关无法生效，明确告知而不是静默忽略（变更-11）
        emit_agent_event(
            &app,
            &history_session_id,
            &AgentEvent::Error {
                session_id: Some(session_id.clone()),
                message: "Codex 订阅登录暂不支持会话级 MCP 开关，本轮沿用全局 MCP 配置".to_string(),
                recoverable: true,
                kind: None,
            },
        );
    }
    for (key, value) in &env {
        if key.starts_with("HELM_") {
            continue;
        }
        cmd.env(key, value);
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    emit_agent_event(
        &app,
        &history_session_id,
        &AgentEvent::TurnStage {
            session_id: session_id.clone(),
            stage: if is_resume {
                TurnStage::RestoringSession
            } else {
                TurnStage::StartingEngine
            },
            ts: now_millis() as i64,
            engine_reported_ttft_ms: None,
            retry_attempt: None,
        },
    );
    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => {
            emit_agent_event(
                &app,
                &history_session_id,
                &AgentEvent::Error {
                    session_id: Some(session_id),
                    message: format!("无法启动 codex 进程：{e}"),
                    recoverable: false,
                    kind: Some("not_installed".to_string()),
                },
            );
            return;
        }
    };
    set_running_pid(&running_pid, child.id()).await;

    let last_activity_ms = Arc::new(AtomicU64::new(now_millis() as u64));
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let stdout_app = app.clone();
    let stdout_history_session_id = history_session_id.clone();
    let stdout_session_id = session_id.clone();
    let stdout_cwd = cwd.clone();
    let stdout_running_pid = running_pid.clone();
    let stdout_last_activity = last_activity_ms.clone();
    let stdout_thread_id = thread_id.clone();
    let stdout_force_history_rebuild = force_history_rebuild.clone();
    let out_task = tokio::spawn(async move {
        let mut result = CodexStreamResult::default();
        if let Some(stdout) = stdout {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                stdout_last_activity.store(now_millis() as u64, Ordering::Release);
                result.stdout.push_str(&line);
                result.stdout.push('\n');
                if is_resume {
                    if let Some(message) = codex_thread_missing_message_from_line(&line) {
                        result.thread_missing_error = Some(message);
                        continue;
                    }
                }
                if let Some(native_thread_id) = codex_thread_id_from_line(&line) {
                    if let Ok(mut guard) = stdout_thread_id.lock() {
                        *guard = Some(native_thread_id.clone());
                    }
                    stdout_force_history_rebuild.store(false, Ordering::Release);
                    if let Some(history_store) = stdout_app.try_state::<SessionHistoryStore>() {
                        if let Err(err) = history_store.attach_native_thread_to_session(
                            &stdout_history_session_id,
                            &native_thread_id,
                        ) {
                            emit_agent_event(
                                &stdout_app,
                                &stdout_history_session_id,
                                &AgentEvent::Error {
                                    session_id: Some(stdout_session_id.clone()),
                                    message: format!("保存 Codex thread id 失败：{err}"),
                                    recoverable: true,
                                    kind: Some("session_persist_failed".to_string()),
                                },
                            );
                        }
                    }
                }
                for event in parse_codex_line(&stdout_session_id, &line) {
                    match &event {
                        AgentEvent::MessageComplete { .. } => result.emitted_message = true,
                        AgentEvent::TokenUsage { .. } => result.emitted_usage = true,
                        AgentEvent::TurnComplete { .. } => result.emitted_turn_complete = true,
                        AgentEvent::ToolCall {
                            session_id,
                            id,
                            name,
                            input,
                            ..
                        } => {
                            if let Some(history_store) =
                                stdout_app.try_state::<SessionHistoryStore>()
                            {
                                let cwd = PathBuf::from(&stdout_cwd);
                                let checkpoint_result = stdout_app
                                    .path()
                                    .app_data_dir()
                                    .map_err(|err| format!("获取检查点目录失败：{err}"))
                                    .and_then(|app_data_dir| {
                                        create_auto_checkpoint_for_tool(
                                            &history_store,
                                            &app_data_dir.join("snapshots"),
                                            &stdout_history_session_id,
                                            session_id,
                                            &cwd,
                                            id,
                                            name,
                                            input,
                                        )
                                    });
                                match checkpoint_result {
                                    Ok(Some(checkpoint)) => emit_agent_event(
                                        &stdout_app,
                                        &stdout_history_session_id,
                                        &checkpoint,
                                    ),
                                    Ok(None) => {}
                                    Err(err) => {
                                        emit_agent_event(
                                            &stdout_app,
                                            &stdout_history_session_id,
                                            &AgentEvent::Error {
                                                session_id: Some(session_id.clone()),
                                                message: format!(
                                                    "自动创建检查点失败，已终止本轮：{err}"
                                                ),
                                                recoverable: false,
                                                kind: Some("checkpoint_failed".to_string()),
                                            },
                                        );
                                        let pid = *stdout_running_pid.lock().await;
                                        kill_tree(pid).await;
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                    emit_agent_event(&stdout_app, &stdout_history_session_id, &event);
                }
            }
        }
        result
    });
    let err_last_activity = last_activity_ms.clone();
    let err_task = tokio::spawn(async move {
        let mut text = String::new();
        if let Some(stderr) = stderr {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                err_last_activity.store(now_millis() as u64, Ordering::Release);
                text.push_str(&line);
                text.push('\n');
            }
        }
        text
    });

    // 看门狗：codex 长时间无输出时提示用户（不强杀）。
    let watchdog_app = app.clone();
    let watchdog_history = history_session_id.clone();
    let watchdog_session = session_id.clone();
    let watchdog_activity = last_activity_ms.clone();
    let watchdog = tokio::spawn(async move {
        const IDLE_WARN_MS: u64 = 300_000;
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            let idle =
                (now_millis() as u64).saturating_sub(watchdog_activity.load(Ordering::Acquire));
            if idle >= IDLE_WARN_MS {
                emit_agent_event(
                    &watchdog_app,
                    &watchdog_history,
                    &AgentEvent::Error {
                        session_id: Some(watchdog_session),
                        message:
                            "codex 已超过 5 分钟没有任何输出，可能已挂起；可点击停止按钮中断本轮"
                                .to_string(),
                        recoverable: true,
                        kind: Some("timeout".to_string()),
                    },
                );
                return;
            }
        }
    });

    let status = child.wait().await;
    watchdog.abort();
    set_running_pid(&running_pid, None).await;
    let stream_result = out_task.await.unwrap_or_default();
    let stderr_text = err_task.await.unwrap_or_default();
    let output_summary = codex_output_summary(&stream_result.stdout);
    let final_text = output_summary.final_text.unwrap_or_default();

    // 用户中断（变更-09）：杀进程导致的非零退出是预期行为，
    // 发 TurnComplete{Interrupted} 而不是红色错误卡（与 Claude 路径对齐）
    if interrupted.load(Ordering::Acquire) {
        emit_agent_event(
            &app,
            &history_session_id,
            &AgentEvent::TurnComplete {
                session_id,
                stop_reason: StopReason::Interrupted,
            },
        );
        return;
    }

    if is_resume && allow_missing_thread_fallback {
        if let Some(message) = stream_result.thread_missing_error.as_deref().or_else(|| {
            let trimmed = stderr_text.trim();
            is_codex_thread_missing_error(trimmed).then_some(trimmed)
        }) {
            if let Ok(mut guard) = thread_id.lock() {
                *guard = None;
            }
            force_history_rebuild.store(true, Ordering::Release);
            emit_agent_event(
                &app,
                &history_session_id,
                &AgentEvent::Error {
                    session_id: Some(session_id.clone()),
                    message: format!("Codex 原生 thread 已不存在，将用本地历史重建一次：{message}"),
                    recoverable: true,
                    kind: Some("thread_missing".to_string()),
                },
            );
            emit_agent_event(
                &app,
                &history_session_id,
                &AgentEvent::TurnStage {
                    session_id: session_id.clone(),
                    stage: TurnStage::Retrying,
                    ts: now_millis() as i64,
                    engine_reported_ttft_ms: None,
                    retry_attempt: Some(1),
                },
            );
            Box::pin(run_codex_turn(
                app,
                history_session_id,
                bin,
                model,
                cwd,
                env,
                sandbox_mode,
                running_pid,
                history_messages,
                interrupted,
                disabled_mcp,
                thread_id,
                auth_home,
                force_history_rebuild,
                prompt,
                attachments,
                mode,
                false,
            ))
            .await;
            return;
        }
    }

    if !stream_result.emitted_message && !final_text.is_empty() {
        emit_agent_event(
            &app,
            &history_session_id,
            &AgentEvent::MessageComplete {
                session_id: session_id.clone(),
                role: Role::Assistant,
                text: final_text.clone(),
            },
        );
    }

    if !stream_result.emitted_usage
        && (output_summary.input_tokens > 0 || output_summary.output_tokens > 0)
    {
        emit_agent_event(
            &app,
            &history_session_id,
            &AgentEvent::TokenUsage {
                session_id: session_id.clone(),
                input_tokens: output_summary.input_tokens,
                output_tokens: output_summary.output_tokens,
                cost_usd: output_summary.cost_usd,
                context_window: None,
            },
        );
    }

    let ok = status.map(|status| status.success()).unwrap_or(false);
    if ok && !final_text.is_empty() {
        match history_messages.lock() {
            Ok(mut history) => append_codex_turn_history(
                &mut history,
                &history_user_prompt,
                &final_text,
                now_millis() as i64,
            ),
            Err(_) => emit_agent_event(
                &app,
                &history_session_id,
                &AgentEvent::Error {
                    session_id: Some(session_id.clone()),
                    message: "Codex 运行时历史更新失败，下一轮上下文可能不完整".to_string(),
                    recoverable: true,
                    kind: Some("history_update_failed".to_string()),
                },
            ),
        }
    }
    if !ok {
        let detail = output_summary.error_message.as_deref().or_else(|| {
            let trimmed = stderr_text.trim();
            (!trimmed.is_empty()).then_some(trimmed)
        });
        emit_agent_event(
            &app,
            &history_session_id,
            &AgentEvent::Error {
                session_id: Some(session_id.clone()),
                message: if let Some(detail) = detail {
                    format!("codex 进程异常退出：{detail}")
                } else {
                    "codex 进程异常退出".to_string()
                },
                recoverable: false,
                kind: detail
                    .and_then(classify_error)
                    .or_else(|| Some("process_crash".to_string())),
            },
        );
    }
    if !stream_result.emitted_turn_complete {
        emit_agent_event(
            &app,
            &history_session_id,
            &AgentEvent::TurnComplete {
                session_id,
                stop_reason: if ok {
                    StopReason::End
                } else {
                    StopReason::Error
                },
            },
        );
    }
}

#[derive(Default)]
struct CodexStreamResult {
    stdout: String,
    emitted_message: bool,
    emitted_usage: bool,
    emitted_turn_complete: bool,
    thread_missing_error: Option<String>,
}

#[derive(Default)]
struct CodexOutputSummary {
    final_text: Option<String>,
    error_message: Option<String>,
    input_tokens: u64,
    output_tokens: u64,
    cost_usd: f64,
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
                    output_tokens,
                    cost_usd,
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
        ts: now_millis() as i64,
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
        output,
        diff,
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

fn codex_output_summary(stdout: &str) -> CodexOutputSummary {
    let mut summary = CodexOutputSummary::default();
    for line in stdout.lines() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if let Some(text) = codex_text_from_value(&value) {
            summary.final_text = Some(text);
        }
        if let Some((input_tokens, output_tokens, cost_usd)) = codex_usage_from_value(&value) {
            summary.input_tokens = input_tokens;
            summary.output_tokens = output_tokens;
            summary.cost_usd = cost_usd;
        }
        if let Some(message) = value
            .get("error")
            .and_then(|error| error.get("message"))
            .and_then(|message| message.as_str())
            .or_else(|| {
                (value.get("type").and_then(|kind| kind.as_str()) == Some("error"))
                    .then(|| value.get("message").and_then(|message| message.as_str()))
                    .flatten()
            })
        {
            summary.error_message = Some(message.to_string());
        }
    }
    summary
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

fn codex_text_from_value(value: &serde_json::Value) -> Option<String> {
    if value.get("type").and_then(|kind| kind.as_str()) == Some("error") {
        return None;
    }
    value
        .get("message")
        .and_then(|message| message.as_str())
        .or_else(|| value.get("text").and_then(|message| message.as_str()))
        .or_else(|| value.get("content").and_then(|message| message.as_str()))
        .map(ToString::to_string)
        .or_else(|| codex_text_from_item(value.get("item")?))
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
    use super::parse_codex_line;

    fn serialized_events(raw: serde_json::Value) -> Vec<serde_json::Value> {
        parse_codex_line("codex-session", &raw.to_string())
            .into_iter()
            .map(|event| serde_json::to_value(event).unwrap())
            .collect()
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
}
