//! AgentEvent 的 Rust serde 镜像（对应 `packages/protocol/src/events.ts`）。
//!
//! 协议的唯一真值在 TypeScript（见 CLAUDE.md「单一真值」与 ADR 0002）：这里用 serde
//! 镜像出**同一套形状**，序列化的 JSON 必须能通过 TS 的 `isAgentEvent` 校验——
//! 因此标签字段为 `type`（值用 snake_case），所有内部字段一律 camelCase。
//! 改协议时先改 `packages/protocol`，再同步本文件，并让契约测试守住一致性。

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 引擎标识，对应 TS `EngineId`（kebab-case：`claude-code` / `codex`）。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum EngineId {
    ClaudeCode,
    Codex,
}

/// 消息角色（lowercase：`user` / `assistant`）。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TurnStage {
    Preparing,
    StartingEngine,
    RestoringSession,
    WaitingModel,
    Reasoning,
    UsingTool,
    Responding,
    Finalizing,
    WaitingApproval,
    Retrying,
}

/// 轮次结束原因，对应 `turn_complete.stopReason`。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum StopReason {
    End,
    Interrupted,
    Error,
}

/// `tool_call` 的状态恒为 `pending`（与协议一致）。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum CallStatus {
    Pending,
}

/// `tool_result` 的最终状态。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ToolStatus {
    Success,
    Error,
}

/// 计划条目状态。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PlanStatus {
    Pending,
    Active,
    Done,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlanStep {
    pub text: String,
    pub status: PlanStatus,
}

/// 差异行类型。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DiffKind {
    Add,
    Del,
    Ctx,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: DiffKind,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DiffHunk {
    pub old_start: u32,
    pub new_start: u32,
    pub lines: Vec<DiffLine>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Diff {
    pub path: String,
    pub hunks: Vec<DiffHunk>,
}

/// 归一化的「后端 → UI」事件，对应 TS `AgentEvent`。
///
/// internally tagged：序列化为 `{ "type": "message_delta", "sessionId": "...", ... }`。
/// 任何引擎适配器都只能对外产出这里定义的事件，不得泄漏某个 CLI 的原始格式。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    #[serde(rename_all = "camelCase")]
    SessionStarted {
        session_id: String,
        engine: EngineId,
        model: String,
        cwd: String,
        ts: i64,
    },
    #[serde(rename_all = "camelCase")]
    MessageDelta {
        session_id: String,
        role: Role,
        text: String,
    },
    #[serde(rename_all = "camelCase")]
    MessageComplete {
        session_id: String,
        role: Role,
        text: String,
    },
    #[serde(rename_all = "camelCase")]
    ThinkingDelta { session_id: String, text: String },
    #[serde(rename_all = "camelCase")]
    ThinkingComplete { session_id: String, text: String },
    #[serde(rename_all = "camelCase")]
    TurnStage {
        session_id: String,
        stage: TurnStage,
        ts: i64,
        #[serde(skip_serializing_if = "Option::is_none")]
        engine_reported_ttft_ms: Option<f64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        retry_attempt: Option<u64>,
    },
    #[serde(rename_all = "camelCase")]
    ToolCall {
        session_id: String,
        id: String,
        name: String,
        input: Value,
        status: CallStatus,
    },
    #[serde(rename_all = "camelCase")]
    ToolProgress {
        session_id: String,
        id: String,
        chunk: String,
    },
    #[serde(rename_all = "camelCase")]
    ToolResult {
        session_id: String,
        id: String,
        status: ToolStatus,
        #[serde(skip_serializing_if = "Option::is_none")]
        output: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        diff: Option<Diff>,
    },
    #[serde(rename_all = "camelCase")]
    ApprovalRequest {
        session_id: String,
        id: String,
        action: String,
        detail: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        input: Option<Value>,
    },
    #[serde(rename_all = "camelCase")]
    PlanUpdate {
        session_id: String,
        steps: Vec<PlanStep>,
    },
    #[serde(rename_all = "camelCase")]
    Checkpoint {
        session_id: String,
        id: String,
        label: String,
        ts: i64,
    },
    #[serde(rename_all = "camelCase")]
    TokenUsage {
        session_id: String,
        input_tokens: u64,
        output_tokens: u64,
        cost_usd: f64,
        #[serde(skip_serializing_if = "Option::is_none")]
        context_window: Option<u64>,
    },
    #[serde(rename_all = "camelCase")]
    TurnComplete {
        session_id: String,
        stop_reason: StopReason,
    },
    #[serde(rename_all = "camelCase")]
    Error {
        #[serde(skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        message: String,
        recoverable: bool,
        /// 错误分类（引导修复用）：not_installed | auth_missing | version_incompatible
        /// | cwd_invalid | no_binding | network | process_crash | timeout | unknown。
        /// 缺省视为 unknown（向后兼容旧事件）。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        kind: Option<String>,
    },
}
