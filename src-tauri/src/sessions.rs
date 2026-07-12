use crate::protocol::{AgentEvent, CallStatus, Diff, EngineId, Role, StopReason, ToolStatus};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// setting 表里「始终允许」工具清单的 key（P2-4 跨会话持久化）
const ALWAYS_ALLOW_TOOLS_KEY: &str = "approval_always_allow";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SessionStatus {
    Active,
    Idle,
    Done,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SessionSummary {
    pub id: String,
    pub cli_session_id: Option<String>,
    pub title: String,
    pub engine: EngineId,
    pub model: String,
    pub cwd: String,
    pub status: SessionStatus,
    pub message_count: u32,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
    pub created_at: i64,
    pub updated_at: i64,
    /// fast model 生成的一句话摘要（P3-5）；None = 尚未生成
    #[serde(default)]
    pub summary: Option<String>,
    /// 置顶（变更-12）：侧栏排在最前
    #[serde(default)]
    pub pinned: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SessionMessage {
    pub role: Role,
    pub text: String,
    pub ts: i64,
    /// 是否已被检查点回溯（P2-5）：重建上下文与续聊序列化时会剔除
    #[serde(default)]
    pub reverted: bool,
}

#[derive(Debug)]
pub struct PreparedUserTurn {
    session_id: String,
    message_id: i64,
    previous_title: String,
    previous_status: String,
    previous_updated_at: i64,
    previous_active_session_id: Option<String>,
    expired_approval_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SessionToolCall {
    pub id: String,
    pub name: String,
    pub status: HistoryToolStatus,
    pub input: serde_json::Value,
    pub output: Option<String>,
    pub diff: Option<Diff>,
    /// 毫秒时间戳（变更-10）：历史恢复按时间线穿插排序用
    #[serde(default)]
    pub ts: i64,
}

/// 审批请求的持久化记录（变更-07）：切走/重启后审批卡可重建，不再永久悬置。
/// status：pending（待处理）/ resolved（已处理）/ expired（用户发新消息后作废）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SessionApproval {
    pub id: String,
    pub action: String,
    pub detail: String,
    pub status: String,
    pub ts: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SessionCheckpoint {
    pub id: String,
    pub label: String,
    pub ts: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum HistoryToolStatus {
    Pending,
    Success,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SessionDetail {
    #[serde(flatten)]
    pub summary: SessionSummary,
    pub messages: Vec<SessionMessage>,
    pub tool_calls: Vec<SessionToolCall>,
    pub checkpoints: Vec<SessionCheckpoint>,
    /// 审批请求（变更-07）：含 pending 的悬空审批，前端恢复时重建审批卡
    #[serde(default)]
    pub approvals: Vec<SessionApproval>,
}

#[derive(Debug, Clone)]
pub struct NewSessionRecord {
    pub id: String,
    pub engine: EngineId,
    pub model: String,
    pub cwd: String,
    pub created_at: i64,
}

#[derive(Debug, Clone)]
struct ModelPrice {
    input_price_per_mtok: f64,
    output_price_per_mtok: f64,
}

#[derive(Clone)]
pub struct SessionHistoryStore {
    path: PathBuf,
    write_lock: Arc<Mutex<()>>,
    initialized: Arc<Mutex<bool>>,
    model_prices: Arc<Mutex<HashMap<String, ModelPrice>>>,
}

impl SessionHistoryStore {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            write_lock: Arc::new(Mutex::new(())),
            initialized: Arc::new(Mutex::new(false)),
            model_prices: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn set_model_price(
        &self,
        model: &str,
        input_price_per_mtok: f64,
        output_price_per_mtok: f64,
    ) {
        if let Ok(mut prices) = self.model_prices.lock() {
            prices.insert(
                model.to_string(),
                ModelPrice {
                    input_price_per_mtok,
                    output_price_per_mtok,
                },
            );
        }
    }

    pub fn create_session(&self, record: NewSessionRecord) -> Result<SessionDetail, String> {
        let _guard = self.write_guard()?;
        let conn = self.open()?;
        conn.execute(
            "INSERT INTO session
             (id, cli_session_id, title, engine, model, cwd, status, created_at, updated_at)
             VALUES (?1, NULL, '未命名会话', ?2, ?3, ?4, 'active', ?5, ?5)",
            params![
                record.id,
                engine_to_str(record.engine),
                record.model,
                record.cwd,
                record.created_at
            ],
        )
        .map_err(db_err)?;
        self.set_setting_on_conn(&conn, "active_session_id", &record.id)?;
        self.get_session(&record.id)
    }

    pub fn list_sessions(&self) -> Result<Vec<SessionSummary>, String> {
        let conn = self.open()?;
        let mut stmt = conn
            .prepare(
                "SELECT
                   s.id, s.cli_session_id, s.title, s.engine, s.model, s.cwd, s.status,
                   COALESCE(m.message_count, 0) AS message_count,
                   COALESCE(u.input_tokens, 0) AS input_tokens,
                   COALESCE(u.output_tokens, 0) AS output_tokens,
                   COALESCE(u.cost_usd, 0.0) AS cost_usd,
                   s.created_at, s.updated_at, s.summary, s.pinned
                 FROM session s
                 LEFT JOIN (
                   SELECT session_id, COUNT(*) AS message_count
                   FROM message
                   GROUP BY session_id
                 ) m ON m.session_id = s.id
                 LEFT JOIN (
                   SELECT
                     session_id,
                     SUM(input_tokens) AS input_tokens,
                     SUM(output_tokens) AS output_tokens,
                     SUM(cost_usd) AS cost_usd
                   FROM usage
                   GROUP BY session_id
                 ) u ON u.session_id = s.id
                 ORDER BY s.pinned DESC, s.updated_at DESC",
            )
            .map_err(db_err)?;
        let rows = stmt
            .query_map([], summary_from_row)
            .map_err(db_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_err)?;
        Ok(rows)
    }

    pub fn get_session(&self, id: &str) -> Result<SessionDetail, String> {
        let conn = self.open()?;
        let local_id = self.resolve_local_id(&conn, id)?;
        let summary = conn
            .query_row(
                "SELECT
                   s.id, s.cli_session_id, s.title, s.engine, s.model, s.cwd, s.status,
                   COALESCE((SELECT COUNT(*) FROM message WHERE session_id = s.id), 0),
                   COALESCE((SELECT SUM(input_tokens) FROM usage WHERE session_id = s.id), 0),
                   COALESCE((SELECT SUM(output_tokens) FROM usage WHERE session_id = s.id), 0),
                   COALESCE((SELECT SUM(cost_usd) FROM usage WHERE session_id = s.id), 0.0),
                   s.created_at, s.updated_at, s.summary, s.pinned
                 FROM session s
                 WHERE s.id = ?1",
                params![local_id],
                summary_from_row,
            )
            .map_err(db_err)?;
        Ok(SessionDetail {
            messages: self.messages_for_conn(&conn, &summary.id)?,
            tool_calls: self.tools_for_conn(&conn, &summary.id)?,
            checkpoints: self.checkpoints_for_conn(&conn, &summary.id)?,
            approvals: self.approvals_for_conn(&conn, &summary.id)?,
            summary,
        })
    }

    /// 记录用户消息。`ts_millis` 为毫秒时间戳（变更-07：message.ts 与 checkpoint.ts 同单位）。
    pub fn record_user_message(
        &self,
        session_id: &str,
        text: &str,
        ts_millis: i64,
    ) -> Result<(), String> {
        retry_locked(|| {
            let _guard = self.write_guard()?;
            let mut conn = self.open()?;
            let local_id = self.resolve_local_id(&conn, session_id)?;
            let tx = conn.transaction().map_err(db_err)?;
            let current_title: String = tx
                .query_row(
                    "SELECT title FROM session WHERE id = ?1",
                    params![local_id],
                    |row| row.get(0),
                )
                .map_err(db_err)?;
            if current_title == "未命名会话" {
                tx.execute(
                    "UPDATE session SET title = ?1 WHERE id = ?2",
                    params![title_from_text(text), local_id],
                )
                .map_err(db_err)?;
            }
            tx.execute(
                "INSERT INTO message (session_id, role, text, ts) VALUES (?1, 'user', ?2, ?3)",
                params![local_id, text, ts_millis],
            )
            .map_err(db_err)?;
            tx.execute(
                "UPDATE session SET status = 'active', updated_at = ?1 WHERE id = ?2",
                params![ts_millis / 1000, local_id],
            )
            .map_err(db_err)?;
            tx.commit().map_err(db_err)?;
            Ok(())
        })
    }

    /// 在启动 CLI 前原子准备本轮历史副作用；若运行时拒绝启动，可用返回值完整回滚。
    pub fn prepare_user_turn(
        &self,
        session_id: &str,
        text: &str,
        ts_millis: i64,
    ) -> Result<PreparedUserTurn, String> {
        retry_locked(|| {
            let _guard = self.write_guard()?;
            let mut conn = self.open()?;
            let local_id = self.resolve_local_id(&conn, session_id)?;
            let tx = conn.transaction().map_err(db_err)?;
            let (previous_title, previous_status, previous_updated_at) = tx
                .query_row(
                    "SELECT title, status, updated_at FROM session WHERE id = ?1",
                    params![local_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .map_err(db_err)?;
            let previous_active_session_id = self.get_setting_on_conn(&tx, "active_session_id")?;
            let expired_approval_ids = {
                let mut stmt = tx
                    .prepare("SELECT id FROM approval WHERE session_id = ?1 AND status = 'pending'")
                    .map_err(db_err)?;
                let ids = stmt
                    .query_map(params![local_id], |row| row.get::<_, String>(0))
                    .map_err(db_err)?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(db_err)?;
                ids
            };

            if previous_title == "未命名会话" {
                tx.execute(
                    "UPDATE session SET title = ?1 WHERE id = ?2",
                    params![title_from_text(text), local_id],
                )
                .map_err(db_err)?;
            }
            tx.execute(
                "INSERT INTO message (session_id, role, text, ts) VALUES (?1, 'user', ?2, ?3)",
                params![local_id, text, ts_millis],
            )
            .map_err(db_err)?;
            let message_id = tx.last_insert_rowid();
            tx.execute(
                "UPDATE session SET status = 'active', updated_at = ?1 WHERE id = ?2",
                params![ts_millis / 1000, local_id],
            )
            .map_err(db_err)?;
            tx.execute(
                "UPDATE approval SET status = 'expired' WHERE session_id = ?1 AND status = 'pending'",
                params![local_id],
            )
            .map_err(db_err)?;
            tx.execute(
                "INSERT INTO setting (key, value_json) VALUES ('active_session_id', ?1)
                 ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json",
                params![serde_json::to_string(&local_id).map_err(|e| e.to_string())?],
            )
            .map_err(db_err)?;
            tx.commit().map_err(db_err)?;

            Ok(PreparedUserTurn {
                session_id: local_id,
                message_id,
                previous_title,
                previous_status,
                previous_updated_at,
                previous_active_session_id,
                expired_approval_ids,
            })
        })
    }

    pub fn rollback_prepared_user_turn(&self, prepared: PreparedUserTurn) -> Result<(), String> {
        retry_locked(|| {
            let _guard = self.write_guard()?;
            let mut conn = self.open()?;
            let tx = conn.transaction().map_err(db_err)?;
            let deleted = tx
                .execute(
                    "DELETE FROM message WHERE id = ?1 AND session_id = ?2",
                    params![prepared.message_id, prepared.session_id],
                )
                .map_err(db_err)?;
            if deleted != 1 {
                return Err("无法回滚未启动轮次：用户消息记录不存在".to_string());
            }
            tx.execute(
                "UPDATE session SET title = ?1, status = ?2, updated_at = ?3 WHERE id = ?4",
                params![
                    prepared.previous_title,
                    prepared.previous_status,
                    prepared.previous_updated_at,
                    prepared.session_id
                ],
            )
            .map_err(db_err)?;
            for approval_id in &prepared.expired_approval_ids {
                tx.execute(
                    "UPDATE approval SET status = 'pending' WHERE session_id = ?1 AND id = ?2 AND status = 'expired'",
                    params![prepared.session_id, approval_id],
                )
                .map_err(db_err)?;
            }
            if let Some(active_id) = &prepared.previous_active_session_id {
                tx.execute(
                    "INSERT INTO setting (key, value_json) VALUES ('active_session_id', ?1)
                     ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json",
                    params![serde_json::to_string(active_id).map_err(|e| e.to_string())?],
                )
                .map_err(db_err)?;
            } else {
                tx.execute("DELETE FROM setting WHERE key = 'active_session_id'", [])
                    .map_err(db_err)?;
            }
            tx.commit().map_err(db_err)?;
            Ok(())
        })
    }

    pub fn record_event(&self, event: &AgentEvent) -> Result<(), String> {
        match event {
            AgentEvent::SessionStarted {
                session_id,
                engine,
                model,
                cwd,
                ts,
            } => self.attach_cli_session(session_id, *engine, model, cwd, *ts),
            AgentEvent::MessageComplete {
                session_id,
                role,
                text,
            } => {
                // message.ts 毫秒 / updated_at 秒（变更-07，与 record_event_for_session 一致）
                let ts = now_millis();
                let updated_at = ts / 1000;
                self.with_session(session_id, |conn, local_id| {
                    conn.execute(
                        "INSERT INTO message (session_id, role, text, ts) VALUES (?1, ?2, ?3, ?4)",
                        params![local_id, role_to_str(*role), text, ts],
                    )?;
                    conn.execute(
                        "UPDATE session SET updated_at = ?1 WHERE id = ?2",
                        params![updated_at, local_id],
                    )?;
                    Ok(())
                })
            }
            AgentEvent::ToolCall {
                session_id,
                id,
                name,
                input,
                status,
            } => {
                let ts = now_millis();
                let updated_at = ts / 1000;
                self.with_session(session_id, |conn, local_id| {
                    conn.execute(
                        "INSERT OR REPLACE INTO tool_call
                         (id, session_id, name, input_json, status, output, ts)
                         VALUES (?1, ?2, ?3, ?4, ?5, NULL, ?6)",
                        params![
                            id,
                            local_id,
                            name,
                            serde_json::to_string(input).unwrap_or_else(|_| "null".to_string()),
                            history_call_status(*status),
                            ts
                        ],
                    )?;
                    conn.execute(
                        "UPDATE session SET updated_at = ?1 WHERE id = ?2",
                        params![updated_at, local_id],
                    )?;
                    Ok(())
                })
            }
            AgentEvent::ToolResult {
                session_id,
                id,
                status,
                output,
                diff,
            } => {
                let ts = now_seconds();
                let diff_json = diff
                    .as_ref()
                    .map(serde_json::to_string)
                    .transpose()
                    .map_err(|e| e.to_string())?;
                self.with_session(session_id, |conn, local_id| {
                    conn.execute(
                        "UPDATE tool_call SET status = ?1, output = ?2, diff_json = ?3 WHERE id = ?4 AND session_id = ?5",
                        params![history_tool_status(*status), output, diff_json, id, local_id],
                    )?;
                    conn.execute(
                        "UPDATE session SET updated_at = ?1 WHERE id = ?2",
                        params![ts, local_id],
                    )?;
                    Ok(())
                })
            }
            AgentEvent::TokenUsage {
                session_id,
                input_tokens,
                output_tokens,
                cost_usd,
                ..
            } => {
                let ts = now_seconds();
                self.with_session(session_id, |conn, local_id| {
                    conn.execute(
                        "INSERT INTO usage
                         (session_id, model, input_tokens, output_tokens, cost_usd, ts)
                         VALUES (?1, (SELECT model FROM session WHERE id = ?1), ?2, ?3, ?4, ?5)",
                        params![local_id, input_tokens, output_tokens, cost_usd, ts],
                    )?;
                    conn.execute(
                        "UPDATE session SET updated_at = ?1 WHERE id = ?2",
                        params![ts, local_id],
                    )?;
                    Ok(())
                })
            }
            AgentEvent::TurnComplete {
                session_id,
                stop_reason,
            } => {
                let status = match stop_reason {
                    StopReason::End => "done",
                    StopReason::Interrupted | StopReason::Error => "idle",
                };
                let ts = now_seconds();
                self.with_session(session_id, |conn, local_id| {
                    conn.execute(
                        "UPDATE session SET status = ?1, updated_at = ?2 WHERE id = ?3",
                        params![status, ts, local_id],
                    )?;
                    Ok(())
                })
            }
            AgentEvent::Error {
                session_id,
                recoverable,
                ..
            } => {
                // 可恢复警告（如看门狗提示）不改会话状态——轮次实际还在跑（变更-12）
                if *recoverable {
                    return Ok(());
                }
                if let Some(session_id) = session_id {
                    let ts = now_seconds();
                    self.with_session(session_id, |conn, local_id| {
                        conn.execute(
                            "UPDATE session SET status = 'idle', updated_at = ?1 WHERE id = ?2",
                            params![ts, local_id],
                        )?;
                        Ok(())
                    })
                } else {
                    Ok(())
                }
            }
            AgentEvent::MessageDelta { .. }
            | AgentEvent::ThinkingDelta { .. }
            | AgentEvent::ThinkingComplete { .. }
            | AgentEvent::TurnStage { .. }
            | AgentEvent::ToolProgress { .. }
            | AgentEvent::PlanUpdate { .. } => Ok(()),
            AgentEvent::ApprovalRequest {
                session_id,
                id,
                action,
                detail,
                ..
            } => {
                let ts = now_millis();
                self.with_session(session_id, |conn, local_id| {
                    conn.execute(
                        "INSERT OR REPLACE INTO approval (id, session_id, action, detail, status, ts)
                         VALUES (?1, ?2, ?3, ?4, 'pending', ?5)",
                        params![id, local_id, action, detail, ts],
                    )?;
                    Ok(())
                })
            }
            AgentEvent::Checkpoint {
                id,
                label,
                ts,
                session_id,
            } => self.with_session(session_id, |conn, local_id| {
                conn.execute(
                    "INSERT OR IGNORE INTO checkpoint (id, session_id, turn_idx, label, snapshot_ref, ts)
                         VALUES (?1, ?2, 0, ?3, '', ?4)",
                    params![id, local_id, label, ts],
                )?;
                Ok(())
            }),
        }
    }

    pub fn record_event_for_session(
        &self,
        history_session_id: &str,
        event: &AgentEvent,
    ) -> Result<(), String> {
        match event {
            AgentEvent::SessionStarted {
                session_id,
                engine,
                model,
                cwd,
                ts,
            } => {
                self.attach_cli_session_to(history_session_id, session_id, *engine, model, cwd, *ts)
            }
            AgentEvent::MessageComplete { role, text, .. } => {
                // message.ts 用毫秒（与 checkpoint.ts 同单位，回溯截断依赖比较，变更-07）；
                // session.updated_at 维持秒
                let ts = now_millis();
                let updated_at = ts / 1000;
                self.with_local_session(history_session_id, |conn, local_id| {
                    conn.execute(
                        "INSERT INTO message (session_id, role, text, ts) VALUES (?1, ?2, ?3, ?4)",
                        params![local_id, role_to_str(*role), text, ts],
                    )?;
                    conn.execute(
                        "UPDATE session SET updated_at = ?1 WHERE id = ?2",
                        params![updated_at, local_id],
                    )?;
                    Ok(())
                })
            }
            AgentEvent::ToolCall {
                id,
                name,
                input,
                status,
                ..
            } => {
                let ts = now_millis();
                let updated_at = ts / 1000;
                self.with_local_session(history_session_id, |conn, local_id| {
                    conn.execute(
                        "INSERT OR REPLACE INTO tool_call
                         (id, session_id, name, input_json, status, output, ts)
                         VALUES (?1, ?2, ?3, ?4, ?5, NULL, ?6)",
                        params![
                            id,
                            local_id,
                            name,
                            serde_json::to_string(input).unwrap_or_else(|_| "null".to_string()),
                            history_call_status(*status),
                            ts
                        ],
                    )?;
                    conn.execute(
                        "UPDATE session SET updated_at = ?1 WHERE id = ?2",
                        params![updated_at, local_id],
                    )?;
                    Ok(())
                })
            }
            AgentEvent::ToolResult {
                id,
                status,
                output,
                diff,
                ..
            } => {
                let ts = now_seconds();
                let diff_json = diff
                    .as_ref()
                    .map(serde_json::to_string)
                    .transpose()
                    .map_err(|e| e.to_string())?;
                self.with_local_session(history_session_id, |conn, local_id| {
                    conn.execute(
                        "UPDATE tool_call SET status = ?1, output = ?2, diff_json = ?3 WHERE id = ?4 AND session_id = ?5",
                        params![history_tool_status(*status), output, diff_json, id, local_id],
                    )?;
                    conn.execute(
                        "UPDATE session SET updated_at = ?1 WHERE id = ?2",
                        params![ts, local_id],
                    )?;
                    Ok(())
                })
            }
            AgentEvent::TokenUsage {
                input_tokens,
                output_tokens,
                cost_usd,
                ..
            } => {
                let ts = now_seconds();
                self.with_local_session(history_session_id, |conn, local_id| {
                    let model: String = conn.query_row(
                        "SELECT model FROM session WHERE id = ?1",
                        params![local_id],
                        |row| row.get(0),
                    )?;
                    let cost =
                        self.cost_with_fallback(&model, *input_tokens, *output_tokens, *cost_usd);
                    conn.execute(
                        "INSERT INTO usage
                         (session_id, model, input_tokens, output_tokens, cost_usd, ts)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                        params![local_id, model, input_tokens, output_tokens, cost, ts],
                    )?;
                    conn.execute(
                        "UPDATE session SET updated_at = ?1 WHERE id = ?2",
                        params![ts, local_id],
                    )?;
                    Ok(())
                })
            }
            AgentEvent::TurnComplete { stop_reason, .. } => {
                let status = match stop_reason {
                    StopReason::End => "done",
                    StopReason::Interrupted | StopReason::Error => "idle",
                };
                let ts = now_seconds();
                self.with_local_session(history_session_id, |conn, local_id| {
                    conn.execute(
                        "UPDATE session SET status = ?1, updated_at = ?2 WHERE id = ?3",
                        params![status, ts, local_id],
                    )?;
                    Ok(())
                })
            }
            AgentEvent::Error { recoverable, .. } => {
                // 可恢复警告不改会话状态——轮次实际还在跑（变更-12）
                if *recoverable {
                    return Ok(());
                }
                let ts = now_seconds();
                self.with_local_session(history_session_id, |conn, local_id| {
                    conn.execute(
                        "UPDATE session SET status = 'idle', updated_at = ?1 WHERE id = ?2",
                        params![ts, local_id],
                    )?;
                    Ok(())
                })
            }
            AgentEvent::MessageDelta { .. }
            | AgentEvent::ThinkingDelta { .. }
            | AgentEvent::ThinkingComplete { .. }
            | AgentEvent::TurnStage { .. }
            | AgentEvent::ToolProgress { .. }
            | AgentEvent::PlanUpdate { .. } => Ok(()),
            // 审批请求落库（变更-07）：切走/重启后审批卡可重建，不再永久悬置
            AgentEvent::ApprovalRequest {
                id,
                action,
                detail,
                ..
            } => {
                let ts = now_millis();
                self.with_local_session(history_session_id, |conn, local_id| {
                    conn.execute(
                        "INSERT OR REPLACE INTO approval (id, session_id, action, detail, status, ts)
                         VALUES (?1, ?2, ?3, ?4, 'pending', ?5)",
                        params![id, local_id, action, detail, ts],
                    )?;
                    Ok(())
                })
            }
            AgentEvent::Checkpoint { id, label, ts, .. } => {
                self.with_local_session(history_session_id, |conn, local_id| {
                    conn.execute(
                        "INSERT OR IGNORE INTO checkpoint (id, session_id, turn_idx, label, snapshot_ref, ts)
                         VALUES (?1, ?2, 0, ?3, '', ?4)",
                        params![id, local_id, label, ts],
                    )?;
                    Ok(())
                })
            }
        }
    }

    pub fn set_active_session(&self, session_id: &str) -> Result<(), String> {
        let _guard = self.write_guard()?;
        let conn = self.open()?;
        let local_id = self.resolve_local_id(&conn, session_id)?;
        self.set_setting_on_conn(&conn, "active_session_id", &local_id)
    }

    pub fn active_session(&self) -> Result<Option<SessionDetail>, String> {
        let conn = self.open()?;
        let Some(active_id) = self.get_setting_on_conn(&conn, "active_session_id")? else {
            return Ok(None);
        };
        match self.get_session(&active_id) {
            Ok(detail) => Ok(Some(detail)),
            Err(err) if err.contains("Query returned no rows") => Ok(None),
            Err(err) => Err(err),
        }
    }

    fn attach_cli_session(
        &self,
        cli_session_id: &str,
        engine: EngineId,
        model: &str,
        cwd: &str,
        ts: i64,
    ) -> Result<(), String> {
        let _guard = self.write_guard()?;
        let conn = self.open()?;
        let target_id = self
            .get_setting_on_conn(&conn, "active_session_id")?
            .or_else(|| {
                conn.query_row(
                    "SELECT id FROM session ORDER BY updated_at DESC LIMIT 1",
                    [],
                    |row| row.get(0),
                )
                .optional()
                .ok()
                .flatten()
            })
            .ok_or_else(|| "没有可关联的本地会话".to_string())?;
        conn.execute(
            "UPDATE session
             SET cli_session_id = ?1, engine = ?2, model = ?3, cwd = ?4, updated_at = ?5
             WHERE id = ?6",
            params![
                cli_session_id,
                engine_to_str(engine),
                model,
                cwd,
                // SessionStarted 的 ts 是毫秒，updated_at 维持秒（变更-07 单位修复）
                ts / 1000,
                target_id
            ],
        )
        .map_err(db_err)?;
        Ok(())
    }

    fn attach_cli_session_to(
        &self,
        history_session_id: &str,
        cli_session_id: &str,
        engine: EngineId,
        model: &str,
        cwd: &str,
        ts: i64,
    ) -> Result<(), String> {
        retry_locked(|| {
            let _guard = self.write_guard()?;
            let conn = self.open()?;
            let local_id = self.resolve_local_id(&conn, history_session_id)?;
            conn.execute(
                "UPDATE session
                 SET cli_session_id = ?1, engine = ?2, model = ?3, cwd = ?4, updated_at = ?5
                 WHERE id = ?6",
                params![
                    cli_session_id,
                    engine_to_str(engine),
                    model,
                    cwd,
                    // SessionStarted 的 ts 是毫秒，updated_at 维持秒（变更-07 单位修复）
                    ts / 1000,
                    local_id
                ],
            )
            .map_err(db_err)?;
            Ok(())
        })
    }

    fn with_session<F>(&self, session_id: &str, update: F) -> Result<(), String>
    where
        F: Fn(&Connection, &str) -> rusqlite::Result<()>,
    {
        retry_locked(|| {
            let _guard = self.write_guard()?;
            let conn = self.open()?;
            let local_id = self.resolve_local_id(&conn, session_id)?;
            update(&conn, &local_id).map_err(db_err)
        })
    }

    fn with_local_session<F>(&self, history_session_id: &str, update: F) -> Result<(), String>
    where
        F: Fn(&Connection, &str) -> rusqlite::Result<()>,
    {
        retry_locked(|| {
            let _guard = self.write_guard()?;
            let conn = self.open()?;
            let local_id = self.resolve_local_id(&conn, history_session_id)?;
            update(&conn, &local_id).map_err(db_err)
        })
    }

    fn cost_with_fallback(
        &self,
        model: &str,
        input_tokens: u64,
        output_tokens: u64,
        cost_usd: f64,
    ) -> f64 {
        if cost_usd > 0.0 || (input_tokens == 0 && output_tokens == 0) {
            return cost_usd;
        }
        let Ok(prices) = self.model_prices.lock() else {
            return cost_usd;
        };
        let Some(price) = prices.get(model) else {
            return cost_usd;
        };
        ((input_tokens as f64 / 1_000_000.0) * price.input_price_per_mtok)
            + ((output_tokens as f64 / 1_000_000.0) * price.output_price_per_mtok)
    }

    fn open(&self) -> Result<Connection, String> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("创建会话数据库目录失败：{e}"))?;
        }
        let conn = Connection::open(&self.path).map_err(db_err)?;
        configure_connection(&conn)?;
        self.ensure_initialized(&conn)?;
        Ok(conn)
    }

    fn write_guard(&self) -> Result<std::sync::MutexGuard<'_, ()>, String> {
        self.write_lock
            .lock()
            .map_err(|_| "会话数据库写锁中毒".to_string())
    }

    fn ensure_initialized(&self, conn: &Connection) -> Result<(), String> {
        let mut initialized = self
            .initialized
            .lock()
            .map_err(|_| "会话数据库初始化锁中毒".to_string())?;
        if *initialized {
            return Ok(());
        }
        init_schema(conn)?;
        *initialized = true;
        Ok(())
    }

    fn resolve_local_id(&self, conn: &Connection, id: &str) -> Result<String, String> {
        conn.query_row(
            "SELECT id FROM session WHERE id = ?1 OR cli_session_id = ?1",
            params![id],
            |row| row.get(0),
        )
        .map_err(db_err)
    }

    fn messages_for_conn(
        &self,
        conn: &Connection,
        session_id: &str,
    ) -> Result<Vec<SessionMessage>, String> {
        let mut stmt = conn
            .prepare(
                "SELECT role, text, ts, reverted FROM message WHERE session_id = ?1 ORDER BY id ASC",
            )
            .map_err(db_err)?;
        let messages = stmt
            .query_map(params![session_id], |row| {
                Ok(SessionMessage {
                    role: role_from_str(row.get::<_, String>(0)?.as_str())?,
                    text: row.get(1)?,
                    ts: row.get(2)?,
                    reverted: row.get::<_, i64>(3)? != 0,
                })
            })
            .map_err(db_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_err)?;
        Ok(messages)
    }

    fn tools_for_conn(
        &self,
        conn: &Connection,
        session_id: &str,
    ) -> Result<Vec<SessionToolCall>, String> {
        let mut stmt = conn
            .prepare(
                "SELECT id, name, status, input_json, output, diff_json, ts
                 FROM tool_call WHERE session_id = ?1 ORDER BY ts ASC",
            )
            .map_err(db_err)?;
        let tool_calls = stmt
            .query_map(params![session_id], |row| {
                let input_text: String = row.get(3)?;
                let diff_text: Option<String> = row.get(5)?;
                Ok(SessionToolCall {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    status: history_status_from_str(row.get::<_, String>(2)?.as_str())?,
                    input: serde_json::from_str(&input_text).unwrap_or(serde_json::Value::Null),
                    output: row.get(4)?,
                    diff: diff_text.and_then(|text| serde_json::from_str(&text).ok()),
                    ts: row.get(6)?,
                })
            })
            .map_err(db_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_err)?;
        Ok(tool_calls)
    }

    fn checkpoints_for_conn(
        &self,
        conn: &Connection,
        session_id: &str,
    ) -> Result<Vec<SessionCheckpoint>, String> {
        let mut stmt = conn
            .prepare(
                "SELECT id, label, ts
                 FROM checkpoint WHERE session_id = ?1 ORDER BY ts ASC",
            )
            .map_err(db_err)?;
        let checkpoints = stmt
            .query_map(params![session_id], |row| {
                Ok(SessionCheckpoint {
                    id: row.get(0)?,
                    label: row.get(1)?,
                    ts: row.get(2)?,
                })
            })
            .map_err(db_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_err)?;
        Ok(checkpoints)
    }

    fn approvals_for_conn(
        &self,
        conn: &Connection,
        session_id: &str,
    ) -> Result<Vec<SessionApproval>, String> {
        let mut stmt = conn
            .prepare(
                "SELECT id, action, detail, status, ts
                 FROM approval WHERE session_id = ?1 ORDER BY ts ASC",
            )
            .map_err(db_err)?;
        let approvals = stmt
            .query_map(params![session_id], |row| {
                Ok(SessionApproval {
                    id: row.get(0)?,
                    action: row.get(1)?,
                    detail: row.get(2)?,
                    status: row.get(3)?,
                    ts: row.get(4)?,
                })
            })
            .map_err(db_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_err)?;
        Ok(approvals)
    }

    /// 审批已处理（变更-07）：用户点了允许/始终允许/拒绝
    pub fn resolve_approval(&self, session_id: &str, approval_id: &str) -> Result<(), String> {
        let _guard = self.write_guard()?;
        let conn = self.open()?;
        let local_id = self.resolve_local_id(&conn, session_id)?;
        conn.execute(
            "UPDATE approval SET status = 'resolved' WHERE session_id = ?1 AND id = ?2",
            params![local_id, approval_id],
        )
        .map_err(db_err)?;
        Ok(())
    }

    /// 审批恢复执行失败时的补偿：把刚标记 resolved 的记录恢复为 pending，允许用户重试。
    pub fn reopen_approval(&self, session_id: &str, approval_id: &str) -> Result<(), String> {
        let _guard = self.write_guard()?;
        let conn = self.open()?;
        let local_id = self.resolve_local_id(&conn, session_id)?;
        let changed = conn
            .execute(
                "UPDATE approval SET status = 'pending' WHERE session_id = ?1 AND id = ?2 AND status = 'resolved'",
                params![local_id, approval_id],
            )
            .map_err(db_err)?;
        if changed != 1 {
            return Err(format!("找不到可重新打开的审批：{approval_id}"));
        }
        Ok(())
    }

    /// 作废悬空审批（变更-07）：用户绕过审批直接发新消息时，被 defer 的工具已被
    /// CLI 丢弃，旧审批卡不可再响应——统一标记 expired，防止事后误点触发陈旧恢复轮。
    pub fn expire_pending_approvals(&self, session_id: &str) -> Result<(), String> {
        let _guard = self.write_guard()?;
        let conn = self.open()?;
        let local_id = self.resolve_local_id(&conn, session_id)?;
        conn.execute(
            "UPDATE approval SET status = 'expired' WHERE session_id = ?1 AND status = 'pending'",
            params![local_id],
        )
        .map_err(db_err)?;
        Ok(())
    }

    /// 删除会话（变更-12）：连同消息/工具/用量/审批/检查点级联删除（外键 CASCADE），
    /// 返回被删检查点的快照 ref 供调用方清理磁盘快照文件。
    pub fn delete_session(&self, session_id: &str) -> Result<Vec<String>, String> {
        let _guard = self.write_guard()?;
        let conn = self.open()?;
        let local_id = self.resolve_local_id(&conn, session_id)?;
        let mut stmt = conn
            .prepare("SELECT snapshot_ref FROM checkpoint WHERE session_id = ?1")
            .map_err(db_err)?;
        let snapshot_refs: Vec<String> = stmt
            .query_map(params![local_id], |row| row.get::<_, String>(0))
            .map_err(db_err)?
            .filter_map(|item| item.ok())
            .filter(|item| !item.is_empty())
            .collect();
        conn.execute("DELETE FROM session WHERE id = ?1", params![local_id])
            .map_err(db_err)?;
        // 若删的是「上次活跃会话」，清掉指针防止自动恢复指向不存在的会话
        if self.get_setting_on_conn(&conn, "active_session_id")? == Some(local_id.clone()) {
            conn.execute("DELETE FROM setting WHERE key = 'active_session_id'", [])
                .map_err(db_err)?;
        }
        Ok(snapshot_refs)
    }

    /// 重命名会话（变更-12）：手动改名后不再被自动起标题覆盖（title 已非「未命名会话」）
    pub fn rename_session(&self, session_id: &str, title: &str) -> Result<(), String> {
        let trimmed = title.trim();
        if trimmed.is_empty() {
            return Err("标题不能为空".to_string());
        }
        let _guard = self.write_guard()?;
        let conn = self.open()?;
        let local_id = self.resolve_local_id(&conn, session_id)?;
        conn.execute(
            "UPDATE session SET title = ?1 WHERE id = ?2",
            params![trimmed, local_id],
        )
        .map_err(db_err)?;
        Ok(())
    }

    /// 置顶/取消置顶（变更-12）
    pub fn set_session_pinned(&self, session_id: &str, pinned: bool) -> Result<(), String> {
        let _guard = self.write_guard()?;
        let conn = self.open()?;
        let local_id = self.resolve_local_id(&conn, session_id)?;
        conn.execute(
            "UPDATE session SET pinned = ?1 WHERE id = ?2",
            params![pinned as i64, local_id],
        )
        .map_err(db_err)?;
        Ok(())
    }

    /// 启动归位（变更-12）：应用启动时进程必然不存在，把误留在 active 的会话
    /// 归位为 idle（强杀退出/崩溃会留下 active 尸体，「活跃」筛选从此不可信）。
    pub fn normalize_stale_active_sessions(&self) -> Result<(), String> {
        let _guard = self.write_guard()?;
        let conn = self.open()?;
        conn.execute(
            "UPDATE session SET status = 'idle' WHERE status = 'active'",
            [],
        )
        .map_err(db_err)?;
        Ok(())
    }

    fn set_setting_on_conn(&self, conn: &Connection, key: &str, value: &str) -> Result<(), String> {
        conn.execute(
            "INSERT INTO setting (key, value_json) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json",
            params![
                key,
                serde_json::to_string(value).map_err(|e| e.to_string())?
            ],
        )
        .map_err(db_err)?;
        Ok(())
    }

    pub fn set_json_setting<T: Serialize>(&self, key: &str, value: &T) -> Result<(), String> {
        let _guard = self.write_guard()?;
        let conn = self.open()?;
        conn.execute(
            "INSERT INTO setting (key, value_json) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json",
            params![
                key,
                serde_json::to_string(value).map_err(|e| e.to_string())?
            ],
        )
        .map_err(db_err)?;
        Ok(())
    }

    pub fn get_json_setting<T: for<'de> Deserialize<'de>>(
        &self,
        key: &str,
    ) -> Result<Option<T>, String> {
        let conn = self.open()?;
        let raw: Option<String> = conn
            .query_row(
                "SELECT value_json FROM setting WHERE key = ?1",
                params![key],
                |row| row.get(0),
            )
            .optional()
            .map_err(db_err)?;
        raw.map(|value| serde_json::from_str(&value).map_err(|e| e.to_string()))
            .transpose()
    }

    /// 当前自然月起点（unix 秒）：预算与阈值提醒的月度边界（与 get_budget 同一算法）
    pub fn current_month_start(&self) -> Result<i64, String> {
        let conn = self.open()?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_secs() as i64;
        conn.query_row(
            "SELECT unixepoch(date(?1, 'unixepoch', 'start of month'))",
            params![now],
            |row| row.get(0),
        )
        .map_err(db_err)
    }

    /// 「始终允许」的工具清单（P2-4）：跨会话持久化，key 独立于 app_settings，
    /// 只由审批链路与显式撤销命令写入，避免设置页整体保存时被旧快照覆盖。
    pub fn get_always_allow_tools(&self) -> Result<Vec<String>, String> {
        Ok(self
            .get_json_setting::<Vec<String>>(ALWAYS_ALLOW_TOOLS_KEY)?
            .unwrap_or_default())
    }

    pub fn add_always_allow_tool(&self, tool: &str) -> Result<Vec<String>, String> {
        let mut tools = self.get_always_allow_tools()?;
        if !tools.iter().any(|item| item == tool) {
            tools.push(tool.to_string());
            self.set_json_setting(ALWAYS_ALLOW_TOOLS_KEY, &tools)?;
        }
        Ok(tools)
    }

    pub fn remove_always_allow_tool(&self, tool: &str) -> Result<Vec<String>, String> {
        let mut tools = self.get_always_allow_tools()?;
        let before = tools.len();
        tools.retain(|item| item != tool);
        if tools.len() != before {
            self.set_json_setting(ALWAYS_ALLOW_TOOLS_KEY, &tools)?;
        }
        Ok(tools)
    }

    fn get_setting_on_conn(&self, conn: &Connection, key: &str) -> Result<Option<String>, String> {
        let raw: Option<String> = conn
            .query_row(
                "SELECT value_json FROM setting WHERE key = ?1",
                params![key],
                |row| row.get(0),
            )
            .optional()
            .map_err(db_err)?;
        raw.map(|value| serde_json::from_str(&value).map_err(|e| e.to_string()))
            .transpose()
    }

    pub fn save_checkpoint(
        &self,
        checkpoint_id: &str,
        session_id: &str,
        turn_idx: i64,
        label: &str,
        snapshot_ref: &str,
        ts: i64,
    ) -> Result<(), String> {
        let _guard = self.write_guard()?;
        let conn = self.open()?;
        let local_id = self.resolve_local_id(&conn, session_id)?;
        conn.execute(
            "INSERT INTO checkpoint (id, session_id, turn_idx, label, snapshot_ref, ts)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![checkpoint_id, local_id, turn_idx, label, snapshot_ref, ts],
        )
        .map_err(db_err)?;
        Ok(())
    }

    pub fn get_checkpoint(&self, checkpoint_id: &str) -> Result<Option<CheckpointRecord>, String> {
        let conn = self.open()?;
        let result: Option<CheckpointRecord> = conn
            .query_row(
                "SELECT id, session_id, turn_idx, label, snapshot_ref, ts FROM checkpoint WHERE id = ?1",
                params![checkpoint_id],
                |row| {
                    Ok(CheckpointRecord {
                        id: row.get(0)?,
                        session_id: row.get(1)?,
                        turn_idx: row.get(2)?,
                        label: row.get(3)?,
                        snapshot_ref: row.get(4)?,
                        ts: row.get(5)?,
                    })
                },
            )
            .optional()
            .map_err(db_err)?;
        Ok(result)
    }

    /// 回溯语义（P2-5 / 变更-07）：把检查点之后的消息打上 reverted 标记，
    /// 重建上下文/续聊序列化会剔除它们，让 Agent 记忆与文件状态一致。
    /// `ts_millis` 与 message.ts 同为毫秒（v4 迁移统一单位后比较才有效）。
    pub fn revert_messages_after(&self, session_id: &str, ts_millis: i64) -> Result<(), String> {
        let _guard = self.write_guard()?;
        let conn = self.open()?;
        let local_id = self.resolve_local_id(&conn, session_id)?;
        conn.execute(
            "UPDATE message SET reverted = 1 WHERE session_id = ?1 AND ts > ?2",
            params![local_id, ts_millis],
        )
        .map_err(db_err)?;
        Ok(())
    }

    pub fn unrevert_messages(&self, session_id: &str) -> Result<(), String> {
        let _guard = self.write_guard()?;
        let conn = self.open()?;
        let local_id = self.resolve_local_id(&conn, session_id)?;
        conn.execute(
            "UPDATE message SET reverted = 0 WHERE session_id = ?1",
            params![local_id],
        )
        .map_err(db_err)?;
        Ok(())
    }

    /// 回溯后作废旧的 CLI 会话 id：下次恢复不再 `--resume`，改用截断历史重建上下文（P2-5）
    pub fn clear_cli_session(&self, session_id: &str) -> Result<(), String> {
        let _guard = self.write_guard()?;
        let conn = self.open()?;
        let local_id = self.resolve_local_id(&conn, session_id)?;
        conn.execute(
            "UPDATE session SET cli_session_id = NULL WHERE id = ?1",
            params![local_id],
        )
        .map_err(db_err)?;
        Ok(())
    }

    /// Codex `thread.started` 返回的原生 continuation id。它会替换进程启动时的临时 id，
    /// 供关闭应用后的 `exec resume <thread_id>` 使用。
    pub fn attach_native_thread_to_session(
        &self,
        session_id: &str,
        thread_id: &str,
    ) -> Result<(), String> {
        let _guard = self.write_guard()?;
        let conn = self.open()?;
        let local_id = self.resolve_local_id(&conn, session_id)?;
        conn.execute(
            "UPDATE session SET cli_session_id = ?1, updated_at = ?2 WHERE id = ?3",
            params![thread_id, now_millis() / 1000, local_id],
        )
        .map_err(db_err)?;
        Ok(())
    }

    // 用量统计查询
    pub fn get_usage_stats(&self, days: u32) -> Result<UsageStats, String> {
        let conn = self.open()?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let cutoff_ts = now - (days as i64 * 86400);

        let mut stmt = conn
            .prepare(
                "SELECT
                    COALESCE(SUM(cost_usd), 0.0),
                    COALESCE(SUM(input_tokens), 0),
                    COALESCE(SUM(output_tokens), 0),
                    COUNT(DISTINCT id)
                 FROM usage
                 WHERE ts >= ?1",
            )
            .map_err(db_err)?;

        let (total_cost, input_tokens, output_tokens, request_count): (f64, i64, i64, i64) = stmt
            .query_row(params![cutoff_ts], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .map_err(db_err)?;

        let session_count: i64 = conn
            .query_row(
                "SELECT COUNT(DISTINCT session_id) FROM usage WHERE ts >= ?1",
                params![cutoff_ts],
                |row| row.get(0),
            )
            .map_err(db_err)?;

        Ok(UsageStats {
            total_cost,
            total_tokens: (input_tokens + output_tokens) as u64,
            input_tokens: input_tokens as u64,
            output_tokens: output_tokens as u64,
            request_count: request_count as u32,
            session_count: session_count as u32,
        })
    }

    pub fn get_usage_by_model(&self, days: u32) -> Result<Vec<ModelUsage>, String> {
        let conn = self.open()?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let cutoff_ts = now - (days as i64 * 86400);

        let total_cost: f64 = conn
            .query_row(
                "SELECT COALESCE(SUM(cost_usd), 0.0) FROM usage WHERE ts >= ?1",
                params![cutoff_ts],
                |row| row.get(0),
            )
            .map_err(db_err)?;

        let mut stmt = conn
            .prepare(
                "SELECT
                    u.model,
                    s.engine,
                    COUNT(DISTINCT u.id) as request_count,
                    SUM(u.input_tokens) as input_tokens,
                    SUM(u.output_tokens) as output_tokens,
                    SUM(u.cost_usd) as cost_usd
                 FROM usage u
                 LEFT JOIN session s ON u.session_id = s.id
                 WHERE u.ts >= ?1
                 GROUP BY u.model, s.engine
                 ORDER BY cost_usd DESC",
            )
            .map_err(db_err)?;

        let rows = stmt
            .query_map(params![cutoff_ts], |row| {
                let cost: f64 = row.get(5)?;
                let share = if total_cost > 0.0 {
                    cost / total_cost
                } else {
                    0.0
                };
                Ok(ModelUsage {
                    model: row.get(0)?,
                    engine: row.get::<_, Option<String>>(1)?.unwrap_or_default(),
                    request_count: row.get::<_, i64>(2)? as u32,
                    input_tokens: row.get::<_, i64>(3)? as u64,
                    output_tokens: row.get::<_, i64>(4)? as u64,
                    cost_usd: cost,
                    share,
                })
            })
            .map_err(db_err)?;

        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(db_err)
    }

    /// 按服务商聚合用量（P3-6）：以 session.provider_id 真实归属，不再按模型名猜。
    /// 老会话（provider_id 为空串）归入空 key，前端显示「未标注」。
    pub fn get_usage_by_provider(&self, days: u32) -> Result<Vec<ProviderUsage>, String> {
        let conn = self.open()?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let cutoff_ts = now - (days as i64 * 86400);

        let total_cost: f64 = conn
            .query_row(
                "SELECT COALESCE(SUM(cost_usd), 0.0) FROM usage WHERE ts >= ?1",
                params![cutoff_ts],
                |row| row.get(0),
            )
            .map_err(db_err)?;

        let mut stmt = conn
            .prepare(
                "SELECT
                    COALESCE(s.provider_id, '') as provider_id,
                    SUM(u.cost_usd) as cost_usd
                 FROM usage u
                 LEFT JOIN session s ON u.session_id = s.id
                 WHERE u.ts >= ?1
                 GROUP BY provider_id
                 ORDER BY cost_usd DESC",
            )
            .map_err(db_err)?;

        let rows = stmt
            .query_map(params![cutoff_ts], |row| {
                let cost: f64 = row.get(1)?;
                let share = if total_cost > 0.0 {
                    cost / total_cost
                } else {
                    0.0
                };
                Ok(ProviderUsage {
                    provider: row.get(0)?,
                    cost_usd: cost,
                    share,
                })
            })
            .map_err(db_err)?;

        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(db_err)
    }

    /// 记录会话实际使用的服务商（P3-6）：create/resume 时以当前绑定为准
    pub fn set_session_provider(&self, session_id: &str, provider_id: &str) -> Result<(), String> {
        let _guard = self.write_guard()?;
        let conn = self.open()?;
        let local_id = self.resolve_local_id(&conn, session_id)?;
        conn.execute(
            "UPDATE session SET provider_id = ?1 WHERE id = ?2",
            params![provider_id, local_id],
        )
        .map_err(db_err)?;
        Ok(())
    }

    /// 会话记录的服务商 id（P3-5 起标题时定位计费方用）
    pub fn session_provider_id(&self, session_id: &str) -> Result<String, String> {
        let conn = self.open()?;
        let local_id = self.resolve_local_id(&conn, session_id)?;
        conn.query_row(
            "SELECT COALESCE(provider_id, '') FROM session WHERE id = ?1",
            params![local_id],
            |row| row.get(0),
        )
        .map_err(db_err)
    }

    /// 是否需要自动起标题（P3-5）：摘要还没生成，且已有至少一轮完整对话
    pub fn session_needs_auto_title(&self, session_id: &str) -> Result<bool, String> {
        let conn = self.open()?;
        let local_id = self.resolve_local_id(&conn, session_id)?;
        let has_summary: bool = conn
            .query_row(
                "SELECT summary IS NOT NULL FROM session WHERE id = ?1",
                params![local_id],
                |row| row.get(0),
            )
            .map_err(db_err)?;
        if has_summary {
            return Ok(false);
        }
        let assistant_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM message WHERE session_id = ?1 AND role = 'assistant'",
                params![local_id],
                |row| row.get(0),
            )
            .map_err(db_err)?;
        Ok(assistant_count > 0)
    }

    /// 写入 fast model 生成的标题与摘要（P3-5）
    pub fn set_session_title_and_summary(
        &self,
        session_id: &str,
        title: &str,
        summary: &str,
    ) -> Result<(), String> {
        let _guard = self.write_guard()?;
        let conn = self.open()?;
        let local_id = self.resolve_local_id(&conn, session_id)?;
        conn.execute(
            "UPDATE session SET title = ?1, summary = ?2 WHERE id = ?3",
            params![title, summary, local_id],
        )
        .map_err(db_err)?;
        Ok(())
    }

    pub fn get_daily_usage(&self, days: u32) -> Result<Vec<DailyUsage>, String> {
        let conn = self.open()?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let cutoff_ts = now - (days as i64 * 86400);

        let mut stmt = conn
            .prepare(
                "SELECT
                    DATE(ts, 'unixepoch') as date,
                    SUM(cost_usd) as cost
                 FROM usage
                 WHERE ts >= ?1
                 GROUP BY date
                 ORDER BY date",
            )
            .map_err(db_err)?;

        let rows = stmt
            .query_map(params![cutoff_ts], |row| {
                Ok(DailyUsage {
                    date: row.get(0)?,
                    cost_usd: row.get(1)?,
                })
            })
            .map_err(db_err)?;

        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(db_err)
    }

    pub fn get_top_sessions(&self, days: u32, limit: usize) -> Result<Vec<TopSession>, String> {
        let conn = self.open()?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let cutoff_ts = now - (days as i64 * 86400);

        let mut stmt = conn
            .prepare(
                "SELECT
                    s.id,
                    s.title,
                    s.model,
                    s.engine,
                    COALESCE(SUM(u.cost_usd), 0.0) as cost,
                    COALESCE(SUM(u.input_tokens + u.output_tokens), 0) as tokens
                 FROM session s
                 LEFT JOIN usage u ON s.id = u.session_id AND u.ts >= ?1
                 GROUP BY s.id
                 HAVING cost > 0
                 ORDER BY cost DESC
                 LIMIT ?2",
            )
            .map_err(db_err)?;

        let rows = stmt
            .query_map(params![cutoff_ts, limit], |row| {
                Ok(TopSession {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    model: row.get(2)?,
                    engine: row.get(3)?,
                    cost_usd: row.get(4)?,
                    total_tokens: row.get::<_, i64>(5)? as u64,
                })
            })
            .map_err(db_err)?;

        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(db_err)
    }

    pub fn get_budget(&self) -> Result<Budget, String> {
        let conn = self.open()?;

        let monthly_limit: f64 = conn
            .query_row(
                "SELECT value_json FROM setting WHERE key = 'monthly_budget'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(db_err)?
            .and_then(|v| v.parse().ok())
            .unwrap_or(0.0);

        let alert_at_80: bool = conn
            .query_row(
                "SELECT value_json FROM setting WHERE key = 'budget_alert_80'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(db_err)?
            .and_then(|v| v.parse().ok())
            .unwrap_or(true);

        let stop_at_100: bool = conn
            .query_row(
                "SELECT value_json FROM setting WHERE key = 'budget_stop_100'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(db_err)?
            .and_then(|v| v.parse().ok())
            .unwrap_or(false);

        // 计算本月花费
        // 简化方案：使用 SQL 的 date 函数获取本月第一天
        // 对于更精确的月初计算，可以用外部库，但这里 SQL 足够准确
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        // 使用 SQLite 的 date 函数计算本月第一天的 Unix 时间戳
        let month_start: i64 = conn
            .query_row(
                "SELECT unixepoch(date(?1, 'unixepoch', 'start of month'))",
                params![now],
                |row| row.get(0),
            )
            .map_err(db_err)?;

        let current_month_cost: f64 = conn
            .query_row(
                "SELECT COALESCE(SUM(cost_usd), 0.0) FROM usage WHERE ts >= ?1",
                params![month_start],
                |row| row.get(0),
            )
            .map_err(db_err)?;

        let percentage = if monthly_limit > 0.0 {
            (current_month_cost / monthly_limit * 100.0).min(100.0)
        } else {
            0.0
        };

        Ok(Budget {
            monthly_limit,
            alert_at_80,
            stop_at_100,
            current_month_cost,
            percentage,
        })
    }

    pub fn set_budget(
        &self,
        monthly_limit: f64,
        alert_at_80: bool,
        stop_at_100: bool,
    ) -> Result<(), String> {
        let _guard = self.write_guard()?;
        let conn = self.open()?;

        conn.execute(
            "INSERT OR REPLACE INTO setting (key, value_json) VALUES ('monthly_budget', ?1)",
            params![monthly_limit.to_string()],
        )
        .map_err(db_err)?;

        conn.execute(
            "INSERT OR REPLACE INTO setting (key, value_json) VALUES ('budget_alert_80', ?1)",
            params![alert_at_80.to_string()],
        )
        .map_err(db_err)?;

        conn.execute(
            "INSERT OR REPLACE INTO setting (key, value_json) VALUES ('budget_stop_100', ?1)",
            params![stop_at_100.to_string()],
        )
        .map_err(db_err)?;

        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct CheckpointRecord {
    pub id: String,
    pub session_id: String,
    pub turn_idx: i64,
    pub label: String,
    pub snapshot_ref: String,
    pub ts: i64,
}

/// 当前数据库 schema 版本。任何加列/改表都必须：把版本 +1，并在
/// `apply_migrations` 中补一段从旧版本到新版本的迁移 SQL。
const SCHEMA_VERSION: i64 = 5;

fn init_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "
        PRAGMA journal_mode = WAL;
        PRAGMA foreign_keys = ON;
        CREATE TABLE IF NOT EXISTS session (
          id TEXT PRIMARY KEY,
          cli_session_id TEXT UNIQUE,
          title TEXT NOT NULL,
          engine TEXT NOT NULL,
          model TEXT NOT NULL,
          cwd TEXT NOT NULL,
          status TEXT NOT NULL,
          created_at INTEGER NOT NULL,
          updated_at INTEGER NOT NULL,
          summary TEXT,
          provider_id TEXT NOT NULL DEFAULT '',
          pinned INTEGER NOT NULL DEFAULT 0
        );
        CREATE TABLE IF NOT EXISTS message (
          id INTEGER PRIMARY KEY AUTOINCREMENT,
          session_id TEXT NOT NULL REFERENCES session(id) ON DELETE CASCADE,
          role TEXT NOT NULL,
          text TEXT NOT NULL,
          ts INTEGER NOT NULL,
          reverted INTEGER NOT NULL DEFAULT 0
        );
        CREATE TABLE IF NOT EXISTS tool_call (
          id TEXT NOT NULL,
          session_id TEXT NOT NULL REFERENCES session(id) ON DELETE CASCADE,
          name TEXT NOT NULL,
          input_json TEXT NOT NULL,
          status TEXT NOT NULL,
          output TEXT,
          diff_json TEXT,
          ts INTEGER NOT NULL,
          PRIMARY KEY (id, session_id)
        );
        CREATE TABLE IF NOT EXISTS usage (
          id INTEGER PRIMARY KEY AUTOINCREMENT,
          session_id TEXT NOT NULL REFERENCES session(id) ON DELETE CASCADE,
          model TEXT NOT NULL,
          input_tokens INTEGER NOT NULL,
          output_tokens INTEGER NOT NULL,
          cost_usd REAL NOT NULL,
          ts INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS setting (
          key TEXT PRIMARY KEY,
          value_json TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS approval (
          id TEXT NOT NULL,
          session_id TEXT NOT NULL REFERENCES session(id) ON DELETE CASCADE,
          action TEXT NOT NULL,
          detail TEXT NOT NULL,
          status TEXT NOT NULL DEFAULT 'pending',
          ts INTEGER NOT NULL,
          PRIMARY KEY (id, session_id)
        );
        CREATE TABLE IF NOT EXISTS checkpoint (
          id TEXT PRIMARY KEY,
          session_id TEXT NOT NULL REFERENCES session(id) ON DELETE CASCADE,
          turn_idx INTEGER NOT NULL,
          label TEXT NOT NULL,
          snapshot_ref TEXT NOT NULL,
          ts INTEGER NOT NULL
        );
        ",
    )
    .map_err(db_err)?;
    apply_migrations(conn)
}

/// 基于 PRAGMA user_version 的迁移框架：老库按版本逐级升级，新库直接盖章当前版本。
fn apply_migrations(conn: &Connection) -> Result<(), String> {
    let current: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(db_err)?;
    if current > SCHEMA_VERSION {
        return Err(format!(
            "数据库版本（{current}）比当前应用支持的版本（{SCHEMA_VERSION}）更新，请升级 Helm"
        ));
    }
    // 未来的迁移写在这里：if current < 4 { conn.execute_batch("ALTER TABLE ...")?; }
    if current < 2 && !column_exists(conn, "message", "reverted")? {
        // v2（P2-5 回溯语义）：message 表加 reverted 标记。
        // 新库的 CREATE TABLE 已含该列，这里只补老库。
        conn.execute_batch("ALTER TABLE message ADD COLUMN reverted INTEGER NOT NULL DEFAULT 0;")
            .map_err(db_err)?;
    }
    if current < 3 && !column_exists(conn, "session", "provider_id")? {
        // v3（P3-6 用量归属）：session 表记录创建/恢复时实际使用的服务商 id，
        // 用量按服务商聚合不再靠模型名猜。老会话保持空串，前端显示「未标注」。
        conn.execute_batch("ALTER TABLE session ADD COLUMN provider_id TEXT NOT NULL DEFAULT '';")
            .map_err(db_err)?;
    }
    if current < 4 {
        // v4（变更-07 回溯时间戳修复）：message/tool_call 的 ts 从秒统一为毫秒，
        // 与 checkpoint.ts（一直是毫秒）同单位——否则 revert_messages_after 的
        // `ts > 检查点毫秒` 永远不成立，回溯的消息截断完全失效。
        // 阈值 1e11：秒级时间戳（~1.7e9）远小于它，毫秒级（~1.7e12）远大于它，幂等安全。
        // 同时删除从未写入过的 turn 表（turn 级回溯是死代码，改由消息级 reverted 承担）。
        conn.execute_batch(
            "UPDATE message SET ts = ts * 1000 WHERE ts > 0 AND ts < 100000000000;
             UPDATE tool_call SET ts = ts * 1000 WHERE ts > 0 AND ts < 100000000000;
             DROP TABLE IF EXISTS turn;",
        )
        .map_err(db_err)?;
    }
    if current < 5 && !column_exists(conn, "session", "pinned")? {
        // v5（变更-12 会话管理）：置顶标记。新库的 CREATE TABLE 已含该列，这里只补老库。
        conn.execute_batch("ALTER TABLE session ADD COLUMN pinned INTEGER NOT NULL DEFAULT 0;")
            .map_err(db_err)?;
    }
    if current < SCHEMA_VERSION {
        conn.execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION};"))
            .map_err(db_err)?;
    }
    Ok(())
}

fn column_exists(conn: &Connection, table: &str, column: &str) -> Result<bool, String> {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .map_err(db_err)?;
    let mut rows = stmt.query([]).map_err(db_err)?;
    while let Some(row) = rows.next().map_err(db_err)? {
        let name: String = row.get(1).map_err(db_err)?;
        if name == column {
            return Ok(true);
        }
    }
    Ok(false)
}

fn configure_connection(conn: &Connection) -> Result<(), String> {
    conn.busy_timeout(Duration::from_secs(10)).map_err(db_err)?;
    conn.pragma_update(None, "foreign_keys", "ON")
        .map_err(db_err)?;
    Ok(())
}

fn summary_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SessionSummary> {
    Ok(SessionSummary {
        id: row.get(0)?,
        cli_session_id: row.get(1)?,
        title: row.get(2)?,
        engine: engine_from_str(row.get::<_, String>(3)?.as_str())?,
        model: row.get(4)?,
        cwd: row.get(5)?,
        status: session_status_from_str(row.get::<_, String>(6)?.as_str())?,
        message_count: row.get::<_, i64>(7)? as u32,
        input_tokens: row.get::<_, i64>(8)? as u64,
        output_tokens: row.get::<_, i64>(9)? as u64,
        cost_usd: row.get(10)?,
        created_at: row.get(11)?,
        updated_at: row.get(12)?,
        summary: row.get(13)?,
        pinned: row.get::<_, i64>(14)? != 0,
    })
}

fn db_err(error: rusqlite::Error) -> String {
    format!("会话数据库错误：{error}")
}

fn retry_locked<T, F>(mut op: F) -> Result<T, String>
where
    F: FnMut() -> Result<T, String>,
{
    let started = Instant::now();
    loop {
        match op() {
            Ok(value) => return Ok(value),
            Err(err)
                if err.contains("database is locked")
                    && started.elapsed() < Duration::from_secs(10) =>
            {
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(err) => return Err(err),
        }
    }
}

fn engine_to_str(engine: EngineId) -> &'static str {
    match engine {
        EngineId::ClaudeCode => "claude-code",
        EngineId::Codex => "codex",
    }
}

// 用量统计结构
#[derive(Debug, Clone, serde::Serialize)]
pub struct UsageStats {
    pub total_cost: f64,
    pub total_tokens: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub request_count: u32,
    pub session_count: u32,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ModelUsage {
    pub model: String,
    pub engine: String,
    pub request_count: u32,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
    pub share: f64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ProviderUsage {
    pub provider: String,
    pub cost_usd: f64,
    pub share: f64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct DailyUsage {
    pub date: String,
    pub cost_usd: f64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct TopSession {
    pub id: String,
    pub title: String,
    pub model: String,
    pub engine: String,
    pub cost_usd: f64,
    pub total_tokens: u64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Budget {
    pub monthly_limit: f64,
    pub alert_at_80: bool,
    pub stop_at_100: bool,
    pub current_month_cost: f64,
    pub percentage: f64,
}

fn engine_from_str(value: &str) -> rusqlite::Result<EngineId> {
    match value {
        "claude-code" => Ok(EngineId::ClaudeCode),
        "codex" => Ok(EngineId::Codex),
        _ => Err(rusqlite::Error::InvalidQuery),
    }
}

fn role_to_str(role: Role) -> &'static str {
    match role {
        Role::User => "user",
        Role::Assistant => "assistant",
    }
}

fn role_from_str(value: &str) -> rusqlite::Result<Role> {
    match value {
        "user" => Ok(Role::User),
        "assistant" => Ok(Role::Assistant),
        _ => Err(rusqlite::Error::InvalidQuery),
    }
}

fn session_status_from_str(value: &str) -> rusqlite::Result<SessionStatus> {
    match value {
        "active" => Ok(SessionStatus::Active),
        "idle" => Ok(SessionStatus::Idle),
        "done" => Ok(SessionStatus::Done),
        _ => Err(rusqlite::Error::InvalidQuery),
    }
}

fn history_status_from_str(value: &str) -> rusqlite::Result<HistoryToolStatus> {
    match value {
        "pending" => Ok(HistoryToolStatus::Pending),
        "success" => Ok(HistoryToolStatus::Success),
        "error" => Ok(HistoryToolStatus::Error),
        _ => Err(rusqlite::Error::InvalidQuery),
    }
}

fn history_call_status(status: CallStatus) -> &'static str {
    match status {
        CallStatus::Pending => "pending",
    }
}

fn history_tool_status(status: ToolStatus) -> &'static str {
    match status {
        ToolStatus::Success => "success",
        ToolStatus::Error => "error",
    }
}

fn title_from_text(text: &str) -> String {
    let title = text.trim().lines().next().unwrap_or("未命名会话").trim();
    if title.chars().count() > 30 {
        format!("{}…", title.chars().take(30).collect::<String>())
    } else if title.is_empty() {
        "未命名会话".to_string()
    } else {
        title.to_string()
    }
}

fn now_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_default()
}

/// message/tool_call 的 ts 单位（变更-07）：毫秒，与 checkpoint.ts 同单位。
/// session.created_at/updated_at 与 usage.ts 维持秒（用量按 date(ts,'unixepoch') 聚合）。
fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or_default()
}
