//! Backend-authoritative stream ordering and Turn finalization.
//!
//! Adapters submit normalized candidates only. This module assigns the public
//! event sequence, rejects stale ownership, persists boundary facts, updates
//! history, and is the sole writer of the terminal Turn snapshot.

use crate::budget::{BudgetDimension, BudgetEnforcementMode, TurnBudgetSnapshot};
use crate::protocol::{AgentEvent, StopReason, TurnStage, TurnStreamFailure};
use crate::runtime_registry::RuntimeOwnerRef;
use crate::sessions::{SessionHistoryStore, TurnSnapshotRecord};
use crate::turn_presentation::TurnPresentationBuffer;
use crate::util::now_millis;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::{mpsc, oneshot};

const EVENT_NAME: &str = "agent-event";
const STREAM_FAILURE_EVENT_NAME: &str = "turn-stream-failed";
const STREAM_FAILURE_MESSAGE: &str =
    "消息流保存/投递失败，已请求停止执行；部分显示内容可能尚未保存，请检查磁盘空间与访问权限";
const DEFAULT_QUEUE_CAPACITY: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnStatus {
    Running,
    WaitingApproval,
    Stalled,
    Succeeded,
    Failed,
    Interrupted,
}

impl TurnStatus {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed | Self::Interrupted)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnSnapshot {
    pub history_session_id: String,
    pub turn_id: String,
    pub turn_epoch: u64,
    pub status: TurnStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub terminal_reason: Option<String>,
    pub recoverable: bool,
    pub event_seq: u64,
    pub updated_at: i64,
    pub mode: String,
    pub permission_profile: String,
    pub started_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineEventCandidate {
    pub owner: RuntimeOwnerRef,
    pub history_session_id: String,
    pub turn_id: String,
    pub turn_epoch: u64,
    pub attempt_no: u64,
    pub runtime_generation_id: String,
    pub source_seq: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub native_event_id: Option<String>,
    pub observed_at: i64,
    pub event: AgentEvent,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AgentEventEnvelope<'a> {
    history_id: &'a str,
    event_seq: u64,
    turn_id: &'a str,
    turn_epoch: u64,
    attempt_no: u64,
    runtime_generation_id: &'a str,
    event: &'a AgentEvent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandidateDisposition {
    Accepted,
    Duplicate,
    Stale,
    Orphan,
    InvalidTransition,
    PersistenceFailed,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StreamDiagnostics {
    pub accepted: u64,
    pub duplicate: u64,
    pub stale: u64,
    pub orphan: u64,
    pub invalid_transition: u64,
    pub backpressure: u64,
    pub coalesced_delta: u64,
    pub persistence_failed: u64,
}

#[derive(Debug, Clone)]
struct AttemptBinding {
    owner: RuntimeOwnerRef,
    attempt_no: u64,
    runtime_generation_id: String,
}

#[derive(Debug, Clone)]
struct BudgetRuntimeState {
    snapshot: TurnBudgetSnapshot,
    output_bytes: u64,
    tool_output_bytes: HashMap<String, u64>,
    tool_count: u64,
    repeat_digests: HashMap<String, u64>,
    started_at: i64,
    last_event_at: i64,
    exceeded: HashSet<BudgetDimension>,
}

#[derive(Debug, Clone)]
struct BudgetTrigger {
    dimension: BudgetDimension,
    observed: u64,
    limit: u64,
    enforcement_mode: BudgetEnforcementMode,
    interrupt: bool,
}

#[derive(Debug, Clone)]
struct SupervisedTurn {
    snapshot: TurnSnapshot,
    binding: AttemptBinding,
    stream_failure_reported: bool,
    /// 本轮是否已向用户投递过任何可见内容（正文/思考/工具/审批/错误/计划/检查点）。
    /// 终态时仍为 false = 用户从头到尾什么都没看到，必须补一条可读终态，
    /// 不能静默收尾（否则 UI 只会显示「已完成」且没有任何结果）。
    saw_visible_content: bool,
    next_source_seq: u64,
    last_source_seq: u64,
    seen_native_events: HashSet<String>,
    native_session_id: Option<String>,
    tool_stream_bytes: HashMap<String, u64>,
    presentation: TurnPresentationBuffer,
    budget: BudgetRuntimeState,
}

struct QueuedCandidate {
    candidate: EngineEventCandidate,
    reply: oneshot::Sender<CandidateDisposition>,
}

struct SupervisorInner {
    store: SessionHistoryStore,
    app: Option<AppHandle>,
    current: Mutex<HashMap<String, SupervisedTurn>>,
    diagnostics: Mutex<StreamDiagnostics>,
    processing: Mutex<()>,
    ingress: tokio::sync::Mutex<()>,
    queue: mpsc::Sender<QueuedCandidate>,
}

#[derive(Clone)]
pub struct TurnSupervisor {
    inner: Arc<SupervisorInner>,
}

impl TurnSupervisor {
    pub fn new(store: SessionHistoryStore) -> Self {
        Self::build(store, None, DEFAULT_QUEUE_CAPACITY)
    }

    pub fn with_app(store: SessionHistoryStore, app: AppHandle) -> Self {
        Self::build(store, Some(app), DEFAULT_QUEUE_CAPACITY)
    }

    fn build(store: SessionHistoryStore, app: Option<AppHandle>, queue_capacity: usize) -> Self {
        let (queue, mut receiver) = mpsc::channel::<QueuedCandidate>(queue_capacity.max(1));
        let inner = Arc::new(SupervisorInner {
            store,
            app,
            current: Mutex::new(HashMap::new()),
            diagnostics: Mutex::new(StreamDiagnostics::default()),
            processing: Mutex::new(()),
            ingress: tokio::sync::Mutex::new(()),
            queue,
        });
        let weak = Arc::downgrade(&inner);
        std::thread::Builder::new()
            .name("helm-turn-events".to_string())
            .spawn(move || {
                while let Some(queued) = receiver.blocking_recv() {
                    let Some(inner) = weak.upgrade() else {
                        break;
                    };
                    let supervisor = TurnSupervisor { inner };
                    let disposition = supervisor.process_candidate(queued.candidate);
                    let _ = queued.reply.send(disposition);
                }
            })
            .expect("无法启动 Turn 事件持久化线程");
        Self { inner }
    }

    pub fn begin_attempt(
        &self,
        history_session_id: &str,
        turn_id: &str,
        turn_epoch: u64,
        mode: &str,
        permission_profile: &str,
        owner: RuntimeOwnerRef,
        attempt_no: u64,
        runtime_generation_id: &str,
    ) -> Result<(), String> {
        if owner != RuntimeOwnerRef::Session(history_session_id.to_string()) {
            return Err("Stream Supervisor owner 与历史 Session 不匹配".to_string());
        }
        let started_at = now_millis();
        let budget_snapshot = match self.inner.store.load_turn_budget_snapshot(turn_id) {
            Ok(snapshot) => snapshot,
            Err(_) if attempt_no == 0 => TurnBudgetSnapshot::standard(started_at),
            Err(error) => return Err(error),
        };
        let snapshot = TurnSnapshot {
            history_session_id: history_session_id.to_string(),
            turn_id: turn_id.to_string(),
            turn_epoch,
            status: TurnStatus::Running,
            terminal_reason: None,
            recoverable: true,
            event_seq: 0,
            updated_at: started_at,
            mode: mode.to_string(),
            permission_profile: normalize_profile(permission_profile),
            started_at,
        };
        let binding = AttemptBinding {
            owner,
            attempt_no,
            runtime_generation_id: runtime_generation_id.to_string(),
        };
        {
            let current = self
                .inner
                .current
                .lock()
                .map_err(|_| "Stream Supervisor 状态锁中毒".to_string())?;
            if let Some(existing) = current.get(history_session_id) {
                if existing.snapshot.turn_id == turn_id
                    && existing.binding.attempt_no == attempt_no
                    && existing.binding.runtime_generation_id == runtime_generation_id
                {
                    return Ok(());
                }
                if !existing.snapshot.status.is_terminal() {
                    return Err("同一 Session 仍有未收口的 TurnAttempt".to_string());
                }
            }
        }
        self.inner.store.begin_supervised_attempt(
            (&snapshot).into(),
            attempt_no,
            runtime_generation_id,
        )?;
        self.inner
            .current
            .lock()
            .map_err(|_| "Stream Supervisor 状态锁中毒".to_string())?
            .insert(
                history_session_id.to_string(),
                SupervisedTurn {
                    snapshot,
                    binding,
                    stream_failure_reported: false,
                    saw_visible_content: false,
                    next_source_seq: 0,
                    last_source_seq: 0,
                    seen_native_events: HashSet::new(),
                    native_session_id: None,
                    tool_stream_bytes: HashMap::new(),
                    presentation: TurnPresentationBuffer::default(),
                    budget: BudgetRuntimeState {
                        snapshot: budget_snapshot,
                        output_bytes: 0,
                        tool_output_bytes: HashMap::new(),
                        tool_count: 0,
                        repeat_digests: HashMap::new(),
                        started_at,
                        last_event_at: started_at,
                        exceeded: HashSet::new(),
                    },
                },
            );
        // 状态徽标实时刷新（#1 补缝，9/4）：新轮次落库即广播侧栏刷新。此前
        // idle→running 的起点无广播，侧栏「运行中」徽标要等首条引擎事件跨过
        // 状态边界才点亮（delta 合批/CLI 冷启动期间可达数秒盲区）。
        // 此处 prev 必为空或终态（上方未收口检查已挡掉并发轮），必然是真实边界。
        if let Some(app) = self.inner.app.as_ref() {
            let _ = app.emit("helm-sessions-changed", history_session_id);
        }
        self.spawn_budget_watchdog(history_session_id, turn_id, attempt_no);
        Ok(())
    }

    pub fn retry_attempt(
        &self,
        history_session_id: &str,
        turn_id: &str,
        attempt_no: u64,
        runtime_generation_id: &str,
        receipt: &str,
    ) -> Result<(), String> {
        let now = now_millis();
        let mut current = self
            .inner
            .current
            .lock()
            .map_err(|_| "Stream Supervisor 状态锁中毒".to_string())?;
        let turn = current
            .get_mut(history_session_id)
            .ok_or_else(|| "兼容恢复找不到进行中的 Turn".to_string())?;
        if turn.snapshot.turn_id != turn_id
            || turn.snapshot.status.is_terminal()
            || turn.binding.runtime_generation_id != runtime_generation_id
            || attempt_no != turn.binding.attempt_no.saturating_add(1)
        {
            return Err("兼容恢复的 TurnAttempt 身份或顺序不匹配".to_string());
        }
        self.inner.store.finish_turn_attempt(
            turn_id,
            turn.binding.attempt_no,
            "error",
            Some(receipt),
            now,
        )?;
        turn.snapshot.updated_at = now;
        self.inner.store.begin_supervised_attempt(
            (&turn.snapshot).into(),
            attempt_no,
            runtime_generation_id,
        )?;
        turn.binding.attempt_no = attempt_no;
        turn.stream_failure_reported = false;
        turn.saw_visible_content = false;
        turn.next_source_seq = 0;
        turn.last_source_seq = 0;
        turn.seen_native_events.clear();
        turn.budget.last_event_at = now;
        drop(current);
        self.spawn_budget_watchdog(history_session_id, turn_id, attempt_no);
        Ok(())
    }

    /// Compatibility entry point for older unit tests and legacy callers. A
    /// real production dispatch is upgraded by `begin_attempt` before events.
    pub fn begin(
        &self,
        history_session_id: &str,
        turn_id: &str,
        turn_epoch: u64,
        mode: &str,
        permission_profile: &str,
    ) {
        if self
            .inner
            .current
            .lock()
            .ok()
            .and_then(|current| current.get(history_session_id).cloned())
            .is_some_and(|turn| turn.snapshot.turn_id == turn_id)
        {
            return;
        }
        let _ = self.begin_attempt(
            history_session_id,
            turn_id,
            turn_epoch,
            mode,
            permission_profile,
            RuntimeOwnerRef::Session(history_session_id.to_string()),
            0,
            "legacy_runtime_generation",
        );
    }

    /// Adapter-facing entry point. Metadata is copied from the binding frozen
    /// by RuntimeRegistry; adapters do not allocate EventSeq or write history.
    pub async fn submit_event(
        &self,
        history_session_id: &str,
        turn_id: Option<&str>,
        turn_epoch: Option<u64>,
        event: AgentEvent,
    ) -> Result<bool, String> {
        let (binding, frozen_turn_id, frozen_epoch) = {
            let current = self
                .inner
                .current
                .lock()
                .map_err(|_| "Stream Supervisor 状态锁中毒".to_string())?;
            let Some(turn) = current.get(history_session_id) else {
                self.bump_diagnostic(|value| value.orphan += 1);
                return Ok(false);
            };
            (
                turn.binding.clone(),
                turn_id.unwrap_or(&turn.snapshot.turn_id).to_string(),
                turn_epoch.unwrap_or(turn.snapshot.turn_epoch),
            )
        };
        let ingress = self.inner.ingress.lock().await;
        let candidate = {
            let mut current = self
                .inner
                .current
                .lock()
                .map_err(|_| "Stream Supervisor 状态锁中毒".to_string())?;
            let Some(turn) = current.get_mut(history_session_id) else {
                self.bump_diagnostic(|value| value.orphan += 1);
                return Ok(false);
            };
            if turn.snapshot.turn_id != frozen_turn_id
                || turn.snapshot.turn_epoch != frozen_epoch
                || turn.binding.owner != binding.owner
                || turn.binding.attempt_no != binding.attempt_no
                || turn.binding.runtime_generation_id != binding.runtime_generation_id
            {
                self.bump_diagnostic(|value| value.stale += 1);
                return Ok(false);
            }
            if turn.stream_failure_reported
                && !matches!(
                    &event,
                    AgentEvent::TurnComplete { .. }
                        | AgentEvent::Error {
                            recoverable: false,
                            ..
                        }
                )
            {
                self.bump_diagnostic(|value| value.invalid_transition += 1);
                return Ok(false);
            }
            turn.next_source_seq = turn.next_source_seq.saturating_add(1);
            EngineEventCandidate {
                owner: binding.owner.clone(),
                history_session_id: history_session_id.to_string(),
                turn_id: frozen_turn_id.clone(),
                turn_epoch: frozen_epoch,
                attempt_no: binding.attempt_no,
                runtime_generation_id: binding.runtime_generation_id.clone(),
                source_seq: turn.next_source_seq,
                native_event_id: native_event_identity(&event),
                observed_at: now_millis(),
                event,
            }
        };
        let report_failure = || {
            self.report_stream_failure(history_session_id, &frozen_turn_id, frozen_epoch, &binding)
        };
        let reply = self.enqueue(candidate).await;
        drop(ingress);
        let reply = match reply {
            Ok(reply) => reply,
            Err(error) => {
                report_failure();
                return Err(error);
            }
        };
        let disposition = reply.await.map_err(|_| {
            report_failure();
            format!("[stream_worker_closed] {STREAM_FAILURE_MESSAGE}")
        })?;
        match disposition {
            CandidateDisposition::PersistenceFailed => {
                report_failure();
                Err(format!(
                    "[stream_persistence_failed] {STREAM_FAILURE_MESSAGE}"
                ))
            }
            CandidateDisposition::Accepted => Ok(true),
            _ => Ok(false),
        }
    }

    async fn enqueue(
        &self,
        candidate: EngineEventCandidate,
    ) -> Result<oneshot::Receiver<CandidateDisposition>, String> {
        let (reply, received) = oneshot::channel();
        let queued = QueuedCandidate { candidate, reply };
        match self.inner.queue.try_send(queued) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(queued)) => {
                self.bump_diagnostic(|value| value.backpressure += 1);
                self.inner
                    .queue
                    .send(queued)
                    .await
                    .map_err(|_| "[stream_worker_closed] 事件持久化队列已关闭".to_string())?;
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                return Err("[stream_worker_closed] 事件持久化队列已关闭".to_string());
            }
        }
        Ok(received)
    }

    pub fn process_candidate(&self, candidate: EngineEventCandidate) -> CandidateDisposition {
        let _processing = self
            .inner
            .processing
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut event = crate::redaction::sanitize_agent_event(&candidate.event);
        let (
            snapshot,
            native_identity,
            mut budget,
            tool_stream_update,
            presentations,
            tool_outputs,
            terminal_fallback,
        ) = {
            let mut current = match self.inner.current.lock() {
                Ok(current) => current,
                Err(poisoned) => {
                    drop(poisoned);
                    self.bump_diagnostic(|value| value.persistence_failed += 1);
                    self.report_stream_failure(
                        &candidate.history_session_id,
                        &candidate.turn_id,
                        candidate.turn_epoch,
                        &AttemptBinding {
                            owner: candidate.owner.clone(),
                            attempt_no: candidate.attempt_no,
                            runtime_generation_id: candidate.runtime_generation_id.clone(),
                        },
                    );
                    return CandidateDisposition::PersistenceFailed;
                }
            };
            let Some(turn) = current.get_mut(&candidate.history_session_id) else {
                drop(current);
                self.reject_candidate(&candidate, "orphan", CandidateDisposition::Orphan);
                return CandidateDisposition::Orphan;
            };
            if turn.binding.owner != candidate.owner
                || turn.snapshot.turn_id != candidate.turn_id
                || turn.snapshot.turn_epoch != candidate.turn_epoch
                || turn.binding.attempt_no != candidate.attempt_no
                || turn.binding.runtime_generation_id != candidate.runtime_generation_id
            {
                drop(current);
                self.reject_candidate(&candidate, "stale_identity", CandidateDisposition::Stale);
                return CandidateDisposition::Stale;
            }
            if candidate.source_seq < turn.last_source_seq {
                drop(current);
                self.reject_candidate(
                    &candidate,
                    "out_of_order_source_seq",
                    CandidateDisposition::Stale,
                );
                return CandidateDisposition::Stale;
            }
            if candidate.source_seq == turn.last_source_seq {
                drop(current);
                self.reject_candidate(
                    &candidate,
                    "duplicate_source_seq",
                    CandidateDisposition::Duplicate,
                );
                return CandidateDisposition::Duplicate;
            }
            if let Some(identity) = candidate.native_event_id.as_ref() {
                if turn.seen_native_events.contains(identity) {
                    drop(current);
                    self.reject_candidate(
                        &candidate,
                        "duplicate_native_event",
                        CandidateDisposition::Duplicate,
                    );
                    return CandidateDisposition::Duplicate;
                }
            }
            if let AgentEvent::ToolProgress { id, .. } = &event {
                if turn
                    .seen_native_events
                    .contains(&format!("tool_result:{id}"))
                {
                    drop(current);
                    self.reject_candidate(
                        &candidate,
                        "progress_after_tool_result",
                        CandidateDisposition::Stale,
                    );
                    return CandidateDisposition::Stale;
                }
            }
            let native_identity = match &event {
                AgentEvent::SessionStarted { session_id, .. } => {
                    if turn
                        .native_session_id
                        .as_ref()
                        .is_some_and(|existing| existing != session_id)
                    {
                        drop(current);
                        self.reject_candidate(
                            &candidate,
                            "native_session_rebound",
                            CandidateDisposition::InvalidTransition,
                        );
                        return CandidateDisposition::InvalidTransition;
                    }
                    Some(session_id.clone())
                }
                _ => None,
            };
            if turn.snapshot.status.is_terminal() {
                drop(current);
                self.reject_candidate(
                    &candidate,
                    "late_after_terminal",
                    CandidateDisposition::Stale,
                );
                return CandidateDisposition::Stale;
            }
            if turn.stream_failure_reported
                && !matches!(
                    &event,
                    AgentEvent::TurnComplete { .. }
                        | AgentEvent::Error {
                            recoverable: false,
                            ..
                        }
                )
            {
                drop(current);
                self.reject_candidate(
                    &candidate,
                    "stream_failed",
                    CandidateDisposition::InvalidTransition,
                );
                return CandidateDisposition::InvalidTransition;
            }
            if !valid_transition(turn.snapshot.status, &event) {
                drop(current);
                self.reject_candidate(
                    &candidate,
                    "invalid_transition",
                    CandidateDisposition::InvalidTransition,
                );
                return CandidateDisposition::InvalidTransition;
            }
            let mut snapshot = turn.snapshot.clone();
            snapshot.event_seq = snapshot.event_seq.saturating_add(1);
            snapshot.updated_at = candidate.observed_at;
            apply_transition(&mut snapshot, &event);
            let tool_stream_update = if let AgentEvent::ToolProgress { id, chunk, .. } = &mut event
            {
                let previous = turn.tool_stream_bytes.get(id).copied().unwrap_or_default();
                *chunk = crate::output_limits::bounded_progress(chunk, previous);
                Some((id.clone(), previous.saturating_add(chunk.len() as u64)))
            } else {
                None
            };
            if let AgentEvent::ToolResult {
                id,
                output,
                diff,
                has_output,
                ..
            } = &mut event
            {
                let fallback = output
                    .as_deref()
                    .or_else(|| turn.presentation.tool_output(id));
                let (bounded_output, bounded_diff) =
                    crate::output_limits::bounded_tool_result(fallback, diff.as_ref());
                if has_output.is_none()
                    && bounded_output
                        .as_ref()
                        .is_some_and(|value| !value.is_empty())
                {
                    *has_output = Some(true);
                }
                *output = bounded_output;
                *diff = bounded_diff;
            }
            let terminal = snapshot.status.is_terminal();
            // 可见内容记账：只有真正会被用户看到的事件才算「本轮有产出」。
            // 终态事件本身（TurnComplete）不算，否则每条轮次都会自带一次记账。
            if !terminal && is_user_visible_event(&event) {
                turn.saw_visible_content = true;
            }
            // 兜底：终态时若用户一个字都没看到，补一条可读终态，避免静默收尾。
            // 事件序号先给兜底 Error，终态事件顺延一位，保证前端先看到原因再收到收尾。
            let terminal_fallback = if terminal && !turn.saw_visible_content {
                no_output_terminal_note(&snapshot).map(|message| {
                    let fallback_seq = snapshot.event_seq;
                    snapshot.event_seq = snapshot.event_seq.saturating_add(1);
                    // 同时写进 terminal_reason：重开会话也能从轮次记录里查到原因。
                    snapshot.terminal_reason = Some(message.clone());
                    (
                        fallback_seq,
                        AgentEvent::Error {
                            session_id: turn.native_session_id.clone(),
                            message,
                            recoverable: false,
                            kind: Some("turn_no_output".to_string()),
                            stalled_kind: None,
                        },
                    )
                })
            } else {
                None
            };
            let presentations = turn.presentation.records_for_event(
                &candidate.turn_id,
                snapshot.event_seq,
                candidate.observed_at,
                &event,
                terminal,
            );
            let tool_outputs = if terminal {
                turn.presentation.pending_tool_outputs()
            } else {
                Vec::new()
            };
            (
                snapshot,
                native_identity,
                turn.budget.clone(),
                tool_stream_update,
                presentations,
                tool_outputs,
                terminal_fallback,
            )
        };
        let event_seq = snapshot.event_seq;
        let terminal_status = snapshot.status;
        budget.last_event_at = candidate.observed_at;
        let budget_triggers = apply_budget_event(&mut budget, &candidate.event);

        let event_kind = event_kind(&event);
        // 性能：digest 只服务于落库（stream_boundary 与终态 finalize），delta 事件不落库，
        // 不必为每条 delta 做一次全量序列化 + SHA-256。合批后约 30 条/秒，省下的量可观。
        let event_digest = if !is_boundary(&event) {
            String::new()
        } else {
            crate::turn_start::digest_json(&event)
                .unwrap_or_else(|_| "sha256:unavailable".to_string())
        };
        let persisted = if snapshot.status.is_terminal() {
            self.inner
                .store
                .finalize_supervised_turn_with_presentations(
                    &snapshot,
                    candidate.attempt_no,
                    &candidate.runtime_generation_id,
                    event_kind,
                    &event_digest,
                    &presentations,
                    &tool_outputs,
                )
        } else if is_boundary(&event) {
            self.inner.store.record_supervised_boundary(
                &snapshot,
                candidate.attempt_no,
                &candidate.runtime_generation_id,
                event_kind,
                &event_digest,
                &event,
                &presentations,
            )
        } else {
            Ok(())
        };
        if let Err(error) = persisted {
            self.bump_diagnostic(|value| value.persistence_failed += 1);
            let _ = self.inner.store.record_stream_diagnostic(
                Some(&candidate),
                event_kind,
                "persistence_failed",
                Some(&error),
            );
            if let Some(app) = self.inner.app.as_ref() {
                crate::adapter::log_runtime_event(
                    app,
                    "stream-persistence-failed",
                    &crate::redaction::redact_text(&error),
                );
            }
            self.report_stream_failure(
                &candidate.history_session_id,
                &candidate.turn_id,
                candidate.turn_epoch,
                &AttemptBinding {
                    owner: candidate.owner.clone(),
                    attempt_no: candidate.attempt_no,
                    runtime_generation_id: candidate.runtime_generation_id.clone(),
                },
            );
            return CandidateDisposition::PersistenceFailed;
        }

        if let Ok(mut current) = self.inner.current.lock() {
            if let Some(turn) = current.get_mut(&candidate.history_session_id) {
                if turn.snapshot.turn_id == candidate.turn_id
                    && turn.binding.attempt_no == candidate.attempt_no
                    && turn.binding.runtime_generation_id == candidate.runtime_generation_id
                {
                    let prev_status = turn.snapshot.status;
                    turn.last_source_seq = candidate.source_seq;
                    if let Some(identity) = candidate.native_event_id.as_ref() {
                        turn.seen_native_events.insert(identity.clone());
                    }
                    if native_identity.is_some() {
                        turn.native_session_id = native_identity;
                    }
                    let new_status = snapshot.status;
                    turn.budget = budget;
                    if let Some((tool_id, bytes)) = tool_stream_update {
                        turn.tool_stream_bytes.insert(tool_id, bytes);
                    }
                    turn.presentation.apply(
                        &event,
                        event_seq,
                        candidate.observed_at,
                        new_status.is_terminal(),
                    );
                    if new_status.is_terminal() {
                        turn.tool_stream_bytes.clear();
                    }
                    turn.snapshot = snapshot;
                    // 状态徽标实时刷新（#1）：lastTurnStatus 实际变更时广播侧栏刷新事件，
                    // 仅在该边界事件改变状态时才发，避免每个事件都触发 Rail 全量重读。
                    if prev_status != new_status {
                        if let Some(app) = self.inner.app.as_ref() {
                            let _ =
                                app.emit("helm-sessions-changed", &candidate.history_session_id);
                        }
                    }
                }
            }
        }

        self.bump_diagnostic(|value| value.accepted += 1);
        if matches!(
            event,
            AgentEvent::Error {
                recoverable: false,
                ..
            } | AgentEvent::TurnComplete {
                stop_reason: StopReason::Error,
                ..
            }
        ) {
            self.invalidate_runtime_probes(&candidate.history_session_id);
        }
        for trigger in budget_triggers {
            self.record_budget_trigger(&candidate, &trigger);
        }
        // 先发兜底原因再发终态事件：前端按顺序收到「为什么结束」+「结束了」，
        // 不会出现「已完成但一片空白」的静默收尾。
        if let Some((fallback_seq, fallback_event)) = terminal_fallback.as_ref() {
            if let AgentEvent::Error { message, .. } = fallback_event {
                if let Some(app) = self.inner.app.as_ref() {
                    crate::adapter::log_runtime_event(
                        app,
                        "turn-no-output",
                        &format!(
                            "history={} turn={} status={:?} note={}",
                            candidate.history_session_id, candidate.turn_id, terminal_status, message
                        ),
                    );
                }
            }
            self.publish(&candidate, *fallback_seq, fallback_event);
        }
        self.publish(&candidate, event_seq, &event);
        CandidateDisposition::Accepted
    }

    /// Legacy test helper. EventSeq is now allocated by the Supervisor; the
    /// supplied value is treated as the adapter/source sequence only.
    pub fn accept_event(
        &self,
        history_session_id: &str,
        turn_id: Option<&str>,
        turn_epoch: Option<u64>,
        source_seq: u64,
        event: &AgentEvent,
    ) -> bool {
        let binding = self
            .inner
            .current
            .lock()
            .ok()
            .and_then(|current| current.get(history_session_id).cloned());
        let Some(turn) = binding else {
            return turn_id.is_none();
        };
        self.process_candidate(EngineEventCandidate {
            owner: turn.binding.owner,
            history_session_id: history_session_id.to_string(),
            turn_id: turn_id.unwrap_or(&turn.snapshot.turn_id).to_string(),
            turn_epoch: turn_epoch.unwrap_or(turn.snapshot.turn_epoch),
            attempt_no: turn.binding.attempt_no,
            runtime_generation_id: turn.binding.runtime_generation_id,
            source_seq,
            native_event_id: native_event_identity(event),
            observed_at: now_millis(),
            event: event.clone(),
        }) == CandidateDisposition::Accepted
    }

    pub fn mark_stalled(
        &self,
        history_session_id: &str,
        turn_id: &str,
        turn_epoch: u64,
        source_seq: u64,
    ) {
        let session_id = history_session_id.to_string();
        let _ = self.accept_event(
            history_session_id,
            Some(turn_id),
            Some(turn_epoch),
            source_seq,
            &AgentEvent::TurnStage {
                session_id,
                stage: TurnStage::Stalled,
                ts: now_millis(),
                engine_reported_ttft_ms: None,
                retry_attempt: None,
            },
        );
    }

    pub fn snapshot(&self, history_session_id: &str) -> Result<Option<TurnSnapshot>, String> {
        if let Ok(current) = self.inner.current.lock() {
            if let Some(turn) = current.get(history_session_id) {
                return Ok(Some(turn.snapshot.clone()));
            }
        }
        self.inner
            .store
            .load_turn_snapshot(history_session_id)
            .map(|record| record.map(snapshot_from_record))
    }

    pub fn diagnostics(&self) -> StreamDiagnostics {
        self.inner
            .diagnostics
            .lock()
            .map(|value| value.clone())
            .unwrap_or_default()
    }

    fn reject_candidate(
        &self,
        candidate: &EngineEventCandidate,
        reason: &str,
        disposition: CandidateDisposition,
    ) {
        self.bump_diagnostic(|value| match disposition {
            CandidateDisposition::Duplicate => value.duplicate += 1,
            CandidateDisposition::Stale => value.stale += 1,
            CandidateDisposition::Orphan => value.orphan += 1,
            CandidateDisposition::InvalidTransition => value.invalid_transition += 1,
            CandidateDisposition::PersistenceFailed => value.persistence_failed += 1,
            CandidateDisposition::Accepted => value.accepted += 1,
        });
        let _ = self.inner.store.record_stream_diagnostic(
            Some(candidate),
            event_kind(&candidate.event),
            reason,
            None,
        );
    }

    fn bump_diagnostic(&self, update: impl FnOnce(&mut StreamDiagnostics)) {
        if let Ok(mut diagnostics) = self.inner.diagnostics.lock() {
            update(&mut diagnostics);
        }
    }

    fn report_stream_failure(
        &self,
        history_session_id: &str,
        turn_id: &str,
        turn_epoch: u64,
        binding: &AttemptBinding,
    ) -> bool {
        let mut current = self
            .inner
            .current
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(turn) = current.get_mut(history_session_id) else {
            return false;
        };
        if turn.snapshot.turn_id != turn_id
            || turn.snapshot.turn_epoch != turn_epoch
            || turn.binding.owner != binding.owner
            || turn.binding.attempt_no != binding.attempt_no
            || turn.binding.runtime_generation_id != binding.runtime_generation_id
            || turn.snapshot.status.is_terminal()
            || turn.stream_failure_reported
        {
            return false;
        }
        turn.stream_failure_reported = true;
        if let Some(app) = self.inner.app.as_ref() {
            let failure = TurnStreamFailure {
                history_id: history_session_id.to_string(),
                turn_id: turn_id.to_string(),
                turn_epoch,
                attempt_no: binding.attempt_no,
                runtime_generation_id: binding.runtime_generation_id.clone(),
                message: STREAM_FAILURE_MESSAGE.to_string(),
            };
            if app.emit(STREAM_FAILURE_EVENT_NAME, &failure).is_err() {
                crate::adapter::log_runtime_event(
                    app,
                    "stream-failure-notification-failed",
                    "消息流故障通知发送失败",
                );
            }
        }
        drop(current);
        self.interrupt_after_delivery_failure(&binding.owner);
        true
    }

    fn interrupt_after_delivery_failure(&self, owner: &RuntimeOwnerRef) {
        let Some(app) = self.inner.app.as_ref() else {
            return;
        };
        let Some(registry) = app.try_state::<crate::runtime_registry::RuntimeRegistry>() else {
            return;
        };
        let registry = registry.inner().clone();
        let owner = owner.clone();
        tauri::async_runtime::spawn(async move {
            let _ = registry.interrupt(&owner).await;
        });
    }

    fn invalidate_runtime_probes(&self, history_session_id: &str) {
        let Some(app) = self.inner.app.as_ref() else {
            return;
        };
        let Some(profiles) =
            app.try_state::<crate::subscription_profiles::SubscriptionProfileStore>()
        else {
            return;
        };
        let Some(registry) =
            app.try_state::<crate::capability_registry::EngineCapabilityRegistry>()
        else {
            return;
        };
        if let Ok(engine) = self.inner.store.engine_for_session(history_session_id) {
            let _ = crate::settings::invalidate_engine_probes(&profiles, &registry, &engine);
        }
    }

    fn spawn_budget_watchdog(&self, history_session_id: &str, turn_id: &str, attempt_no: u64) {
        if self.inner.app.is_none() {
            return;
        }
        let supervisor = self.clone();
        let history_session_id = history_session_id.to_string();
        let turn_id = turn_id.to_string();
        tauri::async_runtime::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                let trigger = {
                    let Ok(mut current) = supervisor.inner.current.lock() else {
                        return;
                    };
                    let Some(turn) = current.get_mut(&history_session_id) else {
                        return;
                    };
                    if turn.snapshot.turn_id != turn_id
                        || turn.binding.attempt_no != attempt_no
                        || turn.snapshot.status.is_terminal()
                        || turn.stream_failure_reported
                    {
                        return;
                    }
                    watchdog_budget_trigger(&mut turn.budget, now_millis())
                };
                if let Some(trigger) = trigger {
                    let candidate = EngineEventCandidate {
                        owner: RuntimeOwnerRef::Session(history_session_id.clone()),
                        history_session_id: history_session_id.clone(),
                        turn_id: turn_id.clone(),
                        turn_epoch: 0,
                        attempt_no,
                        runtime_generation_id: String::new(),
                        source_seq: 0,
                        native_event_id: None,
                        observed_at: now_millis(),
                        event: AgentEvent::Error {
                            session_id: Some(history_session_id.clone()),
                            message: format!("预算超限：{}", trigger.dimension.as_str()),
                            recoverable: false,
                            kind: Some("budget_exceeded".to_string()),
                            stalled_kind: None,
                        },
                    };
                    supervisor.record_budget_trigger(&candidate, &trigger);
                    return;
                }
            }
        });
    }

    fn record_budget_trigger(&self, candidate: &EngineEventCandidate, trigger: &BudgetTrigger) {
        let action = if trigger.interrupt {
            "interrupt"
        } else {
            "post_facto"
        };
        let _ = self.inner.store.record_turn_budget_fact(
            &candidate.turn_id,
            candidate.attempt_no,
            trigger.dimension,
            trigger.observed,
            trigger.limit,
            trigger.enforcement_mode,
            action,
        );
        if !trigger.interrupt {
            return;
        }
        let Some(app) = self.inner.app.as_ref() else {
            return;
        };
        let Some(registry) = app.try_state::<crate::runtime_registry::RuntimeRegistry>() else {
            return;
        };
        let registry = registry.inner().clone();
        let owner = candidate.owner.clone();
        tauri::async_runtime::spawn(async move {
            let _ = registry.interrupt(&owner).await;
        });
    }

    fn publish(&self, candidate: &EngineEventCandidate, event_seq: u64, event: &AgentEvent) {
        if matches!(event, AgentEvent::ToolProgress { chunk, .. } if chunk.is_empty()) {
            return;
        }
        let Some(app) = self.inner.app.as_ref() else {
            return;
        };
        let current = self
            .inner
            .current
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(turn) = current.get(&candidate.history_session_id) else {
            return;
        };
        if turn.snapshot.turn_id != candidate.turn_id
            || turn.snapshot.turn_epoch != candidate.turn_epoch
            || turn.binding.owner != candidate.owner
            || turn.binding.attempt_no != candidate.attempt_no
            || turn.binding.runtime_generation_id != candidate.runtime_generation_id
            || (turn.stream_failure_reported
                && !matches!(
                    event,
                    AgentEvent::TurnComplete { .. }
                        | AgentEvent::Error {
                            recoverable: false,
                            ..
                        }
                ))
        {
            return;
        }
        let _ = app.emit(
            EVENT_NAME,
            &AgentEventEnvelope {
                history_id: &candidate.history_session_id,
                event_seq,
                turn_id: &candidate.turn_id,
                turn_epoch: candidate.turn_epoch,
                attempt_no: candidate.attempt_no,
                runtime_generation_id: &candidate.runtime_generation_id,
                event,
            },
        );
        drop(current);
        if matches!(event, AgentEvent::TokenUsage { .. }) {
            crate::tray::refresh_usage(app);
        }
        if let AgentEvent::ApprovalRequest { action, .. } = event {
            use tauri_plugin_notification::NotificationExt;
            let _ = app
                .notification()
                .builder()
                .title("Helm 等待审批")
                .body(format!("Agent 请求执行「{action}」，请回到会话处理。"))
                .show();
        }
        if matches!(event, AgentEvent::TurnComplete { .. }) {
            crate::titler::maybe_generate_title(app, &candidate.history_session_id);
        }
    }
}

fn apply_transition(snapshot: &mut TurnSnapshot, event: &AgentEvent) {
    match event {
        AgentEvent::ApprovalRequest { .. } => snapshot.status = TurnStatus::WaitingApproval,
        AgentEvent::TurnComplete { stop_reason, .. } => {
            // 先行的 Error 事件已把真实失败原因（脱敏后）写进 terminal_reason 并置 Failed；
            // 裸 stop reason 不得覆盖丢细节，否则重开会话只剩一行 "error"。
            let already_failed = snapshot.status == TurnStatus::Failed;
            snapshot.status = match stop_reason {
                StopReason::End => TurnStatus::Succeeded,
                StopReason::Interrupted => TurnStatus::Interrupted,
                StopReason::Error => TurnStatus::Failed,
            };
            match stop_reason {
                StopReason::End => snapshot.terminal_reason = Some("end".to_string()),
                StopReason::Interrupted => {
                    snapshot.terminal_reason = Some("interrupted".to_string());
                }
                StopReason::Error => {
                    if !already_failed || snapshot.terminal_reason.is_none() {
                        snapshot.terminal_reason = Some("error".to_string());
                    }
                }
            }
            snapshot.recoverable = matches!(stop_reason, StopReason::Interrupted);
        }
        AgentEvent::Error {
            recoverable: false,
            message,
            ..
        } => {
            snapshot.status = TurnStatus::Failed;
            snapshot.terminal_reason = Some(crate::redaction::redact_text(message));
            snapshot.recoverable = false;
        }
        AgentEvent::TurnStage { stage, .. } if matches!(stage, TurnStage::Stalled) => {
            snapshot.status = TurnStatus::Stalled;
        }
        AgentEvent::TurnStage { .. }
        | AgentEvent::MessageDelta { .. }
        | AgentEvent::MessageComplete { .. }
        | AgentEvent::ThinkingDelta { .. }
        | AgentEvent::ThinkingComplete { .. }
        | AgentEvent::ToolCall { .. }
        | AgentEvent::ToolProgress { .. }
        | AgentEvent::ToolResult { .. }
        | AgentEvent::PlanUpdate { .. }
        | AgentEvent::TokenUsage { .. }
        | AgentEvent::ContextUsage { .. }
        | AgentEvent::ContextCompaction { .. }
        | AgentEvent::SessionStarted { .. }
        | AgentEvent::Error { .. } => snapshot.status = TurnStatus::Running,
    }
}

fn apply_budget_event(state: &mut BudgetRuntimeState, event: &AgentEvent) -> Vec<BudgetTrigger> {
    match event {
        AgentEvent::MessageDelta { text, .. } | AgentEvent::ThinkingDelta { text, .. } => {
            state.output_bytes = state
                .output_bytes
                .saturating_add(text.as_bytes().len() as u64);
        }
        AgentEvent::ToolProgress { id, chunk, .. } => {
            let size = chunk.len() as u64;
            state.output_bytes = state.output_bytes.saturating_add(size);
            let previous = state.tool_output_bytes.entry(id.clone()).or_default();
            *previous = previous.saturating_add(size);
        }
        AgentEvent::ToolResult {
            id, output, diff, ..
        } => {
            let size = crate::output_limits::tool_result_bytes(output.as_deref(), diff.as_ref());
            let previous = state.tool_output_bytes.entry(id.clone()).or_default();
            state.output_bytes = state
                .output_bytes
                .saturating_add(size.saturating_sub(*previous));
            *previous = (*previous).max(size);
        }
        AgentEvent::MessageComplete { text, .. } | AgentEvent::ThinkingComplete { text, .. } => {
            if state.output_bytes == 0 {
                state.output_bytes = text.as_bytes().len() as u64;
            }
        }
        AgentEvent::ToolCall { name, input, .. } => {
            state.tool_count = state.tool_count.saturating_add(1);
            if let Ok(digest) = crate::turn_start::digest_json(&(name, input)) {
                *state.repeat_digests.entry(digest).or_insert(0) += 1;
            }
        }
        _ => {}
    }
    let mut triggers = Vec::new();
    push_budget_trigger(
        state,
        &mut triggers,
        BudgetDimension::OutputBytes,
        state.output_bytes,
    );
    push_budget_trigger(
        state,
        &mut triggers,
        BudgetDimension::ToolCount,
        state.tool_count,
    );
    let repeats = state
        .repeat_digests
        .values()
        .copied()
        .max()
        .unwrap_or_default();
    push_budget_trigger(state, &mut triggers, BudgetDimension::RepeatDigest, repeats);
    if let AgentEvent::TokenUsage {
        input_tokens,
        output_tokens,
        cost_usd,
        ..
    } = event
    {
        push_budget_trigger(
            state,
            &mut triggers,
            BudgetDimension::Token,
            input_tokens.saturating_add(*output_tokens),
        );
        push_budget_trigger(
            state,
            &mut triggers,
            BudgetDimension::CostMicrousd,
            (cost_usd.max(0.0) * 1_000_000.0).round() as u64,
        );
    }
    if let AgentEvent::ContextUsage {
        context_tokens,
        context_window: Some(context_window),
        ..
    } = event
    {
        let ratio = if *context_window == 0 {
            0
        } else {
            context_tokens.saturating_mul(1000) / context_window
        };
        push_budget_trigger(
            state,
            &mut triggers,
            BudgetDimension::ContextRatioPermille,
            ratio,
        );
    }
    triggers
}

fn watchdog_budget_trigger(
    state: &mut BudgetRuntimeState,
    observed_at: i64,
) -> Option<BudgetTrigger> {
    let wall = observed_at.saturating_sub(state.started_at).max(0) as u64;
    let idle = observed_at.saturating_sub(state.last_event_at).max(0) as u64;
    for (dimension, observed) in [
        (BudgetDimension::WallClockMs, wall),
        (BudgetDimension::IdleMs, idle),
    ] {
        let Some(limit) = state.snapshot.limit(dimension).cloned() else {
            continue;
        };
        if observed > limit.limit && state.exceeded.insert(dimension) {
            return Some(BudgetTrigger {
                dimension,
                observed,
                limit: limit.limit,
                enforcement_mode: limit.enforcement_mode,
                interrupt: limit.enforcement_mode == BudgetEnforcementMode::Streaming,
            });
        }
    }
    None
}

fn push_budget_trigger(
    state: &mut BudgetRuntimeState,
    triggers: &mut Vec<BudgetTrigger>,
    dimension: BudgetDimension,
    observed: u64,
) {
    let Some(limit) = state.snapshot.limit(dimension).cloned() else {
        return;
    };
    if observed <= limit.limit || !state.exceeded.insert(dimension) {
        return;
    }
    triggers.push(BudgetTrigger {
        dimension,
        observed,
        limit: limit.limit,
        enforcement_mode: limit.enforcement_mode,
        interrupt: limit.enforcement_mode == BudgetEnforcementMode::Streaming,
    });
}

fn valid_transition(status: TurnStatus, event: &AgentEvent) -> bool {
    if status.is_terminal() {
        return false;
    }
    let _ = event;
    true
}

fn event_kind(event: &AgentEvent) -> &'static str {
    match event {
        AgentEvent::SessionStarted { .. } => "session_started",
        AgentEvent::MessageDelta { .. } => "message_delta",
        AgentEvent::MessageComplete { .. } => "message_complete",
        AgentEvent::ThinkingDelta { .. } => "thinking_delta",
        AgentEvent::ThinkingComplete { .. } => "thinking_complete",
        AgentEvent::TurnStage { .. } => "turn_stage",
        AgentEvent::ToolCall { .. } => "tool_call",
        AgentEvent::ToolProgress { .. } => "tool_progress",
        AgentEvent::ToolResult { .. } => "tool_result",
        AgentEvent::ApprovalRequest { .. } => "approval_request",
        AgentEvent::PlanUpdate { .. } => "plan_update",
        AgentEvent::TokenUsage { .. } => "token_usage",
        AgentEvent::ContextUsage { .. } => "context_usage",
        AgentEvent::ContextCompaction { .. } => "context_compaction",
        AgentEvent::TurnComplete { .. } => "turn_complete",
        AgentEvent::Error { .. } => "error",
    }
}

/// 「用户看得见」的事件类型：正文、思考、工具、审批、计划、错误。
/// 纯控制面事件（session_started / turn_stage / usage / turn_complete）不算，
/// 它们只在界面上表现为状态，不构成「本轮有产出」。
fn is_user_visible_event(event: &AgentEvent) -> bool {
    matches!(
        event,
        AgentEvent::MessageDelta { .. }
            | AgentEvent::MessageComplete { .. }
            | AgentEvent::ThinkingDelta { .. }
            | AgentEvent::ThinkingComplete { .. }
            | AgentEvent::ToolCall { .. }
            | AgentEvent::ToolProgress { .. }
            | AgentEvent::ToolResult { .. }
            | AgentEvent::ApprovalRequest { .. }
            | AgentEvent::PlanUpdate { .. }
            | AgentEvent::Error { .. }
    )
}

/// 终态时「用户一个字都没看到」的兜底说明。返回 None 表示无需补发
/// （已有真实原因，或本就不该打扰用户）。
fn no_output_terminal_note(snapshot: &TurnSnapshot) -> Option<String> {
    match snapshot.status {
        TurnStatus::Interrupted => Some(
            "本轮已被中断：进程在返回任何内容前就结束了（未收到模型输出）。\
             可重新发送这条消息。"
                .to_string(),
        ),
        TurnStatus::Failed => {
            let reason = snapshot.terminal_reason.as_deref().unwrap_or_default().trim();
            if reason.is_empty() {
                Some("本轮失败，但引擎没有返回具体原因，且结束前没有任何输出。".to_string())
            } else {
                None
            }
        }
        TurnStatus::Succeeded => {
            Some("本轮已结束，但模型没有返回任何内容（无正文、无工具调用、无错误）。".to_string())
        }
        TurnStatus::Running | TurnStatus::WaitingApproval | TurnStatus::Stalled => None,
    }
}

fn is_boundary(event: &AgentEvent) -> bool {
    !matches!(
        event,
        AgentEvent::MessageDelta { .. }
            | AgentEvent::ThinkingDelta { .. }
            | AgentEvent::ToolProgress { .. }
    )
}

fn native_event_identity(event: &AgentEvent) -> Option<String> {
    match event {
        AgentEvent::SessionStarted { session_id, .. } => Some(format!("session:{session_id}")),
        AgentEvent::ToolCall { id, .. } => Some(format!("tool_call:{id}")),
        AgentEvent::ToolResult { id, .. } => Some(format!("tool_result:{id}")),
        AgentEvent::ApprovalRequest { id, .. } => Some(format!("approval:{id}")),
        AgentEvent::TurnComplete { .. } => Some("turn_complete".to_string()),
        _ => None,
    }
}

fn snapshot_from_record(record: TurnSnapshotRecord) -> TurnSnapshot {
    TurnSnapshot {
        history_session_id: record.history_session_id,
        turn_id: record.turn_id,
        turn_epoch: record.turn_epoch,
        status: record.status,
        terminal_reason: record.terminal_reason,
        recoverable: record.recoverable,
        event_seq: record.event_seq,
        updated_at: record.updated_at,
        mode: record.mode,
        permission_profile: record.permission_profile,
        started_at: record.started_at,
    }
}

impl From<&TurnSnapshot> for TurnSnapshotRecord {
    fn from(snapshot: &TurnSnapshot) -> Self {
        Self {
            history_session_id: snapshot.history_session_id.clone(),
            turn_id: snapshot.turn_id.clone(),
            turn_epoch: snapshot.turn_epoch,
            status: snapshot.status,
            terminal_reason: snapshot.terminal_reason.clone(),
            recoverable: snapshot.recoverable,
            event_seq: snapshot.event_seq,
            updated_at: snapshot.updated_at,
            mode: snapshot.mode.clone(),
            permission_profile: snapshot.permission_profile.clone(),
            started_at: snapshot.started_at,
        }
    }
}

fn normalize_profile(profile: &str) -> String {
    match profile {
        "auto" | "full_access" => profile.to_string(),
        _ => "standard".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability_registry::{CapabilityIdentity, CapabilitySet, EngineCapabilitySnapshot};
    use crate::protocol::{EngineId, Role};
    use crate::reasoning::ReasoningEffort;
    use crate::runtime_registry::RuntimeGeneration;
    use crate::turn_start::{PricingBasisSnapshot, TurnExecutionSpec, TurnStartCommand};

    fn store(name: &str) -> SessionHistoryStore {
        SessionHistoryStore::new(std::env::temp_dir().join(format!(
            "helm-turn-supervisor-{name}-{}.sqlite",
            now_millis()
        )))
    }

    fn supervisor(name: &str) -> (SessionHistoryStore, TurnSupervisor) {
        let history = store(name);
        history
            .create_session(crate::sessions::NewSessionRecord {
                id: "history".into(),
                engine: EngineId::ClaudeCode,
                model: "model".into(),
                cwd: "D:/repo".into(),
                created_at: now_millis(),
            })
            .unwrap();
        let supervisor = TurnSupervisor::new(history.clone());
        supervisor.begin("history", "turn-1", 1, "build", "auto");
        (history, supervisor)
    }

    fn prepared_supervisor(
        history: &SessionHistoryStore,
    ) -> (TurnSupervisor, TurnExecutionSpec, RuntimeGeneration) {
        let created_at = now_millis();
        history
            .create_session(crate::sessions::NewSessionRecord {
                id: "history".into(),
                engine: EngineId::Codex,
                model: "model".into(),
                cwd: "D:/repo".into(),
                created_at,
            })
            .unwrap();
        let capability = EngineCapabilitySnapshot {
            id: "capability-fixture".into(),
            identity: CapabilityIdentity {
                engine_id: "codex".into(),
                adapter_version: "test".into(),
                binary_identity: "test-binary".into(),
                engine_profile_digest: "sha256:engine".into(),
                provider_launch_profile_ref: "provider:fixture:api".into(),
                provider_launch_profile_digest: "sha256:provider".into(),
                launch_profile_identity: "sha256:launch".into(),
                model_capability_key: "model".into(),
            },
            capabilities: CapabilitySet::unknown("test_fixture"),
            probe_kind: "test_fixture".into(),
            probed_at: created_at,
        };
        history
            .save_capability_snapshot(&capability.identity.cache_key().unwrap(), &capability)
            .unwrap();
        let command = TurnStartCommand {
            history_session_id: "history".into(),
            display_text: "question".into(),
            turn_mode: "build".into(),
            permission_profile: "standard".into(),
            requested_reasoning_effort: None,
            requested_model_id: Some("model".into()),
            attachments: Vec::new(),
            created_at,
        };
        let spec = TurnExecutionSpec {
            turn_id: "turn-1".into(),
            history_session_id: "history".into(),
            turn_epoch: 0,
            engine_id: "codex".into(),
            provider_id: "fixture".into(),
            provider_kind: "api".into(),
            provider_display_name: "Fixture".into(),
            route_label_snapshot: "Fixture / model".into(),
            requested_model_id: "model".into(),
            routed_model_id: "model".into(),
            model_label_snapshot: "Model".into(),
            requested_reasoning_effort: ReasoningEffort::Auto,
            routed_reasoning_effort: ReasoningEffort::Auto,
            turn_mode: command.turn_mode.clone(),
            permission_profile: command.permission_profile.clone(),
            binding_id: Some("codex".into()),
            binding_revision: Some(1),
            engine_profile_digest: capability.identity.engine_profile_digest.clone(),
            provider_launch_profile_ref: capability.identity.provider_launch_profile_ref.clone(),
            launch_config_digest: capability.identity.launch_profile_identity.clone(),
            routing_capability_snapshot_id: Some(capability.id.clone()),
            resolution_source: "binding_live".into(),
            legacy_route_snapshot_digest: None,
            pricing_basis_snapshot: PricingBasisSnapshot { profile: None },
            session_context: Vec::new(),
            created_at,
        };
        let (_, spec) = history.start_turn(&command, spec).unwrap();
        let generation = RuntimeGeneration {
            id: "runtime-fixture".into(),
            owner: RuntimeOwnerRef::Session("history".into()),
            engine_id: spec.engine_id.clone(),
            compatibility_key: "sha256:compatibility".into(),
            engine_profile_digest: spec.engine_profile_digest.clone(),
            provider_launch_profile_ref: spec.provider_launch_profile_ref.clone(),
            provider_launch_profile_digest: capability.identity.provider_launch_profile_digest,
            capability_snapshot_id: capability.id,
            canonical_cwd: "d:\\repo".into(),
            created_at,
        };
        history.create_runtime_generation(&generation).unwrap();
        let attempt = history
            .create_turn_attempt(&spec, &generation, None)
            .unwrap();
        let supervisor = TurnSupervisor::new(history.clone());
        supervisor
            .begin_attempt(
                &spec.history_session_id,
                &spec.turn_id,
                spec.turn_epoch,
                &spec.turn_mode,
                &spec.permission_profile,
                generation.owner.clone(),
                attempt.attempt_no,
                &generation.id,
            )
            .unwrap();
        (supervisor, spec, generation)
    }

    #[test]
    fn rejects_duplicate_terminal_and_stale_turn_events() {
        let (_history, supervisor) = supervisor("terminal");
        let end = AgentEvent::TurnComplete {
            session_id: "cli".into(),
            stop_reason: StopReason::End,
        };
        assert!(supervisor.accept_event("history", Some("turn-1"), Some(1), 1, &end));
        assert!(!supervisor.accept_event("history", Some("turn-1"), Some(1), 2, &end));
        assert!(!supervisor.accept_event(
            "history",
            Some("turn-0"),
            Some(0),
            3,
            &AgentEvent::MessageDelta {
                session_id: "cli".into(),
                role: Role::Assistant,
                text: "late".into(),
            }
        ));
    }

    #[test]
    fn rejects_duplicate_source_sequence_and_generation() {
        let (_history, supervisor) = supervisor("identity");
        let turn = supervisor
            .inner
            .current
            .lock()
            .unwrap()
            .get("history")
            .unwrap()
            .clone();
        let mut candidate = EngineEventCandidate {
            owner: turn.binding.owner,
            history_session_id: "history".into(),
            turn_id: "turn-1".into(),
            turn_epoch: 1,
            attempt_no: turn.binding.attempt_no,
            runtime_generation_id: turn.binding.runtime_generation_id,
            source_seq: 1,
            native_event_id: None,
            observed_at: now_millis(),
            event: AgentEvent::MessageDelta {
                session_id: "cli".into(),
                role: Role::Assistant,
                text: "a".into(),
            },
        };
        assert_eq!(
            supervisor.process_candidate(candidate.clone()),
            CandidateDisposition::Accepted
        );
        assert_eq!(
            supervisor.process_candidate(candidate.clone()),
            CandidateDisposition::Duplicate
        );
        candidate.source_seq = 2;
        candidate.runtime_generation_id = "stale-generation".into();
        assert_eq!(
            supervisor.process_candidate(candidate),
            CandidateDisposition::Stale
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn bounded_queue_waits_asynchronously_and_never_drops_boundary_events() {
        let history = store("backpressure");
        history
            .create_session(crate::sessions::NewSessionRecord {
                id: "history".into(),
                engine: EngineId::ClaudeCode,
                model: "model".into(),
                cwd: "D:/repo".into(),
                created_at: now_millis(),
            })
            .unwrap();
        let supervisor = TurnSupervisor::build(history, None, 1);
        supervisor.begin("history", "turn-1", 1, "build", "standard");
        let turn = supervisor
            .inner
            .current
            .lock()
            .unwrap()
            .get("history")
            .unwrap()
            .clone();
        let candidate = |source_seq, event| EngineEventCandidate {
            owner: turn.binding.owner.clone(),
            history_session_id: "history".into(),
            turn_id: "turn-1".into(),
            turn_epoch: 1,
            attempt_no: turn.binding.attempt_no,
            runtime_generation_id: turn.binding.runtime_generation_id.clone(),
            source_seq,
            native_event_id: None,
            observed_at: now_millis(),
            event,
        };
        let processing = supervisor.inner.processing.lock().unwrap();
        let first = supervisor
            .enqueue(candidate(
                1,
                AgentEvent::MessageDelta {
                    session_id: "cli".into(),
                    role: Role::Assistant,
                    text: "a".into(),
                },
            ))
            .await
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while supervisor.inner.queue.capacity() == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let second = supervisor
            .enqueue(candidate(
                2,
                AgentEvent::MessageDelta {
                    session_id: "cli".into(),
                    role: Role::Assistant,
                    text: "b".into(),
                },
            ))
            .await
            .unwrap();
        let worker = supervisor.clone();
        let boundary = candidate(
            3,
            AgentEvent::MessageComplete {
                session_id: "cli".into(),
                role: Role::Assistant,
                text: "ab".into(),
            },
        );
        let mut queued = tokio::spawn(async move { worker.enqueue(boundary).await.unwrap() });
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(30), &mut queued)
                .await
                .is_err()
        );
        assert_eq!(supervisor.diagnostics().backpressure, 1);
        drop(processing);
        let third = queued.await.unwrap();
        assert_eq!(first.await.unwrap(), CandidateDisposition::Accepted);
        assert_eq!(second.await.unwrap(), CandidateDisposition::Accepted);
        assert_eq!(third.await.unwrap(), CandidateDisposition::Accepted);
        let detail = supervisor.inner.store.get_session("history").unwrap();
        assert_eq!(detail.messages.len(), 1);
        assert_eq!(detail.messages[0].text, "ab");
    }

    #[tokio::test]
    async fn persistence_failure_is_reported_without_committing_budget_or_event_sequence() {
        let path = std::env::temp_dir().join(format!(
            "helm-event-failure-{}-{}.sqlite",
            std::process::id(),
            rand::random::<u64>()
        ));
        let history = SessionHistoryStore::new(path.clone());
        let (supervisor, spec, generation) = prepared_supervisor(&history);
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection.execute_batch("CREATE TRIGGER reject_tool_boundary BEFORE INSERT ON stream_boundary_event WHEN NEW.event_kind = 'tool_call' BEGIN SELECT RAISE(FAIL, 'test boundary failure'); END;").unwrap();
        let event = AgentEvent::ToolCall {
            session_id: "cli".into(),
            id: "tool".into(),
            name: "Read".into(),
            input: serde_json::json!({"path":"file.txt"}),
            status: crate::protocol::CallStatus::Pending,
        };
        let result = supervisor
            .submit_event("history", Some("turn-1"), Some(1), event.clone())
            .await;
        assert!(result.unwrap_err().contains("stream_persistence_failed"));
        assert!(history
            .get_session("history")
            .unwrap()
            .tool_calls
            .is_empty());
        let binding = {
            let current = supervisor.inner.current.lock().unwrap();
            let turn = current.get("history").unwrap();
            assert_eq!(turn.budget.tool_count, 0);
            assert_eq!(turn.budget.output_bytes, 0);
            assert_eq!(turn.snapshot.event_seq, 0);
            assert_eq!(turn.snapshot.status, TurnStatus::Running);
            assert_eq!(turn.last_source_seq, 0);
            assert!(turn.stream_failure_reported);
            turn.binding.clone()
        };
        let stored = history.load_turn_snapshot("history").unwrap().unwrap();
        assert_eq!(stored.event_seq, 0);
        assert_eq!(stored.status, TurnStatus::Running);
        assert!(!supervisor.report_stream_failure("history", "turn-1", 1, &binding));
        connection
            .execute_batch("DROP TRIGGER reject_tool_boundary;")
            .unwrap();
        assert!(!supervisor
            .submit_event("history", Some("turn-1"), Some(1), event.clone())
            .await
            .unwrap());
        assert_eq!(
            supervisor.snapshot("history").unwrap().unwrap().event_seq,
            0
        );
        let retry = history
            .create_turn_attempt(&spec, &generation, None)
            .unwrap();
        assert_eq!(retry.attempt_no, binding.attempt_no + 1);
        supervisor
            .retry_attempt(
                "history",
                "turn-1",
                retry.attempt_no,
                &binding.runtime_generation_id,
                "test pre-execution retry",
            )
            .unwrap();
        assert!(
            !supervisor
                .inner
                .current
                .lock()
                .unwrap()
                .get("history")
                .unwrap()
                .stream_failure_reported
        );
        assert!(supervisor
            .submit_event("history", Some("turn-1"), Some(1), event)
            .await
            .unwrap());
        assert_eq!(
            supervisor
                .inner
                .current
                .lock()
                .unwrap()
                .get("history")
                .unwrap()
                .budget
                .tool_count,
            1
        );
        assert_eq!(
            supervisor.snapshot("history").unwrap().unwrap().event_seq,
            1
        );
        connection.execute_batch("CREATE TRIGGER reject_message_boundary BEFORE INSERT ON stream_boundary_event WHEN NEW.event_kind = 'message_complete' BEGIN SELECT RAISE(FAIL, 'test message failure'); END;").unwrap();
        assert!(supervisor
            .submit_event(
                "history",
                Some("turn-1"),
                Some(1),
                AgentEvent::MessageDelta {
                    session_id: "cli".into(),
                    role: Role::Assistant,
                    text: "partial reply".into(),
                }
            )
            .await
            .unwrap());
        assert!(supervisor
            .submit_event(
                "history",
                Some("turn-1"),
                Some(1),
                AgentEvent::MessageComplete {
                    session_id: "cli".into(),
                    role: Role::Assistant,
                    text: "partial reply".into(),
                }
            )
            .await
            .is_err());
        let messages = history.get_session("history").unwrap().messages;
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].role, Role::User);
        assert_eq!(messages[0].text, "question");
        for event in [
            AgentEvent::MessageDelta {
                session_id: "cli".into(),
                role: Role::Assistant,
                text: "late reply".into(),
            },
            AgentEvent::ToolProgress {
                session_id: "cli".into(),
                id: "tool".into(),
                chunk: "late output".into(),
            },
            AgentEvent::Error {
                session_id: Some("cli".into()),
                message: "recoverable runtime warning".into(),
                recoverable: true,
                kind: None,
                stalled_kind: None,
            },
        ] {
            assert!(!supervisor
                .submit_event("history", Some("turn-1"), Some(1), event)
                .await
                .unwrap());
        }
        let snapshot = supervisor.snapshot("history").unwrap().unwrap();
        assert_eq!(snapshot.event_seq, 2);
        assert_eq!(snapshot.status, TurnStatus::Running);
        connection
            .execute_batch("DROP TRIGGER reject_message_boundary;")
            .unwrap();
        assert!(supervisor
            .submit_event(
                "history",
                Some("turn-1"),
                Some(1),
                AgentEvent::TurnComplete {
                    session_id: "cli".into(),
                    stop_reason: StopReason::Interrupted,
                }
            )
            .await
            .unwrap());
        let detail = history.get_session("history").unwrap();
        assert_eq!(detail.messages.len(), 1);
        assert_eq!(detail.messages[0].role, Role::User);
        assert_eq!(detail.messages[0].text, "question");
        assert_eq!(detail.presentations.iter().filter(|record| matches!(&record.content, crate::protocol::TurnPresentationContent::Message { text, .. } if text == "partial reply")).count(), 1);
        let attempt_state: String = connection
            .query_row(
                "SELECT delivery_state FROM turn_attempt WHERE turn_id = ?1 AND attempt_no = ?2",
                rusqlite::params![spec.turn_id, retry.attempt_no],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(attempt_state, "interrupted");
        drop(connection);
        drop(supervisor);
        drop(history);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn stream_failure_claim_is_once_per_attempt_and_rejects_stale_or_terminal_identity() {
        let history = store("failure-claim");
        let (supervisor, spec, generation) = prepared_supervisor(&history);
        let binding = supervisor
            .inner
            .current
            .lock()
            .unwrap()
            .get("history")
            .unwrap()
            .binding
            .clone();
        for (history_id, turn_id, epoch) in [
            ("missing", "turn-1", 1),
            ("history", "old-turn", 1),
            ("history", "turn-1", 0),
        ] {
            assert!(!supervisor.report_stream_failure(history_id, turn_id, epoch, &binding));
        }
        for stale_binding in [
            AttemptBinding {
                owner: RuntimeOwnerRef::Session("other-history".into()),
                ..binding.clone()
            },
            AttemptBinding {
                attempt_no: binding.attempt_no + 1,
                ..binding.clone()
            },
            AttemptBinding {
                runtime_generation_id: "old-generation".into(),
                ..binding.clone()
            },
        ] {
            assert!(!supervisor.report_stream_failure("history", "turn-1", 1, &stale_binding));
        }
        assert!(supervisor.report_stream_failure("history", "turn-1", 1, &binding));
        assert!(!supervisor.report_stream_failure("history", "turn-1", 1, &binding));
        assert!(!supervisor.accept_event(
            "history",
            Some("turn-1"),
            Some(1),
            1,
            &AgentEvent::MessageDelta {
                session_id: "cli".into(),
                role: Role::Assistant,
                text: "already queued before failure".into(),
            },
        ));

        let retry = history
            .create_turn_attempt(&spec, &generation, None)
            .unwrap();
        assert_eq!(retry.attempt_no, binding.attempt_no + 1);
        supervisor
            .retry_attempt(
                "history",
                "turn-1",
                retry.attempt_no,
                &binding.runtime_generation_id,
                "test pre-execution retry",
            )
            .unwrap();
        let next_binding = AttemptBinding {
            attempt_no: binding.attempt_no + 1,
            ..binding.clone()
        };
        assert!(!supervisor.report_stream_failure("history", "turn-1", 1, &binding));
        assert!(supervisor.report_stream_failure("history", "turn-1", 1, &next_binding));
        assert!(!supervisor.report_stream_failure("history", "turn-1", 1, &next_binding));
        assert!(supervisor.accept_event(
            "history",
            Some("turn-1"),
            Some(1),
            1,
            &AgentEvent::Error {
                session_id: Some("cli".into()),
                message: "runtime exited".into(),
                recoverable: false,
                kind: Some("process_crash".into()),
                stalled_kind: None,
            },
        ));
        assert_eq!(
            supervisor.snapshot("history").unwrap().unwrap().status,
            TurnStatus::Failed
        );

        supervisor.begin("history", "turn-2", 2, "build", "auto");
        assert!(supervisor.accept_event(
            "history",
            Some("turn-2"),
            Some(2),
            1,
            &AgentEvent::MessageDelta {
                session_id: "cli".into(),
                role: Role::Assistant,
                text: "new turn".into(),
            },
        ));
        assert!(!supervisor.report_stream_failure("history", "turn-1", 1, &next_binding));
        assert!(supervisor.accept_event(
            "history",
            Some("turn-2"),
            Some(2),
            2,
            &AgentEvent::TurnComplete {
                session_id: "cli".into(),
                stop_reason: StopReason::End,
            },
        ));
        assert!(!supervisor.report_stream_failure("history", "turn-2", 2, &binding));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn closed_queue_or_worker_reply_reports_only_the_frozen_attempt() {
        for (queue_closed, stale_reply) in [(true, false), (false, false), (false, true)] {
            let (history, original) =
                supervisor(&format!("failure-transport-{queue_closed}-{stale_reply}"));
            let current = original.inner.current.lock().unwrap().clone();
            let (queue, mut receiver) = mpsc::channel::<QueuedCandidate>(1);
            let supervisor = TurnSupervisor {
                inner: Arc::new(SupervisorInner {
                    store: history.clone(),
                    app: None,
                    current: Mutex::new(current),
                    diagnostics: Mutex::new(StreamDiagnostics::default()),
                    processing: Mutex::new(()),
                    ingress: tokio::sync::Mutex::new(()),
                    queue,
                }),
            };
            drop(original);
            let submitted = supervisor.submit_event(
                "history",
                Some("turn-1"),
                Some(1),
                AgentEvent::MessageDelta {
                    session_id: "cli".into(),
                    role: Role::Assistant,
                    text: "uncommitted reply".into(),
                },
            );
            tokio::pin!(submitted);
            if queue_closed {
                receiver.close();
            } else {
                let queued = tokio::time::timeout(std::time::Duration::from_secs(1), async {
                    tokio::select! {
                        result = &mut submitted => panic!("提交必须等待 worker 回执：{result:?}"),
                        queued = receiver.recv() => queued.expect("候选必须先入队"),
                    }
                })
                .await
                .expect("入队不应阻塞 Tokio");
                if stale_reply {
                    supervisor
                        .retry_attempt(
                            "history",
                            "turn-1",
                            queued.candidate.attempt_no + 1,
                            &queued.candidate.runtime_generation_id,
                            "test retry before worker reply closes",
                        )
                        .unwrap();
                }
                drop(queued);
            }
            let error = tokio::time::timeout(std::time::Duration::from_secs(1), &mut submitted)
                .await
                .expect("传输关闭必须结束提交等待")
                .unwrap_err();
            assert!(error.contains("[stream_worker_closed]"));
            {
                let current = supervisor.inner.current.lock().unwrap();
                let turn = current.get("history").unwrap();
                assert_eq!(turn.stream_failure_reported, !stale_reply);
                assert_eq!(turn.binding.attempt_no, u64::from(stale_reply));
                assert_eq!(turn.snapshot.event_seq, 0);
                assert_eq!(turn.last_source_seq, 0);
                assert_eq!(turn.budget.output_bytes, 0);
                assert_eq!(turn.budget.tool_count, 0);
                assert_eq!(turn.snapshot.status, TurnStatus::Running);
            }
            let stored = history.load_turn_snapshot("history").unwrap().unwrap();
            assert_eq!(stored.event_seq, 0);
            assert_eq!(stored.status, TurnStatus::Running);
        }
    }

    #[test]
    fn turn_stream_failure_serializes_only_control_plane_identity_and_safe_message() {
        let failure = TurnStreamFailure {
            history_id: "history".into(),
            turn_id: "turn-1".into(),
            turn_epoch: 7,
            attempt_no: 2,
            runtime_generation_id: "generation-3".into(),
            message: STREAM_FAILURE_MESSAGE.to_string(),
        };
        let payload = serde_json::to_value(&failure).unwrap();
        assert_eq!(
            payload,
            serde_json::json!({
                "historyId": "history",
                "turnId": "turn-1",
                "turnEpoch": 7,
                "attemptNo": 2,
                "runtimeGenerationId": "generation-3",
                "message": "消息流保存/投递失败，已请求停止执行；部分显示内容可能尚未保存，请检查磁盘空间与访问权限",
            })
        );
        assert_eq!(
            serde_json::from_value::<TurnStreamFailure>(payload).unwrap(),
            failure
        );
    }

    #[tokio::test]
    async fn queued_event_cannot_adopt_a_new_attempt_identity() {
        let (_history, supervisor) = supervisor("frozen-ingress");
        let ingress = supervisor.inner.ingress.lock().await;
        let submitted = supervisor.submit_event(
            "history",
            Some("turn-1"),
            Some(1),
            AgentEvent::MessageDelta {
                session_id: "cli".into(),
                role: Role::Assistant,
                text: "old".into(),
            },
        );
        tokio::pin!(submitted);
        assert!(futures_util::poll!(&mut submitted).is_pending());
        supervisor
            .inner
            .current
            .lock()
            .unwrap()
            .get_mut("history")
            .unwrap()
            .binding
            .attempt_no += 1;
        drop(ingress);
        assert!(!submitted.await.unwrap());
        assert_eq!(
            supervisor.snapshot("history").unwrap().unwrap().event_seq,
            0
        );
    }

    #[test]
    fn final_tool_diff_and_fallback_progress_share_a_budget_and_reject_late_progress() {
        let (history, supervisor) = supervisor("tool-progress-fallback");
        let progress = AgentEvent::ToolProgress {
            session_id: "cli".into(),
            id: "tool".into(),
            chunk: "中文".repeat(40_000),
        };
        let events = [
            AgentEvent::ToolCall {
                session_id: "cli".into(),
                id: "tool".into(),
                name: "Edit".into(),
                input: serde_json::json!({"path":"file.txt"}),
                status: crate::protocol::CallStatus::Pending,
            },
            progress.clone(),
            AgentEvent::ToolResult {
                session_id: "cli".into(),
                id: "tool".into(),
                status: crate::protocol::ToolStatus::Success,
                output: None,
                diff: Some(crate::protocol::Diff {
                    path: "file.txt".into(),
                    hunks: vec![crate::protocol::DiffHunk {
                        old_start: 1,
                        new_start: 1,
                        lines: vec![crate::protocol::DiffLine {
                            kind: crate::protocol::DiffKind::Add,
                            text: "line".repeat(1024),
                        }],
                    }],
                }),
                outcome: None,
                started: None,
                has_output: None,
                retryable: None,
                denial_source: None,
                native_denial_code: None,
            },
        ];
        for (index, event) in events.iter().enumerate() {
            assert!(supervisor.accept_event(
                "history",
                Some("turn-1"),
                Some(1),
                index as u64 + 1,
                event
            ));
        }
        let detail = history.get_session("history").unwrap();
        let tool = &detail.tool_calls[0];
        let diff_bytes = serde_json::to_vec(tool.diff.as_ref().unwrap())
            .unwrap()
            .len();
        assert!(
            tool.output.as_ref().unwrap().len() + diff_bytes
                <= crate::output_limits::MAX_TOOL_OUTPUT_BYTES
        );
        assert!(!supervisor.accept_event("history", Some("turn-1"), Some(1), 4, &progress));
        assert_eq!(
            history.get_session("history").unwrap().tool_calls,
            detail.tool_calls
        );
    }

    #[test]
    fn stalled_then_crashed_has_one_failed_terminal() {
        let (_history, supervisor) = supervisor("crash");
        assert!(supervisor.accept_event(
            "history",
            Some("turn-1"),
            Some(1),
            1,
            &AgentEvent::TurnStage {
                session_id: "cli".into(),
                stage: TurnStage::Stalled,
                ts: now_millis(),
                engine_reported_ttft_ms: None,
                retry_attempt: None,
            },
        ));
        assert!(supervisor.accept_event(
            "history",
            Some("turn-1"),
            Some(1),
            2,
            &AgentEvent::Error {
                session_id: Some("cli".into()),
                message: "process crashed".into(),
                recoverable: false,
                kind: Some("process_crashed".into()),
                stalled_kind: None,
            },
        ));
        assert!(!supervisor.accept_event(
            "history",
            Some("turn-1"),
            Some(1),
            3,
            &AgentEvent::TurnComplete {
                session_id: "cli".into(),
                stop_reason: StopReason::End,
            },
        ));
        assert_eq!(
            supervisor.snapshot("history").unwrap().unwrap().status,
            TurnStatus::Failed
        );
    }

    #[test]
    fn persists_terminal_snapshot_for_restart_reconciliation() {
        let (history, supervisor) = supervisor("persist");
        let event = AgentEvent::TurnComplete {
            session_id: "cli".into(),
            stop_reason: StopReason::Interrupted,
        };
        assert!(supervisor.accept_event("history", Some("turn-1"), Some(1), 4, &event));
        let loaded = history.load_turn_snapshot("history").unwrap().unwrap();
        assert_eq!(loaded.turn_id, "turn-1");
        assert_eq!(loaded.status, TurnStatus::Interrupted);
        // 本轮零可见输出 → 先补一条兜底说明（占 1 个序号），终态事件顺延到 2。
        assert_eq!(loaded.event_seq, 2);
        assert!(loaded
            .terminal_reason
            .as_deref()
            .unwrap_or_default()
            .contains("本轮已被中断"));
        assert_eq!(loaded.permission_profile, "auto");
    }

    #[test]
    fn silent_terminal_gets_readable_reason_and_extra_event_seq() {
        // 用户在被中断前什么都没收到：必须落库一条可读原因，并把终态事件顺延一位。
        let (history, supervisor) = supervisor("silent-interrupt");
        assert!(supervisor.accept_event(
            "history",
            Some("turn-1"),
            Some(1),
            1,
            &AgentEvent::TurnComplete {
                session_id: "cli".into(),
                stop_reason: StopReason::Interrupted,
            },
        ));
        let loaded = history.load_turn_snapshot("history").unwrap().unwrap();
        assert_eq!(loaded.status, TurnStatus::Interrupted);
        // 兜底 Error 占 1 个序号，终态事件顺延到 2。
        assert_eq!(loaded.event_seq, 2);
        let reason = loaded.terminal_reason.unwrap_or_default();
        assert!(reason.contains("本轮已被中断"), "实际原因：{reason}");
    }

    #[test]
    fn terminal_with_visible_content_keeps_native_reason() {
        // 有正文的轮次不该被兜底打扰：终态原因仍是引擎原值，序号正常累加。
        let (history, supervisor) = supervisor("loud-interrupt");
        assert!(supervisor.accept_event(
            "history",
            Some("turn-1"),
            Some(1),
            1,
            &AgentEvent::MessageDelta {
                session_id: "cli".into(),
                role: Role::Assistant,
                text: "一半答案".into(),
            },
        ));
        assert!(supervisor.accept_event(
            "history",
            Some("turn-1"),
            Some(1),
            2,
            &AgentEvent::TurnComplete {
                session_id: "cli".into(),
                stop_reason: StopReason::Interrupted,
            },
        ));
        let loaded = history.load_turn_snapshot("history").unwrap().unwrap();
        assert_eq!(loaded.status, TurnStatus::Interrupted);
        assert_eq!(loaded.event_seq, 2);
        assert_eq!(loaded.terminal_reason.as_deref(), Some("interrupted"));
    }

    #[test]
    fn completed_thinking_and_plan_are_restored_but_not_repeated_at_terminal() {
        let (history, supervisor) = supervisor("presentation-complete");
        let events = [
            AgentEvent::ThinkingDelta {
                session_id: "cli".into(),
                text: "visible reasoning".into(),
            },
            AgentEvent::ThinkingComplete {
                session_id: "cli".into(),
                text: "visible reasoning".into(),
            },
            AgentEvent::PlanUpdate {
                session_id: "cli".into(),
                steps: vec![crate::protocol::PlanStep {
                    text: "Read source".into(),
                    status: crate::protocol::PlanStatus::Active,
                }],
            },
            AgentEvent::MessageDelta {
                session_id: "cli".into(),
                role: Role::Assistant,
                text: "answer".into(),
            },
            AgentEvent::MessageComplete {
                session_id: "cli".into(),
                role: Role::Assistant,
                text: "answer".into(),
            },
            AgentEvent::TurnComplete {
                session_id: "cli".into(),
                stop_reason: StopReason::End,
            },
        ];
        for (index, event) in events.iter().enumerate() {
            assert!(supervisor.accept_event(
                "history",
                Some("turn-1"),
                Some(1),
                index as u64 + 1,
                event
            ));
        }
        let detail = history.get_session("history").unwrap();
        assert_eq!(detail.presentations.len(), 2);
        assert_eq!(
            detail
                .messages
                .iter()
                .filter(|message| message.text == "answer")
                .count(),
            1
        );
        assert!(
            matches!(&detail.presentations[0].content, crate::protocol::TurnPresentationContent::Thinking { complete: true, text } if text == "visible reasoning")
        );
        assert!(
            matches!(&detail.presentations[1].content, crate::protocol::TurnPresentationContent::Plan { steps, truncated: false } if steps.len() == 1)
        );
        assert!(detail
            .presentations
            .iter()
            .all(|record| record.turn_id == "turn-1"));
    }

    #[test]
    fn interrupted_turn_keeps_partial_text_thinking_and_tool_output() {
        let (history, supervisor) = supervisor("presentation-interrupted");
        let events = [
            AgentEvent::MessageDelta {
                session_id: "cli".into(),
                role: Role::Assistant,
                text: "partial answer".into(),
            },
            AgentEvent::ThinkingDelta {
                session_id: "cli".into(),
                text: "visible partial".into(),
            },
            AgentEvent::ToolCall {
                session_id: "cli".into(),
                id: "tool".into(),
                name: "Read".into(),
                input: serde_json::json!({"path":"file.txt"}),
                status: crate::protocol::CallStatus::Pending,
            },
            AgentEvent::ToolProgress {
                session_id: "cli".into(),
                id: "tool".into(),
                chunk: "partial output".into(),
            },
            AgentEvent::TurnComplete {
                session_id: "cli".into(),
                stop_reason: StopReason::Interrupted,
            },
        ];
        for (index, event) in events.iter().enumerate() {
            assert!(supervisor.accept_event(
                "history",
                Some("turn-1"),
                Some(1),
                index as u64 + 1,
                event
            ));
        }
        let detail = history.get_session("history").unwrap();
        assert_eq!(detail.presentations.len(), 2);
        assert!(detail.presentations.iter().any(|record| matches!(&record.content, crate::protocol::TurnPresentationContent::Message { text, complete: false, .. } if text == "partial answer")));
        assert!(detail.presentations.iter().any(|record| matches!(&record.content, crate::protocol::TurnPresentationContent::Thinking { text, complete: false } if text == "visible partial")));
        assert_eq!(detail.tool_calls.len(), 1);
        assert_eq!(
            detail.tool_calls[0].output.as_deref(),
            Some("partial output")
        );
        assert_eq!(
            detail.tool_calls[0].status,
            crate::sessions::HistoryToolStatus::Error
        );
        assert!(!supervisor.accept_event("history", Some("turn-1"), Some(1), 9, &events[0]));
        assert_eq!(
            history.get_session("history").unwrap().presentations,
            detail.presentations
        );
    }

    #[test]
    fn waiting_approval_can_resume_on_the_same_attempt() {
        let (_history, supervisor) = supervisor("approval");
        let approval = AgentEvent::ApprovalRequest {
            session_id: "cli".into(),
            id: "request-1".into(),
            action: "Bash".into(),
            detail: "echo ok".into(),
            input: None,
            available_decisions: Vec::new(),
            persistent_label: None,
            matcher_summary: None,
        };
        assert!(supervisor.accept_event("history", Some("turn-1"), Some(1), 1, &approval));
        let resumed = AgentEvent::MessageDelta {
            session_id: "cli".into(),
            role: Role::Assistant,
            text: "resumed".into(),
        };
        assert!(supervisor.accept_event("history", Some("turn-1"), Some(1), 2, &resumed));
        assert_eq!(
            supervisor.snapshot("history").unwrap().unwrap().status,
            TurnStatus::Running
        );
    }

    #[test]
    fn budget_events_cover_streaming_post_facto_repeat_and_watchdogs() {
        let mut snapshot = TurnBudgetSnapshot::standard(100);
        for limit in &mut snapshot.limits {
            limit.limit = match limit.dimension {
                BudgetDimension::OutputBytes => 3,
                BudgetDimension::ToolCount => 1,
                BudgetDimension::RepeatDigest => 1,
                BudgetDimension::Token => 9,
                BudgetDimension::CostMicrousd => 99,
                BudgetDimension::ContextRatioPermille => 500,
                BudgetDimension::WallClockMs | BudgetDimension::IdleMs => 10,
                BudgetDimension::InputBytes => limit.limit,
            };
        }
        let mut state = BudgetRuntimeState {
            snapshot,
            output_bytes: 0,
            tool_output_bytes: HashMap::new(),
            tool_count: 0,
            repeat_digests: HashMap::new(),
            started_at: 100,
            last_event_at: 100,
            exceeded: HashSet::new(),
        };
        let output = apply_budget_event(
            &mut state,
            &AgentEvent::MessageDelta {
                session_id: "cli".into(),
                role: Role::Assistant,
                text: "four".into(),
            },
        );
        assert!(output.iter().any(|trigger| {
            trigger.dimension == BudgetDimension::OutputBytes && trigger.interrupt
        }));

        let tool = |id: &str| AgentEvent::ToolCall {
            session_id: "cli".into(),
            id: id.into(),
            name: "Read".into(),
            input: serde_json::json!({"path":"same"}),
            status: crate::protocol::CallStatus::Pending,
        };
        assert!(apply_budget_event(&mut state, &tool("tool-1")).is_empty());
        let tool_triggers = apply_budget_event(&mut state, &tool("tool-2"));
        assert!(tool_triggers
            .iter()
            .any(|trigger| trigger.dimension == BudgetDimension::ToolCount && trigger.interrupt));
        assert!(tool_triggers.iter().any(|trigger| {
            trigger.dimension == BudgetDimension::RepeatDigest && trigger.interrupt
        }));

        let usage = apply_budget_event(
            &mut state,
            &AgentEvent::TokenUsage {
                session_id: "cli".into(),
                input_tokens: 8,
                cached_input_tokens: None,
                cache_write_input_tokens: None,
                output_tokens: 2,
                cost_usd: 0.0001,
                service_tier: None,
                context_window: None,
            },
        );
        assert!(usage
            .iter()
            .any(|trigger| { trigger.dimension == BudgetDimension::Token && !trigger.interrupt }));
        assert!(usage.iter().any(|trigger| {
            trigger.dimension == BudgetDimension::CostMicrousd && !trigger.interrupt
        }));
        let context = apply_budget_event(
            &mut state,
            &AgentEvent::ContextUsage {
                session_id: "cli".into(),
                context_tokens: 6,
                context_window: Some(10),
            },
        );
        assert!(context.iter().any(|trigger| {
            trigger.dimension == BudgetDimension::ContextRatioPermille && !trigger.interrupt
        }));

        let wall = watchdog_budget_trigger(&mut state, 111).unwrap();
        assert_eq!(wall.dimension, BudgetDimension::WallClockMs);
        assert!(wall.interrupt);
        let idle = watchdog_budget_trigger(&mut state, 112).unwrap();
        assert_eq!(idle.dimension, BudgetDimension::IdleMs);
        assert!(idle.interrupt);
    }
}
