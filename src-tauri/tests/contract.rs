//! Rust 解析层的契约测试（对应 ADR 0002 的跨语言契约要求）：
//! 用 Rust parser 自有的去敏录制 fixture，断言解析出的事件序列形状合理，
//! 并验证序列化 JSON 用 `type` 标签 + camelCase 字段（与 TS `isAgentEvent` 对齐）。

use helm_lib::adapter::parse_codex_line_for_contract;
use helm_lib::parse::parse_claude_line;
use helm_lib::protocol::AgentEvent;
use std::fs;
use std::path::PathBuf;

fn fixture(name: &str) -> String {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("tests/fixtures");
    p.push(name);
    fs::read_to_string(&p).unwrap_or_else(|e| panic!("读不到 fixture {}: {e}", p.display()))
}

fn parse_all(content: &str) -> Vec<AgentEvent> {
    content.lines().flat_map(parse_claude_line).collect()
}

fn parse_codex(raw: serde_json::Value) -> Vec<serde_json::Value> {
    parse_codex_line_for_contract("codex-session", &raw.to_string())
        .into_iter()
        .map(|event| serde_json::to_value(event).unwrap())
        .collect()
}

#[test]
fn stream_fixture_shapes_match_protocol() {
    let events = parse_all(&fixture("claude-stream.jsonl"));
    assert!(!events.is_empty(), "应解析出事件");

    assert!(
        matches!(events.first(), Some(AgentEvent::SessionStarted { .. })),
        "首事件应为 session_started"
    );
    assert!(
        matches!(events.last(), Some(AgentEvent::TurnComplete { .. })),
        "末事件应为 turn_complete"
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::TokenUsage { .. })),
        "应含 token_usage"
    );
    assert!(
        events.iter().any(|e| matches!(
            e,
            AgentEvent::TokenUsage {
                context_window: Some(window),
                ..
            } if *window > 0
        )),
        "应从真实 Claude Code modelUsage 解析 contextWindow"
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::MessageDelta { .. })),
        "应含逐字增量 message_delta"
    );

    // tool_call 与 tool_result 按 id 配对。
    let calls: Vec<&str> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::ToolCall { id, .. } => Some(id.as_str()),
            _ => None,
        })
        .collect();
    let results: Vec<&str> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::ToolResult { id, .. } => Some(id.as_str()),
            _ => None,
        })
        .collect();
    assert!(!calls.is_empty(), "stream fixture 应含 tool_call");
    for id in &calls {
        assert!(
            results.contains(id),
            "tool_call {id} 应有配对的 tool_result"
        );
    }
}

#[test]
fn hello_fixture_starts_and_ends_cleanly() {
    let events = parse_all(&fixture("claude-hello.jsonl"));
    assert!(
        matches!(events.first(), Some(AgentEvent::SessionStarted { .. })),
        "首事件应为 session_started"
    );
    assert!(
        matches!(events.last(), Some(AgentEvent::TurnComplete { .. })),
        "末事件应为 turn_complete"
    );
}

#[test]
fn serialized_event_uses_type_tag_and_camelcase() {
    let events = parse_all(&fixture("claude-stream.jsonl"));
    let first = serde_json::to_value(events.first().unwrap()).unwrap();
    assert_eq!(first["type"], "session_started", "标签字段应为 type");
    assert!(
        first.get("sessionId").is_some(),
        "字段应为 camelCase sessionId（与 TS isAgentEvent 对齐）"
    );
    assert_eq!(first["engine"], "claude-code", "engine 应为 kebab-case");
}

#[test]
fn parses_tool_result_diff_block() {
    let line = serde_json::json!({
        "type": "user",
        "session_id": "s1",
        "message": {
            "role": "user",
            "content": [{
                "type": "tool_result",
                "tool_use_id": "toolu_1",
                "content": [
                    { "type": "text", "text": "Updated file" },
                    {
                        "type": "diff",
                        "path": "demo.txt",
                        "old_string": "one\ntwo\nthree\n",
                        "new_string": "one\nTWO\nthree\n"
                    }
                ]
            }]
        }
    })
    .to_string();

    let events = parse_claude_line(&line);
    let Some(AgentEvent::ToolResult {
        diff: Some(diff), ..
    }) = events.first()
    else {
        panic!("应从 tool_result 解析出 diff");
    };
    assert_eq!(diff.path, "demo.txt");
    let kinds: Vec<_> = diff.hunks[0]
        .lines
        .iter()
        .map(|line| serde_json::to_value(line.kind).unwrap())
        .collect();
    assert_eq!(kinds, vec!["del", "add"]);
}

#[test]
fn parses_deferred_tool_use_as_approval_request_without_turn_complete() {
    let line = serde_json::json!({
        "type": "result",
        "subtype": "success",
        "session_id": "s1",
        "stop_reason": "tool_deferred",
        "terminal_reason": "tool_deferred",
        "total_cost_usd": 0.01,
        "usage": { "input_tokens": 10, "output_tokens": 20 },
        "deferred_tool_use": {
            "id": "toolu_approval",
            "name": "Write",
            "input": { "file_path": "demo.txt", "content": "hello" }
        }
    })
    .to_string();

    let events = parse_claude_line(&line);
    assert!(
        matches!(events.first(), Some(AgentEvent::TokenUsage { .. })),
        "首事件应保留 token_usage"
    );
    let Some(AgentEvent::ApprovalRequest {
        id, action, detail, ..
    }) = events.get(1)
    else {
        panic!("deferred_tool_use 应解析为 approval_request");
    };
    assert_eq!(id, "toolu_approval");
    assert_eq!(action, "Write");
    assert!(detail.contains("demo.txt"), "审批详情应包含工具输入");
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, AgentEvent::TurnComplete { .. })),
        "等待审批时不能提前 turn_complete"
    );
}

#[test]
fn parses_permission_denials_as_completed_turn_not_approval_request() {
    let line = serde_json::json!({
        "type": "result",
        "subtype": "success",
        "session_id": "s1",
        "stop_reason": "end_turn",
        "terminal_reason": "completed",
        "total_cost_usd": 0.01,
        "usage": { "input_tokens": 10, "output_tokens": 20 },
        "permission_denials": [{
            "tool_name": "Edit",
            "tool_use_id": "toolu_denied",
            "tool_input": {
                "file_path": "demo.txt",
                "old_string": "before",
                "new_string": "after"
            }
        }]
    })
    .to_string();

    let events = parse_claude_line(&line);
    assert!(
        matches!(events.first(), Some(AgentEvent::TokenUsage { .. })),
        "首事件应保留 token_usage"
    );
    assert!(
        events
            .iter()
            .all(|event| !matches!(event, AgentEvent::ApprovalRequest { .. })),
        "permission_denials 是拒绝结果，不应再次生成 approval_request"
    );
    assert!(
        matches!(events.last(), Some(AgentEvent::TurnComplete { .. })),
        "拒绝后本轮应结束，而不是等待再次审批"
    );
}

#[test]
fn auto_review_denial_serializes_controlled_not_started_facts() {
    let line = serde_json::json!({
        "type": "user",
        "session_id": "s1",
        "message": { "content": [{
            "type": "tool_result",
            "tool_use_id": "toolu_auto",
            "is_error": true,
            "toolDenialKind": "automode-unavailable",
            "content": "classifier unavailable"
        }] }
    })
    .to_string();
    let value = serde_json::to_value(
        parse_claude_line(&line)
            .into_iter()
            .next()
            .expect("应解析 Auto denial"),
    )
    .unwrap();
    assert_eq!(value["type"], "tool_result");
    assert_eq!(value["outcome"], "auto_review_unavailable");
    assert_eq!(value["started"], false);
    assert_eq!(value["retryable"], true);
    assert_eq!(value["denialSource"], "auto_reviewer");
    assert_eq!(value["nativeDenialCode"], "automode-unavailable");
}

#[test]
fn parses_thinking_delta_as_protocol_event() {
    let line = serde_json::json!({
        "type": "stream_event",
        "session_id": "s1",
        "event": {
            "type": "content_block_delta",
            "index": 0,
            "delta": { "type": "thinking_delta", "thinking": "先读文件再改。" }
        }
    })
    .to_string();

    let events = parse_claude_line(&line);
    let Some(AgentEvent::ThinkingDelta { text, .. }) = events.first() else {
        panic!("thinking_delta 应解析为协议事件");
    };
    assert_eq!(text, "先读文件再改。");
}

#[test]
fn parses_assistant_thinking_block_as_complete_event() {
    let line = serde_json::json!({
        "type": "assistant",
        "session_id": "s1",
        "message": {
            "content": [{ "type": "thinking", "thinking": "已经确认修改点。" }]
        }
    })
    .to_string();

    let events = parse_claude_line(&line);
    let Some(AgentEvent::ThinkingComplete { text, .. }) = events.first() else {
        panic!("assistant thinking block 应解析为 thinking_complete");
    };
    assert_eq!(text, "已经确认修改点。");
}

#[test]
fn ignores_subagent_lines_with_parent_tool_use_id() {
    // 变更-09：并行子代理（Task）的行带 parent_tool_use_id，与主线程共用 session_id。
    // 不过滤会把子代理输出串进主回复。
    let sub = serde_json::json!({
        "type": "assistant",
        "session_id": "s1",
        "parent_tool_use_id": "toolu_sub_1",
        "message": { "content": [{ "type": "text", "text": "子代理内部输出" }] }
    })
    .to_string();
    assert!(
        parse_claude_line(&sub).is_empty(),
        "带 parent_tool_use_id 的子代理行必须被过滤，不进主线程"
    );

    // 主线程行（parent_tool_use_id 为 null）照常解析
    let main = serde_json::json!({
        "type": "assistant",
        "session_id": "s1",
        "parent_tool_use_id": serde_json::Value::Null,
        "message": { "content": [{ "type": "text", "text": "主线程回复" }] }
    })
    .to_string();
    let events = parse_claude_line(&main);
    let Some(AgentEvent::MessageComplete { text, .. }) = events.first() else {
        panic!("主线程行应正常解析");
    };
    assert_eq!(text, "主线程回复");
}

#[test]
fn parses_claude_status_requesting_as_waiting_model_stage() {
    let line = serde_json::json!({
        "type": "system",
        "subtype": "status",
        "status": "requesting",
        "session_id": "s1"
    })
    .to_string();

    let events = parse_claude_line(&line);
    let value = serde_json::to_value(events.first().expect("应解析 turn_stage")).unwrap();
    assert_eq!(value["type"], "turn_stage");
    assert_eq!(value["sessionId"], "s1");
    assert_eq!(value["stage"], "waiting_model");
    assert!(value["ts"].as_i64().is_some(), "turn_stage 应携带时间戳");
}

#[test]
fn parses_claude_message_start_as_responding_with_engine_ttft() {
    let line = serde_json::json!({
        "type": "stream_event",
        "session_id": "s1",
        "ttft_ms": 321,
        "event": { "type": "message_start", "message": { "role": "assistant" } }
    })
    .to_string();

    let events = parse_claude_line(&line);
    let value = serde_json::to_value(events.first().expect("应解析 turn_stage")).unwrap();
    assert_eq!(value["type"], "turn_stage");
    assert_eq!(value["sessionId"], "s1");
    assert_eq!(value["stage"], "responding");
    assert_eq!(value["engineReportedTtftMs"].as_f64(), Some(321.0));
}

#[test]
fn parses_codex_thread_and_turn_started_as_waiting_model() {
    for raw in [
        serde_json::json!({ "type": "thread.started", "thread_id": "thread-1" }),
        serde_json::json!({ "type": "turn.started" }),
    ] {
        let events = parse_codex(raw);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["type"], "turn_stage");
        assert_eq!(events[0]["sessionId"], "codex-session");
        assert_eq!(events[0]["stage"], "waiting_model");
    }
}

#[test]
fn parses_codex_reasoning_and_message_item_started_as_turn_stages() {
    let reasoning = parse_codex(serde_json::json!({
        "type": "item.started",
        "item": { "id": "reasoning-1", "type": "reasoning" }
    }));
    assert_eq!(reasoning.len(), 1);
    assert_eq!(reasoning[0]["type"], "turn_stage");
    assert_eq!(reasoning[0]["stage"], "reasoning");

    let responding = parse_codex(serde_json::json!({
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
    let mut events = parse_codex(serde_json::json!({
        "type": "item.started",
        "item": item.clone()
    }));
    events.extend(parse_codex(serde_json::json!({
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
