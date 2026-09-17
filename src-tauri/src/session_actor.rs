use crate::adapter::{AgentSession, ApprovalDecision, PermissionProfile};
use crate::capability_registry::EngineCapabilitySnapshot;
use crate::runtime_registry::{PendingRuntimeSession, RuntimeOwnerRef, RuntimeRegistry};
use crate::sessions::SessionMessage;
use crate::turn_start::{RuntimeRoute, TurnExecutionSpec};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};
use tokio::sync::{mpsc, oneshot};

enum SessionActorCommand {
    ReserveTurn {
        epoch: u64,
        responder: oneshot::Sender<Result<(), String>>,
    },
    ReleaseTurn {
        epoch: u64,
    },
    ConfigureReservedRuntime {
        epoch: u64,
        replacement: Option<PendingRuntimeSession>,
        route: RuntimeRoute,
        capability: EngineCapabilitySnapshot,
        cwd: String,
        responder: oneshot::Sender<Result<(), String>>,
    },
    SendReserved {
        epoch: u64,
        text: String,
        attachments: Vec<String>,
        spec: TurnExecutionSpec,
        responder: oneshot::Sender<Result<(), String>>,
    },
    PermissionProfile(oneshot::Sender<Result<PermissionProfile, String>>),
    SetPermissionProfile {
        profile: PermissionProfile,
        responder: oneshot::Sender<Result<(), String>>,
    },
    PermissionConfirmation(oneshot::Sender<Result<(String, String), String>>),
    Approve {
        request_id: String,
        decision: ApprovalDecision,
        responder: oneshot::Sender<Result<(), String>>,
    },
    Interrupt(oneshot::Sender<Result<(), String>>),
    CompactContext(oneshot::Sender<Result<(), String>>),
    ResetContext {
        messages: Vec<SessionMessage>,
        responder: oneshot::Sender<Result<(), String>>,
    },
    SetDisabledMcp {
        disabled: Vec<String>,
        responder: oneshot::Sender<Result<(), String>>,
    },
    Close(oneshot::Sender<Result<(), String>>),
}

#[derive(Default)]
struct DispatchReservation {
    epoch: Option<u64>,
}

impl DispatchReservation {
    fn reserve(&mut self, epoch: u64) {
        self.epoch = Some(epoch);
    }

    fn consume(&mut self, epoch: u64) -> Result<(), String> {
        if self.epoch != Some(epoch) {
            return Err("SessionActor 没有对应的 Send reservation".to_string());
        }
        *self = Self::default();
        Ok(())
    }

    fn clear(&mut self) {
        *self = Self::default();
    }
}

#[derive(Clone)]
pub struct SessionActorHandle {
    owner: RuntimeOwnerRef,
    tx: mpsc::UnboundedSender<SessionActorCommand>,
    dispatch_epoch: Arc<AtomicU64>,
}

impl SessionActorHandle {
    pub fn start(owner: RuntimeOwnerRef, registry: RuntimeRegistry) -> Self {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let actor_owner = owner.clone();
        let dispatch_epoch = Arc::new(AtomicU64::new(0));
        let actor_epoch = dispatch_epoch.clone();
        tauri::async_runtime::spawn(async move {
            let mut dispatch = DispatchReservation::default();
            while let Some(command) = rx.recv().await {
                match command {
                    SessionActorCommand::ReserveTurn { epoch, responder } => {
                        if responder.is_closed() || actor_epoch.load(Ordering::SeqCst) != epoch {
                            let _ = responder
                                .send(Err("发送准备期间已取消，当前 Turn 未投递".to_string()));
                            continue;
                        }
                        let mut result = registry.reserve_turn(&actor_owner).await;
                        if result.is_ok() {
                            if !responder.is_closed() && actor_epoch.load(Ordering::SeqCst) == epoch
                            {
                                dispatch.reserve(epoch);
                            } else {
                                let _ = registry.release_turn_reservation(&actor_owner).await;
                                result = Err("发送准备期间已取消，当前 Turn 未投递".to_string());
                            }
                        }
                        let reserved = result.is_ok();
                        if responder.send(result).is_err()
                            && reserved
                            && dispatch.consume(epoch).is_ok()
                        {
                            let _ = registry.release_turn_reservation(&actor_owner).await;
                        }
                    }
                    SessionActorCommand::ReleaseTurn { epoch } => {
                        if dispatch.consume(epoch).is_ok() {
                            let _ = registry.release_turn_reservation(&actor_owner).await;
                        }
                    }
                    SessionActorCommand::ConfigureReservedRuntime {
                        epoch,
                        replacement,
                        route,
                        capability,
                        cwd,
                        responder,
                    } => {
                        if responder.is_closed()
                            || dispatch.epoch != Some(epoch)
                            || actor_epoch.load(Ordering::SeqCst) != epoch
                        {
                            if let Some(replacement) = replacement {
                                replacement.shutdown().await;
                            }
                            if dispatch.consume(epoch).is_ok() {
                                let _ = registry.release_turn_reservation(&actor_owner).await;
                            }
                            let _ = responder
                                .send(Err("发送准备期间已取消，当前 Turn 未投递".to_string()));
                            continue;
                        }
                        let mut result = if let Some(replacement) = replacement {
                            registry
                                .replace_reserved_session(
                                    &actor_owner,
                                    replacement.into_session(),
                                    &route,
                                    &capability,
                                    &cwd,
                                )
                                .await
                                .map(|_| ())
                        } else {
                            registry
                                .update_reserved_capability_snapshot(&actor_owner, capability)
                                .await
                        };
                        if responder.is_closed() || actor_epoch.load(Ordering::SeqCst) != epoch {
                            if dispatch.consume(epoch).is_ok() {
                                let _ = registry.release_turn_reservation(&actor_owner).await;
                            }
                            result = Err("发送准备期间已取消，当前 Turn 未投递".to_string());
                        }
                        let _ = responder.send(result);
                    }
                    SessionActorCommand::SendReserved {
                        epoch,
                        text,
                        attachments,
                        spec,
                        responder,
                    } => {
                        let result = match dispatch.consume(epoch) {
                            Err(error) => Err(error),
                            Ok(()) if actor_epoch.load(Ordering::SeqCst) != epoch => {
                                let _ = registry.release_turn_reservation(&actor_owner).await;
                                Err("发送提交前已收到 Stop，当前 Turn 未投递".to_string())
                            }
                            Ok(()) => {
                                registry
                                    .send_reserved(&actor_owner, text, attachments, spec)
                                    .await
                            }
                        };
                        let _ = responder.send(result);
                    }
                    SessionActorCommand::PermissionProfile(responder) => {
                        let _ = responder.send(registry.permission_profile(&actor_owner).await);
                    }
                    SessionActorCommand::SetPermissionProfile { profile, responder } => {
                        let _ = responder
                            .send(registry.set_permission_profile(&actor_owner, profile).await);
                    }
                    SessionActorCommand::PermissionConfirmation(responder) => {
                        let _ = responder
                            .send(registry.permission_confirmation_context(&actor_owner).await);
                    }
                    SessionActorCommand::Approve {
                        request_id,
                        decision,
                        responder,
                    } => {
                        let _ = responder
                            .send(registry.approve(&actor_owner, request_id, decision).await);
                    }
                    SessionActorCommand::Interrupt(responder) => {
                        if dispatch.epoch.is_some() {
                            dispatch.clear();
                            let _ = registry.release_turn_reservation(&actor_owner).await;
                        }
                        let _ = responder.send(registry.interrupt(&actor_owner).await);
                    }
                    SessionActorCommand::CompactContext(responder) => {
                        let _ = responder.send(registry.compact_context(&actor_owner).await);
                    }
                    SessionActorCommand::ResetContext {
                        messages,
                        responder,
                    } => {
                        let _ =
                            responder.send(registry.reset_context(&actor_owner, messages).await);
                    }
                    SessionActorCommand::SetDisabledMcp {
                        disabled,
                        responder,
                    } => {
                        let _ =
                            responder.send(registry.set_disabled_mcp(&actor_owner, disabled).await);
                    }
                    SessionActorCommand::Close(responder) => {
                        let _ = responder.send(registry.close(&actor_owner).await);
                        break;
                    }
                }
            }
        });
        Self {
            owner,
            tx,
            dispatch_epoch,
        }
    }

    pub fn owner(&self) -> &RuntimeOwnerRef {
        &self.owner
    }

    async fn request<T>(
        &self,
        make: impl FnOnce(oneshot::Sender<Result<T, String>>) -> SessionActorCommand,
    ) -> Result<T, String> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(make(tx))
            .map_err(|_| "SessionActor 已关闭".to_string())?;
        rx.await
            .map_err(|_| "SessionActor 未返回结果".to_string())?
    }

    pub fn begin_dispatch(&self) -> u64 {
        self.dispatch_epoch.fetch_add(1, Ordering::SeqCst) + 1
    }

    pub fn ensure_dispatch_current(&self, epoch: u64) -> Result<(), String> {
        if self.dispatch_epoch.load(Ordering::SeqCst) == epoch {
            Ok(())
        } else {
            Err("发送准备期间已取消，当前 Turn 未投递".to_string())
        }
    }

    pub async fn reserve_turn(&self, epoch: u64) -> Result<(), String> {
        self.request(|responder| SessionActorCommand::ReserveTurn { epoch, responder })
            .await
    }

    pub fn release_turn_reservation(&self, epoch: u64) {
        let _ = self.tx.send(SessionActorCommand::ReleaseTurn { epoch });
    }

    pub async fn configure_reserved_runtime(
        &self,
        epoch: u64,
        replacement: Option<AgentSession>,
        route: RuntimeRoute,
        capability: EngineCapabilitySnapshot,
        cwd: String,
    ) -> Result<(), String> {
        self.request(|responder| SessionActorCommand::ConfigureReservedRuntime {
            epoch,
            replacement: replacement.map(PendingRuntimeSession::new),
            route,
            capability,
            cwd,
            responder,
        })
        .await
    }

    pub async fn send_reserved(
        &self,
        epoch: u64,
        text: String,
        attachments: Vec<String>,
        spec: TurnExecutionSpec,
    ) -> Result<(), String> {
        self.request(|responder| SessionActorCommand::SendReserved {
            epoch,
            text,
            attachments,
            spec,
            responder,
        })
        .await
    }

    pub async fn permission_profile(&self) -> Result<PermissionProfile, String> {
        self.request(SessionActorCommand::PermissionProfile).await
    }

    pub async fn set_permission_profile(&self, profile: PermissionProfile) -> Result<(), String> {
        self.request(|responder| SessionActorCommand::SetPermissionProfile { profile, responder })
            .await
    }

    pub async fn permission_confirmation_context(&self) -> Result<(String, String), String> {
        self.request(SessionActorCommand::PermissionConfirmation)
            .await
    }

    pub async fn approve(
        &self,
        request_id: String,
        decision: ApprovalDecision,
    ) -> Result<(), String> {
        self.request(|responder| SessionActorCommand::Approve {
            request_id,
            decision,
            responder,
        })
        .await
    }

    pub async fn interrupt(&self) -> Result<(), String> {
        self.dispatch_epoch.fetch_add(1, Ordering::SeqCst);
        self.request(SessionActorCommand::Interrupt).await
    }

    pub async fn compact_context(&self) -> Result<(), String> {
        self.request(SessionActorCommand::CompactContext).await
    }

    pub async fn reset_context(&self, messages: Vec<SessionMessage>) -> Result<(), String> {
        self.request(|responder| SessionActorCommand::ResetContext {
            messages,
            responder,
        })
        .await
    }

    pub async fn set_disabled_mcp(&self, disabled: Vec<String>) -> Result<(), String> {
        self.request(|responder| SessionActorCommand::SetDisabledMcp {
            disabled,
            responder,
        })
        .await
    }

    pub async fn close(&self) -> Result<(), String> {
        self.dispatch_epoch.fetch_add(1, Ordering::SeqCst);
        self.request(SessionActorCommand::Close).await
    }
}

#[cfg(test)]
mod tests {
    use super::DispatchReservation;

    #[test]
    fn stale_dispatch_cannot_consume_or_release_a_new_reservation() {
        let mut state = DispatchReservation::default();
        state.reserve(1);
        state.clear();
        state.reserve(2);
        assert!(state.consume(1).is_err());
        assert!(state.consume(2).is_ok());
        assert!(state.consume(2).is_err());
    }

    #[tokio::test]
    async fn stop_invalidates_preflight_before_any_reservation_exists() {
        let (tx, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        let handle = super::SessionActorHandle {
            owner: crate::runtime_registry::RuntimeOwnerRef::Session("session".into()),
            tx,
            dispatch_epoch: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
        };
        let first = handle.begin_dispatch();
        assert!(handle.ensure_dispatch_current(first).is_ok());
        let pending_stop = handle.interrupt();
        tokio::pin!(pending_stop);
        tokio::select! {
            _ = &mut pending_stop => panic!("interrupt must wait for the actor"),
            command = receiver.recv() => match command.unwrap() {
                super::SessionActorCommand::Interrupt(responder) => { responder.send(Ok(())).unwrap(); }
                _ => panic!("unexpected command"),
            }
        }
        pending_stop.await.unwrap();
        assert!(handle.ensure_dispatch_current(first).is_err());
        let next = handle.begin_dispatch();
        assert!(handle.ensure_dispatch_current(next).is_ok());
    }
}
