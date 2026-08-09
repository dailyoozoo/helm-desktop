//! Backend-authoritative stream ordering and Turn finalization.
//!
//! Adapters submit normalized candidates only. This module assigns the public
//! event sequence, rejects stale ownership, persists boundary facts, updates
//! history, and is the sole writer of the terminal Turn snapshot.

use crate::budget::{BudgetDimension, BudgetEnforcementMode, TurnBudgetSnapshot};
use crate::protocol::{AgentEvent, StopReason, TurnStage};
use crate::runtime_registry::RuntimeOwnerRef;
use crate::sessions::{SessionHistoryStore, TurnSnapshotRecord};
use crate::util::now_millis;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Condvar, Mutex};
use tauri::{AppHandle, Emitter, Manager};

const EVENT_NAME: &str = "agent-event";
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
    next_source_seq: u64,
    last_source_seq: u64,
    seen_native_events: HashSet<String>,
    native_session_id: Option<String>,
    budget: BudgetRuntimeState,
}

struct QueueState {
    items: VecDeque<EngineEventCandidate>,
    draining: bool,
}

struct SupervisorInner {
    store: SessionHistoryStore,
    app: Option<AppHandle>,
    current: Mutex<HashMap<String, SupervisedTurn>>,
    diagnostics: Mutex<StreamDiagnostics>,
    queue: Mutex<QueueState>,
    queue_space: Condvar,
    queue_capacity: usize,
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
        Self {
            inner: Arc::new(SupervisorInner {
                store,
                app,
                current: Mutex::new(HashMap::new()),
                diagnostics: Mutex::new(StreamDiagnostics::default()),
                queue: Mutex::new(QueueState {
                    items: VecDeque::new(),
                    draining: false,
                }),
                queue_space: Condvar::new(),
                queue_capacity: queue_capacity.max(1),
            }),
        }
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
                    next_source_seq: 0,
                    last_source_seq: 0,
                    seen_native_events: HashSet::new(),
                    native_session_id: None,
                    budget: BudgetRuntimeState {
                        snapshot: budget_snapshot,
                        output_bytes: 0,
                        tool_count: 0,
                        repeat_digests: HashMap::new(),
                        started_at,
                        last_event_at: started_at,
                        exceeded: HashSet::new(),
                    },
                },
            );
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
    pub fn submit_event(
        &self,
        history_session_id: &str,
        turn_id: Option<&str>,
        turn_epoch: Option<u64>,
        event: AgentEvent,
    ) -> bool {
        let candidate = {
            let Ok(mut current) = self.inner.current.lock() else {
                return false;
            };
            let Some(turn) = current.get_mut(history_session_id) else {
                self.bump_diagnostic(|value| value.orphan += 1);
                let _ = self.inner.store.record_stream_diagnostic(
                    None,
                    event_kind(&event),
                    "orphan_session",
                    None,
                );
                return false;
            };
            turn.next_source_seq = turn.next_source_seq.saturating_add(1);
            EngineEventCandidate {
                owner: turn.binding.owner.clone(),
                history_session_id: history_session_id.to_string(),
                turn_id: turn_id.unwrap_or(&turn.snapshot.turn_id).to_string(),
                turn_epoch: turn_epoch.unwrap_or(turn.snapshot.turn_epoch),
                attempt_no: turn.binding.attempt_no,
                runtime_generation_id: turn.binding.runtime_generation_id.clone(),
                source_seq: turn.next_source_seq,
                native_event_id: native_event_identity(&event),
                observed_at: now_millis(),
                event,
            }
        };
        self.enqueue(candidate);
        true
    }

    fn enqueue(&self, mut candidate: EngineEventCandidate) {
        let mut queue = self
            .inner
            .queue
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        while queue.items.len() >= self.inner.queue_capacity {
            self.bump_diagnostic(|value| value.backpressure += 1);
            if is_delta(&candidate.event)
                && queue
                    .items
                    .back_mut()
                    .is_some_and(|pending| merge_candidate_delta(pending, &mut candidate))
            {
                self.bump_diagnostic(|value| value.coalesced_delta += 1);
                return;
            }
            queue = self
                .inner
                .queue_space
                .wait(queue)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
        queue.items.push_back(candidate);
        if queue.draining {
            return;
        }
        queue.draining = true;
        drop(queue);

        loop {
            let candidate = {
                let mut queue = self
                    .inner
                    .queue
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                let candidate = queue.items.pop_front();
                self.inner.queue_space.notify_all();
                if candidate.is_none() {
                    queue.draining = false;
                }
                candidate
            };
            let Some(candidate) = candidate else {
                break;
            };
            let _ = self.process_candidate(candidate);
        }
    }

    pub fn process_candidate(&self, candidate: EngineEventCandidate) -> CandidateDisposition {
        let event = crate::redaction::sanitize_agent_event(&candidate.event);
        let (snapshot, event_seq, native_identity, budget_triggers) = {
            let Ok(mut current) = self.inner.current.lock() else {
                self.bump_diagnostic(|value| value.persistence_failed += 1);
                return CandidateDisposition::PersistenceFailed;
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
            turn.budget.last_event_at = candidate.observed_at;
            let budget_triggers = apply_budget_event(&mut turn.budget, &event);
            let event_seq = snapshot.event_seq;
            (snapshot, event_seq, native_identity, budget_triggers)
        };

        let event_kind = event_kind(&event);
        let event_digest = crate::turn_start::digest_json(&event)
            .unwrap_or_else(|_| "sha256:unavailable".to_string());
        let persisted = if snapshot.status.is_terminal() {
            self.inner.store.finalize_supervised_turn(
                &snapshot,
                candidate.attempt_no,
                &candidate.runtime_generation_id,
                event_kind,
                &event_digest,
            )
        } else {
            self.inner
                .store
                .record_event_for_session_in_turn(
                    &candidate.history_session_id,
                    Some(&candidate.turn_id),
                    &event,
                )
                .and_then(|_| {
                    if is_boundary(&event) {
                        self.inner.store.upsert_turn_snapshot((&snapshot).into())?;
                        self.inner.store.record_stream_boundary(
                            &candidate.history_session_id,
                            &candidate.turn_id,
                            candidate.attempt_no,
                            &candidate.runtime_generation_id,
                            event_seq,
                            event_kind,
                            "accepted",
                            &event_digest,
                            candidate.observed_at,
                        )?;
                    }
                    Ok(())
                })
        };
        if let Err(error) = persisted {
            self.bump_diagnostic(|value| value.persistence_failed += 1);
            let _ = self.inner.store.record_stream_diagnostic(
                Some(&candidate),
                event_kind,
                "persistence_failed",
                Some(&error),
            );
            return CandidateDisposition::PersistenceFailed;
        }

        if let Ok(mut current) = self.inner.current.lock() {
            if let Some(turn) = current.get_mut(&candidate.history_session_id) {
                if turn.snapshot.turn_id == candidate.turn_id
                    && turn.binding.attempt_no == candidate.attempt_no
                    && turn.binding.runtime_generation_id == candidate.runtime_generation_id
                {
                    turn.last_source_seq = candidate.source_seq;
                    if let Some(identity) = candidate.native_event_id.as_ref() {
                        turn.seen_native_events.insert(identity.clone());
                    }
                    if native_identity.is_some() {
                        turn.native_session_id = native_identity;
                    }
                    turn.snapshot = snapshot;
                }
            }
        }

        self.bump_diagnostic(|value| value.accepted += 1);
        for trigger in budget_triggers {
            self.record_budget_trigger(&candidate, &trigger);
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
        let Some(app) = self.inner.app.as_ref() else {
            return;
        };
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
    }
}

fn apply_transition(snapshot: &mut TurnSnapshot, event: &AgentEvent) {
    match event {
        AgentEvent::ApprovalRequest { .. } => snapshot.status = TurnStatus::WaitingApproval,
        AgentEvent::TurnComplete { stop_reason, .. } => {
            snapshot.status = match stop_reason {
                StopReason::End => TurnStatus::Succeeded,
                StopReason::Interrupted => TurnStatus::Interrupted,
                StopReason::Error => TurnStatus::Failed,
            };
            snapshot.terminal_reason = Some(
                match stop_reason {
                    StopReason::End => "end",
                    StopReason::Interrupted => "interrupted",
                    StopReason::Error => "error",
                }
                .to_string(),
            );
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
        | AgentEvent::Checkpoint { .. }
        | AgentEvent::TokenUsage { .. }
        | AgentEvent::ContextUsage { .. }
        | AgentEvent::SessionStarted { .. }
        | AgentEvent::Error { .. } => snapshot.status = TurnStatus::Running,
    }
}

fn apply_budget_event(state: &mut BudgetRuntimeState, event: &AgentEvent) -> Vec<BudgetTrigger> {
    match event {
        AgentEvent::MessageDelta { text, .. }
        | AgentEvent::ThinkingDelta { text, .. }
        | AgentEvent::ToolProgress { chunk: text, .. } => {
            state.output_bytes = state
                .output_bytes
                .saturating_add(text.as_bytes().len() as u64);
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
        AgentEvent::Checkpoint { .. } => "checkpoint",
        AgentEvent::TokenUsage { .. } => "token_usage",
        AgentEvent::ContextUsage { .. } => "context_usage",
        AgentEvent::TurnComplete { .. } => "turn_complete",
        AgentEvent::Error { .. } => "error",
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

fn is_delta(event: &AgentEvent) -> bool {
    matches!(
        event,
        AgentEvent::MessageDelta { .. } | AgentEvent::ThinkingDelta { .. }
    )
}

fn merge_candidate_delta(
    pending: &mut EngineEventCandidate,
    incoming: &mut EngineEventCandidate,
) -> bool {
    if pending.turn_id != incoming.turn_id
        || pending.attempt_no != incoming.attempt_no
        || pending.runtime_generation_id != incoming.runtime_generation_id
    {
        return false;
    }
    let merged = match (&mut pending.event, &incoming.event) {
        (
            AgentEvent::MessageDelta {
                session_id: left_session,
                role: left_role,
                text: left,
            },
            AgentEvent::MessageDelta {
                session_id: right_session,
                role: right_role,
                text: right,
            },
        ) if left_session == right_session && left_role == right_role => {
            left.push_str(right);
            true
        }
        (
            AgentEvent::ThinkingDelta {
                session_id: left_session,
                text: left,
            },
            AgentEvent::ThinkingDelta {
                session_id: right_session,
                text: right,
            },
        ) if left_session == right_session => {
            left.push_str(right);
            true
        }
        _ => false,
    };
    if merged {
        pending.source_seq = incoming.source_seq;
        pending.observed_at = incoming.observed_at;
    }
    merged
}

fn native_event_identity(event: &AgentEvent) -> Option<String> {
    match event {
        AgentEvent::SessionStarted { session_id, .. } => Some(format!("session:{session_id}")),
        AgentEvent::ToolCall { id, .. } => Some(format!("tool_call:{id}")),
        AgentEvent::ToolResult { id, .. } => Some(format!("tool_result:{id}")),
        AgentEvent::ApprovalRequest { id, .. } => Some(format!("approval:{id}")),
        AgentEvent::Checkpoint { id, .. } => Some(format!("checkpoint:{id}")),
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
    use crate::protocol::{EngineId, Role};

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

    #[test]
    fn bounded_queue_coalesces_delta_and_never_drops_boundary_events() {
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
        {
            let mut queue = supervisor.inner.queue.lock().unwrap();
            queue.draining = true;
            queue.items.push_back(candidate(
                1,
                AgentEvent::MessageDelta {
                    session_id: "cli".into(),
                    role: Role::Assistant,
                    text: "a".into(),
                },
            ));
        }
        supervisor.enqueue(candidate(
            2,
            AgentEvent::MessageDelta {
                session_id: "cli".into(),
                role: Role::Assistant,
                text: "b".into(),
            },
        ));
        {
            let queue = supervisor.inner.queue.lock().unwrap();
            assert_eq!(queue.items.len(), 1);
            assert!(matches!(
                &queue.items[0].event,
                AgentEvent::MessageDelta { text, .. } if text == "ab"
            ));
        }
        assert_eq!(supervisor.diagnostics().backpressure, 1);
        assert_eq!(supervisor.diagnostics().coalesced_delta, 1);

        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let worker = supervisor.clone();
        let boundary = candidate(
            3,
            AgentEvent::TurnStage {
                session_id: "cli".into(),
                stage: TurnStage::UsingTool,
                ts: now_millis(),
                engine_reported_ttft_ms: None,
                retry_attempt: None,
            },
        );
        let join = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            worker.enqueue(boundary);
            done_tx.send(()).unwrap();
        });
        started_rx.recv().unwrap();
        assert!(done_rx
            .recv_timeout(std::time::Duration::from_millis(30))
            .is_err());
        {
            let mut queue = supervisor.inner.queue.lock().unwrap();
            queue.items.pop_front();
        }
        supervisor.inner.queue_space.notify_all();
        done_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap();
        join.join().unwrap();
        let mut queue = supervisor.inner.queue.lock().unwrap();
        assert_eq!(queue.items.len(), 1, "非 delta 边界事件不得在队列满时丢失");
        assert!(matches!(queue.items[0].event, AgentEvent::TurnStage { .. }));
        queue.items.clear();
        queue.draining = false;
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
        assert_eq!(loaded.event_seq, 1);
        assert_eq!(loaded.permission_profile, "auto");
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
