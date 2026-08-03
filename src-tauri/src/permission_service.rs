use crate::permissions::{PermissionDecision, PermissionEffect};
use crate::sessions::SessionHistoryStore;
use crate::util::now_millis;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{oneshot, Mutex, RwLock};

const MAX_HEADER_BYTES: usize = 16 * 1024;
const MAX_BODY_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone)]
pub struct PermissionSessionContext {
    pub engine: String,
    pub history_session_id: String,
    pub turn_id: String,
    pub cwd: String,
    pub permission_profile: String,
}

pub struct PermissionRegistration {
    pub endpoint: String,
    pub token: String,
}

impl fmt::Debug for PermissionRegistration {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PermissionRegistration")
            .field("endpoint", &self.endpoint)
            .field("token", &"[redacted]")
            .finish()
    }
}

pub struct PermissionService {
    addr: SocketAddr,
    store: SessionHistoryStore,
    contexts: Arc<RwLock<HashMap<String, PermissionSessionContext>>>,
    health: Arc<RwLock<PermissionServiceHealth>>,
    shutdown: Mutex<Option<oneshot::Sender<()>>>,
    task: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionServiceHealth {
    pub started_at: i64,
    pub last_decision_at: Option<i64>,
    pub last_error: Option<String>,
    pub policy_version: u64,
    pub registered_sessions: usize,
}

impl PermissionService {
    pub async fn start(store: SessionHistoryStore) -> Result<Self, String> {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .map_err(|e| format!("启动权限服务失败：{e}"))?;
        let addr = listener.local_addr().map_err(|e| e.to_string())?;
        let contexts = Arc::new(RwLock::new(HashMap::new()));
        let server_contexts = contexts.clone();
        let health = Arc::new(RwLock::new(PermissionServiceHealth {
            started_at: now_millis(),
            last_decision_at: None,
            last_error: None,
            policy_version: store.permission_policy_version()?,
            registered_sessions: 0,
        }));
        let server_health = health.clone();
        let server_store = store.clone();
        let (shutdown_tx, mut shutdown_rx) = oneshot::channel();
        let task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = &mut shutdown_rx => break,
                    accepted = listener.accept() => {
                        let Ok((stream, peer)) = accepted else { continue; };
                        if !peer.ip().is_loopback() { continue; }
                        let contexts = server_contexts.clone();
                        let health = server_health.clone();
                        let store = server_store.clone();
                        tokio::spawn(async move {
                            if let Err(error) = handle_connection(stream, contexts, health.clone(), store).await {
                                health.write().await.last_error = Some(error);
                            }
                        });
                    }
                }
            }
        });
        Ok(Self {
            addr,
            store,
            contexts,
            health,
            shutdown: Mutex::new(Some(shutdown_tx)),
            task: Mutex::new(Some(task)),
        })
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    pub async fn register(&self, context: PermissionSessionContext) -> PermissionRegistration {
        let mut random = [0_u8; 32];
        rand::rng().fill_bytes(&mut random);
        let token = random
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        self.contexts.write().await.insert(token.clone(), context);
        PermissionRegistration {
            endpoint: format!("http://{}", self.addr),
            token,
        }
    }

    pub async fn health(&self) -> PermissionServiceHealth {
        let mut health = self.health.read().await.clone();
        if let Ok(policy_version) = self.store.permission_policy_version() {
            health.policy_version = policy_version;
        }
        health.registered_sessions = self.contexts.read().await.len();
        health
    }

    pub async fn update_context(
        &self,
        token: &str,
        context: PermissionSessionContext,
    ) -> Result<(), String> {
        let mut contexts = self.contexts.write().await;
        let slot = contexts
            .get_mut(token)
            .ok_or_else(|| "permission session token is not registered".to_string())?;
        *slot = context;
        Ok(())
    }

    pub async fn self_check(&self, token: &str) -> Result<(), String> {
        let running = self
            .task
            .lock()
            .await
            .as_ref()
            .is_some_and(|task| !task.is_finished());
        if !running {
            return Err("权限服务不可用".to_string());
        }
        if !self.contexts.read().await.contains_key(token) {
            return Err("权限会话令牌未注册或已失效".to_string());
        }
        self.store.permission_policy_version()?;
        Ok(())
    }

    pub async fn unregister(&self, token: &str) {
        self.contexts.write().await.remove(token);
    }

    pub async fn shutdown(&self) {
        if let Some(shutdown) = self.shutdown.lock().await.take() {
            let _ = shutdown.send(());
        }
        if let Some(task) = self.task.lock().await.take() {
            let _ = task.await;
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DecideRequest {
    history_session_id: String,
    turn_id: String,
    tool_call_id: String,
    principal: String,
    tool_name: String,
    input: serde_json::Value,
    cwd: String,
}

async fn handle_connection(
    mut stream: TcpStream,
    contexts: Arc<RwLock<HashMap<String, PermissionSessionContext>>>,
    health: Arc<RwLock<PermissionServiceHealth>>,
    store: SessionHistoryStore,
) -> Result<(), String> {
    let request = match read_http_request(&mut stream).await {
        Ok(request) => request,
        Err(error) => {
            return write_json(
                &mut stream,
                error.status,
                serde_json::json!({"error": error.code}),
            )
            .await
        }
    };
    let policy_version = store.permission_policy_version().unwrap_or(1);
    let token = request
        .headers
        .get("authorization")
        .and_then(|value| value.strip_prefix("Bearer "));
    let Some(token) = token else {
        return write_json(
            &mut stream,
            401,
            serde_json::json!({"error":"unauthorized"}),
        )
        .await;
    };
    let context = contexts.read().await.get(token).cloned();
    let Some(context) = context else {
        return write_json(
            &mut stream,
            401,
            serde_json::json!({"error":"unauthorized"}),
        )
        .await;
    };
    if request.path == "/health" {
        return write_json(&mut stream, 200, serde_json::json!({"ok":true})).await;
    }
    if request.path != "/v1/decide" || request.method != "POST" {
        return write_json(&mut stream, 404, serde_json::json!({"error":"not_found"})).await;
    }
    let parsed: DecideRequest = match serde_json::from_slice(&request.body) {
        Ok(value) => value,
        Err(_) => {
            return write_json(
                &mut stream,
                400,
                serde_json::json!({"error":"invalid_json"}),
            )
            .await
        }
    };
    let identity_matches = parsed.history_session_id == context.history_session_id
        && parsed.turn_id == context.turn_id
        && normalize_path(&parsed.cwd) == normalize_path(&context.cwd);
    let decision = if !identity_matches {
        PermissionDecision {
            effect: PermissionEffect::Deny,
            reason: "permission context identity mismatch".to_string(),
            rule_id: None,
            policy_version,
        }
    } else {
        let action = crate::permissions::normalize_tool_action_for_principal(
            &context.engine,
            &parsed.history_session_id,
            &parsed.turn_id,
            &parsed.tool_call_id,
            &parsed.principal,
            &parsed.tool_name,
            &parsed.input,
            Some(&parsed.cwd),
        );
        let decision = store
            .evaluate_permission_action(&action)
            .unwrap_or_else(|error| PermissionDecision {
                effect: PermissionEffect::Deny,
                reason: format!("permission kernel failure: {error}"),
                rule_id: None,
                policy_version,
            });
        if decision.effect == PermissionEffect::Ask
            && context.permission_profile == "auto"
            && crate::permissions::safe_network_read_action_is_eligible(&action)
        {
            PermissionDecision {
                effect: PermissionEffect::Allow,
                reason: "safe network read auto-allowed by runtime profile".to_string(),
                rule_id: None,
                policy_version,
            }
        } else {
            decision
        }
    };
    {
        let mut current = health.write().await;
        current.last_decision_at = Some(now_millis());
        current.last_error = decision
            .reason
            .starts_with("permission kernel failure")
            .then(|| decision.reason.clone());
    }
    write_json(
        &mut stream,
        200,
        serde_json::to_value(decision).map_err(|e| e.to_string())?,
    )
    .await
}

struct HttpRequest {
    method: String,
    path: String,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

struct HttpRequestError {
    status: u16,
    code: &'static str,
}

impl HttpRequestError {
    fn invalid() -> Self {
        Self {
            status: 400,
            code: "invalid_request",
        }
    }

    fn too_large() -> Self {
        Self {
            status: 413,
            code: "request_too_large",
        }
    }
}

async fn read_http_request(stream: &mut TcpStream) -> Result<HttpRequest, HttpRequestError> {
    let mut data = Vec::new();
    let header_end = loop {
        if data.len() > MAX_HEADER_BYTES {
            return Err(HttpRequestError::too_large());
        }
        let mut chunk = [0_u8; 2048];
        let read = stream
            .read(&mut chunk)
            .await
            .map_err(|_| HttpRequestError::invalid())?;
        if read == 0 {
            return Err(HttpRequestError::invalid());
        }
        data.extend_from_slice(&chunk[..read]);
        if let Some(index) = data.windows(4).position(|window| window == b"\r\n\r\n") {
            break index + 4;
        }
    };
    let header_text =
        std::str::from_utf8(&data[..header_end]).map_err(|_| HttpRequestError::invalid())?;
    let mut lines = header_text.split("\r\n");
    let mut request_line = lines.next().unwrap_or_default().split_whitespace();
    let method = request_line.next().unwrap_or_default().to_string();
    let path = request_line.next().unwrap_or_default().to_string();
    let mut headers = HashMap::new();
    for line in lines.filter(|line| !line.is_empty()) {
        if let Some((name, value)) = line.split_once(':') {
            headers.insert(name.trim().to_lowercase(), value.trim().to_string());
        }
    }
    let content_length = headers
        .get("content-length")
        .map(|value| {
            value
                .parse::<usize>()
                .map_err(|_| HttpRequestError::invalid())
        })
        .transpose()?
        .unwrap_or(0);
    if content_length > MAX_BODY_BYTES {
        return Err(HttpRequestError::too_large());
    }
    while data.len() - header_end < content_length {
        let remaining = content_length - (data.len() - header_end);
        let mut chunk = vec![0_u8; remaining.min(4096)];
        stream
            .read_exact(&mut chunk)
            .await
            .map_err(|_| HttpRequestError::invalid())?;
        data.extend_from_slice(&chunk);
    }
    Ok(HttpRequest {
        method,
        path,
        headers,
        body: data[header_end..header_end + content_length].to_vec(),
    })
}

async fn write_json(
    stream: &mut TcpStream,
    status: u16,
    body: serde_json::Value,
) -> Result<(), String> {
    let bytes = serde_json::to_vec(&body).map_err(|e| e.to_string())?;
    let status_text = match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        413 => "Payload Too Large",
        404 => "Not Found",
        _ => "Error",
    };
    let headers = format!(
        "HTTP/1.1 {status} {status_text}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        bytes.len()
    );
    stream
        .write_all(headers.as_bytes())
        .await
        .map_err(|e| e.to_string())?;
    stream.write_all(&bytes).await.map_err(|e| e.to_string())
}

fn normalize_path(value: &str) -> String {
    value
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::{PermissionService, PermissionSessionContext, MAX_BODY_BYTES};
    use crate::permissions::{
        Capability, PermissionEffect, PermissionRule, PermissionScope, PermissionScopeBinding,
    };
    use crate::sessions::SessionHistoryStore;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;

    fn context() -> PermissionSessionContext {
        PermissionSessionContext {
            engine: "claude-code".to_string(),
            history_session_id: "history-1".to_string(),
            turn_id: "turn-1".to_string(),
            cwd: "D:/repo".to_string(),
            permission_profile: "standard".to_string(),
        }
    }

    #[tokio::test]
    async fn runtime_actions_are_evaluated_without_an_engine_version_gate() {
        let root =
            std::env::temp_dir().join(format!("helm-permission-service-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let store = SessionHistoryStore::new(root.join("sessions.sqlite"));
        let service = PermissionService::start(store).await.unwrap();
        assert!(service.addr().ip().is_loopback());
        let registration = service.register(context()).await;
        assert!(registration.token.len() >= 32);
        assert!(!format!("{registration:?}").contains(&registration.token));

        let client = reqwest::Client::new();
        let health = client
            .get(format!("http://{}/health", service.addr()))
            .send()
            .await
            .unwrap();
        assert_eq!(health.status(), reqwest::StatusCode::UNAUTHORIZED);

        let response = client
            .post(format!("http://{}/v1/decide", service.addr()))
            .bearer_auth(&registration.token)
            .json(&serde_json::json!({
                "historySessionId": "history-1",
                "turnId": "turn-1",
                "toolCallId": "tool-1",
                "principal": "main-agent",
                "toolName": "Bash",
                "input": {"command": "ls"},
                "cwd": "D:/repo"
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let body: serde_json::Value = response.json().await.unwrap();
        assert_eq!(body["effect"], "ask");
        let health = service.health().await;
        assert_eq!(health.registered_sessions, 1);
        assert!(health.last_decision_at.is_some());
        assert_eq!(health.policy_version, 1);

        service.shutdown().await;
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn identity_mismatch_fails_closed_without_using_another_session_context() {
        let root = std::env::temp_dir().join(format!(
            "helm-permission-service-mismatch-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let service = PermissionService::start(SessionHistoryStore::new(root.join("db.sqlite")))
            .await
            .unwrap();
        let registration = service.register(context()).await;
        let response = reqwest::Client::new()
            .post(format!("http://{}/v1/decide", service.addr()))
            .bearer_auth(&registration.token)
            .json(&serde_json::json!({
                "historySessionId": "history-other",
                "turnId": "turn-1",
                "toolCallId": "tool-1",
                "principal": "main-agent",
                "toolName": "Write",
                "input": {"file_path": "secret.txt"},
                "cwd": "D:/repo"
            }))
            .send()
            .await
            .unwrap();
        let body: serde_json::Value = response.json().await.unwrap();
        assert_eq!(body["effect"], "deny");
        assert!(body["reason"]
            .as_str()
            .unwrap()
            .contains("identity mismatch"));
        service.shutdown().await;
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn malformed_and_oversized_requests_receive_explicit_client_errors() {
        let root = std::env::temp_dir().join(format!(
            "helm-permission-service-bounds-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let service = PermissionService::start(SessionHistoryStore::new(root.join("db.sqlite")))
            .await
            .unwrap();

        let malformed = send_raw_request(
            service.addr(),
            b"POST /v1/decide HTTP/1.1\r\nContent-Length: nope\r\n\r\n",
        )
        .await;
        assert!(malformed.starts_with("HTTP/1.1 400 Bad Request"));
        assert!(malformed.contains("invalid_request"));

        let oversized = format!(
            "POST /v1/decide HTTP/1.1\r\nContent-Length: {}\r\n\r\n",
            MAX_BODY_BYTES + 1
        );
        let oversized = send_raw_request(service.addr(), oversized.as_bytes()).await;
        assert!(oversized.starts_with("HTTP/1.1 413 Payload Too Large"));
        assert!(oversized.contains("request_too_large"));

        service.shutdown().await;
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn health_reports_the_persisted_permission_policy_version() {
        let root = std::env::temp_dir().join(format!(
            "helm-permission-service-policy-version-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let store = SessionHistoryStore::new(root.join("db.sqlite"));
        let service = PermissionService::start(store.clone()).await.unwrap();
        assert_eq!(service.health().await.policy_version, 1);

        store
            .save_permission_rule(&PermissionRule {
                id: "health-policy-version".to_string(),
                principal: "main-agent".to_string(),
                effect: PermissionEffect::Allow,
                scope: PermissionScope::Global,
                scope_binding: PermissionScopeBinding::default(),
                engine: Some("claude-code".to_string()),
                capability: Capability::FileRead,
                operation: None,
                resource_pattern: None,
                created_at: 1,
                expires_at: None,
                max_uses: None,
                uses: 0,
            })
            .unwrap();

        assert_eq!(service.health().await.policy_version, 2);
        service.shutdown().await;
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn session_tokens_rotate_unregister_and_fail_self_check_after_shutdown() {
        let root = std::env::temp_dir().join(format!(
            "helm-permission-service-lifecycle-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let service = PermissionService::start(SessionHistoryStore::new(root.join("db.sqlite")))
            .await
            .unwrap();
        let first = service.register(context()).await;
        let second = service.register(context()).await;
        assert_ne!(
            first.token, second.token,
            "新 Session 必须轮换 bearer token"
        );
        service.self_check(&first.token).await.unwrap();

        service.unregister(&first.token).await;
        assert!(service.self_check(&first.token).await.is_err());
        service.self_check(&second.token).await.unwrap();

        service.shutdown().await;
        assert!(service.self_check(&second.token).await.is_err());
        let _ = std::fs::remove_dir_all(root);
    }

    async fn send_raw_request(addr: std::net::SocketAddr, request: &[u8]) -> String {
        let mut stream = TcpStream::connect(addr).await.unwrap();
        stream.write_all(request).await.unwrap();
        stream.shutdown().await.unwrap();
        let mut response = Vec::new();
        stream.read_to_end(&mut response).await.unwrap();
        String::from_utf8(response).unwrap()
    }
}
