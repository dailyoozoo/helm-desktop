//! 把 Claude Code 的 stream-json stdout 行解析为归一化 `AgentEvent`。
//!
//! 这是 `packages/engine-claude-code/src/parse.ts` 的 Rust 移植：**行为必须一致**，
//! 以 `packages/engine-claude-code/test/fixtures/*.jsonl`（真实录制）为回归基准（见 ADR 0002）。
//!
//! 映射（严格依据实拍 schema）：
//!   system/init                                   → session_started
//!   stream_event content_block_delta/text_delta   → message_delta（逐字打字机）
//!   assistant message.content[].text              → message_complete
//!   assistant message.content[].tool_use          → tool_call
//!   user message.content[].tool_result            → tool_result
//!   result                                         → token_usage + turn_complete
//! thinking_delta                                  → thinking_delta
//! input_json_delta / message_start/stop 等噪声 → 忽略（空 vec）。
//!
//! 无状态：所需 sessionId 每行都带；tool_use 入参直接取自已定稿的 assistant 消息。

use crate::protocol::{
    AgentEvent, CallStatus, Diff, DiffHunk, DiffKind, DiffLine, EngineId, PlanStatus, PlanStep,
    Role, RuntimeCapabilityAvailability, RuntimeCapabilitySnapshot, StopReason, ToolDenialSource,
    ToolOutcomeKind, ToolStatus, TurnStage,
};
use crate::util::now_millis;
use serde_json::{Map, Value};
use std::cell::RefCell;
use std::collections::HashMap;

const ENGINE: EngineId = EngineId::ClaudeCode;
const DIFF_LCS_CELL_LIMIT: usize = 262_144;
const DIFF_LOOKAHEAD_LINES: usize = 64;

/// 缓存 tool_use 的 input（key = tool_use_id），用于在 tool_result 时构造 diff。
/// Claude Code 的 Write 工具在 tool_result 中只返回纯文本，不包含 diff 结构，
/// 但 tool_use 阶段的 input 中有 file_path 和 content，可以用来生成 diff。
thread_local! {
    static TOOL_USE_INPUT_CACHE: RefCell<HashMap<String, Value>> = RefCell::new(HashMap::new());
}

/// 取字符串字段，缺失/非字符串时返回空串（对应 TS `asString`）。
fn str_field(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or("").to_string()
}

/// 把 `tool_result.content` 归一化为一段纯文本（content 可能是字符串或内容块数组）。
fn stringify_tool_content(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .map(|block| match block.get("text").and_then(Value::as_str) {
                Some(text) => text.to_string(),
                None => block.to_string(),
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// 尝试从 tool_result.content 中提取 Diff 结构。
/// Claude Code 对 Edit/Write 类工具会在 content 中嵌入 {type:"diff", old_string, new_string} 块。
fn extract_diff(content: &Value) -> Option<Diff> {
    let blocks = match content {
        Value::Array(arr) => arr,
        _ => return None,
    };

    let mut path = String::new();
    let mut old_text = String::new();
    let mut new_text = String::new();

    for block in blocks {
        if block.get("type").and_then(Value::as_str) == Some("diff") {
            path = block
                .get("path")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            old_text = block
                .get("old_string")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            new_text = block
                .get("new_string")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
        }
    }

    if old_text.is_empty() && new_text.is_empty() {
        return None;
    }

    let old_lines: Vec<&str> = old_text.lines().collect();
    let new_lines: Vec<&str> = new_text.lines().collect();

    let hunks = compute_diff_hunks(&old_lines, &new_lines);
    if hunks.is_empty() {
        return None;
    }

    Some(Diff { path, hunks })
}

fn compute_diff_hunks(old_lines: &[&str], new_lines: &[&str]) -> Vec<DiffHunk> {
    let mut prefix = 0;
    while prefix < old_lines.len()
        && prefix < new_lines.len()
        && old_lines[prefix] == new_lines[prefix]
    {
        prefix += 1;
    }
    let mut suffix = 0;
    while suffix < old_lines.len() - prefix
        && suffix < new_lines.len() - prefix
        && old_lines[old_lines.len() - 1 - suffix] == new_lines[new_lines.len() - 1 - suffix]
    {
        suffix += 1;
    }
    let old = &old_lines[prefix..old_lines.len() - suffix];
    let new = &new_lines[prefix..new_lines.len() - suffix];
    if old.is_empty() && new.is_empty() {
        return Vec::new();
    }
    let width = new.len() + 1;
    let cells = (old.len() + 1).saturating_mul(width);
    let matrix = if cells <= DIFF_LCS_CELL_LIMIT {
        let mut matrix = vec![0u32; cells];
        for old_index in (0..old.len()).rev() {
            for new_index in (0..new.len()).rev() {
                matrix[old_index * width + new_index] = if old[old_index] == new[new_index] {
                    matrix[(old_index + 1) * width + new_index + 1] + 1
                } else {
                    matrix[(old_index + 1) * width + new_index]
                        .max(matrix[old_index * width + new_index + 1])
                };
            }
        }
        Some(matrix)
    } else {
        None
    };
    let mut lines = Vec::with_capacity(old.len() + new.len());
    let mut old_index = 0;
    let mut new_index = 0;
    while old_index < old.len() || new_index < new.len() {
        if old_index < old.len() && new_index < new.len() && old[old_index] == new[new_index] {
            lines.push(DiffLine {
                kind: DiffKind::Ctx,
                text: old[old_index].to_string(),
            });
            old_index += 1;
            new_index += 1;
            continue;
        }
        let delete_first = if new_index == new.len() {
            true
        } else if old_index == old.len() {
            false
        } else if let Some(matrix) = matrix.as_ref() {
            matrix[(old_index + 1) * width + new_index] >= matrix[old_index * width + new_index + 1]
        } else {
            let next_old = old[old_index + 1..old.len().min(old_index + DIFF_LOOKAHEAD_LINES + 1)]
                .iter()
                .position(|line| *line == new[new_index]);
            let next_new = new[new_index + 1..new.len().min(new_index + DIFF_LOOKAHEAD_LINES + 1)]
                .iter()
                .position(|line| *line == old[old_index]);
            match (next_old, next_new) {
                (Some(deletions), Some(additions)) => deletions <= additions,
                (Some(_), None) => true,
                (None, Some(_)) => false,
                (None, None) => {
                    lines.push(DiffLine {
                        kind: DiffKind::Del,
                        text: old[old_index].to_string(),
                    });
                    lines.push(DiffLine {
                        kind: DiffKind::Add,
                        text: new[new_index].to_string(),
                    });
                    old_index += 1;
                    new_index += 1;
                    continue;
                }
            }
        };
        if delete_first {
            lines.push(DiffLine {
                kind: DiffKind::Del,
                text: old[old_index].to_string(),
            });
            old_index += 1;
        } else {
            lines.push(DiffLine {
                kind: DiffKind::Add,
                text: new[new_index].to_string(),
            });
            new_index += 1;
        }
    }
    vec![DiffHunk {
        old_start: (prefix + 1) as u32,
        new_start: (prefix + 1) as u32,
        lines,
    }]
}

fn parse_system(obj: &Value, session_id: &str) -> Vec<AgentEvent> {
    if obj.get("subtype").and_then(Value::as_str) == Some("status")
        && obj.get("status").and_then(Value::as_str) == Some("requesting")
    {
        return vec![AgentEvent::TurnStage {
            session_id: session_id.to_string(),
            stage: TurnStage::WaitingModel,
            ts: now_millis(),
            engine_reported_ttft_ms: None,
            retry_attempt: None,
        }];
    }
    if obj.get("subtype").and_then(Value::as_str) != Some("init") {
        return vec![];
    }
    let tools = obj.get("tools").and_then(Value::as_array);
    let availability = |name: &str| match tools {
        None => RuntimeCapabilityAvailability::Unknown,
        Some(tools) if tools.is_empty() => RuntimeCapabilityAvailability::Unknown,
        Some(tools) if tools.iter().any(|tool| tool.as_str() == Some(name)) => {
            RuntimeCapabilityAvailability::Available
        }
        Some(_) => RuntimeCapabilityAvailability::Unavailable,
    };
    vec![AgentEvent::SessionStarted {
        session_id: session_id.to_string(),
        engine: ENGINE,
        model: str_field(obj, "model"),
        cwd: str_field(obj, "cwd"),
        ts: now_millis(),
        capabilities: Some(RuntimeCapabilitySnapshot {
            web_search: availability("WebSearch"),
            web_fetch: availability("WebFetch"),
            approval_contract_version: "claude-hook-bridge-v1".to_string(),
            capability_snapshot_id: None,
            auto_review_strategy: None,
        }),
    }]
}

fn parse_stream_event(obj: &Value, session_id: &str) -> Vec<AgentEvent> {
    let Some(event) = obj.get("event").filter(|e| e.is_object()) else {
        return vec![];
    };
    if event.get("type").and_then(Value::as_str) == Some("message_start") {
        return vec![AgentEvent::TurnStage {
            session_id: session_id.to_string(),
            stage: TurnStage::Responding,
            ts: now_millis(),
            engine_reported_ttft_ms: obj.get("ttft_ms").and_then(Value::as_f64),
            retry_attempt: None,
        }];
    }
    if event.get("type").and_then(Value::as_str) != Some("content_block_delta") {
        return vec![];
    }
    let Some(delta) = event.get("delta").filter(|d| d.is_object()) else {
        return vec![];
    };
    if delta.get("type").and_then(Value::as_str) == Some("text_delta") {
        if let Some(text) = delta.get("text").and_then(Value::as_str) {
            if !text.is_empty() {
                return vec![AgentEvent::MessageDelta {
                    session_id: session_id.to_string(),
                    role: Role::Assistant,
                    text: text.to_string(),
                }];
            }
        }
    }
    if delta.get("type").and_then(Value::as_str) == Some("thinking_delta") {
        if let Some(text) = delta.get("thinking").and_then(Value::as_str) {
            if !text.is_empty() {
                return vec![AgentEvent::ThinkingDelta {
                    session_id: session_id.to_string(),
                    text: text.to_string(),
                }];
            }
        }
    }
    vec![]
}

fn parse_assistant(obj: &Value, session_id: &str) -> Vec<AgentEvent> {
    let Some(content) = obj
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(Value::as_array)
    else {
        return vec![];
    };
    let mut out = Vec::new();
    for block in content {
        match block.get("type").and_then(Value::as_str) {
            Some("text") => {
                if let Some(text) = block.get("text").and_then(Value::as_str) {
                    if is_claude_model_unavailable_message(text) {
                        out.push(AgentEvent::Error {
                            session_id: Some(session_id.to_string()),
                            message: text.to_string(),
                            recoverable: false,
                            kind: Some("model_unavailable".to_string()),
                            stalled_kind: None,
                        });
                        continue;
                    }
                    out.push(AgentEvent::MessageComplete {
                        session_id: session_id.to_string(),
                        role: Role::Assistant,
                        text: text.to_string(),
                    });
                }
            }
            Some("thinking") => {
                if let Some(text) = block.get("thinking").and_then(Value::as_str) {
                    out.push(AgentEvent::ThinkingComplete {
                        session_id: session_id.to_string(),
                        text: text.to_string(),
                    });
                }
            }
            Some("tool_use") => {
                if let (Some(id), Some(name)) = (
                    block.get("id").and_then(Value::as_str),
                    block.get("name").and_then(Value::as_str),
                ) {
                    out.push(AgentEvent::ToolCall {
                        session_id: session_id.to_string(),
                        id: id.to_string(),
                        name: name.to_string(),
                        input: block
                            .get("input")
                            .cloned()
                            .unwrap_or(Value::Object(Map::new())),
                        status: CallStatus::Pending,
                    });
                }
            }
            _ => {}
        }
    }
    if let Some(usage) = obj.get("message").and_then(|message| message.get("usage")) {
        let fresh = usage
            .get("input_tokens")
            .and_then(Value::as_u64)
            .unwrap_or_default();
        let cached = usage
            .get("cache_read_input_tokens")
            .and_then(Value::as_u64)
            .unwrap_or_default();
        let cache_write = usage
            .get("cache_creation_input_tokens")
            .and_then(Value::as_u64)
            .unwrap_or_default();
        let context_tokens = fresh.saturating_add(cached).saturating_add(cache_write);
        if context_tokens > 0 {
            out.push(AgentEvent::ContextUsage {
                session_id: session_id.to_string(),
                context_tokens,
                context_window: None,
            });
        }
    }
    out
}

pub(crate) fn is_claude_model_unavailable_message(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("there's an issue with the selected model")
        && lower.contains("may not exist or you may not have access")
}

fn parse_user(obj: &Value, session_id: &str) -> Vec<AgentEvent> {
    let Some(content) = obj
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(Value::as_array)
    else {
        return vec![];
    };
    let mut out = Vec::new();
    for block in content {
        if block.get("type").and_then(Value::as_str) != Some("tool_result") {
            continue;
        }
        if let Some(tool_use_id) = block.get("tool_use_id").and_then(Value::as_str) {
            let is_error = block.get("is_error").and_then(Value::as_bool) == Some(true);
            let tool_content = block.get("content");
            let denial_code = block
                .get("toolDenialKind")
                .or_else(|| block.get("tool_denial_kind"))
                .or_else(|| {
                    block
                        .get("content")
                        .and_then(|content| content.get("toolDenialKind"))
                })
                .and_then(Value::as_str)
                .map(ToString::to_string);
            let (outcome, started, retryable, denial_source) = match denial_code.as_deref() {
                Some("automode-unavailable") => (
                    ToolOutcomeKind::AutoReviewUnavailable,
                    false,
                    true,
                    Some(ToolDenialSource::AutoReviewer),
                ),
                Some("automode-parsing-error") => (
                    ToolOutcomeKind::AutoReviewParseError,
                    false,
                    true,
                    Some(ToolDenialSource::AutoReviewer),
                ),
                Some("automode-blocked") => (
                    ToolOutcomeKind::AutoReviewBlocked,
                    false,
                    false,
                    Some(ToolDenialSource::AutoReviewer),
                ),
                Some(_) => (
                    ToolOutcomeKind::RuntimeDenied,
                    false,
                    false,
                    Some(ToolDenialSource::Runtime),
                ),
                None if is_error => (
                    ToolOutcomeKind::ToolFailed,
                    true,
                    false,
                    Some(ToolDenialSource::Tool),
                ),
                None => (ToolOutcomeKind::ToolSucceeded, true, false, None),
            };
            let output = tool_content.map(stringify_tool_content);
            out.push(AgentEvent::ToolResult {
                session_id: session_id.to_string(),
                id: tool_use_id.to_string(),
                status: if is_error {
                    ToolStatus::Error
                } else {
                    ToolStatus::Success
                },
                has_output: Some(output.as_deref().is_some_and(|value| !value.is_empty())),
                output,
                diff: tool_content.and_then(extract_diff),
                outcome: Some(outcome),
                started: Some(started),
                retryable: Some(retryable),
                denial_source,
                native_denial_code: denial_code,
            });
        }
    }
    out
}

fn map_stop_reason(obj: &Value) -> StopReason {
    // API 错误（is_error=true 或 terminal_reason 为已知错误类型）优先判定为 Error，
    // 即使 CLI 同时标记 subtype="success"。否则进程以 code!=0 退出时 Helm 只显示泛化
    // "进程异常退出"，淹没真实 API 错误原因。
    let is_api_error = obj.get("is_error").and_then(Value::as_bool) == Some(true);
    let terminal_reason = obj.get("terminal_reason").and_then(Value::as_str);
    let is_error_terminal = matches!(
        terminal_reason,
        Some("api_error" | "tool_error" | "authentication_failed")
    );
    if is_api_error || is_error_terminal {
        return StopReason::Error;
    }
    if obj.get("subtype").and_then(Value::as_str) == Some("success") {
        return StopReason::End;
    }
    let interrupted = terminal_reason == Some("interrupted")
        || obj.get("stop_reason").and_then(Value::as_str) == Some("interrupted");
    if interrupted {
        StopReason::Interrupted
    } else {
        StopReason::Error
    }
}

fn parse_result(obj: &Value, session_id: &str) -> Vec<AgentEvent> {
    let usage = obj.get("usage");
    let uncached_input_tokens = usage
        .and_then(|u| u.get("input_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let cached_input_tokens = usage
        .and_then(|u| u.get("cache_read_input_tokens"))
        .and_then(Value::as_u64);
    let cache_write_input_tokens = usage
        .and_then(|u| u.get("cache_creation_input_tokens"))
        .and_then(Value::as_u64);
    let input_tokens = uncached_input_tokens
        .saturating_add(cached_input_tokens.unwrap_or_default())
        .saturating_add(cache_write_input_tokens.unwrap_or_default());
    let output_tokens = usage
        .and_then(|u| u.get("output_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let cost_usd = obj
        .get("total_cost_usd")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    let context_window = context_window_from_model_usage(obj.get("modelUsage"));
    let mut out = vec![AgentEvent::TokenUsage {
        session_id: session_id.to_string(),
        input_tokens,
        cached_input_tokens,
        cache_write_input_tokens,
        output_tokens,
        cost_usd,
        service_tier: usage
            .and_then(|u| u.get("service_tier"))
            .and_then(Value::as_str)
            .map(str::to_string),
        context_window,
    }];

    if let Some(deferred) = obj
        .get("deferred_tool_use")
        .filter(|value| value.is_object())
    {
        if let (Some(id), Some(name)) = (
            deferred.get("id").and_then(Value::as_str),
            deferred.get("name").and_then(Value::as_str),
        ) {
            let input = deferred.get("input").cloned();
            let detail = input
                .as_ref()
                .and_then(|i| serde_json::to_string_pretty(i).ok())
                .unwrap_or_else(|| input.as_ref().map(|i| i.to_string()).unwrap_or_default());
            out.push(AgentEvent::ApprovalRequest {
                session_id: session_id.to_string(),
                id: id.to_string(),
                action: name.to_string(),
                detail,
                input,
                available_decisions: vec![
                    crate::protocol::ApprovalDecisionOption::Allow,
                    crate::protocol::ApprovalDecisionOption::Deny,
                ],
                persistent_label: None,
                matcher_summary: None,
            });
            return out;
        }
    }

    out.push(AgentEvent::TurnComplete {
        session_id: session_id.to_string(),
        stop_reason: map_stop_reason(obj),
    });
    out
}

fn context_window_from_model_usage(model_usage: Option<&Value>) -> Option<u64> {
    let usage = model_usage?.as_object()?;
    usage
        .values()
        .filter_map(|value| value.get("contextWindow").and_then(Value::as_u64))
        .find(|window| *window > 0)
}

/// 解析一行 Claude Code stream-json 输出，产出 0..n 个归一化事件。
/// 空行 / 非 JSON / 无关类型 → 空 vec（与 TS 版一致）。
pub fn parse_claude_line(raw: &str) -> Vec<AgentEvent> {
    let line = raw.trim();
    if line.is_empty() {
        return vec![];
    }
    let Ok(obj) = serde_json::from_str::<Value>(line) else {
        return vec![];
    };
    if !obj.is_object() {
        return vec![];
    }
    // 子代理隔离（变更-09）：parent_tool_use_id 非空的行来自并行子代理（Task），
    // 与主线程共用同一 session_id——不过滤会把子代理的 text_delta 拼进主回复、
    // 工具调用平铺进主线程。子代理的最终产出会以主线程 Task 工具的 tool_result 回来。
    if obj
        .get("parent_tool_use_id")
        .map(|v| !v.is_null())
        .unwrap_or(false)
    {
        return vec![];
    }
    let session_id = str_field(&obj, "session_id");
    match obj.get("type").and_then(Value::as_str) {
        Some("system") => parse_system(&obj, &session_id),
        Some("stream_event") => parse_stream_event(&obj, &session_id),
        Some("assistant") => parse_assistant(&obj, &session_id),
        Some("user") => parse_user(&obj, &session_id),
        Some("result") => parse_result(&obj, &session_id),
        _ => vec![],
    }
}

pub(crate) fn parse_codex_app_server_progress_notification(
    session_id: &str,
    notification: &Value,
) -> Vec<AgentEvent> {
    let Some(method) = notification.get("method").and_then(Value::as_str) else {
        return Vec::new();
    };
    let Some(params) = notification.get("params") else {
        return Vec::new();
    };
    if session_id.is_empty()
        || ["threadId", "turnId"].iter().any(|field| {
            !params
                .get(*field)
                .and_then(Value::as_str)
                .is_some_and(|value| !value.is_empty())
        })
    {
        return Vec::new();
    }
    match method {
        "turn/plan/updated" => {
            let Some(plan) = params.get("plan").and_then(Value::as_array) else {
                return Vec::new();
            };
            let mut steps = Vec::with_capacity(plan.len());
            for step in plan {
                let Some(text) = step.get("step").and_then(Value::as_str) else {
                    return Vec::new();
                };
                let status = match step.get("status").and_then(Value::as_str) {
                    Some("pending") => PlanStatus::Pending,
                    Some("inProgress") => PlanStatus::Active,
                    Some("completed") => PlanStatus::Done,
                    _ => return Vec::new(),
                };
                steps.push(PlanStep {
                    text: text.to_string(),
                    status,
                });
            }
            vec![AgentEvent::PlanUpdate {
                session_id: session_id.to_string(),
                steps,
            }]
        }
        "item/commandExecution/outputDelta"
        | "item/fileChange/outputDelta"
        | "item/mcpToolCall/progress" => {
            let Some(id) = params
                .get("itemId")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
            else {
                return Vec::new();
            };
            let chunk_field = if method == "item/mcpToolCall/progress" {
                "message"
            } else {
                "delta"
            };
            let Some(chunk) = params
                .get(chunk_field)
                .and_then(Value::as_str)
                .filter(|chunk| !chunk.is_empty())
            else {
                return Vec::new();
            };
            vec![AgentEvent::ToolProgress {
                session_id: session_id.to_string(),
                id: id.to_string(),
                chunk: chunk.to_string(),
            }]
        }
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod codex_app_server_progress_tests {
    use super::parse_codex_app_server_progress_notification;
    use crate::protocol::AgentEvent;
    use serde_json::{json, Value};

    const TOOL_NOTIFICATIONS: [(&str, &str); 3] = [
        ("item/commandExecution/outputDelta", "delta"),
        ("item/fileChange/outputDelta", "delta"),
        ("item/mcpToolCall/progress", "message"),
    ];

    fn notification(method: &str, params: Value) -> Value {
        let mut fields = json!({"threadId": "native-thread", "turnId": "native-turn"});
        fields
            .as_object_mut()
            .unwrap()
            .extend(params.as_object().unwrap().clone());
        json!({"method": method, "params": fields})
    }

    fn parse(notification: &Value) -> Vec<AgentEvent> {
        parse_codex_app_server_progress_notification("helm-session", notification)
    }

    #[test]
    fn plan_notification_maps_only_native_steps_and_statuses() {
        let raw = notification(
            "turn/plan/updated",
            json!({
                "explanation": "This is not an extra step or a progress percentage",
                "plan": [
                    {"step": "检查源码", "status": "completed"},
                    {"step": "执行测试🙂", "status": "inProgress"},
                    {"step": "  保留文本  ", "status": "pending"}
                ]
            }),
        );
        assert_eq!(
            serde_json::to_value(parse(&raw)).unwrap(),
            json!([{
                "type": "plan_update",
                "sessionId": "helm-session",
                "steps": [
                    {"text": "检查源码", "status": "done"},
                    {"text": "执行测试🙂", "status": "active"},
                    {"text": "  保留文本  ", "status": "pending"}
                ]
            }])
        );
    }

    #[test]
    fn empty_native_plan_is_a_valid_clear_not_an_invented_step() {
        for params in [
            json!({"plan": []}),
            json!({"plan": [], "explanation": null}),
        ] {
            let raw = notification("turn/plan/updated", params);
            assert_eq!(
                serde_json::to_value(parse(&raw)).unwrap(),
                json!([{"type": "plan_update", "sessionId": "helm-session", "steps": []}])
            );
        }
    }

    #[test]
    fn malformed_plan_snapshots_are_not_guessed_or_partially_emitted() {
        for params in [
            json!({}),
            json!({"plan": null}),
            json!({"plan": {"step": "not-an-array", "status": "pending"}}),
            json!({"plan": [null]}),
            json!({"plan": [{"step": 1, "status": "pending"}]}),
            json!({"plan": [{"step": "missing-status"}]}),
            json!({"plan": [{"title": "missing-step", "status": "pending"}]}),
        ] {
            assert!(parse(&notification("turn/plan/updated", params)).is_empty());
        }
        for status in ["in_progress", "active", "done", "unknown", ""] {
            let raw = notification(
                "turn/plan/updated",
                json!({"plan": [
                    {"step": "valid", "status": "completed"},
                    {"step": "invalid", "status": status}
                ]}),
            );
            assert!(parse(&raw).is_empty(), "unexpected status: {status}");
        }
    }

    #[test]
    fn native_tool_notifications_preserve_item_id_and_exact_text() {
        for (method, field) in TOOL_NOTIFICATIONS {
            for chunk in ["  输出🙂\r\n下一行\n", " \n", "100%"] {
                let mut params = json!({
                    "itemId": "native-tool-工具",
                    "delta": "wrong field",
                    "message": "wrong field"
                });
                params[field] = json!(chunk);
                let raw = notification(method, params);
                assert_eq!(
                    serde_json::to_value(parse(&raw)).unwrap(),
                    json!([{
                        "type": "tool_progress",
                        "sessionId": "helm-session",
                        "id": "native-tool-工具",
                        "chunk": chunk
                    }]),
                    "method: {method}"
                );
            }
        }
    }

    #[test]
    fn missing_or_empty_tool_output_does_not_fabricate_progress() {
        for (method, field) in TOOL_NOTIFICATIONS {
            for invalid_chunk in [Value::Null, json!(42), json!({"text": "nested"}), json!("")] {
                let mut params = json!({"itemId": "native-tool"});
                params[field] = invalid_chunk;
                assert!(parse(&notification(method, params)).is_empty());
            }
            assert!(parse(&notification(method, json!({"itemId": "native-tool"}))).is_empty());
            let mut params = json!({});
            params[field] = json!("real output");
            assert!(parse(&notification(method, params.clone())).is_empty());
            for invalid_id in [Value::Null, json!(42), json!("")] {
                params["itemId"] = invalid_id;
                assert!(parse(&notification(method, params.clone())).is_empty());
            }
            let wrong_field = if field == "delta" { "message" } else { "delta" };
            let mut wrong_params = json!({"itemId": "native-tool"});
            wrong_params[wrong_field] = json!("do not use another notification's field");
            assert!(parse(&notification(method, wrong_params)).is_empty());
        }
    }

    #[test]
    fn progress_notifications_require_session_thread_and_turn_attribution() {
        let valid = [
            notification("turn/plan/updated", json!({"plan": []})),
            notification(
                "item/commandExecution/outputDelta",
                json!({"itemId": "native-tool", "delta": "output"}),
            ),
            notification(
                "item/fileChange/outputDelta",
                json!({"itemId": "native-tool", "delta": "output"}),
            ),
            notification(
                "item/mcpToolCall/progress",
                json!({"itemId": "native-tool", "message": "output"}),
            ),
        ];
        for raw in valid {
            assert!(parse_codex_app_server_progress_notification("", &raw).is_empty());
            for field in ["threadId", "turnId"] {
                let mut missing = raw.clone();
                missing["params"].as_object_mut().unwrap().remove(field);
                assert!(parse(&missing).is_empty());
                for invalid_id in [Value::Null, json!(42), json!("")] {
                    let mut invalid = raw.clone();
                    invalid["params"][field] = invalid_id;
                    assert!(parse(&invalid).is_empty());
                }
            }
        }
    }

    #[test]
    fn unrelated_and_legacy_notifications_do_not_invent_progress() {
        for raw in [
            Value::Null,
            json!({}),
            json!({"method": "turn/plan/updated"}),
            json!({"method": "turn/plan/updated", "params": null}),
            notification(
                "item/commandExecution/terminalInteraction",
                json!({"delta": "input"}),
            ),
            notification(
                "item/unknown/progress",
                json!({"itemId": "native-tool", "delta": "text"}),
            ),
            json!({"type": "item.updated", "item": {"type": "todo_list", "items": []}}),
        ] {
            assert!(parse(&raw).is_empty());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::parse_claude_line;
    use crate::protocol::{AgentEvent, StopReason};

    #[test]
    fn model_unavailable_assistant_text_is_an_error() {
        let raw = serde_json::json!({
            "type":"assistant",
            "session_id":"s1",
            "message":{
                "content":[{
                    "type":"text",
                    "text":"There's an issue with the selected model (missing-model). It may not exist or you may not have access to it. Run --model to pick a different model."
                }]
            }
        });
        let events = parse_claude_line(&raw.to_string());
        assert!(matches!(
            events.as_slice(),
            [AgentEvent::Error { kind: Some(kind), recoverable: false, .. }]
                if kind == "model_unavailable"
        ));
    }

    #[test]
    fn assistant_usage_emits_replacement_context_usage() {
        let raw = serde_json::json!({
            "type":"assistant",
            "session_id":"s1",
            "message":{
                "content":[{"type":"text","text":"ok"}],
                "usage":{
                    "input_tokens":2,
                    "cache_read_input_tokens":37187,
                    "cache_creation_input_tokens":6024,
                    "output_tokens":2
                }
            }
        });
        let events = parse_claude_line(&raw.to_string());
        assert!(events.iter().any(|event| matches!(
            event,
            AgentEvent::ContextUsage {
                context_tokens: 43213,
                context_window: None,
                ..
            }
        )));
    }

    #[test]
    fn api_error_result_maps_to_error_stop_reason() {
        // CLI 输出 is_error=true + subtype=success 的 result 行，应映射为 Error 而非 End
        let raw = serde_json::json!({
            "type":"result",
            "session_id":"s1",
            "is_error":true,
            "subtype":"success",
            "terminal_reason":"api_error",
            "api_error_status":403,
            "result":"Failed to authenticate. API Error: 403"
        });
        let events = parse_claude_line(&raw.to_string());
        assert!(events.iter().any(|event| matches!(
            event,
            AgentEvent::TurnComplete {
                stop_reason: StopReason::Error,
                ..
            }
        )));
        // 正常成功的 result 行仍映射为 End
        let ok_raw = serde_json::json!({
            "type":"result",
            "session_id":"s1",
            "subtype":"success",
            "terminal_reason":"end_turn"
        });
        let ok_events = parse_claude_line(&ok_raw.to_string());
        assert!(ok_events.iter().any(|event| matches!(
            event,
            AgentEvent::TurnComplete {
                stop_reason: StopReason::End,
                ..
            }
        )));
    }

    #[test]
    fn automode_unavailable_preserves_not_started_denial_evidence() {
        let raw = serde_json::json!({
            "type":"user",
            "session_id":"s1",
            "message":{"content":[{
                "type":"tool_result",
                "tool_use_id":"tool-1",
                "is_error":true,
                "toolDenialKind":"automode-unavailable",
                "content":"mimo-v2.5-pro is temporarily unavailable"
            }]}
        });
        let events = parse_claude_line(&raw.to_string());
        assert!(matches!(
            events.as_slice(),
            [AgentEvent::ToolResult {
                outcome: Some(crate::protocol::ToolOutcomeKind::AutoReviewUnavailable),
                started: Some(false),
                retryable: Some(true),
                native_denial_code: Some(code),
                ..
            }] if code == "automode-unavailable"
        ));
    }

    #[test]
    fn automode_blocked_is_not_retryable_or_started() {
        let raw = serde_json::json!({
            "type":"user",
            "session_id":"s1",
            "message":{"content":[{
                "type":"tool_result",
                "tool_use_id":"tool-1",
                "is_error":true,
                "toolDenialKind":"automode-blocked",
                "content":"blocked"
            }]}
        });
        let events = parse_claude_line(&raw.to_string());
        assert!(matches!(
            events.as_slice(),
            [AgentEvent::ToolResult {
                outcome: Some(crate::protocol::ToolOutcomeKind::AutoReviewBlocked),
                started: Some(false),
                retryable: Some(false),
                ..
            }]
        ));
    }
}
