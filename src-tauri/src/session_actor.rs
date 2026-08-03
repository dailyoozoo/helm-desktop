use crate::adapter::{ApprovalDecision, PermissionProfile};
use crate::runtime_registry::{RuntimeOwnerRef, RuntimeRegistry};
use crate::sessions::SessionMessage;
use crate::turn_start::TurnExecutionSpec;
use tokio::sync::{mpsc, oneshot};

enum SessionActorCommand {
    ReserveTurn(oneshot::Sender<Result<(), String>>),
    ReleaseTurn,
    SendReserved {
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
    reserved: bool,
    cancelled: bool,
}

impl DispatchReservation {
    fn reserve(&mut self) {
        self.reserved = true;
        self.cancelled = false;
    }

    fn cancel(&mut self) {
        if self.reserved {
            self.cancelled = true;
        }
    }

    fn consume(&mut self) -> Result<bool, String> {
        if !self.reserved {
            return Err("SessionActor 没有对应的 Send reservation".to_string());
        }
        let cancelled = self.cancelled;
        *self = Self::default();
        Ok(cancelled)
    }

    fn clear(&mut self) {
        *self = Self::default();
    }
}

#[derive(Clone)]
pub struct SessionActorHandle {
    owner: RuntimeOwnerRef,
    tx: mpsc::UnboundedSender<SessionActorCommand>,
}

impl SessionActorHandle {
    pub fn start(owner: RuntimeOwnerRef, registry: RuntimeRegistry) -> Self {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let actor_owner = owner.clone();
        tauri::async_runtime::spawn(async move {
            let mut dispatch = DispatchReservation::default();
            while let Some(command) = rx.recv().await {
                match command {
                    SessionActorCommand::ReserveTurn(responder) => {
                        let result = registry.reserve_turn(&actor_owner).await;
                        if result.is_ok() {
                            dispatch.reserve();
                        }
                        let _ = responder.send(result);
                    }
                    SessionActorCommand::ReleaseTurn => {
                        let _ = registry.release_turn_reservation(&actor_owner).await;
                        dispatch.clear();
                    }
                    SessionActorCommand::SendReserved {
                        text,
                        attachments,
                        spec,
                        responder,
                    } => {
                        let result = match dispatch.consume() {
                            Err(error) => Err(error),
                            Ok(true) => {
                                let _ = registry.release_turn_reservation(&actor_owner).await;
                                Err("发送提交前已收到 Stop，当前 Turn 未投递".to_string())
                            }
                            Ok(false) => {
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
                        dispatch.cancel();
                        let _ = responder.send(registry.interrupt(&actor_owner).await);
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
        Self { owner, tx }
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

    pub async fn reserve_turn(&self) -> Result<(), String> {
        self.request(SessionActorCommand::ReserveTurn).await
    }

    pub fn release_turn_reservation(&self) {
        let _ = self.tx.send(SessionActorCommand::ReleaseTurn);
    }

    pub async fn send_reserved(
        &self,
        text: String,
        attachments: Vec<String>,
        spec: TurnExecutionSpec,
    ) -> Result<(), String> {
        self.request(|responder| SessionActorCommand::SendReserved {
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
        self.request(SessionActorCommand::Interrupt).await
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
        self.request(SessionActorCommand::Close).await
    }
}

#[cfg(test)]
mod tests {
    use super::DispatchReservation;

    #[test]
    fn stop_between_reserve_and_dispatch_cancels_exactly_that_send() {
        let mut state = DispatchReservation::default();
        state.reserve();
        state.cancel();
        assert_eq!(state.consume().unwrap(), true);
        assert!(state.consume().is_err());
        state.reserve();
        assert_eq!(state.consume().unwrap(), false);
    }
}
