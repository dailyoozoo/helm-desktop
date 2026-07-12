use helm_lib::protocol::{
    AgentEvent, Diff, DiffHunk, DiffKind, DiffLine, EngineId, Role, StopReason,
};
use helm_lib::sessions::{NewSessionRecord, SessionHistoryStore, SessionStatus};
use std::fs;
use std::time::Duration;

fn temp_history_path(name: &str) -> std::path::PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "helm-session-history-{}-{name}.sqlite",
        std::process::id()
    ));
    let _ = fs::remove_file(&path);
    path
}

#[test]
fn session_history_creates_sqlite_schema() {
    let path = temp_history_path("schema");
    let store = SessionHistoryStore::new(path.clone());
    store
        .create_session(NewSessionRecord {
            id: "local-1".to_string(),
            engine: EngineId::ClaudeCode,
            model: "claude-sonnet-4.6".to_string(),
            cwd: "D:\\work\\demo".to_string(),
            created_at: 1_717_171_700,
        })
        .unwrap();

    let conn = rusqlite::Connection::open(path).unwrap();
    let table_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('session','message','tool_call','usage','setting')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(table_count, 5);
}

#[test]
fn session_history_returns_the_explicit_active_session() {
    let path = temp_history_path("active-session");
    let store = SessionHistoryStore::new(path);
    store
        .create_session(NewSessionRecord {
            id: "local-a".to_string(),
            engine: EngineId::ClaudeCode,
            model: "claude-sonnet-4.6".to_string(),
            cwd: "D:\\work\\a".to_string(),
            created_at: 1_717_171_700,
        })
        .unwrap();
    store
        .create_session(NewSessionRecord {
            id: "local-b".to_string(),
            engine: EngineId::Codex,
            model: "gpt-5-codex".to_string(),
            cwd: "D:\\work\\b".to_string(),
            created_at: 1_717_171_800,
        })
        .unwrap();

    store.set_active_session("local-a").unwrap();

    let active = store.active_session().unwrap().unwrap();
    assert_eq!(active.summary.id, "local-a");
    assert_eq!(active.summary.cwd, "D:\\work\\a");
}

#[test]
fn session_history_configures_sqlite_for_ui_event_concurrency() {
    let path = temp_history_path("sqlite-pragmas");
    let store = SessionHistoryStore::new(path.clone());
    store
        .create_session(NewSessionRecord {
            id: "local-1".to_string(),
            engine: EngineId::ClaudeCode,
            model: "claude-sonnet-4.6".to_string(),
            cwd: "D:\\work\\demo".to_string(),
            created_at: 1_717_171_700,
        })
        .unwrap();

    let conn = rusqlite::Connection::open(path).unwrap();
    let journal_mode: String = conn
        .query_row("PRAGMA journal_mode", [], |row| row.get(0))
        .unwrap();

    assert_eq!(journal_mode.to_lowercase(), "wal");
}

#[test]
fn session_history_records_new_sessions_and_user_messages() {
    let path = temp_history_path("new-session");
    let store = SessionHistoryStore::new(path);

    store
        .create_session(NewSessionRecord {
            id: "local-1".to_string(),
            engine: EngineId::ClaudeCode,
            model: "claude-sonnet-4.6".to_string(),
            cwd: "D:\\work\\demo".to_string(),
            created_at: 1_717_171_700,
        })
        .unwrap();
    store
        .record_user_message("local-1", "请列出当前目录", 1_717_171_701)
        .unwrap();

    let history = store.list_sessions().unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].id, "local-1");
    assert_eq!(history[0].title, "请列出当前目录");
    assert_eq!(history[0].engine, EngineId::ClaudeCode);
    assert_eq!(history[0].message_count, 1);
    assert_eq!(history[0].status, SessionStatus::Active);

    let detail = store.get_session("local-1").unwrap();
    assert_eq!(detail.messages.len(), 1);
    assert_eq!(detail.messages[0].role, Role::User);
}

#[test]
fn session_history_rejects_duplicate_local_session_id_without_replacing_existing_history() {
    let path = temp_history_path("duplicate-local-id");
    let store = SessionHistoryStore::new(path);

    store
        .create_session(NewSessionRecord {
            id: "local-1".to_string(),
            engine: EngineId::ClaudeCode,
            model: "claude-sonnet-4.6".to_string(),
            cwd: "D:\\work\\first".to_string(),
            created_at: 1_717_171_700,
        })
        .unwrap();
    store
        .record_user_message("local-1", "第一段会话", 1_717_171_701)
        .unwrap();

    let duplicate = store.create_session(NewSessionRecord {
        id: "local-1".to_string(),
        engine: EngineId::Codex,
        model: "gpt-5-codex".to_string(),
        cwd: "D:\\work\\second".to_string(),
        created_at: 1_717_171_800,
    });

    assert!(duplicate.is_err());
    let detail = store.get_session("local-1").unwrap();
    assert_eq!(detail.summary.engine, EngineId::ClaudeCode);
    assert_eq!(detail.summary.cwd, "D:\\work\\first");
    assert_eq!(detail.messages[0].text, "第一段会话");
}

#[test]
fn session_history_archives_agent_events_and_usage() {
    let path = temp_history_path("agent-events");
    let store = SessionHistoryStore::new(path);
    store
        .create_session(NewSessionRecord {
            id: "local-1".to_string(),
            engine: EngineId::Codex,
            model: "gpt-5-codex".to_string(),
            cwd: "D:\\work\\demo".to_string(),
            created_at: 1_717_171_700,
        })
        .unwrap();

    store
        .record_event(&AgentEvent::SessionStarted {
            session_id: "codex-real-1".to_string(),
            engine: EngineId::Codex,
            model: "gpt-5-codex".to_string(),
            cwd: "D:\\work\\demo".to_string(),
            ts: 1_717_171_702,
        })
        .unwrap();
    store
        .record_event(&AgentEvent::MessageComplete {
            session_id: "codex-real-1".to_string(),
            role: Role::Assistant,
            text: "目录包含 README.md".to_string(),
        })
        .unwrap();
    store
        .record_event(&AgentEvent::TokenUsage {
            session_id: "codex-real-1".to_string(),
            input_tokens: 100,
            output_tokens: 25,
            cost_usd: 0.03,
            context_window: None,
        })
        .unwrap();
    store
        .record_event(&AgentEvent::TurnComplete {
            session_id: "codex-real-1".to_string(),
            stop_reason: StopReason::End,
        })
        .unwrap();

    let history = store.list_sessions().unwrap();
    assert_eq!(history[0].id, "local-1");
    assert_eq!(history[0].cli_session_id.as_deref(), Some("codex-real-1"));
    assert_eq!(history[0].message_count, 1);
    assert_eq!(history[0].input_tokens, 100);
    assert_eq!(history[0].output_tokens, 25);
    assert_eq!(history[0].cost_usd, 0.03);
    assert_eq!(history[0].status, SessionStatus::Done);
}

#[test]
fn session_history_records_events_to_explicit_history_session_without_active_session_guessing() {
    let path = temp_history_path("explicit-event-owner");
    let store = SessionHistoryStore::new(path);
    store
        .create_session(NewSessionRecord {
            id: "local-a".to_string(),
            engine: EngineId::ClaudeCode,
            model: "claude-sonnet-4.6".to_string(),
            cwd: "D:\\work\\a".to_string(),
            created_at: 1_717_171_700,
        })
        .unwrap();
    store
        .create_session(NewSessionRecord {
            id: "local-b".to_string(),
            engine: EngineId::ClaudeCode,
            model: "claude-sonnet-4.6".to_string(),
            cwd: "D:\\work\\b".to_string(),
            created_at: 1_717_171_800,
        })
        .unwrap();

    store
        .record_event_for_session(
            "local-a",
            &AgentEvent::SessionStarted {
                session_id: "claude-a".to_string(),
                engine: EngineId::ClaudeCode,
                model: "claude-sonnet-4.6".to_string(),
                cwd: "D:\\work\\a".to_string(),
                ts: 1_717_171_801,
            },
        )
        .unwrap();
    store
        .record_event_for_session(
            "local-a",
            &AgentEvent::MessageComplete {
                session_id: "claude-a".to_string(),
                role: Role::Assistant,
                text: "A 的迟到回复".to_string(),
            },
        )
        .unwrap();

    let a = store.get_session("local-a").unwrap();
    let b = store.get_session("local-b").unwrap();
    assert_eq!(a.summary.cli_session_id.as_deref(), Some("claude-a"));
    assert_eq!(a.messages.len(), 1);
    assert_eq!(a.messages[0].text, "A 的迟到回复");
    assert_eq!(b.summary.cli_session_id, None);
    assert_eq!(b.messages.len(), 0);
}

#[test]
fn codex_native_thread_id_replaces_the_temporary_process_session_id() {
    let path = temp_history_path("codex-native-thread");
    let store = SessionHistoryStore::new(path);
    store
        .create_session(NewSessionRecord {
            id: "local-codex".to_string(),
            engine: EngineId::Codex,
            model: "gpt-5".to_string(),
            cwd: "D:\\work\\demo".to_string(),
            created_at: 1_717_171_700,
        })
        .unwrap();
    store
        .record_event_for_session(
            "local-codex",
            &AgentEvent::SessionStarted {
                session_id: "codex-process-1".to_string(),
                engine: EngineId::Codex,
                model: "gpt-5".to_string(),
                cwd: "D:\\work\\demo".to_string(),
                ts: 1_717_171_702,
            },
        )
        .unwrap();

    store
        .attach_native_thread_to_session("local-codex", "thread-native-1")
        .unwrap();

    let detail = store.get_session("local-codex").unwrap();
    assert_eq!(
        detail.summary.cli_session_id.as_deref(),
        Some("thread-native-1")
    );
}

#[test]
fn session_history_continues_restored_history_without_creating_a_new_row() {
    let path = temp_history_path("restore-append");
    let store = SessionHistoryStore::new(path);
    store
        .create_session(NewSessionRecord {
            id: "local-a".to_string(),
            engine: EngineId::ClaudeCode,
            model: "claude-sonnet-4.6".to_string(),
            cwd: "D:\\work\\a".to_string(),
            created_at: 1_717_171_700,
        })
        .unwrap();
    store
        .record_user_message("local-a", "第一轮", 1_717_171_701)
        .unwrap();

    store
        .record_user_message("local-a", "恢复后的追问", 1_717_171_900)
        .unwrap();
    store
        .record_event_for_session(
            "local-a",
            &AgentEvent::MessageComplete {
                session_id: "claude-a".to_string(),
                role: Role::Assistant,
                text: "恢复后的回答".to_string(),
            },
        )
        .unwrap();

    let history = store.list_sessions().unwrap();
    let detail = store.get_session("local-a").unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(detail.messages.len(), 3);
    assert_eq!(detail.messages[1].text, "恢复后的追问");
    assert_eq!(detail.messages[2].text, "恢复后的回答");
}

#[test]
fn session_history_uses_model_price_as_cost_fallback_when_cli_cost_is_zero() {
    let path = temp_history_path("usage-fallback-cost");
    let store = SessionHistoryStore::new(path);
    store.set_model_price("claude-sonnet-4.6", 3.0, 15.0);
    store
        .create_session(NewSessionRecord {
            id: "local-1".to_string(),
            engine: EngineId::ClaudeCode,
            model: "claude-sonnet-4.6".to_string(),
            cwd: "D:\\work\\demo".to_string(),
            created_at: 1_717_171_700,
        })
        .unwrap();
    store
        .record_event_for_session(
            "local-1",
            &AgentEvent::TokenUsage {
                session_id: "claude-real-1".to_string(),
                input_tokens: 1_000_000,
                output_tokens: 1_000_000,
                cost_usd: 0.0,
                context_window: None,
            },
        )
        .unwrap();

    let history = store.list_sessions().unwrap();
    assert_eq!(history[0].cost_usd, 18.0);
}

#[test]
fn session_history_keeps_non_zero_cli_cost_over_price_fallback() {
    let path = temp_history_path("usage-cli-cost");
    let store = SessionHistoryStore::new(path);
    store.set_model_price("claude-sonnet-4.6", 3.0, 15.0);
    store
        .create_session(NewSessionRecord {
            id: "local-1".to_string(),
            engine: EngineId::ClaudeCode,
            model: "claude-sonnet-4.6".to_string(),
            cwd: "D:\\work\\demo".to_string(),
            created_at: 1_717_171_700,
        })
        .unwrap();
    store
        .record_event_for_session(
            "local-1",
            &AgentEvent::TokenUsage {
                session_id: "claude-real-1".to_string(),
                input_tokens: 1_000_000,
                output_tokens: 1_000_000,
                cost_usd: 0.42,
                context_window: None,
            },
        )
        .unwrap();

    let history = store.list_sessions().unwrap();
    assert_eq!(history[0].cost_usd, 0.42);
}

#[test]
fn session_history_aggregates_messages_and_usage_without_join_multiplication() {
    let path = temp_history_path("aggregate-usage");
    let store = SessionHistoryStore::new(path);
    store
        .create_session(NewSessionRecord {
            id: "local-1".to_string(),
            engine: EngineId::Codex,
            model: "gpt-5-codex".to_string(),
            cwd: "D:\\work\\demo".to_string(),
            created_at: 1_717_171_700,
        })
        .unwrap();
    store
        .record_event(&AgentEvent::SessionStarted {
            session_id: "codex-real-1".to_string(),
            engine: EngineId::Codex,
            model: "gpt-5-codex".to_string(),
            cwd: "D:\\work\\demo".to_string(),
            ts: 1_717_171_702,
        })
        .unwrap();

    for text in ["第一条回复", "第二条回复"] {
        store
            .record_event(&AgentEvent::MessageComplete {
                session_id: "codex-real-1".to_string(),
                role: Role::Assistant,
                text: text.to_string(),
            })
            .unwrap();
    }
    for (input_tokens, output_tokens, cost_usd) in [(100, 25, 0.03), (50, 10, 0.02)] {
        store
            .record_event(&AgentEvent::TokenUsage {
                session_id: "codex-real-1".to_string(),
                input_tokens,
                output_tokens,
                cost_usd,
                context_window: None,
            })
            .unwrap();
    }

    let history = store.list_sessions().unwrap();
    assert_eq!(history[0].message_count, 2);
    assert_eq!(history[0].input_tokens, 150);
    assert_eq!(history[0].output_tokens, 35);
    assert_eq!(history[0].cost_usd, 0.05);
}

#[test]
fn session_history_persists_tool_result_diff() {
    let path = temp_history_path("tool-diff");
    let store = SessionHistoryStore::new(path);
    store
        .create_session(NewSessionRecord {
            id: "local-1".to_string(),
            engine: EngineId::ClaudeCode,
            model: "claude-sonnet-4.6".to_string(),
            cwd: "D:\\work\\demo".to_string(),
            created_at: 1_717_171_700,
        })
        .unwrap();
    store
        .record_event(&AgentEvent::SessionStarted {
            session_id: "claude-real-1".to_string(),
            engine: EngineId::ClaudeCode,
            model: "claude-sonnet-4.6".to_string(),
            cwd: "D:\\work\\demo".to_string(),
            ts: 1_717_171_702,
        })
        .unwrap();
    store
        .record_event(&AgentEvent::ToolCall {
            session_id: "claude-real-1".to_string(),
            id: "tool-1".to_string(),
            name: "Edit".to_string(),
            input: serde_json::json!({ "file_path": "demo.ts" }),
            status: helm_lib::protocol::CallStatus::Pending,
        })
        .unwrap();
    store
        .record_event(&AgentEvent::ToolResult {
            session_id: "claude-real-1".to_string(),
            id: "tool-1".to_string(),
            status: helm_lib::protocol::ToolStatus::Success,
            output: Some("Updated".to_string()),
            diff: Some(Diff {
                path: "demo.ts".to_string(),
                hunks: vec![DiffHunk {
                    old_start: 1,
                    new_start: 1,
                    lines: vec![
                        DiffLine {
                            kind: DiffKind::Del,
                            text: "old".to_string(),
                        },
                        DiffLine {
                            kind: DiffKind::Add,
                            text: "new".to_string(),
                        },
                    ],
                }],
            }),
        })
        .unwrap();

    let detail = store.get_session("local-1").unwrap();
    let diff = detail.tool_calls[0].diff.as_ref().expect("应恢复 diff");
    assert_eq!(diff.path, "demo.ts");
    assert_eq!(diff.hunks[0].lines[0].kind, DiffKind::Del);
    assert_eq!(diff.hunks[0].lines[1].kind, DiffKind::Add);
}

#[test]
fn session_history_returns_checkpoints_for_restore_timeline() {
    let path = temp_history_path("checkpoints");
    let store = SessionHistoryStore::new(path);
    store
        .create_session(NewSessionRecord {
            id: "local-1".to_string(),
            engine: EngineId::ClaudeCode,
            model: "claude-sonnet-4.6".to_string(),
            cwd: "D:\\work\\demo".to_string(),
            created_at: 1_717_171_700,
        })
        .unwrap();
    store
        .record_event(&AgentEvent::SessionStarted {
            session_id: "claude-real-1".to_string(),
            engine: EngineId::ClaudeCode,
            model: "claude-sonnet-4.6".to_string(),
            cwd: "D:\\work\\demo".to_string(),
            ts: 1_717_171_702,
        })
        .unwrap();
    store
        .record_event(&AgentEvent::Checkpoint {
            session_id: "claude-real-1".to_string(),
            id: "ckpt-1".to_string(),
            label: "改动前：demo.ts".to_string(),
            ts: 1_717_171_703_000,
        })
        .unwrap();

    let detail = store.get_session("local-1").unwrap();
    assert_eq!(detail.checkpoints.len(), 1);
    assert_eq!(detail.checkpoints[0].id, "ckpt-1");
    assert_eq!(detail.checkpoints[0].label, "改动前：demo.ts");
}

#[test]
fn session_history_persists_across_store_instances() {
    let path = temp_history_path("roundtrip");
    let store = SessionHistoryStore::new(path.clone());
    store
        .create_session(NewSessionRecord {
            id: "local-1".to_string(),
            engine: EngineId::ClaudeCode,
            model: "claude-sonnet-4.6".to_string(),
            cwd: "D:\\work\\demo".to_string(),
            created_at: 1_717_171_700,
        })
        .unwrap();

    let reloaded = SessionHistoryStore::new(path);
    assert_eq!(reloaded.list_sessions().unwrap()[0].id, "local-1");
}

#[test]
fn session_history_waits_for_temporary_sqlite_write_lock() {
    let path = temp_history_path("busy-timeout");
    let store = SessionHistoryStore::new(path.clone());
    store
        .create_session(NewSessionRecord {
            id: "local-1".to_string(),
            engine: EngineId::ClaudeCode,
            model: "claude-sonnet-4.6".to_string(),
            cwd: "D:\\work\\demo".to_string(),
            created_at: 1_717_171_700,
        })
        .unwrap();

    let blocker = rusqlite::Connection::open(path).unwrap();
    blocker.execute_batch("BEGIN EXCLUSIVE").unwrap();

    let writer = store.clone();
    let handle = std::thread::spawn(move || {
        writer.record_user_message("local-1", "锁释放后应写入", 1_717_171_701)
    });

    std::thread::sleep(Duration::from_millis(200));
    blocker.execute_batch("COMMIT").unwrap();

    handle.join().unwrap().unwrap();
    let detail = store.get_session("local-1").unwrap();
    assert_eq!(detail.messages[0].text, "锁释放后应写入");
}

#[test]
fn session_history_handles_concurrent_internal_writes() {
    let path = temp_history_path("concurrent-writes");
    let store = SessionHistoryStore::new(path);
    store
        .create_session(NewSessionRecord {
            id: "local-1".to_string(),
            engine: EngineId::ClaudeCode,
            model: "claude-sonnet-4.6".to_string(),
            cwd: "D:\\work\\demo".to_string(),
            created_at: 1_717_171_700,
        })
        .unwrap();

    let mut handles = Vec::new();
    for idx in 0..32 {
        let writer = store.clone();
        handles.push(std::thread::spawn(move || {
            writer.record_event(&AgentEvent::MessageComplete {
                session_id: "local-1".to_string(),
                role: Role::Assistant,
                text: format!("并发消息 {idx}"),
            })
        }));
    }

    for handle in handles {
        handle.join().unwrap().unwrap();
    }
    let detail = store.get_session("local-1").unwrap();
    assert_eq!(detail.messages.len(), 32);
}

#[test]
fn checkpoint_revert_truncates_agent_context_semantics() {
    // P2-5 回溯语义：检查点之后的消息打 reverted 标记、CLI 会话 id 作废，
    // 重建上下文时（resume/live reset）据此剔除被回滚的轮次。
    let path = temp_history_path("revert-context");
    let store = SessionHistoryStore::new(path.clone());
    store
        .create_session(NewSessionRecord {
            id: "local-1".to_string(),
            engine: EngineId::ClaudeCode,
            model: "claude-sonnet-4.6".to_string(),
            cwd: r"D:\work\demo".to_string(),
            created_at: 1_717_171_700,
        })
        .unwrap();

    store
        .record_user_message("local-1", "第一轮提问", 1_717_171_701)
        .unwrap();
    store
        .record_event_for_session(
            "local-1",
            &AgentEvent::MessageComplete {
                session_id: "cli-1".to_string(),
                role: Role::Assistant,
                text: "第一轮回复".to_string(),
            },
        )
        .unwrap();
    store
        .record_user_message("local-1", "第二轮提问", 1_717_171_704)
        .unwrap();
    store
        .record_event_for_session(
            "local-1",
            &AgentEvent::MessageComplete {
                session_id: "cli-1".to_string(),
                role: Role::Assistant,
                text: "第二轮回复（将被回滚）".to_string(),
            },
        )
        .unwrap();
    // MessageComplete 落库用的是真实时钟；为了让「检查点之后」的边界确定，
    // 这里直接把四条消息的 ts 依次固定为 1..4，检查点打在 ts=2 之后。
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch("UPDATE message SET ts = id;").unwrap();
    }
    store
        .save_checkpoint("ckpt-1", "local-1", 0, "写文件前", "snap-1", 2)
        .unwrap();

    store.revert_messages_after("local-1", 2).unwrap();
    store.clear_cli_session("local-1").unwrap();

    let detail = store.get_session("local-1").unwrap();
    assert_eq!(
        detail.summary.cli_session_id, None,
        "回溯后必须作废 CLI 会话 id"
    );
    let kept: Vec<&str> = detail
        .messages
        .iter()
        .filter(|message| !message.reverted)
        .map(|message| message.text.as_str())
        .collect();
    assert_eq!(kept, vec!["第一轮提问", "第一轮回复"]);
    assert!(
        detail.messages.iter().any(|message| message.reverted),
        "检查点之后的消息必须带 reverted 标记"
    );

    // 撤销回溯：标记清空，完整历史重新可用
    store.unrevert_messages("local-1").unwrap();
    let detail = store.get_session("local-1").unwrap();
    assert!(detail.messages.iter().all(|message| !message.reverted));
}

#[test]
fn checkpoint_revert_works_with_real_millisecond_timestamps() {
    // 变更-07 回归：检查点 ts 与 message.ts 必须同为毫秒。
    // 修复前 message.ts 是秒（~1.7e9）、检查点是毫秒（~1.7e12），
    // 「ts > 检查点」永远不成立 → 回溯的消息截断完全失效。
    let path = temp_history_path("revert-ms-units");
    let store = SessionHistoryStore::new(path.clone());
    store
        .create_session(NewSessionRecord {
            id: "local-1".to_string(),
            engine: EngineId::ClaudeCode,
            model: "claude-sonnet-4.6".to_string(),
            cwd: r"D:\work\demo".to_string(),
            created_at: 1_717_171_700,
        })
        .unwrap();

    let before_millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;

    // 走真实写入路径（内部时钟），不手工改 ts
    store
        .record_user_message("local-1", "改动前的提问", before_millis)
        .unwrap();
    store
        .record_event_for_session(
            "local-1",
            &AgentEvent::MessageComplete {
                session_id: "cli-1".to_string(),
                role: Role::Assistant,
                text: "检查点之后的回复（应被回滚）".to_string(),
            },
        )
        .unwrap();

    // 落库的 message.ts 必须是毫秒量级
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        let min_ts: i64 = conn
            .query_row("SELECT MIN(ts) FROM message", [], |row| row.get(0))
            .unwrap();
        assert!(
            min_ts > 100_000_000_000,
            "message.ts 应为毫秒（实际 {min_ts}）"
        );
    }

    // 检查点打在用户消息之后、助手回复之前（真实自动检查点的时序）
    store
        .save_checkpoint("ckpt-1", "local-1", 0, "写文件前", "snap-1", before_millis)
        .unwrap();
    store
        .revert_messages_after("local-1", before_millis)
        .unwrap();

    let detail = store.get_session("local-1").unwrap();
    let reverted: Vec<&str> = detail
        .messages
        .iter()
        .filter(|message| message.reverted)
        .map(|message| message.text.as_str())
        .collect();
    assert_eq!(
        reverted,
        vec!["检查点之后的回复（应被回滚）"],
        "检查点之后的助手回复必须被标记 reverted（毫秒单位比较生效）"
    );
}

#[test]
fn schema_v4_migrates_second_timestamps_to_milliseconds() {
    // 变更-07：老库（v3，秒级 message/tool_call ts）升级后统一为毫秒，且 turn 死表被清理。
    let path = temp_history_path("v4-ts-migration");
    {
        let store = SessionHistoryStore::new(path.clone());
        store
            .create_session(NewSessionRecord {
                id: "local-1".to_string(),
                engine: EngineId::ClaudeCode,
                model: "claude-sonnet-4.6".to_string(),
                cwd: r"D:\work\demo".to_string(),
                created_at: 1_717_171_700,
            })
            .unwrap();
        store
            .record_user_message("local-1", "老会话消息", 1_717_171_701_000)
            .unwrap();
    }
    {
        // 手工降级成 v3 形态：秒级 ts + user_version=3
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(
            "UPDATE message SET ts = 1717171701;
             PRAGMA user_version = 3;",
        )
        .unwrap();
    }

    // 新的 store 实例首次 open 会跑迁移
    let store = SessionHistoryStore::new(path.clone());
    let detail = store.get_session("local-1").unwrap();
    assert_eq!(detail.messages.len(), 1);

    let conn = rusqlite::Connection::open(&path).unwrap();
    let ts: i64 = conn
        .query_row("SELECT ts FROM message LIMIT 1", [], |row| row.get(0))
        .unwrap();
    assert_eq!(ts, 1_717_171_701_000, "秒级 ts 应被迁移为毫秒");
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert!(version >= 4);
    let turn_table: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='turn'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(turn_table, 0, "turn 死表应在 v4 迁移中删除");

    // 迁移幂等：已是毫秒的值不会被再乘 1000
    let store2 = SessionHistoryStore::new(path.clone());
    drop(store2.get_session("local-1").unwrap());
    let ts_again: i64 = rusqlite::Connection::open(&path)
        .unwrap()
        .query_row("SELECT ts FROM message LIMIT 1", [], |row| row.get(0))
        .unwrap();
    assert_eq!(ts_again, 1_717_171_701_000);
}

#[test]
fn approval_requests_persist_and_expire() {
    // 变更-07：审批请求落库（pending）→ 处理后 resolved；
    // 用户发新消息则 pending 全部作废为 expired（悬空审批不可再响应）。
    let path = temp_history_path("approval-persist");
    let store = SessionHistoryStore::new(path);
    store
        .create_session(NewSessionRecord {
            id: "local-1".to_string(),
            engine: EngineId::ClaudeCode,
            model: "claude-sonnet-4.6".to_string(),
            cwd: r"D:\work\demo".to_string(),
            created_at: 1_717_171_700,
        })
        .unwrap();

    store
        .record_event_for_session(
            "local-1",
            &AgentEvent::ApprovalRequest {
                session_id: "cli-1".to_string(),
                id: "appr-1".to_string(),
                action: "Bash".to_string(),
                detail: "pnpm test".to_string(),
                input: None,
            },
        )
        .unwrap();
    store
        .record_event_for_session(
            "local-1",
            &AgentEvent::ApprovalRequest {
                session_id: "cli-1".to_string(),
                id: "appr-2".to_string(),
                action: "Write".to_string(),
                detail: "x.txt".to_string(),
                input: None,
            },
        )
        .unwrap();

    let detail = store.get_session("local-1").unwrap();
    assert_eq!(detail.approvals.len(), 2);
    assert!(detail.approvals.iter().all(|a| a.status == "pending"));

    // 用户处理了第一个
    store.resolve_approval("local-1", "appr-1").unwrap();
    let detail = store.get_session("local-1").unwrap();
    assert_eq!(
        detail
            .approvals
            .iter()
            .find(|a| a.id == "appr-1")
            .unwrap()
            .status,
        "resolved"
    );

    // 运行时恢复失败时可补偿回 pending，允许用户重试，不能留下“已处理但未执行”。
    store.reopen_approval("local-1", "appr-1").unwrap();
    let detail = store.get_session("local-1").unwrap();
    assert_eq!(
        detail
            .approvals
            .iter()
            .find(|a| a.id == "appr-1")
            .unwrap()
            .status,
        "pending"
    );
    store.resolve_approval("local-1", "appr-1").unwrap();

    // 用户发新消息：剩余 pending 全部作废
    store.expire_pending_approvals("local-1").unwrap();
    let detail = store.get_session("local-1").unwrap();
    assert_eq!(
        detail
            .approvals
            .iter()
            .find(|a| a.id == "appr-2")
            .unwrap()
            .status,
        "expired"
    );
    assert_eq!(
        detail
            .approvals
            .iter()
            .find(|a| a.id == "appr-1")
            .unwrap()
            .status,
        "resolved",
        "已处理的审批不受作废影响"
    );
}

#[test]
fn prepared_user_turn_rolls_back_all_history_side_effects_when_launch_is_rejected() {
    let path = temp_history_path("prepared-turn-rollback");
    let store = SessionHistoryStore::new(path);
    for id in ["local-1", "local-2"] {
        store
            .create_session(NewSessionRecord {
                id: id.to_string(),
                engine: EngineId::Codex,
                model: "gpt-5-codex".to_string(),
                cwd: r"D:\work\demo".to_string(),
                created_at: 1_717_171_700,
            })
            .unwrap();
    }
    store.set_active_session("local-2").unwrap();
    store
        .record_event_for_session(
            "local-1",
            &AgentEvent::ApprovalRequest {
                session_id: "cli-1".to_string(),
                id: "appr-pending".to_string(),
                action: "Write".to_string(),
                detail: "a.txt".to_string(),
                input: None,
            },
        )
        .unwrap();

    let prepared = store
        .prepare_user_turn("local-1", "不会真正发送", 1_717_171_701_000)
        .unwrap();
    assert_eq!(
        store.active_session().unwrap().unwrap().summary.id,
        "local-1"
    );
    assert_eq!(store.get_session("local-1").unwrap().messages.len(), 1);
    assert_eq!(
        store.get_session("local-1").unwrap().approvals[0].status,
        "expired"
    );

    store.rollback_prepared_user_turn(prepared).unwrap();

    let detail = store.get_session("local-1").unwrap();
    assert!(detail.messages.is_empty());
    assert_eq!(detail.summary.title, "未命名会话");
    assert_eq!(detail.approvals[0].status, "pending");
    assert_eq!(
        store.active_session().unwrap().unwrap().summary.id,
        "local-2"
    );
}

#[test]
fn usage_by_provider_attributes_costs_by_real_provider_id() {
    // P3-6：用量按 session.provider_id 真实归属，不再按模型名推断；
    // 未标注的旧会话归入空 key。
    let path = temp_history_path("usage-by-provider");
    let store = SessionHistoryStore::new(path);
    for (id, provider) in [
        ("s-a", Some("gateway-x")),
        ("s-b", Some("anthropic")),
        ("s-old", None),
    ] {
        store
            .create_session(NewSessionRecord {
                id: id.to_string(),
                engine: EngineId::ClaudeCode,
                model: "claude-sonnet-4.6".to_string(),
                cwd: r"D:\work\demo".to_string(),
                created_at: 1_717_171_700,
            })
            .unwrap();
        if let Some(provider) = provider {
            store.set_session_provider(id, provider).unwrap();
        }
    }
    // 同一个模型名，分属不同服务商（中转场景按模型名猜必错）
    for (session, cli, cost) in [
        ("s-a", "cli-a", 3.0_f64),
        ("s-b", "cli-b", 1.0),
        ("s-old", "cli-old", 1.0),
    ] {
        store
            .record_event_for_session(
                session,
                &AgentEvent::SessionStarted {
                    session_id: cli.to_string(),
                    engine: EngineId::ClaudeCode,
                    model: "claude-sonnet-4.6".to_string(),
                    cwd: r"D:\work\demo".to_string(),
                    ts: 1_717_171_701,
                },
            )
            .unwrap();
        store
            .record_event_for_session(
                session,
                &AgentEvent::TokenUsage {
                    session_id: cli.to_string(),
                    input_tokens: 100,
                    output_tokens: 10,
                    cost_usd: cost,
                    context_window: None,
                },
            )
            .unwrap();
    }

    let rows = store.get_usage_by_provider(30).unwrap();
    assert_eq!(rows.len(), 3, "两个真实服务商 + 一个未标注：{rows:?}");
    assert_eq!(rows[0].provider, "gateway-x");
    assert!((rows[0].cost_usd - 3.0).abs() < 1e-9);
    assert!((rows[0].share - 0.6).abs() < 1e-9);
    assert!(
        rows.iter().any(|row| row.provider.is_empty()),
        "未标注会话归入空 key"
    );
}

#[test]
fn auto_title_guard_and_summary_round_trip() {
    // P3-5：summary 为 NULL 且有助手回复才需要起标题；写入后守卫翻转、摘要进列表
    let path = temp_history_path("auto-title");
    let store = SessionHistoryStore::new(path);
    store
        .create_session(NewSessionRecord {
            id: "local-1".to_string(),
            engine: EngineId::ClaudeCode,
            model: "claude-sonnet-4.6".to_string(),
            cwd: r"D:\work\demo".to_string(),
            created_at: 1_717_171_700,
        })
        .unwrap();

    // 只有用户消息：还没有完整一轮，不起标题
    store
        .record_user_message("local-1", "帮我修登录超时", 1_717_171_701)
        .unwrap();
    assert!(!store.session_needs_auto_title("local-1").unwrap());

    store
        .record_event_for_session(
            "local-1",
            &AgentEvent::MessageComplete {
                session_id: "cli-1".to_string(),
                role: Role::Assistant,
                text: "已定位到超时原因并修复".to_string(),
            },
        )
        .unwrap();
    assert!(store.session_needs_auto_title("local-1").unwrap());

    store
        .set_session_title_and_summary("local-1", "修复登录超时", "排查并修复登录接口 30s 超时")
        .unwrap();
    // 写入后不再重复起标题
    assert!(!store.session_needs_auto_title("local-1").unwrap());

    let sessions = store.list_sessions().unwrap();
    assert_eq!(sessions[0].title, "修复登录超时");
    assert_eq!(
        sessions[0].summary.as_deref(),
        Some("排查并修复登录接口 30s 超时"),
        "摘要必须进入会话列表（供搜索）"
    );
}
