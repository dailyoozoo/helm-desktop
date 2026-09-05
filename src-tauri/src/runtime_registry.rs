use crate::adapter::{AgentSession, ApprovalDecision, PermissionProfile};
use crate::budget::{BudgetDimension, TurnBudgetSnapshot};
use crate::operations::{
    ModelOnlyOperationOutput, ModelOnlyOperationPolicy, OperationExecutionSpec,
};
use crate::protocol::{AgentEvent, EngineId, StopReason};
use crate::reasoning::ReasoningEffort;
use crate::sessions::SessionHistoryStore;
use crate::turn_start::{digest_json, RuntimeRoute, TurnExecutionSpec};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use tokio::io::AsyncReadExt;
use tokio::sync::RwLock;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum RuntimeOwnerRef {
    Session(String),
    Operation(String),
}

impl RuntimeOwnerRef {
    pub fn session(session_id: impl Into<String>) -> Self {
        Self::Session(session_id.into())
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Self::Session(_) => "session",
            Self::Operation(_) => "operation",
        }
    }

    pub fn id(&self) -> &str {
        match self {
            Self::Session(id) | Self::Operation(id) => id,
        }
    }

    fn key(&self) -> String {
        format!("{}:{}", self.kind(), self.id())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeGeneration {
    pub id: String,
    pub owner: RuntimeOwnerRef,
    pub engine_id: String,
    pub compatibility_key: String,
    pub engine_profile_digest: String,
    pub provider_launch_profile_ref: String,
    pub provider_launch_profile_digest: String,
    pub capability_snapshot_id: String,
    pub canonical_cwd: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct NativeSessionRef {
    pub id: String,
    pub generation_id: String,
    pub owner: RuntimeOwnerRef,
    pub engine_id: String,
    pub native_kind: String,
    pub native_id: String,
    pub launch_profile_identity: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TurnAttempt {
    pub turn_id: String,
    pub attempt_no: u64,
    pub owner: RuntimeOwnerRef,
    pub generation_id: String,
    pub runtime_compatibility_key: String,
    pub input_native_ref_id: Option<String>,
    pub output_native_ref_id: Option<String>,
    pub observed_model_id: Option<String>,
    pub observed_reasoning_effort: Option<String>,
    pub actual_capability_snapshot: Option<serde_json::Value>,
    pub delivery_state: String,
    pub terminal_receipt: Option<String>,
    pub created_at: i64,
    pub accepted_at: Option<i64>,
    pub ended_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TurnRecoveryInput {
    pub turn_id: String,
    pub attempt_no: u64,
    pub owner: RuntimeOwnerRef,
    pub generation_id: String,
    pub delivery_state: String,
    pub input_native_ref_id: Option<String>,
    pub output_native_ref_id: Option<String>,
}

#[derive(Clone)]
struct RuntimeEntry {
    generation: RuntimeGeneration,
    session: AgentSession,
    base_compatibility_key: String,
    process_config: RuntimeProcessConfig,
}

#[derive(Clone)]
struct OperationRuntimeEntry {
    generation: RuntimeGeneration,
    cancel: Arc<tokio::sync::Notify>,
}

struct OperationTempDir(std::path::PathBuf);

impl Drop for OperationTempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// 引擎侧模型拒绝映射（九次反馈）：模型目录漂移或旧配置迁移会让分叉/旁路提问路由到
/// 引擎链路不认识的模型 ID，CLI 以非零退出并在 stderr 留下含糊的内部 JSON（如
/// `unrecognized_model`）。命中时翻译成可操作的中文指引并附原始片段（截断＋脱敏），
/// 其余失败保持既有 tag 与形状。
fn map_model_only_cli_failure(
    error_tag_prefix: &str,
    status: &std::process::ExitStatus,
    stderr_bytes: &[u8],
) -> String {
    let stderr = crate::redaction::redact_text(&String::from_utf8_lossy(stderr_bytes));
    let clipped: String = stderr.chars().take(1000).collect();
    if stderr.contains("unrecognized_model") {
        let hint = extract_json_string_field(&stderr, "model")
            .map(|model| format!("被拒模型：{model}。"))
            .unwrap_or_default();
        return format!(
            "[{error_tag_prefix}_model_rejected] 引擎链路拒绝了本次路由的模型。{hint}请到「AI 配置」核对该引擎绑定的快速模型/主模型是否为服务商实际提供的 ID 后重试 | 原始输出：{clipped}"
        );
    }
    format!("[{error_tag_prefix}_cli_failed] Claude CLI exit={status} {clipped}")
}

/// 从混有非 JSON 前后缀的文本中提取字符串字段值（引擎 stderr 不保证是完整 JSON，
/// 因此只做定位提取，不做整体解析）。
fn extract_json_string_field(text: &str, field: &str) -> Option<String> {
    let needle = format!("\"{field}\":\"");
    let start = text.find(&needle)? + needle.len();
    let rest = &text[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

async fn read_bounded_operation_output(
    mut stdout: tokio::process::ChildStdout,
    mut stderr: tokio::process::ChildStderr,
    limit: u64,
    exceeded: Arc<tokio::sync::Notify>,
) -> Result<(Vec<u8>, Vec<u8>), String> {
    let mut stdout_bytes = Vec::new();
    let mut stderr_bytes = Vec::new();
    let mut stdout_done = false;
    let mut stderr_done = false;
    let mut stdout_chunk = [0_u8; 8192];
    let mut stderr_chunk = [0_u8; 8192];
    while !stdout_done || !stderr_done {
        tokio::select! {
            read = stdout.read(&mut stdout_chunk), if !stdout_done => {
                let read = read.map_err(|error| format!("[operation_stdout_failed] {error}"))?;
                if read == 0 {
                    stdout_done = true;
                } else {
                    stdout_bytes.extend_from_slice(&stdout_chunk[..read]);
                }
            }
            read = stderr.read(&mut stderr_chunk), if !stderr_done => {
                let read = read.map_err(|error| format!("[operation_stderr_failed] {error}"))?;
                if read == 0 {
                    stderr_done = true;
                } else {
                    stderr_bytes.extend_from_slice(&stderr_chunk[..read]);
                }
            }
        }
        if stdout_bytes.len().saturating_add(stderr_bytes.len()) as u64 > limit {
            exceeded.notify_one();
            return Err("[budget_output_bytes_exceeded] BackgroundOperation 输出超过上限".into());
        }
    }
    Ok((stdout_bytes, stderr_bytes))
}

#[derive(Clone, PartialEq, Eq)]
struct RuntimeProcessConfig {
    permission_profile: PermissionProfile,
    disabled_mcp: Vec<String>,
}

#[derive(Clone)]
pub struct RuntimeRegistry {
    entries: Arc<RwLock<HashMap<String, RuntimeEntry>>>,
    operation_entries: Arc<RwLock<HashMap<String, OperationRuntimeEntry>>>,
    history: SessionHistoryStore,
    recovery_inputs: Arc<Vec<TurnRecoveryInput>>,
    supervisor: Option<crate::turn_supervisor::TurnSupervisor>,
}

impl RuntimeRegistry {
    pub fn new(history: SessionHistoryStore) -> Result<Self, String> {
        let recovery_inputs = history.load_turn_recovery_inputs()?;
        Ok(Self {
            entries: Arc::new(RwLock::new(HashMap::new())),
            operation_entries: Arc::new(RwLock::new(HashMap::new())),
            history,
            recovery_inputs: Arc::new(recovery_inputs),
            supervisor: None,
        })
    }

    pub fn with_supervisor(
        history: SessionHistoryStore,
        supervisor: crate::turn_supervisor::TurnSupervisor,
    ) -> Result<Self, String> {
        let mut registry = Self::new(history)?;
        registry.supervisor = Some(supervisor);
        Ok(registry)
    }

    pub fn recovery_inputs(&self) -> &[TurnRecoveryInput] {
        self.recovery_inputs.as_slice()
    }

    pub async fn contains(&self, owner: &RuntimeOwnerRef) -> bool {
        self.entries.read().await.contains_key(&owner.key())
            || self
                .operation_entries
                .read()
                .await
                .contains_key(&owner.key())
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn run_model_only_operation(
        &self,
        spec: &OperationExecutionSpec,
        policy: &ModelOnlyOperationPolicy,
        budget: &TurnBudgetSnapshot,
        bin: &str,
        env: &[(String, String)],
        prompt: &str,
    ) -> Result<(u64, ModelOnlyOperationOutput), String> {
        let preflight = if spec.owner != RuntimeOwnerRef::Operation(spec.operation_id.clone())
            || policy.capability_snapshot_id != spec.routing_capability_snapshot_id
            || !policy.tools_disabled
            || !policy.extensions_disabled
            || !policy.persistent_grants_disabled
            || !policy.canonical_cwd.is_empty()
        {
            Err("[operation_policy_mismatch] Operation spec/policy 不一致".to_string())
        } else if let Err(error) = budget.validate() {
            Err(error)
        } else if let Err(error) = budget.enforce_input_bytes(prompt.as_bytes().len()) {
            Err(error)
        } else if spec.engine_id != "claude-code" {
            Err("[operation_tools_not_disableable] Codex 当前合同不能关闭全部内建工具".to_string())
        } else {
            Ok(())
        };
        if let Err(error) = preflight {
            let _ = self
                .history
                .fail_committed_background_operation(&spec.operation_id, &error);
            return Err(error);
        }
        let compatibility_key = digest_json(&(
            &spec.engine_id,
            &spec.provider_launch_profile_digest,
            &spec.routed_model_id,
            &policy.launch_evidence,
        ))?;
        let generation = RuntimeGeneration {
            id: format!("runtime-{:032x}", rand::random::<u128>()),
            owner: spec.owner.clone(),
            engine_id: spec.engine_id.clone(),
            compatibility_key,
            engine_profile_digest: spec.engine_profile_digest.clone(),
            provider_launch_profile_ref: spec.provider_launch_profile_ref.clone(),
            provider_launch_profile_digest: spec.provider_launch_profile_digest.clone(),
            capability_snapshot_id: spec.routing_capability_snapshot_id.clone(),
            canonical_cwd: String::new(),
            created_at: crate::util::now_millis(),
        };
        let cancel = Arc::new(tokio::sync::Notify::new());
        {
            let mut entries = self.operation_entries.write().await;
            if entries.contains_key(&spec.owner.key()) {
                return Err("BackgroundOperation 已有运行中的 RuntimeGeneration".to_string());
            }
            self.history.create_runtime_generation(&generation)?;
            entries.insert(
                spec.owner.key(),
                OperationRuntimeEntry {
                    generation: generation.clone(),
                    cancel: cancel.clone(),
                },
            );
        }
        let attempt_no = match self
            .history
            .create_operation_attempt(&spec.operation_id, &generation)
        {
            Ok(attempt_no) => attempt_no,
            Err(error) => {
                self.operation_entries
                    .write()
                    .await
                    .remove(&spec.owner.key());
                let _ = self.history.close_runtime_generation(
                    &generation.id,
                    "crashed",
                    crate::util::now_millis(),
                );
                return Err(error);
            }
        };
        let operation_dir =
            std::env::temp_dir().join(format!("helm-operation-{}", spec.operation_id));
        if let Err(error) = std::fs::create_dir_all(&operation_dir) {
            return self
                .fail_operation_dispatch(
                    spec,
                    attempt_no,
                    &generation,
                    format!("[operation_temp_cwd_failed] {error}"),
                )
                .await;
        }
        let _operation_dir_guard = OperationTempDir(operation_dir.clone());
        let mut command = match crate::adapter::build_claude_model_only_command(
            bin,
            &spec.routed_model_id,
            env,
            &operation_dir,
            spec.routed_reasoning_effort,
        ) {
            Ok(command) => command,
            Err(error) => {
                return self
                    .fail_operation_dispatch(spec, attempt_no, &generation, error)
                    .await;
            }
        };
        // spawn 前 argv 预检：prompt 走 stdin 后命令行应有界；超限给可定位错误而非 os error 206。
        if let Err(error) = crate::adapter::ensure_command_line_within_limit(&command, "operation")
        {
            return self
                .fail_operation_dispatch(spec, attempt_no, &generation, error)
                .await;
        }
        // 环境块预检（七次反馈）：os error 206 也可能来自环境块超限，同样在 spawn 前
        // 拦截并带 tag 指认最大变量。
        if let Err(error) = crate::adapter::ensure_env_block_within_limit(&command, "operation") {
            return self
                .fail_operation_dispatch(spec, attempt_no, &generation, error)
                .await;
        }
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                // 失败消息携带取证串（diag=v2）：一次报错即可定位超限输入并证明二进制版本。
                return self
                    .fail_operation_dispatch(
                        spec,
                        attempt_no,
                        &generation,
                        format!(
                            "[operation_spawn_failed] {error} | {}",
                            crate::adapter::command_spawn_forensics(&command)
                        ),
                    )
                    .await;
            }
        };
        // prompt 经 stdin 交付（契约见 build_claude_model_only_command）：spawn 后立即
        // 全量写入并关闭，长 Ledger prompt 不再进入 Windows 约 32K 的命令行上限
        // （os error 206）。写入失败按既有 dispatch 失败语义回收子进程并落库。
        if let Err(error) =
            crate::adapter::write_model_only_prompt(&mut child, prompt, "operation").await
        {
            let _ = child.kill().await;
            return self
                .fail_operation_dispatch(spec, attempt_no, &generation, error)
                .await;
        }
        if let Err(error) = self
            .history
            .mark_operation_attempt_accepted(&spec.operation_id, attempt_no)
        {
            let _ = child.kill().await;
            return self
                .fail_operation_dispatch(spec, attempt_no, &generation, error)
                .await;
        }
        let wall_limit = budget
            .limit(BudgetDimension::WallClockMs)
            .map(|limit| limit.limit)
            .unwrap_or(60 * 60 * 1000);
        let output_limit = budget
            .limit(BudgetDimension::OutputBytes)
            .map(|limit| limit.limit)
            .unwrap_or(16 * 1024 * 1024);
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "[operation_stdout_missing] Claude CLI stdout 未建立管道".to_string());
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| "[operation_stderr_missing] Claude CLI stderr 未建立管道".to_string());
        let (stdout, stderr) = match (stdout, stderr) {
            (Ok(stdout), Ok(stderr)) => (stdout, stderr),
            (Err(error), _) | (_, Err(error)) => {
                let _ = child.kill().await;
                self.finish_failed_operation(spec, attempt_no, &generation, "failed", &error)
                    .await;
                return Err(error);
            }
        };
        let output_exceeded = Arc::new(tokio::sync::Notify::new());
        let output_reader = tokio::spawn(read_bounded_operation_output(
            stdout,
            stderr,
            output_limit,
            output_exceeded.clone(),
        ));
        let wait = async {
            tokio::select! {
                status = child.wait() => status.map_err(|error| format!("[operation_wait_failed] {error}")),
                _ = cancel.notified() => Err("[operation_cancelled] 用户取消后台任务".to_string()),
                _ = output_exceeded.notified() => Err("[budget_output_bytes_exceeded] BackgroundOperation 输出超过上限".to_string()),
            }
        };
        let status = match tokio::time::timeout(std::time::Duration::from_millis(wall_limit), wait)
            .await
        {
            Ok(Ok(status)) => status,
            Ok(Err(error)) => {
                let _ = child.kill().await;
                let _ = child.wait().await;
                output_reader.abort();
                let status = if error.starts_with("[operation_cancelled]") {
                    "cancelled"
                } else {
                    "failed"
                };
                self.finish_failed_operation(spec, attempt_no, &generation, status, &error)
                    .await;
                return Err(error);
            }
            Err(_) => {
                let _ = child.kill().await;
                let _ = child.wait().await;
                output_reader.abort();
                let error = "[budget_wall_clock_exceeded] BackgroundOperation 超过 wall-clock 上限";
                self.finish_failed_operation(spec, attempt_no, &generation, "failed", error)
                    .await;
                return Err(error.to_string());
            }
        };
        let (stdout, stderr) = match output_reader.await {
            Ok(Ok(output)) => output,
            Ok(Err(error)) => {
                self.finish_failed_operation(spec, attempt_no, &generation, "failed", &error)
                    .await;
                return Err(error);
            }
            Err(error) => {
                let error = format!("[operation_output_join_failed] {error}");
                self.finish_failed_operation(spec, attempt_no, &generation, "failed", &error)
                    .await;
                return Err(error);
            }
        };
        if !status.success() {
            let error = map_model_only_cli_failure("operation", &status, &stderr);
            self.finish_failed_operation(spec, attempt_no, &generation, "failed", &error)
                .await;
            return Err(error);
        }
        let parsed = match crate::adapter::parse_claude_model_only_output(&stdout) {
            Ok(parsed) => parsed,
            Err(error) => {
                self.finish_failed_operation(spec, attempt_no, &generation, "failed", &error)
                    .await;
                return Err(error);
            }
        };
        self.operation_entries
            .write()
            .await
            .remove(&spec.owner.key());
        self.history.close_runtime_generation(
            &generation.id,
            "closed",
            crate::util::now_millis(),
        )?;
        Ok((attempt_no, parsed))
    }

    pub async fn cancel_operation(&self, operation_id: &str) -> Result<bool, String> {
        let requested = self
            .history
            .request_background_operation_cancel(operation_id)?;
        if let Some(entry) = self
            .operation_entries
            .read()
            .await
            .get(&RuntimeOwnerRef::Operation(operation_id.to_string()).key())
            .cloned()
        {
            entry.cancel.notify_waiters();
        }
        Ok(requested)
    }

    /// 旁路提问（变更-34 · D3）：真实 CLI 的一次性无工具问答，**零持久化**。
    ///
    /// 与 `run_model_only_operation` 的区别：不创建 runtime_generation / operation
    /// attempt / background_operation 任何记录，也不登记提交表（无独立可取消句柄）。
    /// 只借助真实 CLI + ModelOnlyOperationPolicy 能力证明跑一次问答，结果原样返回，
    /// 由调用方决定怎么显示；本就无副作用，进程由 `kill_on_drop` + wall-clock 兜底回收。
    #[allow(clippy::too_many_arguments)]
    pub async fn run_transient_model_only_operation(
        &self,
        engine_id: &str,
        policy: &ModelOnlyOperationPolicy,
        bin: &str,
        env: &[(String, String)],
        cwd: &std::path::Path,
        prompt: &str,
        routed_model_id: &str,
        routed_reasoning_effort: ReasoningEffort,
        output_limit: u64,
        wall_clock_ms: u64,
    ) -> Result<ModelOnlyOperationOutput, String> {
        if engine_id != "claude-code" {
            return Err(
                "[side_query_tools_not_disableable] Codex 当前合同不能关闭全部内建工具，旁路提问不可用"
                    .to_string(),
            );
        }
        if !policy.tools_disabled
            || !policy.extensions_disabled
            || !policy.persistent_grants_disabled
            || !policy.canonical_cwd.is_empty()
        {
            return Err(
                "[side_query_policy_mismatch] 旁路提问策略不满足无工具隔离契约".to_string(),
            );
        }
        let mut command = crate::adapter::build_claude_model_only_command(
            bin,
            routed_model_id,
            env,
            cwd,
            routed_reasoning_effort,
        )?;
        // spawn 前 argv 预检（与后台任务同源契约）：超限给可定位错误而非 os error 206。
        crate::adapter::ensure_command_line_within_limit(&command, "side_query")?;
        // 环境块预检（七次反馈）：与后台任务同源，超限带 tag 指认最大变量。
        crate::adapter::ensure_env_block_within_limit(&command, "side_query")?;
        let mut child = command.spawn().map_err(|error| {
            // 失败消息携带取证串（diag=v2）：一次报错即可定位超限输入并证明二进制版本。
            format!(
                "[side_query_spawn_failed] {error} | {}",
                crate::adapter::command_spawn_forensics(&command)
            )
        })?;
        // prompt 经 stdin 交付（契约见 build_claude_model_only_command），与后台任务
        // 同源；本路径零持久化，失败时仅回收子进程后返回错误。
        if let Err(error) =
            crate::adapter::write_model_only_prompt(&mut child, prompt, "side_query").await
        {
            let _ = child.kill().await;
            let _ = child.wait().await;
            return Err(error);
        }
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "[side_query_stdout_missing] Claude CLI stdout 未建立管道".to_string());
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| "[side_query_stderr_missing] Claude CLI stderr 未建立管道".to_string());
        let (stdout, stderr) = match (stdout, stderr) {
            (Ok(stdout), Ok(stderr)) => (stdout, stderr),
            (Err(error), _) | (_, Err(error)) => {
                let _ = child.kill().await;
                return Err(error);
            }
        };
        let output_exceeded = Arc::new(tokio::sync::Notify::new());
        let output_reader = tokio::spawn(read_bounded_operation_output(
            stdout,
            stderr,
            output_limit,
            output_exceeded.clone(),
        ));
        let wait = async {
            tokio::select! {
                status = child.wait() => status.map_err(|error| format!("[side_query_wait_failed] {error}")),
                _ = output_exceeded.notified() => Err("[budget_output_bytes_exceeded] 旁路提问输出超过上限".to_string()),
            }
        };
        let status =
            match tokio::time::timeout(std::time::Duration::from_millis(wall_clock_ms), wait).await
            {
                Ok(Ok(status)) => status,
                Ok(Err(error)) => {
                    let _ = child.kill().await;
                    let _ = child.wait().await;
                    output_reader.abort();
                    return Err(error);
                }
                Err(_) => {
                    let _ = child.kill().await;
                    let _ = child.wait().await;
                    output_reader.abort();
                    return Err(
                        "[budget_wall_clock_exceeded] 旁路提问超过 wall-clock 上限".to_string()
                    );
                }
            };
        let (stdout, stderr) = match output_reader.await {
            Ok(Ok(output)) => output,
            Ok(Err(error)) => {
                let _ = child.kill().await;
                let _ = child.wait().await;
                return Err(error);
            }
            Err(error) => {
                let _ = child.kill().await;
                let _ = child.wait().await;
                return Err(format!("[side_query_output_join_failed] {error}"));
            }
        };
        if !status.success() {
            return Err(map_model_only_cli_failure("side_query", &status, &stderr));
        }
        crate::adapter::parse_claude_model_only_output(&stdout)
    }

    async fn fail_operation_dispatch<T>(
        &self,
        spec: &OperationExecutionSpec,
        attempt_no: u64,
        generation: &RuntimeGeneration,
        error: String,
    ) -> Result<T, String> {
        self.finish_failed_operation(spec, attempt_no, generation, "failed", &error)
            .await;
        Err(error)
    }

    async fn finish_failed_operation(
        &self,
        spec: &OperationExecutionSpec,
        attempt_no: u64,
        generation: &RuntimeGeneration,
        status: &str,
        error: &str,
    ) {
        self.operation_entries
            .write()
            .await
            .remove(&spec.owner.key());
        let _ = self.history.finish_background_operation(
            &spec.operation_id,
            attempt_no,
            status,
            None,
            Some(error),
        );
        let _ = self.history.close_runtime_generation(
            &generation.id,
            if status == "cancelled" {
                "closed"
            } else {
                "crashed"
            },
            crate::util::now_millis(),
        );
    }

    pub async fn register_session(
        &self,
        owner: RuntimeOwnerRef,
        session: AgentSession,
        route: &RuntimeRoute,
        capability_snapshot: &crate::capability_registry::EngineCapabilitySnapshot,
        cwd: &str,
    ) -> Result<RuntimeGeneration, String> {
        let RuntimeOwnerRef::Session(session_id) = &owner else {
            return Err("Session Runtime 不能注册到 Operation owner".to_string());
        };
        if session.history_session_id() != session_id {
            return Err("Runtime owner 与 AgentSession 身份不匹配".to_string());
        }
        if capability_snapshot.identity.engine_id != route.engine_id
            || capability_snapshot.identity.engine_profile_digest != route.engine_profile_digest
            || capability_snapshot.identity.provider_launch_profile_digest
                != route.provider_launch_profile_digest
            || capability_snapshot.identity.model_capability_key != route.model_id
        {
            return Err("CapabilitySnapshot 与 Session 路由身份不匹配".to_string());
        }
        let canonical_cwd = canonical_runtime_cwd(cwd)?;
        let base_compatibility_key = base_runtime_compatibility_key(route, &canonical_cwd)?;
        let process_config = RuntimeProcessConfig {
            permission_profile: session.permission_profile().await?,
            disabled_mcp: Vec::new(),
        };
        let compatibility_key = compatibility_key_with_process_config(
            &base_compatibility_key,
            &process_config_digest(&process_config)?,
        )?;
        let generation = RuntimeGeneration {
            id: format!("runtime-{:032x}", rand::random::<u128>()),
            owner: owner.clone(),
            engine_id: route.engine_id.clone(),
            compatibility_key,
            engine_profile_digest: route.engine_profile_digest.clone(),
            provider_launch_profile_ref: route.provider_launch_profile_ref.clone(),
            provider_launch_profile_digest: route.provider_launch_profile_digest.clone(),
            capability_snapshot_id: capability_snapshot.id.clone(),
            canonical_cwd,
            created_at: crate::util::now_millis(),
        };

        let mut entries = self.entries.write().await;
        if entries.contains_key(&owner.key()) {
            return Err(format!(
                "Runtime owner 已注册，必须先 Close 或复用现有 SessionActor：{}",
                owner.id()
            ));
        }
        self.history.create_runtime_generation(&generation)?;
        entries.insert(
            owner.key(),
            RuntimeEntry {
                generation: generation.clone(),
                session,
                base_compatibility_key,
                process_config,
            },
        );
        Ok(generation)
    }

    async fn entry(&self, owner: &RuntimeOwnerRef) -> Result<RuntimeEntry, String> {
        self.entries
            .read()
            .await
            .get(&owner.key())
            .cloned()
            .ok_or_else(|| format!("找不到 Runtime owner：{}", owner.id()))
    }

    pub async fn reserve_turn(&self, owner: &RuntimeOwnerRef) -> Result<(), String> {
        self.entry(owner).await?.session.reserve_turn()
    }

    pub async fn release_turn_reservation(&self, owner: &RuntimeOwnerRef) -> Result<(), String> {
        self.entry(owner).await?.session.release_turn_reservation();
        Ok(())
    }

    pub async fn route_requires_replacement(
        &self,
        owner: &RuntimeOwnerRef,
        route: &RuntimeRoute,
        cwd: &str,
    ) -> Result<bool, String> {
        let entry = self.entry(owner).await?;
        let canonical_cwd = canonical_runtime_cwd(cwd)?;
        let candidate = base_runtime_compatibility_key(route, &canonical_cwd)?;
        Ok(candidate != entry.base_compatibility_key)
    }

    pub async fn update_reserved_capability_snapshot(
        &self,
        owner: &RuntimeOwnerRef,
        snapshot: crate::capability_registry::EngineCapabilitySnapshot,
    ) -> Result<(), String> {
        let entry = self.entry(owner).await?;
        if snapshot.identity.engine_id != entry.generation.engine_id
            || snapshot.identity.engine_profile_digest != entry.generation.engine_profile_digest
            || snapshot.identity.provider_launch_profile_digest
                != entry.generation.provider_launch_profile_digest
        {
            return Err("CapabilitySnapshot 与复用 RuntimeGeneration 身份不匹配".to_string());
        }
        entry.session.set_turn_capability_snapshot(snapshot).await
    }

    pub async fn replace_reserved_session(
        &self,
        owner: &RuntimeOwnerRef,
        session: AgentSession,
        route: &RuntimeRoute,
        capability_snapshot: &crate::capability_registry::EngineCapabilitySnapshot,
        cwd: &str,
    ) -> Result<RuntimeGeneration, String> {
        let current = self.entry(owner).await?;
        if session.history_session_id() != owner.id() {
            return Err("替换 Runtime 与 Session owner 身份不匹配".to_string());
        }
        if capability_snapshot.identity.engine_id != route.engine_id
            || capability_snapshot.identity.engine_profile_digest != route.engine_profile_digest
            || capability_snapshot.identity.provider_launch_profile_digest
                != route.provider_launch_profile_digest
            || capability_snapshot.identity.model_capability_key != route.model_id
        {
            return Err("CapabilitySnapshot 与替换 Runtime 路由身份不匹配".to_string());
        }
        let canonical_cwd = canonical_runtime_cwd(cwd)?;
        let base_compatibility_key = base_runtime_compatibility_key(route, &canonical_cwd)?;
        if base_compatibility_key == current.base_compatibility_key {
            return Err("兼容 Runtime 不应创建新 generation".to_string());
        }
        session
            .set_permission_profile(current.process_config.permission_profile)
            .await?;
        session
            .set_disabled_mcp(current.process_config.disabled_mcp.clone())
            .await?;
        session
            .set_turn_capability_snapshot(capability_snapshot.clone())
            .await?;
        session.reserve_turn()?;
        let compatibility_key = compatibility_key_with_process_config(
            &base_compatibility_key,
            &process_config_digest(&current.process_config)?,
        )?;
        let generation = RuntimeGeneration {
            id: format!("runtime-{:032x}", rand::random::<u128>()),
            owner: owner.clone(),
            engine_id: route.engine_id.clone(),
            compatibility_key,
            engine_profile_digest: route.engine_profile_digest.clone(),
            provider_launch_profile_ref: route.provider_launch_profile_ref.clone(),
            provider_launch_profile_digest: route.provider_launch_profile_digest.clone(),
            capability_snapshot_id: capability_snapshot.id.clone(),
            canonical_cwd,
            created_at: crate::util::now_millis(),
        };
        let replaced = {
            let mut entries = self.entries.write().await;
            let live = entries
                .get_mut(&owner.key())
                .ok_or_else(|| format!("找不到 Runtime owner：{}", owner.id()))?;
            if live.generation.id != current.generation.id {
                session.release_turn_reservation();
                return Err("RuntimeGeneration 在路由切换期间发生并发变化".to_string());
            }
            if let Err(error) = self
                .history
                .rotate_runtime_generation(&current.generation.id, &generation)
            {
                session.release_turn_reservation();
                return Err(error);
            }
            std::mem::replace(
                live,
                RuntimeEntry {
                    generation: generation.clone(),
                    session,
                    base_compatibility_key,
                    process_config: current.process_config.clone(),
                },
            )
        };
        replaced.session.release_turn_reservation();
        replaced.session.shutdown().await;
        Ok(generation)
    }

    pub async fn send_reserved(
        &self,
        owner: &RuntimeOwnerRef,
        text: String,
        attachments: Vec<String>,
        spec: TurnExecutionSpec,
    ) -> Result<(), String> {
        let entry = self.entry(owner).await?;
        if owner != &RuntimeOwnerRef::Session(spec.history_session_id.clone()) {
            entry.session.release_turn_reservation();
            return Err("TurnAttempt owner 与 TurnExecutionSpec Session 不匹配".to_string());
        }
        if spec.engine_id != entry.generation.engine_id
            || spec.engine_profile_digest != entry.generation.engine_profile_digest
            || spec.provider_launch_profile_ref != entry.generation.provider_launch_profile_ref
        {
            entry.session.release_turn_reservation();
            return Err(
                "TurnExecutionSpec 与 RuntimeGeneration compatibility key 不匹配".to_string(),
            );
        }

        let input_native = entry.session.native_session_ref().await?;
        let attempt =
            self.history
                .create_turn_attempt(&spec, &entry.generation, input_native.as_deref())?;
        if let Some(supervisor) = self.supervisor.as_ref() {
            if let Err(error) = supervisor.begin_attempt(
                &spec.history_session_id,
                &spec.turn_id,
                spec.turn_epoch,
                &spec.turn_mode,
                &spec.permission_profile,
                owner.clone(),
                attempt.attempt_no,
                &entry.generation.id,
            ) {
                entry.session.release_turn_reservation();
                let _ = self.history.finish_turn_attempt(
                    &attempt.turn_id,
                    attempt.attempt_no,
                    "rejected",
                    Some(&error),
                    crate::util::now_millis(),
                );
                return Err(error);
            }
        }
        match entry.session.send_reserved(text, attachments, spec).await {
            Ok(()) => Ok(()),
            Err(error) => {
                let supervised = self.supervisor.as_ref().is_some_and(|supervisor| {
                    supervisor.submit_event(
                        owner.id(),
                        Some(&attempt.turn_id),
                        None,
                        AgentEvent::Error {
                            session_id: None,
                            message: error.clone(),
                            recoverable: false,
                            kind: Some("dispatch_rejected".to_string()),
                            stalled_kind: None,
                        },
                    )
                });
                if !supervised {
                    let _ = self.history.finish_turn_attempt(
                        &attempt.turn_id,
                        attempt.attempt_no,
                        "rejected",
                        Some(&error),
                        crate::util::now_millis(),
                    );
                }
                Err(error)
            }
        }
    }

    pub async fn begin_compatibility_retry(
        &self,
        owner: &RuntimeOwnerRef,
        spec: &TurnExecutionSpec,
        receipt: &str,
    ) -> Result<u64, String> {
        let entry = self.entry(owner).await?;
        if owner != &RuntimeOwnerRef::Session(spec.history_session_id.clone())
            || spec.engine_id != entry.generation.engine_id
            || spec.engine_profile_digest != entry.generation.engine_profile_digest
            || spec.provider_launch_profile_ref != entry.generation.provider_launch_profile_ref
        {
            return Err("兼容恢复的 TurnExecutionSpec 与 RuntimeGeneration 不匹配".to_string());
        }
        let supervisor = self
            .supervisor
            .as_ref()
            .ok_or_else(|| "Stream Supervisor 未启动".to_string())?;
        let input_native = entry.session.native_session_ref().await?;
        let attempt =
            self.history
                .create_turn_attempt(spec, &entry.generation, input_native.as_deref())?;
        if let Err(error) = supervisor.retry_attempt(
            &spec.history_session_id,
            &spec.turn_id,
            attempt.attempt_no,
            &entry.generation.id,
            receipt,
        ) {
            let _ = self.history.finish_turn_attempt(
                &spec.turn_id,
                attempt.attempt_no,
                "rejected",
                Some(&error),
                crate::util::now_millis(),
            );
            return Err(error);
        }
        Ok(attempt.attempt_no)
    }

    pub async fn permission_profile(
        &self,
        owner: &RuntimeOwnerRef,
    ) -> Result<PermissionProfile, String> {
        self.entry(owner).await?.session.permission_profile().await
    }

    pub async fn set_permission_profile(
        &self,
        owner: &RuntimeOwnerRef,
        profile: PermissionProfile,
    ) -> Result<(), String> {
        let entry = self.entry(owner).await?;
        if entry.process_config.permission_profile == profile {
            return Ok(());
        }
        entry.session.set_permission_profile(profile).await?;
        let mut next = entry.process_config.clone();
        next.permission_profile = profile;
        self.rotate_process_configuration(owner, entry, next).await
    }

    pub async fn permission_confirmation_context(
        &self,
        owner: &RuntimeOwnerRef,
    ) -> Result<(String, String), String> {
        Ok(self
            .entry(owner)
            .await?
            .session
            .permission_confirmation_context())
    }

    pub async fn approve(
        &self,
        owner: &RuntimeOwnerRef,
        request_id: String,
        decision: ApprovalDecision,
    ) -> Result<(), String> {
        self.entry(owner)
            .await?
            .session
            .approve(request_id, decision)
            .await
    }

    pub async fn interrupt(&self, owner: &RuntimeOwnerRef) -> Result<(), String> {
        self.entry(owner).await?.session.interrupt_and_wait().await
    }

    /// 触发引擎原生上下文压缩（变更-34/35 · B4）：只有 Codex 有真实 headless 契约。
    pub async fn compact_context(&self, owner: &RuntimeOwnerRef) -> Result<(), String> {
        self.entry(owner).await?.session.compact_context().await
    }

    pub async fn reset_context(
        &self,
        owner: &RuntimeOwnerRef,
        messages: Vec<crate::sessions::SessionMessage>,
    ) -> Result<(), String> {
        self.entry(owner).await?.session.reset_context(messages)
    }

    pub async fn set_disabled_mcp(
        &self,
        owner: &RuntimeOwnerRef,
        mut disabled: Vec<String>,
    ) -> Result<(), String> {
        disabled.sort();
        disabled.dedup();
        let entry = self.entry(owner).await?;
        if entry.process_config.disabled_mcp == disabled {
            return Ok(());
        }

        entry.session.set_disabled_mcp(disabled.clone()).await?;
        let mut next = entry.process_config.clone();
        next.disabled_mcp = disabled;
        self.rotate_process_configuration(owner, entry, next).await
    }

    async fn rotate_process_configuration(
        &self,
        owner: &RuntimeOwnerRef,
        entry: RuntimeEntry,
        next_process_config: RuntimeProcessConfig,
    ) -> Result<(), String> {
        let compatibility_key = compatibility_key_with_process_config(
            &entry.base_compatibility_key,
            &process_config_digest(&next_process_config)?,
        )?;
        let generation = RuntimeGeneration {
            id: format!("runtime-{:032x}", rand::random::<u128>()),
            compatibility_key,
            created_at: crate::util::now_millis(),
            ..entry.generation.clone()
        };
        let mut entries = self.entries.write().await;
        let current = entries
            .get_mut(&owner.key())
            .ok_or_else(|| format!("找不到 Runtime owner：{}", owner.id()))?;
        if current.generation.id != entry.generation.id {
            return Err("RuntimeGeneration 在进程级配置更新期间发生并发变化".to_string());
        }
        match self
            .history
            .rotate_runtime_generation(&entry.generation.id, &generation)
        {
            Ok(()) => {
                current.generation = generation;
                current.process_config = next_process_config;
                Ok(())
            }
            Err(error) => {
                drop(entries);
                entry.session.shutdown().await;
                let mut entries = self.entries.write().await;
                if entries
                    .get(&owner.key())
                    .is_some_and(|current| current.generation.id == entry.generation.id)
                {
                    entries.remove(&owner.key());
                }
                let _ = self.history.close_runtime_generation(
                    &entry.generation.id,
                    "crashed",
                    crate::util::now_millis(),
                );
                Err(format!(
                    "Runtime 进程配置已更新，但 generation 轮换失败，已关闭 Runtime：{error}"
                ))
            }
        }
    }

    pub async fn close(&self, owner: &RuntimeOwnerRef) -> Result<(), String> {
        let entry = self.entries.write().await.remove(&owner.key());
        if let Some(entry) = entry {
            entry.session.shutdown().await;
            self.history.close_runtime_generation(
                &entry.generation.id,
                "closed",
                crate::util::now_millis(),
            )?;
        }
        Ok(())
    }

    pub async fn shutdown_all(&self) {
        let entries = {
            let mut guard = self.entries.write().await;
            guard.drain().map(|(_, entry)| entry).collect::<Vec<_>>()
        };
        for entry in entries {
            entry.session.shutdown().await;
            let _ = self.history.close_runtime_generation(
                &entry.generation.id,
                "application_exit",
                crate::util::now_millis(),
            );
        }
        let operations = {
            let mut guard = self.operation_entries.write().await;
            guard.drain().map(|(_, entry)| entry).collect::<Vec<_>>()
        };
        for entry in operations {
            entry.cancel.notify_waiters();
            let _ = self.history.close_runtime_generation(
                &entry.generation.id,
                "application_exit",
                crate::util::now_millis(),
            );
        }
    }
}

pub fn runtime_compatibility_key(
    route: &RuntimeRoute,
    canonical_cwd: &str,
) -> Result<String, String> {
    let base = base_runtime_compatibility_key(route, canonical_cwd)?;
    compatibility_key_with_process_config(
        &base,
        &process_config_digest(&RuntimeProcessConfig {
            permission_profile: PermissionProfile::Standard,
            disabled_mcp: Vec::new(),
        })?,
    )
}

fn base_runtime_compatibility_key(
    route: &RuntimeRoute,
    canonical_cwd: &str,
) -> Result<String, String> {
    digest_json(&serde_json::json!({
        "adapterProtocolGeneration": adapter_protocol_generation(&route.engine_id),
        "engineId": route.engine_id,
        "modelId": route.model_id,
        "engineProfileDigest": route.engine_profile_digest,
        "providerLaunchProfileRef": route.provider_launch_profile_ref,
        "providerLaunchProfileDigest": route.provider_launch_profile_digest,
        "canonicalCwd": canonical_cwd
    }))
}

fn process_config_digest(config: &RuntimeProcessConfig) -> Result<String, String> {
    digest_json(&serde_json::json!({
        "processCapabilityPolicy": "runtime-managed-v1",
        "permissionProfile": config.permission_profile.as_str(),
        "disabledMcp": config.disabled_mcp
    }))
}

fn compatibility_key_with_process_config(
    base_compatibility_key: &str,
    process_config_digest: &str,
) -> Result<String, String> {
    digest_json(&serde_json::json!({
        "baseCompatibilityKey": base_compatibility_key,
        "processConfigDigest": process_config_digest
    }))
}

fn adapter_protocol_generation(engine_id: &str) -> &'static str {
    match engine_id {
        "codex" => "codex-app-server-v2",
        "claude-code" => "claude-stream-json-v1",
        _ => "unknown",
    }
}

fn canonical_runtime_cwd(cwd: &str) -> Result<String, String> {
    let path = std::fs::canonicalize(Path::new(cwd))
        .map_err(|error| format!("Runtime 工作目录不可用：{error}"))?;
    #[cfg(windows)]
    {
        Ok(path.to_string_lossy().replace('/', "\\").to_lowercase())
    }
    #[cfg(not(windows))]
    {
        Ok(path.to_string_lossy().into_owned())
    }
}

pub(crate) fn finish_attempt_from_event(
    history: &SessionHistoryStore,
    turn_id: &str,
    event: &AgentEvent,
) -> Result<(), String> {
    match event {
        AgentEvent::TurnComplete { stop_reason, .. } => {
            let (state, receipt) = match stop_reason {
                StopReason::End => ("completed", "end"),
                StopReason::Interrupted => ("interrupted", "interrupted"),
                StopReason::Error => ("error", "error"),
            };
            history.finish_latest_turn_attempt(
                turn_id,
                state,
                Some(receipt),
                crate::util::now_millis(),
            )
        }
        AgentEvent::Error {
            recoverable: false,
            message,
            ..
        } => history.finish_latest_turn_attempt(
            turn_id,
            "error",
            Some(message),
            crate::util::now_millis(),
        ),
        _ => Ok(()),
    }
}

pub fn native_kind(engine: EngineId) -> &'static str {
    match engine {
        EngineId::ClaudeCode => "claude_session_id",
        EngineId::Codex => "codex_thread_id",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reasoning::ReasoningEffort;
    use crate::turn_start::PricingBasisSnapshot;

    /// Windows stable 工具链没有 ExitStatus←ExitCode 转换，用真实子进程构造状态码。
    fn exit_status(code: i32) -> std::process::ExitStatus {
        #[cfg(windows)]
        {
            std::process::Command::new("cmd")
                .args(["/C", &format!("exit {code}")])
                .status()
                .unwrap()
        }
        #[cfg(not(windows))]
        {
            std::process::Command::new("sh")
                .arg("-c")
                .arg(format!("exit {code}"))
                .status()
                .unwrap()
        }
    }

    #[test]
    fn unrecognized_model_stderr_maps_to_actionable_error() {
        let status = exit_status(1);
        let stderr: &[u8] =
            br#"[claude-code:unrecognized_model] {"model":"DeepSeek-V4-Flash-0731","query_source":"generate_session_title"}"#;
        let error = map_model_only_cli_failure("operation", &status, stderr);
        assert!(
            error.starts_with("[operation_model_rejected]"),
            "必须映射为可操作 tag：{error}"
        );
        assert!(
            error.contains("DeepSeek-V4-Flash-0731"),
            "必须指认被拒模型：{error}"
        );
        assert!(error.contains("AI 配置"), "必须给出修复入口：{error}");
    }

    #[test]
    fn other_cli_failures_keep_original_tag_and_shape() {
        let status = exit_status(2);
        let error = map_model_only_cli_failure("side_query", &status, b"boom");
        assert!(error.starts_with("[side_query_cli_failed]"), "{error}");
        assert!(error.contains("boom"), "{error}");
    }

    #[test]
    fn json_string_field_extraction_handles_present_and_missing() {
        assert_eq!(
            extract_json_string_field("prefix {\"model\":\"m-1\",\"x\":2} suffix", "model")
                .as_deref(),
            Some("m-1")
        );
        assert_eq!(extract_json_string_field("no json here", "model"), None);
    }

    fn route(model: &str, effort: ReasoningEffort) -> RuntimeRoute {
        RuntimeRoute {
            engine_id: "codex".into(),
            provider_id: "provider-1".into(),
            provider_kind: "api".into(),
            provider_display_name: "Provider".into(),
            route_label_snapshot: "Provider / Model".into(),
            model_id: model.into(),
            model_label_snapshot: model.into(),
            default_reasoning_effort: effort,
            engine_profile_digest: "sha256:engine".into(),
            provider_launch_profile_ref: "provider:provider-1:api".into(),
            provider_launch_profile_digest: "sha256:provider-launch-1".into(),
            launch_config_digest: format!("sha256:{model}"),
            pricing_basis_snapshot: PricingBasisSnapshot { profile: None },
        }
    }

    #[test]
    fn effort_does_not_change_base_compatibility_key() {
        let first =
            runtime_compatibility_key(&route("gpt-primary", ReasoningEffort::Low), "c:\\repo")
                .unwrap();
        let second =
            runtime_compatibility_key(&route("gpt-primary", ReasoningEffort::High), "c:\\repo")
                .unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn model_change_requires_new_compatibility_key() {
        let first = base_runtime_compatibility_key(
            &route("gpt-primary", ReasoningEffort::Auto),
            "c:\\repo",
        )
        .unwrap();
        let second =
            base_runtime_compatibility_key(&route("gpt-fast", ReasoningEffort::Auto), "c:\\repo")
                .unwrap();
        assert_ne!(first, second);
    }

    #[test]
    fn provider_profile_and_cwd_change_compatibility_key() {
        let base = route("gpt", ReasoningEffort::Auto);
        let first = runtime_compatibility_key(&base, "c:\\repo").unwrap();
        let mut changed = base.clone();
        changed.provider_launch_profile_ref = "provider:provider-2:api".into();
        changed.provider_launch_profile_digest = "sha256:provider-launch-2".into();
        assert_ne!(
            first,
            runtime_compatibility_key(&changed, "c:\\repo").unwrap()
        );
        assert_ne!(
            first,
            runtime_compatibility_key(&base, "c:\\other").unwrap()
        );
    }

    #[test]
    fn process_level_configuration_changes_compatibility_key() {
        let base = base_runtime_compatibility_key(&route("gpt", ReasoningEffort::Auto), "c:\\repo")
            .unwrap();
        let first = compatibility_key_with_process_config(
            &base,
            &process_config_digest(&RuntimeProcessConfig {
                permission_profile: PermissionProfile::Standard,
                disabled_mcp: Vec::new(),
            })
            .unwrap(),
        )
        .unwrap();
        let second = compatibility_key_with_process_config(
            &base,
            &process_config_digest(&RuntimeProcessConfig {
                permission_profile: PermissionProfile::Standard,
                disabled_mcp: vec!["server-a".into()],
            })
            .unwrap(),
        )
        .unwrap();
        assert_ne!(first, second);
        let third = compatibility_key_with_process_config(
            &base,
            &process_config_digest(&RuntimeProcessConfig {
                permission_profile: PermissionProfile::Auto,
                disabled_mcp: Vec::new(),
            })
            .unwrap(),
        )
        .unwrap();
        assert_ne!(first, third);
    }

    #[test]
    fn owner_kinds_are_not_interchangeable() {
        assert_ne!(
            RuntimeOwnerRef::Session("same".into()).key(),
            RuntimeOwnerRef::Operation("same".into()).key()
        );
    }
}
