use crate::budget::TurnBudgetSnapshot;
use crate::operations::{
    BackgroundOperation, ModelOnlyOperationOutput, ModelOnlyOperationPolicy,
    NewBackgroundOperation, OperationExecutionSpec,
};
use crate::permissions::{
    ActionDescriptor, Capability, PermissionDecision, PermissionEffect, PermissionRule,
    PermissionScope,
};
use crate::pricing::{PricingBand, PricingTier, ResolvedPricingProfile, ServiceTier};
use crate::protocol::{
    AgentEvent, CallStatus, Diff, EngineId, Role, RuntimeCapabilitySnapshot, StopReason,
    ToolDenialSource, ToolOutcomeKind, ToolStatus,
};
use crate::turn_start::{FrozenSessionContext, TurnExecutionSpec, TurnStartCommand};
use crate::util::{now_millis, now_seconds};
use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// setting 表里「始终允许」工具清单的 key（P2-4 跨会话持久化）
const ALWAYS_ALLOW_TOOLS_KEY: &str = "approval_always_allow";
const PERMISSION_POLICY_VERSION_KEY: &str = "permission_policy_version";
const APP_SETTINGS_KEY: &str = "app_settings";
pub const PERMISSION_AUDIT_RETENTION_DAYS: i64 = 90;

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
    #[serde(default)]
    pub runtime_capabilities: Option<RuntimeCapabilitySnapshot>,
    #[serde(default = "default_safe_permission_profile")]
    pub safe_permission_profile: String,
    #[serde(default)]
    pub folder_id: String,
    #[serde(default)]
    pub cached_input_tokens: u64,
    #[serde(default)]
    pub cache_write_input_tokens: u64,
    #[serde(default)]
    pub last_context_tokens: Option<u64>,
    #[serde(default)]
    pub last_context_window: Option<u64>,
    /// 用户为下一 Turn 选择的模型；执行时仍必须经当前 Binding 重解析。
    #[serde(default)]
    pub preferred_model: Option<String>,
    /// 用户为下一 Turn 选择的推理强度；空值表示跟随当前 Binding 缺省。
    #[serde(default)]
    pub preferred_reasoning_effort: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SessionFolder {
    pub id: String,
    pub name: String,
    pub sort_order: i64,
    pub collapsed: bool,
    pub locked: bool,
    pub created_at: i64,
    #[serde(default)]
    pub cwd: Option<String>,
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
    #[serde(default)]
    pub turn_id: Option<String>,
    #[serde(default)]
    pub attachments: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SessionContextRecord {
    pub id: String,
    pub kind: String,
    pub source_path: String,
    pub canonical_path: String,
    pub display_name: String,
    pub status: String,
    pub status_detail: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug)]
pub struct PreparedUserTurn {
    session_id: String,
    message_id: i64,
    turn_id: Option<String>,
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
    #[serde(default)]
    pub ended_at: Option<i64>,
    #[serde(default)]
    pub turn_id: Option<String>,
    #[serde(default)]
    pub outcome: Option<String>,
    #[serde(default)]
    pub started: Option<bool>,
    #[serde(default)]
    pub has_output: Option<bool>,
    #[serde(default)]
    pub retryable: Option<bool>,
    #[serde(default)]
    pub denial_source: Option<String>,
    #[serde(default)]
    pub native_denial_code: Option<String>,
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
    #[serde(default)]
    pub decision: Option<String>,
    #[serde(default)]
    pub rule_id: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub resolved_at: Option<i64>,
    #[serde(default)]
    pub persistent_label: Option<String>,
    #[serde(default)]
    pub matcher_summary: Option<String>,
    #[serde(default)]
    pub turn_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SessionCheckpoint {
    pub id: String,
    pub label: String,
    pub ts: i64,
    #[serde(default)]
    pub turn_id: Option<String>,
    #[serde(default)]
    pub restorable: bool,
    #[serde(default)]
    pub file_count: u64,
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PermissionAuditSummary {
    pub record_count: u64,
    pub oldest_at: Option<i64>,
    pub newest_at: Option<i64>,
    pub retention_days: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PermissionAuditExportRecord {
    created_at: i64,
    engine: String,
    capability: String,
    operation: String,
    effect: String,
    reason: String,
    policy_version: i64,
    execution_status: String,
    execution_authorization: Option<String>,
    execution_started_at: Option<i64>,
    execution_finished_at: Option<i64>,
    revocation_too_late_at: Option<i64>,
    resource_digests: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    resources: Option<Vec<String>>,
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
    /// 每轮实际生效的模式与权限档位，用于历史徽标和审计。
    #[serde(default)]
    pub turns: Vec<SessionTurn>,
    #[serde(default)]
    pub session_context: Vec<SessionContextRecord>,
    #[serde(default)]
    pub fork: Option<SessionForkSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SessionForkSummary {
    pub id: String,
    pub handoff_id: String,
    pub source_session_id: Option<String>,
    pub source_title_snapshot: String,
    pub source_engine: String,
    pub target_engine: String,
    pub boundary_turn_id: String,
    pub boundary_turn_epoch: u64,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SessionTurn {
    pub id: String,
    pub epoch: u64,
    pub mode: String,
    pub permission_profile: String,
    pub status: String,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub terminal_reason: Option<String>,
    pub provider_display_name: Option<String>,
    pub requested_model_id: Option<String>,
    pub routed_model_id: Option<String>,
    pub requested_reasoning_effort: Option<String>,
    pub routed_reasoning_effort: Option<String>,
    pub resolution_source: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TurnLedgerRoute {
    pub engine_id: String,
    pub provider_id: String,
    pub requested_model_id: String,
    pub routed_model_id: String,
    pub requested_reasoning_effort: String,
    pub routed_reasoning_effort: String,
    pub resolution_source: String,
    pub launch_config_digest: String,
    pub pricing_basis_snapshot: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TurnLedgerUsage {
    pub model: String,
    pub provider_id: String,
    pub effective_reasoning_effort: Option<String>,
    pub model_evidence: String,
    pub input_tokens: u64,
    pub cached_input_tokens: u64,
    pub cache_write_input_tokens: u64,
    pub output_tokens: u64,
    pub service_tier: String,
    pub cost_usd: f64,
    pub price_snapshot: Option<serde_json::Value>,
    pub ts: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TurnLedgerAttachment {
    pub source_path: String,
    pub path_digest: String,
    pub ordinal: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TurnLedgerContextEvidence {
    pub context_id: String,
    pub kind: String,
    pub canonical_path_digest: String,
    pub identity_digest: String,
    pub validation_status: String,
    pub ordinal: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TurnLedgerRecord {
    pub turn: SessionTurn,
    pub route: TurnLedgerRoute,
    pub messages: Vec<SessionMessage>,
    pub tool_calls: Vec<SessionToolCall>,
    pub approvals: Vec<SessionApproval>,
    pub checkpoints: Vec<SessionCheckpoint>,
    pub usage: Vec<TurnLedgerUsage>,
    pub attachments: Vec<TurnLedgerAttachment>,
    pub session_context: Vec<TurnLedgerContextEvidence>,
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
struct CostResolution {
    cost_usd: f64,
    cost_kind: String,
    price_source: String,
    catalog_version: Option<String>,
    price_snapshot_json: Option<String>,
    reported_cost_usd: Option<f64>,
}

impl CostResolution {
    fn unknown() -> Self {
        Self {
            cost_usd: 0.0,
            cost_kind: "unknown".to_string(),
            price_source: "unknown".to_string(),
            catalog_version: None,
            price_snapshot_json: None,
            reported_cost_usd: None,
        }
    }
}

fn model_price_key(provider_id: &str, model: &str) -> String {
    format!("{provider_id}\u{0}{model}")
}

fn parse_service_tier(value: Option<&str>) -> ServiceTier {
    match value {
        Some("batch") => ServiceTier::Batch,
        Some("flex") => ServiceTier::Flex,
        Some("priority") => ServiceTier::Priority,
        _ => ServiceTier::Standard,
    }
}

fn pricing_band_matches(band: &PricingBand, input_tokens: u64) -> bool {
    band.min_input_tokens
        .map(|min| input_tokens >= min)
        .unwrap_or(true)
        && band
            .max_input_tokens
            .map(|max| input_tokens <= max)
            .unwrap_or(true)
}

#[derive(Clone)]
pub struct SessionHistoryStore {
    path: PathBuf,
    write_lock: Arc<Mutex<()>>,
    initialized: Arc<Mutex<bool>>,
    model_prices: Arc<Mutex<HashMap<String, ResolvedPricingProfile>>>,
    runtime_grants: Arc<Mutex<RuntimeGrantCache>>,
    session_providers: Arc<Mutex<HashMap<String, String>>>,
}

#[derive(Debug, Clone)]
pub struct TurnSnapshotRecord {
    pub history_session_id: String,
    pub turn_id: String,
    pub turn_epoch: u64,
    pub status: crate::turn_supervisor::TurnStatus,
    pub terminal_reason: Option<String>,
    pub recoverable: bool,
    pub event_seq: u64,
    pub updated_at: i64,
    pub mode: String,
    pub permission_profile: String,
    pub started_at: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StreamRecoveryReport {
    pub prepared_interrupted: u64,
    pub approval_interrupted: u64,
    pub delivery_unknown: u64,
    pub runtime_generations_lost: u64,
}

#[derive(Debug, Clone)]
struct RuntimeGrantRecord {
    engine: String,
    provider_id: String,
    project_root: Option<String>,
    matcher_kind: String,
    matcher_value: String,
    scope: String,
    adapter_version: String,
    ceiling_version: String,
}

#[derive(Debug, Default)]
struct RuntimeGrantCache {
    policy_version: u64,
    entries: HashMap<String, RuntimeGrantRecord>,
}

impl SessionHistoryStore {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            write_lock: Arc::new(Mutex::new(())),
            initialized: Arc::new(Mutex::new(false)),
            model_prices: Arc::new(Mutex::new(HashMap::new())),
            runtime_grants: Arc::new(Mutex::new(RuntimeGrantCache::default())),
            session_providers: Arc::new(Mutex::new(HashMap::new())),
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
                model_price_key("", model),
                ResolvedPricingProfile {
                    catalog_version: "legacy-test".to_string(),
                    source: "manual".to_string(),
                    currency: "USD".to_string(),
                    source_url: String::new(),
                    observed_at: String::new(),
                    tiers: HashMap::from([(
                        ServiceTier::Standard,
                        PricingTier {
                            bands: vec![PricingBand {
                                min_input_tokens: None,
                                max_input_tokens: None,
                                input: input_price_per_mtok,
                                cached_input: None,
                                cache_write: None,
                                output: output_price_per_mtok,
                            }],
                        },
                    )]),
                },
            );
        }
    }

    pub fn set_model_pricing_profile(
        &self,
        provider_id: &str,
        model: &str,
        profile: ResolvedPricingProfile,
    ) {
        if let Ok(mut prices) = self.model_prices.lock() {
            prices.insert(model_price_key(provider_id, model), profile);
        }
    }

    pub fn create_session(&self, record: NewSessionRecord) -> Result<SessionDetail, String> {
        self.create_session_in_folder(record, None)
    }

    pub fn create_session_in_folder(
        &self,
        record: NewSessionRecord,
        folder_id: Option<&str>,
    ) -> Result<SessionDetail, String> {
        let _guard = self.write_guard()?;
        let conn = self.open()?;
        let folder_id = folder_id.unwrap_or("folder-default");
        let folder_exists: Option<i64> = conn
            .query_row(
                "SELECT 1 FROM session_folder WHERE id = ?1",
                params![folder_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(db_err)?;
        if folder_exists.is_none() {
            return Err("目标文件夹不存在".to_string());
        }
        conn.execute(
            "INSERT INTO session
             (id, cli_session_id, title, engine, model, cwd, status, created_at, updated_at, folder_id)
             VALUES (?1, NULL, '未命名会话', ?2, ?3, ?4, 'active', ?5, ?5, ?6)",
            params![
                record.id,
                engine_to_str(record.engine),
                record.model,
                record.cwd,
                record.created_at,
                folder_id
            ],
        )
        .map_err(db_err)?;
        self.set_setting_on_conn(&conn, "active_session_id", &record.id)?;
        self.get_session(&record.id)
    }

    /// 生产新建链路：未显式选择 Folder 时，按 canonical cwd 自动复用或创建项目 Folder。
    pub fn create_session_for_cwd(
        &self,
        record: NewSessionRecord,
        folder_id: Option<&str>,
    ) -> Result<SessionDetail, String> {
        self.create_session_for_cwd_tracked(record, folder_id)
            .map(|(session, _)| session)
    }

    /// 与 `create_session_for_cwd` 相同，但额外返回本次事务新建的项目 Folder id，
    /// 供 Runtime 启动失败时精确回滚空 Folder。
    pub(crate) fn create_session_for_cwd_tracked(
        &self,
        record: NewSessionRecord,
        folder_id: Option<&str>,
    ) -> Result<(SessionDetail, Option<String>), String> {
        // canonicalize may block on disconnected drives. Resolve it before taking the
        // process-wide database write lock so unrelated session writes can continue.
        let canonical_cwd = if folder_id.is_none() {
            Some(canonical_folder_cwd(&record.cwd)?)
        } else {
            None
        };
        let _guard = self.write_guard()?;
        let conn = self.open()?;
        let tx = conn.unchecked_transaction().map_err(db_err)?;
        let (folder_id, created_folder_id) = match folder_id {
            Some(folder_id) => {
                let exists: Option<i64> = tx
                    .query_row(
                        "SELECT 1 FROM session_folder WHERE id = ?1",
                        params![folder_id],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(db_err)?;
                if exists.is_none() {
                    return Err("目标文件夹不存在".to_string());
                }
                (folder_id.to_string(), None)
            }
            None => {
                let (canonical_cwd, cwd_key) = canonical_cwd
                    .as_ref()
                    .expect("未显式选择 Folder 时必须已解析 canonical cwd");
                let (folder_id, created) =
                    resolve_or_create_cwd_folder(&tx, canonical_cwd, cwd_key)?;
                let created_folder_id = created.then_some(folder_id.clone());
                (folder_id, created_folder_id)
            }
        };
        tx.execute(
            "INSERT INTO session
             (id, cli_session_id, title, engine, model, cwd, status, created_at, updated_at, folder_id)
             VALUES (?1, NULL, '未命名会话', ?2, ?3, ?4, 'active', ?5, ?5, ?6)",
            params![
                record.id,
                engine_to_str(record.engine),
                record.model,
                record.cwd,
                record.created_at,
                folder_id
            ],
        )
        .map_err(db_err)?;
        self.set_setting_on_conn(&tx, "active_session_id", &record.id)?;
        tx.commit().map_err(db_err)?;
        self.get_session(&record.id)
            .map(|session| (session, created_folder_id))
    }

    /// 仅清理由指定创建事务新建、且当前仍为空的自动项目 Folder。
    pub(crate) fn delete_empty_project_folder(&self, folder_id: &str) -> Result<(), String> {
        let _guard = self.write_guard()?;
        let conn = self.open()?;
        conn.execute(
            "DELETE FROM session_folder
             WHERE id = ?1
               AND cwd_key IS NOT NULL
               AND NOT EXISTS (SELECT 1 FROM session WHERE folder_id = ?1)",
            params![folder_id],
        )
        .map_err(db_err)?;
        Ok(())
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
                   s.created_at, s.updated_at, s.summary, s.pinned, s.runtime_capabilities_json,
                   s.safe_permission_profile, s.folder_id,
                   COALESCE(u.cached_input_tokens, 0),
                   COALESCE(u.cache_write_input_tokens, 0),
                   s.last_context_tokens, s.last_context_window,
                   s.preferred_model, s.preferred_reasoning_effort
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
                     SUM(cached_input_tokens) AS cached_input_tokens,
                     SUM(cache_write_input_tokens) AS cache_write_input_tokens,
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
        self.refresh_session_contexts(id)?;
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
                   s.created_at, s.updated_at, s.summary, s.pinned, s.runtime_capabilities_json,
                   s.safe_permission_profile, s.folder_id,
                   COALESCE((SELECT SUM(cached_input_tokens) FROM usage WHERE session_id = s.id), 0),
                   COALESCE((SELECT SUM(cache_write_input_tokens) FROM usage WHERE session_id = s.id), 0),
                   s.last_context_tokens, s.last_context_window,
                   s.preferred_model, s.preferred_reasoning_effort
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
            turns: self.turns_for_conn(&conn, &summary.id)?,
            session_context: self.session_context_for_conn(&conn, &summary.id)?,
            fork: self.session_fork_for_conn(&conn, &summary.id)?,
            summary,
        })
    }

    fn session_fork_for_conn(
        &self,
        conn: &Connection,
        target_session_id: &str,
    ) -> Result<Option<SessionForkSummary>, String> {
        conn.query_row(
            "SELECT f.id, f.handoff_id, f.source_session_id, h.source_title_snapshot,
                    h.source_engine, f.target_engine, f.boundary_turn_id,
                    f.boundary_turn_epoch, f.created_at
             FROM session_fork f
             JOIN handoff h ON h.id = f.handoff_id
             WHERE f.target_session_id = ?1",
            params![target_session_id],
            |row| {
                let epoch: i64 = row.get(7)?;
                Ok(SessionForkSummary {
                    id: row.get(0)?,
                    handoff_id: row.get(1)?,
                    source_session_id: row.get(2)?,
                    source_title_snapshot: row.get(3)?,
                    source_engine: row.get(4)?,
                    target_engine: row.get(5)?,
                    boundary_turn_id: row.get(6)?,
                    boundary_turn_epoch: u64::try_from(epoch).unwrap_or_default(),
                    created_at: row.get(8)?,
                })
            },
        )
        .optional()
        .map_err(db_err)
    }

    fn turns_for_conn(
        &self,
        conn: &Connection,
        session_id: &str,
    ) -> Result<Vec<SessionTurn>, String> {
        let mut stmt = conn
            .prepare(
                "SELECT t.turn_id, t.turn_epoch, t.turn_mode, t.permission_profile, t.status,
                        t.started_at, t.ended_at, t.terminal_reason,
                        s.provider_display_name, s.requested_model_id, s.routed_model_id,
                        s.requested_reasoning_effort, s.routed_reasoning_effort,
                        s.resolution_source
                 FROM turn t
                 LEFT JOIN turn_execution_spec s ON s.turn_id = t.turn_id
                 WHERE t.history_session_id = ?1
                 ORDER BY t.started_at ASC, t.turn_epoch ASC",
            )
            .map_err(db_err)?;
        let rows = stmt
            .query_map(params![session_id], |row| {
                let epoch: i64 = row.get(1)?;
                Ok(SessionTurn {
                    id: row.get(0)?,
                    epoch: u64::try_from(epoch).unwrap_or_default(),
                    mode: row.get(2)?,
                    permission_profile: row.get(3)?,
                    status: row.get(4)?,
                    started_at: row.get(5)?,
                    ended_at: row.get(6)?,
                    terminal_reason: row.get(7)?,
                    provider_display_name: row.get(8)?,
                    requested_model_id: row.get(9)?,
                    routed_model_id: row.get(10)?,
                    requested_reasoning_effort: row.get(11)?,
                    routed_reasoning_effort: row.get(12)?,
                    resolution_source: row.get(13)?,
                })
            })
            .map_err(db_err)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(db_err)
    }

    pub fn get_turn_ledger(&self, session_id: &str) -> Result<Vec<TurnLedgerRecord>, String> {
        let conn = self.open()?;
        let local_id = self.resolve_local_id(&conn, session_id)?;
        let messages = self.messages_for_conn(&conn, &local_id)?;
        let tools = self.tools_for_conn(&conn, &local_id)?;
        let approvals = self.approvals_for_conn(&conn, &local_id)?;
        let checkpoints = self.checkpoints_for_conn(&conn, &local_id)?;
        let turns = self.turns_for_conn(&conn, &local_id)?;
        let mut ledger = Vec::new();
        for turn in turns {
            let route = conn
                .query_row(
                    "SELECT engine_id, provider_id, requested_model_id, routed_model_id,
                            requested_reasoning_effort, routed_reasoning_effort,
                            resolution_source, launch_config_digest, pricing_basis_snapshot_json
                     FROM turn_execution_spec
                     WHERE history_session_id = ?1 AND turn_id = ?2",
                    params![&local_id, &turn.id],
                    |row| {
                        let pricing: String = row.get(8)?;
                        Ok(TurnLedgerRoute {
                            engine_id: row.get(0)?,
                            provider_id: row.get(1)?,
                            requested_model_id: row.get(2)?,
                            routed_model_id: row.get(3)?,
                            requested_reasoning_effort: row.get(4)?,
                            routed_reasoning_effort: row.get(5)?,
                            resolution_source: row.get(6)?,
                            launch_config_digest: row.get(7)?,
                            pricing_basis_snapshot: serde_json::from_str(&pricing)
                                .unwrap_or(serde_json::Value::Null),
                        })
                    },
                )
                .optional()
                .map_err(db_err)?;
            let Some(route) = route else {
                continue;
            };
            let mut usage_stmt = conn
                .prepare(
                    "SELECT model, provider_id, effective_reasoning_effort, model_evidence,
                            input_tokens, cached_input_tokens, cache_write_input_tokens,
                            output_tokens, service_tier, cost_usd, price_snapshot_json, ts
                     FROM usage WHERE session_id = ?1 AND turn_id = ?2 ORDER BY id ASC",
                )
                .map_err(db_err)?;
            let usage = usage_stmt
                .query_map(params![&local_id, &turn.id], |row| {
                    let input: i64 = row.get(4)?;
                    let cached: i64 = row.get(5)?;
                    let cache_write: i64 = row.get(6)?;
                    let output: i64 = row.get(7)?;
                    let price: Option<String> = row.get(10)?;
                    Ok(TurnLedgerUsage {
                        model: row.get(0)?,
                        provider_id: row.get(1)?,
                        effective_reasoning_effort: row.get(2)?,
                        model_evidence: row.get(3)?,
                        input_tokens: u64::try_from(input).unwrap_or_default(),
                        cached_input_tokens: u64::try_from(cached).unwrap_or_default(),
                        cache_write_input_tokens: u64::try_from(cache_write).unwrap_or_default(),
                        output_tokens: u64::try_from(output).unwrap_or_default(),
                        service_tier: row.get(8)?,
                        cost_usd: row.get(9)?,
                        price_snapshot: price.and_then(|value| serde_json::from_str(&value).ok()),
                        ts: row.get(11)?,
                    })
                })
                .map_err(db_err)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(db_err)?;
            let mut attachment_stmt = conn
                .prepare(
                    "SELECT source_path, path_digest, ordinal FROM message_attachment
                     WHERE session_id = ?1 AND turn_id = ?2 ORDER BY ordinal ASC",
                )
                .map_err(db_err)?;
            let attachments = attachment_stmt
                .query_map(params![&local_id, &turn.id], |row| {
                    let ordinal: i64 = row.get(2)?;
                    Ok(TurnLedgerAttachment {
                        source_path: row.get(0)?,
                        path_digest: row.get(1)?,
                        ordinal: u64::try_from(ordinal).unwrap_or_default(),
                    })
                })
                .map_err(db_err)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(db_err)?;
            let mut context_stmt = conn
                .prepare(
                    "SELECT context_id, kind, canonical_path_digest, identity_digest,
                            validation_status, ordinal
                     FROM turn_context_snapshot
                     WHERE session_id = ?1 AND turn_id = ?2 ORDER BY ordinal ASC",
                )
                .map_err(db_err)?;
            let session_context = context_stmt
                .query_map(params![&local_id, &turn.id], |row| {
                    let ordinal: i64 = row.get(5)?;
                    Ok(TurnLedgerContextEvidence {
                        context_id: row.get(0)?,
                        kind: row.get(1)?,
                        canonical_path_digest: row.get(2)?,
                        identity_digest: row.get(3)?,
                        validation_status: row.get(4)?,
                        ordinal: u64::try_from(ordinal).unwrap_or_default(),
                    })
                })
                .map_err(db_err)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(db_err)?;
            ledger.push(TurnLedgerRecord {
                messages: messages
                    .iter()
                    .filter(|message| message.turn_id.as_deref() == Some(turn.id.as_str()))
                    .cloned()
                    .collect(),
                tool_calls: tools
                    .iter()
                    .filter(|tool| tool.turn_id.as_deref() == Some(turn.id.as_str()))
                    .cloned()
                    .collect(),
                approvals: approvals
                    .iter()
                    .filter(|approval| approval.turn_id.as_deref() == Some(turn.id.as_str()))
                    .cloned()
                    .collect(),
                checkpoints: checkpoints
                    .iter()
                    .filter(|checkpoint| checkpoint.turn_id.as_deref() == Some(turn.id.as_str()))
                    .cloned()
                    .collect(),
                turn,
                route,
                usage,
                attachments,
                session_context,
            });
        }
        Ok(ledger)
    }

    pub fn session_handoff_context(&self, session_id: &str) -> Result<Option<String>, String> {
        let conn = self.open()?;
        let local_id = self.resolve_local_id(&conn, session_id)?;
        let content_json = conn
            .query_row(
                "SELECT h.content_json
                 FROM session_fork f
                 JOIN handoff h ON h.id = f.handoff_id
                 WHERE f.target_session_id = ?1",
                params![local_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(db_err)?;
        content_json
            .map(|raw| {
                serde_json::from_str::<crate::handoff::HandoffContent>(&raw)
                    .map_err(|error| format!("Handoff Context 无效：{error}"))
                    .map(|content| content.as_context())
            })
            .transpose()
    }

    pub fn latest_turn_epoch(&self, session_id: &str) -> Result<u64, String> {
        let conn = self.open()?;
        let local_id = self.resolve_local_id(&conn, session_id)?;
        let epoch = conn
            .query_row(
                "SELECT COALESCE(MAX(turn_epoch), 0) FROM turn WHERE history_session_id = ?1",
                params![local_id],
                |row| row.get::<_, i64>(0),
            )
            .map_err(db_err)?;
        u64::try_from(epoch).map_err(|_| "invalid negative persisted turn epoch".to_string())
    }

    pub fn add_session_context(
        &self,
        session_id: &str,
        source_path: &str,
    ) -> Result<SessionContextRecord, String> {
        let conn = self.open()?;
        let local_id = self.resolve_local_id(&conn, session_id)?;
        let cwd: String = conn
            .query_row(
                "SELECT cwd FROM session WHERE id = ?1",
                params![&local_id],
                |row| row.get(0),
            )
            .map_err(db_err)?;
        ensure_context_mutation_allowed(&conn, &local_id)?;
        drop(conn);
        let validated = crate::session_context::validate_session_context_path(&cwd, source_path)?;
        let now = now_millis();
        let id = format!("context-{:032x}", rand::random::<u128>());
        let canonical_key = context_path_key(&validated.canonical_path);
        retry_locked(|| {
            let _guard = self.write_guard()?;
            let conn = self.open()?;
            let local_id = self.resolve_local_id(&conn, session_id)?;
            ensure_context_mutation_allowed(&conn, &local_id)?;
            conn.execute(
                "INSERT INTO session_context
                 (id, session_id, kind, source_path, canonical_path, canonical_key,
                  display_name, status, status_detail, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'ready', NULL, ?8, ?8)",
                params![
                    &id,
                    local_id,
                    validated.kind,
                    source_path,
                    validated.canonical_path,
                    canonical_key,
                    validated.display_name,
                    now,
                ],
            )
            .map_err(|error| {
                if matches!(error, rusqlite::Error::SqliteFailure(_, _)) {
                    "该路径已在会话上下文中".to_string()
                } else {
                    db_err(error)
                }
            })?;
            Ok(())
        })?;
        Ok(SessionContextRecord {
            id,
            kind: validated.kind.to_string(),
            source_path: source_path.to_string(),
            canonical_path: validated.canonical_path,
            display_name: validated.display_name,
            status: "ready".to_string(),
            status_detail: None,
            created_at: now,
            updated_at: now,
        })
    }

    pub fn remove_session_context(&self, session_id: &str, context_id: &str) -> Result<(), String> {
        retry_locked(|| {
            let _guard = self.write_guard()?;
            let conn = self.open()?;
            let local_id = self.resolve_local_id(&conn, session_id)?;
            ensure_context_mutation_allowed(&conn, &local_id)?;
            let removed = conn
                .execute(
                    "DELETE FROM session_context WHERE id = ?1 AND session_id = ?2",
                    params![context_id, local_id],
                )
                .map_err(db_err)?;
            if removed != 1 {
                return Err("找不到会话上下文".to_string());
            }
            Ok(())
        })
    }

    pub fn list_session_contexts(
        &self,
        session_id: &str,
    ) -> Result<Vec<SessionContextRecord>, String> {
        self.refresh_session_contexts(session_id)?;
        let conn = self.open()?;
        let local_id = self.resolve_local_id(&conn, session_id)?;
        self.session_context_for_conn(&conn, &local_id)
    }

    pub fn freeze_session_contexts(
        &self,
        session_id: &str,
    ) -> Result<Vec<FrozenSessionContext>, String> {
        let contexts = self.list_session_contexts(session_id)?;
        if let Some(context) = contexts.iter().find(|context| context.status != "ready") {
            return Err(format!(
                "会话上下文“{}”不可用：{}",
                context.display_name,
                context
                    .status_detail
                    .as_deref()
                    .unwrap_or("请移除或修复后重试")
            ));
        }
        let conn = self.open()?;
        let local_id = self.resolve_local_id(&conn, session_id)?;
        let cwd: String = conn
            .query_row(
                "SELECT cwd FROM session WHERE id = ?1",
                params![&local_id],
                |row| row.get(0),
            )
            .map_err(db_err)?;
        contexts
            .into_iter()
            .map(|context| {
                let validated = crate::session_context::validate_session_context_path(
                    &cwd,
                    &context.source_path,
                )?;
                if validated.canonical_path != context.canonical_path {
                    return Err(format!(
                        "会话上下文“{}”的 canonical path 已漂移",
                        context.display_name
                    ));
                }
                Ok(FrozenSessionContext {
                    id: context.id,
                    kind: context.kind,
                    canonical_path: context.canonical_path,
                    canonical_path_digest: validated.canonical_path_digest,
                    identity_digest: validated.identity_digest,
                })
            })
            .collect()
    }

    fn refresh_session_contexts(&self, session_id: &str) -> Result<(), String> {
        let conn = self.open()?;
        let local_id = self.resolve_local_id(&conn, session_id)?;
        let cwd: String = conn
            .query_row(
                "SELECT cwd FROM session WHERE id = ?1",
                params![&local_id],
                |row| row.get(0),
            )
            .map_err(db_err)?;
        let contexts = self.session_context_for_conn(&conn, &local_id)?;
        drop(conn);
        let updates = contexts
            .iter()
            .map(|context| {
                match crate::session_context::validate_session_context_path(
                    &cwd,
                    &context.source_path,
                ) {
                    Ok(validated) if validated.canonical_path == context.canonical_path => {
                        (context.id.clone(), "ready", None)
                    }
                    Ok(_) => (
                        context.id.clone(),
                        "blocked",
                        Some("canonical path 已漂移".to_string()),
                    ),
                    Err(error) => {
                        let status = if error.contains("不存在") || error.contains("移动") {
                            "missing"
                        } else {
                            "blocked"
                        };
                        (context.id.clone(), status, Some(error))
                    }
                }
            })
            .collect::<Vec<_>>();
        if updates.is_empty() {
            return Ok(());
        }
        retry_locked(|| {
            let _guard = self.write_guard()?;
            let mut conn = self.open()?;
            let tx = conn.transaction().map_err(db_err)?;
            for (id, status, detail) in &updates {
                tx.execute(
                    "UPDATE session_context
                     SET status = ?1, status_detail = ?2, updated_at = ?3
                     WHERE id = ?4 AND session_id = ?5",
                    params![status, detail, now_millis(), id, &local_id],
                )
                .map_err(db_err)?;
            }
            tx.commit().map_err(db_err)
        })
    }

    /// 记录用户消息。`ts_millis` 为毫秒时间戳（变更-07：message.ts 与 checkpoint.ts 同单位）。
    pub fn record_user_message(
        &self,
        session_id: &str,
        text: &str,
        ts_millis: i64,
    ) -> Result<(), String> {
        let text = crate::redaction::redact_text(text);
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
                    params![title_from_text(&text), local_id],
                )
                .map_err(db_err)?;
            }
            tx.execute(
                "INSERT INTO message (session_id, role, text, ts) VALUES (?1, 'user', ?2, ?3)",
                params![local_id, &text, ts_millis],
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

    /// 27C TurnStart 线性化点：调用方必须先持有 ProviderStore gate。
    /// Turn、用户 Message 与不可变 spec 在同一 SQLite 事务中提交。
    pub fn start_turn(
        &self,
        command: &TurnStartCommand,
        mut spec: TurnExecutionSpec,
    ) -> Result<(PreparedUserTurn, TurnExecutionSpec), String> {
        if command.history_session_id != spec.history_session_id {
            return Err("TurnStartCommand 与 TurnExecutionSpec 的 Session 不一致".to_string());
        }
        if command.turn_mode != spec.turn_mode
            || command.permission_profile != spec.permission_profile
            || command.created_at != spec.created_at
        {
            return Err("TurnStartCommand 与 TurnExecutionSpec 的提交事实不一致".to_string());
        }
        if spec.turn_epoch != 0 {
            return Err("TurnEpoch 必须由 SQLite 提交事务分配".to_string());
        }
        let text = crate::redaction::redact_text(&command.display_text);
        let budget = TurnBudgetSnapshot::standard(command.created_at);
        budget.validate()?;
        let input_bytes = text.as_bytes().len().saturating_add(
            command
                .attachments
                .iter()
                .map(|attachment| attachment.as_bytes().len())
                .sum::<usize>(),
        );
        budget.enforce_input_bytes(input_bytes)?;
        retry_locked(|| {
            let _guard = self.write_guard()?;
            let mut conn = self.open()?;
            let local_id = self.resolve_local_id(&conn, &command.history_session_id)?;
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(db_err)?;
            verify_frozen_context_set(&tx, &local_id, &spec.session_context)?;
            let (previous_title, previous_status, previous_updated_at) = tx
                .query_row(
                    "SELECT title, status, updated_at FROM session WHERE id = ?1",
                    params![local_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .map_err(db_err)?;
            let previous_active_session_id = self.get_setting_on_conn(&tx, "active_session_id")?;
            let next_epoch: i64 = tx
                .query_row(
                    "SELECT COALESCE(MAX(turn_epoch), 0) + 1 FROM turn WHERE history_session_id = ?1",
                    params![local_id],
                    |row| row.get(0),
                )
                .map_err(db_err)?;
            spec.turn_epoch =
                u64::try_from(next_epoch).map_err(|_| "Session TurnEpoch 已溢出".to_string())?;
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
                    params![title_from_text(&text), local_id],
                )
                .map_err(db_err)?;
            }
            tx.execute(
                "INSERT INTO turn
                 (history_session_id, turn_id, turn_epoch, turn_mode, permission_profile,
                  status, started_at, identity_source)
                 VALUES (?1, ?2, ?3, ?4, ?5, 'committed', ?6, 'control_plane')",
                params![
                    local_id,
                    &spec.turn_id,
                    next_epoch,
                    &spec.turn_mode,
                    &spec.permission_profile,
                    spec.created_at,
                ],
            )
            .map_err(db_err)?;
            tx.execute(
                "INSERT INTO turn_execution_spec
                 (turn_id, history_session_id, turn_epoch, engine_id, provider_id, provider_kind,
                  provider_display_name, route_label_snapshot, requested_model_id, routed_model_id,
                  model_label_snapshot, requested_reasoning_effort, routed_reasoning_effort,
                  turn_mode, permission_profile, binding_id, binding_revision,
                  engine_profile_digest, provider_launch_profile_ref, launch_config_digest,
                  routing_capability_snapshot_id, resolution_source, legacy_route_snapshot_digest,
                  pricing_basis_snapshot_json, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13,
                         ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25)",
                params![
                    &spec.turn_id,
                    local_id,
                    next_epoch,
                    &spec.engine_id,
                    &spec.provider_id,
                    &spec.provider_kind,
                    &spec.provider_display_name,
                    &spec.route_label_snapshot,
                    &spec.requested_model_id,
                    &spec.routed_model_id,
                    &spec.model_label_snapshot,
                    spec.requested_reasoning_effort.as_str(),
                    spec.routed_reasoning_effort.as_str(),
                    &spec.turn_mode,
                    &spec.permission_profile,
                    &spec.binding_id,
                    spec.binding_revision
                        .and_then(|value| i64::try_from(value).ok()),
                    &spec.engine_profile_digest,
                    &spec.provider_launch_profile_ref,
                    &spec.launch_config_digest,
                    &spec.routing_capability_snapshot_id,
                    &spec.resolution_source,
                    &spec.legacy_route_snapshot_digest,
                    serde_json::to_string(&spec.pricing_basis_snapshot)
                        .map_err(|error| error.to_string())?,
                    spec.created_at,
                ],
            )
            .map_err(db_err)?;
            tx.execute(
                "INSERT INTO turn_budget_snapshot
                 (turn_id, snapshot_json, created_at) VALUES (?1, ?2, ?3)",
                params![
                    &spec.turn_id,
                    serde_json::to_string(&budget).map_err(|error| error.to_string())?,
                    budget.created_at,
                ],
            )
            .map_err(db_err)?;
            tx.execute(
                "INSERT INTO message (session_id, role, text, ts, turn_id)
                 VALUES (?1, 'user', ?2, ?3, ?4)",
                params![local_id, &text, command.created_at, &spec.turn_id],
            )
            .map_err(db_err)?;
            let message_id = tx.last_insert_rowid();
            for (ordinal, source_path) in command.attachments.iter().enumerate() {
                let source_path = crate::redaction::redact_text(source_path.trim());
                if source_path.is_empty() {
                    continue;
                }
                let path_digest = format!("sha256:{:x}", Sha256::digest(source_path.as_bytes()));
                tx.execute(
                    "INSERT INTO message_attachment
                     (id, message_id, session_id, turn_id, ordinal, source_path, path_digest)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![
                        format!("attachment-{:032x}", rand::random::<u128>()),
                        message_id,
                        local_id,
                        &spec.turn_id,
                        i64::try_from(ordinal).unwrap_or(i64::MAX),
                        source_path,
                        path_digest,
                    ],
                )
                .map_err(db_err)?;
            }
            for (ordinal, context) in spec.session_context.iter().enumerate() {
                tx.execute(
                    "INSERT INTO turn_context_snapshot
                     (turn_id, session_id, context_id, ordinal, kind, canonical_path_digest,
                      identity_digest, validation_status)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'ready')",
                    params![
                        &spec.turn_id,
                        local_id,
                        &context.id,
                        i64::try_from(ordinal).unwrap_or(i64::MAX),
                        &context.kind,
                        &context.canonical_path_digest,
                        &context.identity_digest,
                    ],
                )
                .map_err(db_err)?;
            }
            tx.execute(
                "UPDATE session SET status = 'active', updated_at = ?1 WHERE id = ?2",
                params![command.created_at / 1000, local_id],
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
                params![serde_json::to_string(&local_id).map_err(|error| error.to_string())?],
            )
            .map_err(db_err)?;
            tx.commit().map_err(db_err)?;
            Ok((
                PreparedUserTurn {
                    session_id: local_id,
                    message_id,
                    turn_id: Some(spec.turn_id.clone()),
                    previous_title,
                    previous_status,
                    previous_updated_at,
                    previous_active_session_id,
                    expired_approval_ids,
                },
                spec.clone(),
            ))
        })
    }

    /// 在启动 CLI 前原子准备本轮历史副作用；若运行时拒绝启动，可用返回值完整回滚。
    pub fn prepare_user_turn(
        &self,
        session_id: &str,
        text: &str,
        ts_millis: i64,
    ) -> Result<PreparedUserTurn, String> {
        let text = crate::redaction::redact_text(text);
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
                    params![title_from_text(&text), local_id],
                )
                .map_err(db_err)?;
            }
            tx.execute(
                "INSERT INTO message (session_id, role, text, ts) VALUES (?1, 'user', ?2, ?3)",
                params![local_id, &text, ts_millis],
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
                turn_id: None,
                previous_title,
                previous_status,
                previous_updated_at,
                previous_active_session_id,
                expired_approval_ids,
            })
        })
    }

    pub fn create_background_operation(
        &self,
        new_operation: &NewBackgroundOperation,
    ) -> Result<(BackgroundOperation, bool), String> {
        new_operation.validate()?;
        let _guard = self.write_guard()?;
        let mut conn = self.open()?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_err)?;
        if let Some(existing) = load_background_operation_on_conn(
            &tx,
            "idempotency_key",
            &new_operation.operation.idempotency_key,
        )? {
            tx.commit().map_err(db_err)?;
            return Ok((existing, false));
        }
        let operation = &new_operation.operation;
        let spec = &new_operation.spec;
        let policy = &new_operation.policy;
        tx.execute(
            "INSERT INTO background_operation
             (id, kind, source_session_id, input_digest, input_json, idempotency_key, status, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'committed', ?7)",
            params![
                operation.id,
                operation.kind,
                operation.source_session_id,
                operation.input_digest,
                operation.input.as_ref().map(serde_json::to_string).transpose().map_err(|error| error.to_string())?,
                operation.idempotency_key,
                operation.created_at,
            ],
        )
        .map_err(db_err)?;
        tx.execute(
            "INSERT INTO operation_execution_spec
             (operation_id, engine_id, provider_id, provider_kind, provider_display_name,
              route_label_snapshot, requested_model_id, routed_model_id, model_label_snapshot,
              requested_reasoning_effort, routed_reasoning_effort, binding_id, binding_revision,
              engine_profile_digest, provider_launch_profile_ref, provider_launch_profile_digest,
              launch_config_digest, routing_capability_snapshot_id, pricing_basis_snapshot_json,
              purpose, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14,
                     ?15, ?16, ?17, ?18, ?19, ?20, ?21)",
            params![
                spec.operation_id,
                spec.engine_id,
                spec.provider_id,
                spec.provider_kind,
                spec.provider_display_name,
                spec.route_label_snapshot,
                spec.requested_model_id,
                spec.routed_model_id,
                spec.model_label_snapshot,
                spec.requested_reasoning_effort.as_str(),
                spec.routed_reasoning_effort.as_str(),
                spec.binding_id,
                i64::try_from(spec.binding_revision).map_err(|_| "Binding revision 已溢出")?,
                spec.engine_profile_digest,
                spec.provider_launch_profile_ref,
                spec.provider_launch_profile_digest,
                spec.launch_config_digest,
                spec.routing_capability_snapshot_id,
                serde_json::to_string(&spec.pricing_basis_snapshot)
                    .map_err(|error| error.to_string())?,
                spec.purpose,
                spec.created_at,
            ],
        )
        .map_err(db_err)?;
        tx.execute(
            "INSERT INTO model_only_operation_policy
             (operation_id, contract_version, canonical_cwd, sandbox_mode, tools_disabled,
              extensions_disabled, persistent_grants_disabled, capability_snapshot_id,
              launch_evidence, created_at)
             VALUES (?1, ?2, '', 'read_only', 1, 1, 1, ?3, ?4, ?5)",
            params![
                spec.operation_id,
                policy.contract_version,
                policy.capability_snapshot_id,
                policy.launch_evidence,
                policy.created_at,
            ],
        )
        .map_err(db_err)?;
        tx.execute(
            "INSERT INTO operation_budget_snapshot (operation_id, snapshot_json, created_at)
             VALUES (?1, ?2, ?3)",
            params![
                spec.operation_id,
                serde_json::to_string(&new_operation.budget).map_err(|error| error.to_string())?,
                new_operation.budget.created_at,
            ],
        )
        .map_err(db_err)?;
        tx.commit().map_err(db_err)?;
        Ok((operation.clone(), true))
    }

    pub fn load_turn_budget_snapshot(&self, turn_id: &str) -> Result<TurnBudgetSnapshot, String> {
        let conn = self.open()?;
        let raw: String = conn
            .query_row(
                "SELECT snapshot_json FROM turn_budget_snapshot WHERE turn_id = ?1",
                params![turn_id],
                |row| row.get(0),
            )
            .map_err(db_err)?;
        let snapshot = serde_json::from_str::<TurnBudgetSnapshot>(&raw)
            .map_err(|error| format!("TurnBudgetSnapshot 无效：{error}"))?;
        snapshot.validate()?;
        Ok(snapshot)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn record_turn_budget_fact(
        &self,
        turn_id: &str,
        attempt_no: u64,
        dimension: crate::budget::BudgetDimension,
        observed: u64,
        limit: u64,
        enforcement_mode: crate::budget::BudgetEnforcementMode,
        action: &str,
    ) -> Result<(), String> {
        let _guard = self.write_guard()?;
        let conn = self.open()?;
        conn.execute(
            "INSERT OR IGNORE INTO turn_budget_fact
             (turn_id, attempt_no, dimension, observed, budget_limit, enforcement_mode, action, observed_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                turn_id,
                attempt_no,
                dimension.as_str(),
                observed,
                limit,
                serde_json::to_value(enforcement_mode)
                    .ok()
                    .and_then(|value| value.as_str().map(ToString::to_string))
                    .unwrap_or_else(|| "unknown".to_string()),
                action,
                now_millis(),
            ],
        )
        .map(|_| ())
        .map_err(db_err)
    }

    pub fn load_background_operation(
        &self,
        operation_id: &str,
    ) -> Result<Option<BackgroundOperation>, String> {
        let conn = self.open()?;
        load_background_operation_on_conn(&conn, "id", operation_id)
    }

    pub fn load_background_operation_execution(
        &self,
        operation_id: &str,
    ) -> Result<Option<NewBackgroundOperation>, String> {
        let conn = self.open()?;
        let Some(operation) = load_background_operation_on_conn(&conn, "id", operation_id)? else {
            return Ok(None);
        };
        let spec = conn
            .query_row(
                "SELECT engine_id, provider_id, provider_kind, provider_display_name,
                        route_label_snapshot, requested_model_id, routed_model_id,
                        model_label_snapshot, requested_reasoning_effort,
                        routed_reasoning_effort, binding_id, binding_revision,
                        engine_profile_digest, provider_launch_profile_ref,
                        provider_launch_profile_digest, launch_config_digest,
                        routing_capability_snapshot_id, pricing_basis_snapshot_json,
                        purpose, created_at
                 FROM operation_execution_spec WHERE operation_id = ?1",
                params![operation_id],
                |row| {
                    let requested_raw: String = row.get(8)?;
                    let routed_raw: String = row.get(9)?;
                    let requested_reasoning_effort =
                        crate::reasoning::ReasoningEffort::parse(Some(&requested_raw))
                            .and_then(|value| {
                                value.ok_or_else(|| "缺少 requested effort".to_string())
                            })
                            .map_err(|error| sql_text_conversion_error(8, error))?;
                    let routed_reasoning_effort =
                        crate::reasoning::ReasoningEffort::parse(Some(&routed_raw))
                            .and_then(|value| value.ok_or_else(|| "缺少 routed effort".to_string()))
                            .map_err(|error| sql_text_conversion_error(9, error))?;
                    let binding_revision_raw: i64 = row.get(11)?;
                    let binding_revision = u64::try_from(binding_revision_raw).map_err(|_| {
                        sql_text_conversion_error(11, "Binding revision 无效".to_string())
                    })?;
                    let pricing_raw: String = row.get(17)?;
                    let pricing_basis_snapshot =
                        serde_json::from_str(&pricing_raw).map_err(|error| {
                            rusqlite::Error::FromSqlConversionFailure(
                                17,
                                rusqlite::types::Type::Text,
                                Box::new(error),
                            )
                        })?;
                    Ok(OperationExecutionSpec {
                        operation_id: operation_id.to_string(),
                        owner: crate::runtime_registry::RuntimeOwnerRef::Operation(
                            operation_id.to_string(),
                        ),
                        engine_id: row.get(0)?,
                        provider_id: row.get(1)?,
                        provider_kind: row.get(2)?,
                        provider_display_name: row.get(3)?,
                        route_label_snapshot: row.get(4)?,
                        requested_model_id: row.get(5)?,
                        routed_model_id: row.get(6)?,
                        model_label_snapshot: row.get(7)?,
                        requested_reasoning_effort,
                        routed_reasoning_effort,
                        binding_id: row.get(10)?,
                        binding_revision,
                        engine_profile_digest: row.get(12)?,
                        provider_launch_profile_ref: row.get(13)?,
                        provider_launch_profile_digest: row.get(14)?,
                        launch_config_digest: row.get(15)?,
                        routing_capability_snapshot_id: row.get(16)?,
                        pricing_basis_snapshot,
                        purpose: row.get(18)?,
                        created_at: row.get(19)?,
                    })
                },
            )
            .map_err(db_err)?;
        let policy = conn
            .query_row(
                "SELECT contract_version, canonical_cwd, sandbox_mode, tools_disabled,
                        extensions_disabled, persistent_grants_disabled,
                        capability_snapshot_id, launch_evidence, created_at
                 FROM model_only_operation_policy WHERE operation_id = ?1",
                params![operation_id],
                |row| {
                    Ok(ModelOnlyOperationPolicy {
                        contract_version: row.get(0)?,
                        canonical_cwd: row.get(1)?,
                        sandbox_mode: row.get(2)?,
                        tools_disabled: row.get::<_, i64>(3)? != 0,
                        extensions_disabled: row.get::<_, i64>(4)? != 0,
                        persistent_grants_disabled: row.get::<_, i64>(5)? != 0,
                        capability_snapshot_id: row.get(6)?,
                        launch_evidence: row.get(7)?,
                        created_at: row.get(8)?,
                    })
                },
            )
            .map_err(db_err)?;
        let budget_raw: String = conn
            .query_row(
                "SELECT snapshot_json FROM operation_budget_snapshot WHERE operation_id = ?1",
                params![operation_id],
                |row| row.get(0),
            )
            .map_err(db_err)?;
        let budget = serde_json::from_str(&budget_raw)
            .map_err(|error| format!("Operation budget snapshot 无效：{error}"))?;
        let execution = NewBackgroundOperation {
            operation,
            spec,
            policy,
            budget,
        };
        execution.validate()?;
        Ok(Some(execution))
    }

    pub fn create_operation_attempt(
        &self,
        operation_id: &str,
        generation: &crate::runtime_registry::RuntimeGeneration,
    ) -> Result<u64, String> {
        let _guard = self.write_guard()?;
        let mut conn = self.open()?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_err)?;
        let ready: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM background_operation o
                 JOIN operation_execution_spec s ON s.operation_id = o.id
                 JOIN model_only_operation_policy p ON p.operation_id = o.id
                 JOIN operation_budget_snapshot b ON b.operation_id = o.id
                 WHERE o.id = ?1 AND o.status = 'committed'",
                params![operation_id],
                |row| row.get(0),
            )
            .map_err(db_err)?;
        if ready != 1
            || generation.owner
                != crate::runtime_registry::RuntimeOwnerRef::Operation(operation_id.to_string())
        {
            return Err("OperationAttempt 创建前缺少原子规格/策略/预算或 owner 不匹配".to_string());
        }
        let generation_ready: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM runtime_generation
                 WHERE id = ?1 AND owner_kind = 'operation' AND owner_id = ?2 AND status = 'active'",
                params![generation.id, operation_id],
                |row| row.get(0),
            )
            .map_err(db_err)?;
        if generation_ready != 1 {
            return Err("OperationAttempt 的 RuntimeGeneration 不可用".to_string());
        }
        let attempt_no: u64 = tx
            .query_row(
                "SELECT COALESCE(MAX(attempt_no), 0) + 1 FROM operation_attempt WHERE operation_id = ?1",
                params![operation_id],
                |row| row.get(0),
            )
            .map_err(db_err)?;
        tx.execute(
            "INSERT INTO operation_attempt
             (operation_id, attempt_no, owner_kind, owner_id, generation_id,
              runtime_compatibility_key, delivery_state, created_at)
             VALUES (?1, ?2, 'operation', ?1, ?3, ?4, 'prepared', ?5)",
            params![
                operation_id,
                attempt_no,
                generation.id,
                generation.compatibility_key,
                now_millis(),
            ],
        )
        .map_err(db_err)?;
        tx.execute(
            "UPDATE background_operation SET status = 'running', started_at = ?1
             WHERE id = ?2 AND status = 'committed'",
            params![now_millis(), operation_id],
        )
        .map_err(db_err)?;
        tx.commit().map_err(db_err)?;
        Ok(attempt_no)
    }

    pub fn mark_operation_attempt_accepted(
        &self,
        operation_id: &str,
        attempt_no: u64,
    ) -> Result<(), String> {
        let _guard = self.write_guard()?;
        let conn = self.open()?;
        let changed = conn
            .execute(
                "UPDATE operation_attempt SET delivery_state = 'accepted', accepted_at = ?1
                 WHERE operation_id = ?2 AND attempt_no = ?3 AND delivery_state = 'prepared'",
                params![now_millis(), operation_id, attempt_no],
            )
            .map_err(db_err)?;
        if changed != 1 {
            return Err("OperationAttempt 接受回执状态转换无效".to_string());
        }
        Ok(())
    }

    pub fn finish_background_operation(
        &self,
        operation_id: &str,
        attempt_no: u64,
        status: &str,
        result: Option<&serde_json::Value>,
        error_code: Option<&str>,
    ) -> Result<(), String> {
        let (operation_status, delivery_state) = match status {
            "succeeded" => ("succeeded", "completed"),
            "cancelled" => ("cancelled", "interrupted"),
            "delivery_unknown" => ("delivery_unknown", "delivery_unknown"),
            _ => ("failed", "error"),
        };
        let _guard = self.write_guard()?;
        let mut conn = self.open()?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_err)?;
        let result_json = result
            .map(serde_json::to_string)
            .transpose()
            .map_err(|error| error.to_string())?;
        let changed = tx
            .execute(
                "UPDATE operation_attempt
                 SET delivery_state = ?1, terminal_receipt = ?2, ended_at = ?3
                 WHERE operation_id = ?4 AND attempt_no = ?5
                   AND delivery_state IN ('prepared', 'accepted')",
                params![
                    delivery_state,
                    error_code,
                    now_millis(),
                    operation_id,
                    attempt_no
                ],
            )
            .map_err(db_err)?;
        if changed != 1 {
            return Err("OperationAttempt 已终态或不存在".to_string());
        }
        tx.execute(
            "UPDATE background_operation
             SET status = ?1, result_json = ?2, error_code = ?3, ended_at = ?4
             WHERE id = ?5 AND status = 'running'",
            params![
                operation_status,
                result_json,
                error_code,
                now_millis(),
                operation_id
            ],
        )
        .map_err(db_err)?;
        tx.commit().map_err(db_err)
    }

    pub fn complete_model_only_operation(
        &self,
        operation_id: &str,
        attempt_no: u64,
        output: &ModelOnlyOperationOutput,
        result: &serde_json::Value,
    ) -> Result<(), String> {
        let _guard = self.write_guard()?;
        let mut conn = self.open()?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_err)?;
        let (provider_id, routed_model, routed_effort, pricing_basis_json, budget_json): (
            String,
            String,
            String,
            String,
            String,
        ) = tx
            .query_row(
                "SELECT s.provider_id, s.routed_model_id, s.routed_reasoning_effort,
                        s.pricing_basis_snapshot_json, b.snapshot_json
                 FROM operation_execution_spec s
                 JOIN operation_budget_snapshot b ON b.operation_id = s.operation_id
                 WHERE s.operation_id = ?1",
                params![operation_id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .map_err(db_err)?;
        let pricing_basis: crate::turn_start::PricingBasisSnapshot =
            serde_json::from_str(&pricing_basis_json)
                .map_err(|error| format!("Operation PricingBasisSnapshot 无效：{error}"))?;
        let budget: TurnBudgetSnapshot = serde_json::from_str(&budget_json)
            .map_err(|error| format!("Operation budget snapshot 无效：{error}"))?;
        budget.validate()?;
        let model = output.observed_model_id.as_deref().unwrap_or(&routed_model);
        let resolution = self.cost_with_fallback(
            &provider_id,
            model,
            output.input_tokens,
            Some(output.cached_input_tokens),
            Some(output.cache_write_input_tokens),
            output.output_tokens,
            output.reported_cost_usd.unwrap_or_default(),
            output.service_tier.as_deref(),
            pricing_basis.profile.as_ref(),
            true,
        );
        let changed = tx
            .execute(
                "UPDATE operation_attempt
                 SET delivery_state = 'completed', observed_model_id = ?1,
                     observed_reasoning_effort = ?2, terminal_receipt = 'success', ended_at = ?3
                 WHERE operation_id = ?4 AND attempt_no = ?5 AND delivery_state = 'accepted'",
                params![
                    output.observed_model_id,
                    routed_effort,
                    now_millis(),
                    operation_id,
                    attempt_no,
                ],
            )
            .map_err(db_err)?;
        if changed != 1 {
            return Err("OperationAttempt 成功收口状态转换无效".to_string());
        }
        tx.execute(
            "INSERT INTO usage
             (session_id, operation_id, model, provider_id, input_tokens, cached_input_tokens,
              cache_write_input_tokens, output_tokens, cost_usd, reported_cost_usd, cost_kind,
              price_source, service_tier, pricing_catalog_version, price_snapshot_json, ts,
              turn_id, effective_reasoning_effort, model_evidence)
             VALUES (NULL, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13,
                     ?14, ?15, NULL, ?16, ?17)",
            params![
                operation_id,
                model,
                provider_id,
                output.input_tokens,
                output.cached_input_tokens,
                output.cache_write_input_tokens,
                output.output_tokens,
                resolution.cost_usd,
                resolution.reported_cost_usd,
                resolution.cost_kind,
                resolution.price_source,
                output.service_tier.as_deref().unwrap_or("standard"),
                resolution.catalog_version,
                resolution.price_snapshot_json,
                now_seconds(),
                routed_effort,
                if output.observed_model_id.is_some() {
                    "runtime_observed"
                } else {
                    "launch_spec"
                },
            ],
        )
        .map_err(db_err)?;
        let mut next_seq: i64 = tx
            .query_row(
                "SELECT COALESCE(MAX(seq), 0) + 1 FROM operation_progress_fact
                 WHERE operation_id = ?1 AND attempt_no = ?2",
                params![operation_id, attempt_no],
                |row| row.get(0),
            )
            .map_err(db_err)?;
        let total_tokens = output.input_tokens.saturating_add(output.output_tokens);
        if budget
            .limit(crate::budget::BudgetDimension::Token)
            .is_some_and(|limit| total_tokens > limit.limit)
        {
            tx.execute(
                "INSERT INTO operation_progress_fact
                 (operation_id, attempt_no, seq, kind, value, observed_at)
                 VALUES (?1, ?2, ?3, 'budget_post_facto_token_exceeded', ?4, ?5)",
                params![
                    operation_id,
                    attempt_no,
                    next_seq,
                    total_tokens,
                    now_millis()
                ],
            )
            .map_err(db_err)?;
            next_seq += 1;
        }
        let cost_microusd = (resolution.cost_usd.max(0.0) * 1_000_000.0).round() as u64;
        if budget
            .limit(crate::budget::BudgetDimension::CostMicrousd)
            .is_some_and(|limit| cost_microusd > limit.limit)
        {
            tx.execute(
                "INSERT INTO operation_progress_fact
                 (operation_id, attempt_no, seq, kind, value, observed_at)
                 VALUES (?1, ?2, ?3, 'budget_post_facto_cost_exceeded', ?4, ?5)",
                params![
                    operation_id,
                    attempt_no,
                    next_seq,
                    cost_microusd,
                    now_millis()
                ],
            )
            .map_err(db_err)?;
        }
        tx.execute(
            "UPDATE background_operation
             SET status = 'succeeded', result_json = ?1, error_code = NULL, ended_at = ?2
             WHERE id = ?3 AND status = 'running'",
            params![
                serde_json::to_string(result).map_err(|error| error.to_string())?,
                now_millis(),
                operation_id,
            ],
        )
        .map_err(db_err)?;
        let operation_target: (String, Option<String>) = tx
            .query_row(
                "SELECT kind, source_session_id FROM background_operation WHERE id = ?1",
                params![operation_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(db_err)?;
        if operation_target.0 == "auto_title" {
            let session_id = operation_target
                .1
                .ok_or_else(|| "auto_title Operation 缺少 source Session".to_string())?;
            let title = result
                .get("title")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| "auto_title 结果缺少 title".to_string())?;
            let summary = result
                .get("summary")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| "auto_title 结果缺少 summary".to_string())?;
            let changed = tx
                .execute(
                    "UPDATE session SET title = ?1, summary = ?2, updated_at = ?3 WHERE id = ?4",
                    params![title, summary, now_seconds(), session_id],
                )
                .map_err(db_err)?;
            if changed != 1 {
                return Err("auto_title 结果应用目标 Session 不存在".to_string());
            }
        } else if operation_target.0 == "fork_job" {
            let frozen: crate::handoff::FrozenForkInput = serde_json::from_value(
                result
                    .get("frozenInput")
                    .cloned()
                    .ok_or_else(|| "ForkJob 结果缺少 frozenInput".to_string())?,
            )
            .map_err(|error| format!("ForkJob frozenInput 无效：{error}"))?;
            let handoff: crate::handoff::HandoffContent = serde_json::from_value(
                result
                    .get("handoff")
                    .cloned()
                    .ok_or_else(|| "ForkJob 结果缺少 handoff".to_string())?,
            )
            .map_err(|error| format!("ForkJob handoff 无效：{error}"))?;
            handoff.validate()?;
            let target_session_id = result
                .get("targetSessionId")
                .and_then(serde_json::Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| "ForkJob 结果缺少 targetSessionId".to_string())?;
            let source_session_id = tx
                .query_row(
                    "SELECT source_session_id FROM background_operation WHERE id = ?1",
                    params![operation_id],
                    |row| row.get::<_, Option<String>>(0),
                )
                .map_err(db_err)?;
            let folder_exists: i64 = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM session_folder WHERE id = ?1)",
                    params![frozen.source_folder_id],
                    |row| row.get(0),
                )
                .map_err(db_err)?;
            let folder_id = if folder_exists == 1 {
                frozen.source_folder_id.as_str()
            } else {
                "folder-default"
            };
            tx.execute(
                "INSERT INTO session
                 (id, cli_session_id, title, engine, model, cwd, status, created_at, updated_at,
                  folder_id, preferred_model, preferred_reasoning_effort)
                 VALUES (?1, NULL, ?2, ?3, ?4, ?5, 'idle', ?6, ?6, ?7, ?4, NULL)",
                params![
                    target_session_id,
                    format!("{}（交接）", frozen.source_title),
                    frozen.target_engine,
                    routed_model,
                    frozen.source_cwd,
                    now_seconds(),
                    folder_id,
                ],
            )
            .map_err(db_err)?;
            let handoff_id = format!("handoff-{:032x}", rand::random::<u128>());
            let fork_id = format!("fork-{:032x}", rand::random::<u128>());
            let content_json =
                serde_json::to_string(&handoff).map_err(|error| error.to_string())?;
            tx.execute(
                "INSERT INTO handoff
                 (id, operation_id, source_session_id, source_title_snapshot, source_engine,
                  source_cwd_snapshot, target_engine, boundary_turn_id, boundary_turn_epoch,
                  content_json, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![
                    handoff_id,
                    operation_id,
                    source_session_id,
                    frozen.source_title,
                    frozen.source_engine,
                    frozen.source_cwd,
                    frozen.target_engine,
                    frozen.boundary_turn_id,
                    i64::try_from(frozen.boundary_turn_epoch)
                        .map_err(|_| "边界 Turn epoch 溢出")?,
                    content_json,
                    now_millis(),
                ],
            )
            .map_err(db_err)?;
            tx.execute(
                "INSERT INTO session_fork
                 (id, operation_id, source_session_id, target_session_id, handoff_id,
                  target_engine, boundary_turn_id, boundary_turn_epoch, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    fork_id,
                    operation_id,
                    source_session_id,
                    target_session_id,
                    handoff_id,
                    frozen.target_engine,
                    frozen.boundary_turn_id,
                    i64::try_from(frozen.boundary_turn_epoch)
                        .map_err(|_| "边界 Turn epoch 溢出")?,
                    now_millis(),
                ],
            )
            .map_err(db_err)?;
            tx.execute(
                "INSERT INTO setting (key, value_json) VALUES ('active_session_id', ?1)
                 ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json",
                params![
                    serde_json::to_string(target_session_id).map_err(|error| error.to_string())?
                ],
            )
            .map_err(db_err)?;
        }
        tx.commit().map_err(db_err)
    }

    pub fn complete_model_only_operation_stage(
        &self,
        operation_id: &str,
        attempt_no: u64,
        output: &ModelOnlyOperationOutput,
        stage: &str,
    ) -> Result<(), String> {
        let _guard = self.write_guard()?;
        let mut conn = self.open()?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_err)?;
        let (provider_id, routed_model, routed_effort, pricing_basis_json): (
            String,
            String,
            String,
            String,
        ) = tx
            .query_row(
                "SELECT provider_id, routed_model_id, routed_reasoning_effort,
                        pricing_basis_snapshot_json
                 FROM operation_execution_spec WHERE operation_id = ?1",
                params![operation_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .map_err(db_err)?;
        let pricing_basis: crate::turn_start::PricingBasisSnapshot =
            serde_json::from_str(&pricing_basis_json)
                .map_err(|error| format!("Operation PricingBasisSnapshot 无效：{error}"))?;
        let model = output.observed_model_id.as_deref().unwrap_or(&routed_model);
        let resolution = self.cost_with_fallback(
            &provider_id,
            model,
            output.input_tokens,
            Some(output.cached_input_tokens),
            Some(output.cache_write_input_tokens),
            output.output_tokens,
            output.reported_cost_usd.unwrap_or_default(),
            output.service_tier.as_deref(),
            pricing_basis.profile.as_ref(),
            true,
        );
        let changed = tx
            .execute(
                "UPDATE operation_attempt
                 SET delivery_state = 'completed', observed_model_id = ?1,
                     observed_reasoning_effort = ?2, terminal_receipt = ?3, ended_at = ?4
                 WHERE operation_id = ?5 AND attempt_no = ?6 AND delivery_state = 'accepted'",
                params![
                    output.observed_model_id,
                    routed_effort,
                    stage,
                    now_millis(),
                    operation_id,
                    attempt_no,
                ],
            )
            .map_err(db_err)?;
        if changed != 1 {
            return Err("Operation 摘要 stage 收口状态转换无效".to_string());
        }
        tx.execute(
            "INSERT INTO usage
             (session_id, operation_id, model, provider_id, input_tokens, cached_input_tokens,
              cache_write_input_tokens, output_tokens, cost_usd, reported_cost_usd, cost_kind,
              price_source, service_tier, pricing_catalog_version, price_snapshot_json, ts,
              turn_id, effective_reasoning_effort, model_evidence)
             VALUES (NULL, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13,
                     ?14, ?15, NULL, ?16, ?17)",
            params![
                operation_id,
                model,
                provider_id,
                output.input_tokens,
                output.cached_input_tokens,
                output.cache_write_input_tokens,
                output.output_tokens,
                resolution.cost_usd,
                resolution.reported_cost_usd,
                resolution.cost_kind,
                resolution.price_source,
                output.service_tier.as_deref().unwrap_or("standard"),
                resolution.catalog_version,
                resolution.price_snapshot_json,
                now_seconds(),
                routed_effort,
                if output.observed_model_id.is_some() {
                    "runtime_observed"
                } else {
                    "launch_spec"
                },
            ],
        )
        .map_err(db_err)?;
        let next_seq: i64 = tx
            .query_row(
                "SELECT COALESCE(MAX(seq), 0) + 1 FROM operation_progress_fact
                 WHERE operation_id = ?1 AND attempt_no = ?2",
                params![operation_id, attempt_no],
                |row| row.get(0),
            )
            .map_err(db_err)?;
        tx.execute(
            "INSERT INTO operation_progress_fact
             (operation_id, attempt_no, seq, kind, detail_json, observed_at)
             VALUES (?1, ?2, ?3, 'recursive_summary_stage', ?4, ?5)",
            params![operation_id, attempt_no, next_seq, stage, now_millis()],
        )
        .map_err(db_err)?;
        let changed = tx
            .execute(
                "UPDATE background_operation SET status = 'committed', started_at = NULL
                 WHERE id = ?1 AND status = 'running'",
                params![operation_id],
            )
            .map_err(db_err)?;
        if changed != 1 {
            return Err("ForkJob stage 完成后无法恢复 committed 状态".to_string());
        }
        tx.commit().map_err(db_err)
    }

    pub fn request_background_operation_cancel(&self, operation_id: &str) -> Result<bool, String> {
        let _guard = self.write_guard()?;
        let mut conn = self.open()?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_err)?;
        let now = now_millis();
        let committed = tx
            .execute(
                "UPDATE background_operation
                 SET status = 'cancelled', cancel_requested_at = ?1, ended_at = ?1,
                     error_code = '[operation_cancelled_before_dispatch]'
                 WHERE id = ?2 AND status = 'committed'",
                params![now, operation_id],
            )
            .map_err(db_err)?;
        let running = tx
            .execute(
                "UPDATE background_operation
                 SET cancel_requested_at = COALESCE(cancel_requested_at, ?1)
                 WHERE id = ?2 AND status = 'running'",
                params![now, operation_id],
            )
            .map_err(db_err)?;
        tx.commit().map_err(db_err)?;
        Ok(committed == 1 || running == 1)
    }

    pub fn prepare_background_operation_retry(&self, operation_id: &str) -> Result<(), String> {
        let _guard = self.write_guard()?;
        let conn = self.open()?;
        let changed = conn
            .execute(
                "UPDATE background_operation
                 SET status = 'committed', result_json = NULL, error_code = NULL,
                     started_at = NULL, cancel_requested_at = NULL, ended_at = NULL
                 WHERE id = ?1 AND status IN ('failed', 'cancelled', 'delivery_unknown')",
                params![operation_id],
            )
            .map_err(db_err)?;
        if changed != 1 {
            return Err("BackgroundOperation 当前状态不允许手工重试".to_string());
        }
        Ok(())
    }

    pub fn fail_committed_background_operation(
        &self,
        operation_id: &str,
        error_code: &str,
    ) -> Result<(), String> {
        let _guard = self.write_guard()?;
        let conn = self.open()?;
        let changed = conn
            .execute(
                "UPDATE background_operation
                 SET status = 'failed', error_code = ?1, ended_at = ?2
                 WHERE id = ?3 AND status = 'committed'",
                params![error_code, now_millis(), operation_id],
            )
            .map_err(db_err)?;
        if changed != 1 {
            return Err("BackgroundOperation 启动前失败收口状态转换无效".to_string());
        }
        Ok(())
    }

    pub fn reconcile_background_operations(&self) -> Result<u64, String> {
        let _guard = self.write_guard()?;
        let mut conn = self.open()?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_err)?;
        let changed = tx
            .execute(
                "UPDATE operation_attempt
                 SET delivery_state = 'delivery_unknown',
                     terminal_receipt = '[operation_recovered_delivery_unknown]', ended_at = ?1
                 WHERE delivery_state IN ('prepared', 'accepted')",
                params![now_millis()],
            )
            .map_err(db_err)?;
        tx.execute(
            "UPDATE background_operation
             SET status = 'delivery_unknown', error_code = '[operation_recovered_delivery_unknown]',
                 ended_at = ?1
             WHERE status = 'running'",
            params![now_millis()],
        )
        .map_err(db_err)?;
        tx.commit().map_err(db_err)?;
        Ok(changed as u64)
    }

    pub fn rollback_prepared_user_turn(&self, prepared: PreparedUserTurn) -> Result<(), String> {
        retry_locked(|| {
            let _guard = self.write_guard()?;
            let mut conn = self.open()?;
            let tx = conn.transaction().map_err(db_err)?;
            if let Some(turn_id) = &prepared.turn_id {
                tx.execute(
                    "DELETE FROM turn_execution_spec WHERE turn_id = ?1",
                    params![turn_id],
                )
                .map_err(db_err)?;
                tx.execute(
                    "DELETE FROM turn WHERE history_session_id = ?1 AND turn_id = ?2 AND status = 'committed'",
                    params![prepared.session_id, turn_id],
                )
                .map_err(db_err)?;
            }
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
        let event = crate::redaction::sanitize_agent_event(event);
        match &event {
            AgentEvent::SessionStarted {
                session_id,
                engine,
                model,
                cwd,
                ts,
                capabilities,
            } => {
                self.attach_cli_session(session_id, *engine, model, cwd, *ts)?;
                self.persist_runtime_capabilities(session_id, capabilities.as_ref())
            }
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
                    insert_tool_call(conn, local_id, id, name, input, *status, ts, None)?;
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
                ..
            } => {
                let ended_at = now_millis();
                let ts = ended_at / 1000;
                let diff_json = diff
                    .as_ref()
                    .map(serde_json::to_string)
                    .transpose()
                    .map_err(|e| e.to_string())?;
                self.with_session(session_id, |conn, local_id| {
                    let updated = conn.execute(
                        "UPDATE tool_call SET status = ?1, output = ?2, diff_json = ?3, ended_at = ?4 WHERE id = ?5 AND session_id = ?6",
                        params![history_tool_status(*status), output, diff_json, ended_at, id, local_id],
                    )?;
                    if updated != 1 {
                        return Err(rusqlite::Error::QueryReturnedNoRows);
                    }
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
                cached_input_tokens,
                cache_write_input_tokens,
                output_tokens,
                cost_usd,
                service_tier,
                context_window,
                ..
            } => {
                let ts = now_seconds();
                self.with_session(session_id, |conn, local_id| {
                    let (model, provider_id): (String, String) = conn.query_row(
                        "SELECT model, provider_id FROM session WHERE id = ?1",
                        params![local_id],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )?;
                    let resolution = self.cost_with_fallback(
                        &provider_id,
                        &model,
                        *input_tokens,
                        *cached_input_tokens,
                        *cache_write_input_tokens,
                        *output_tokens,
                        *cost_usd,
                        service_tier.as_deref(),
                        None,
                        false,
                    );
                    conn.execute(
                        "INSERT INTO usage
                         (session_id, model, provider_id, input_tokens, cached_input_tokens,
                          cache_write_input_tokens, output_tokens, cost_usd, reported_cost_usd,
                          cost_kind, price_source, service_tier, pricing_catalog_version,
                           price_snapshot_json, ts, turn_id)
                          VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, NULL)",
                        params![
                            local_id,
                            model,
                            provider_id,
                            input_tokens,
                            cached_input_tokens.unwrap_or_default(),
                            cache_write_input_tokens.unwrap_or_default(),
                            output_tokens,
                            resolution.cost_usd,
                            resolution.reported_cost_usd,
                            resolution.cost_kind,
                            resolution.price_source,
                            service_tier.as_deref().unwrap_or("standard"),
                            resolution.catalog_version,
                            resolution.price_snapshot_json,
                            ts
                        ],
                    )?;
                    conn.execute(
                        "UPDATE session SET last_context_window = COALESCE(?1, last_context_window), updated_at = ?2 WHERE id = ?3",
                        params![context_window, ts, local_id],
                    )?;
                    Ok(())
                })
            }
            AgentEvent::ContextUsage {
                session_id,
                context_tokens,
                context_window,
            } => {
                let ts = now_seconds();
                self.with_session(session_id, |conn, local_id| {
                    conn.execute(
                        "UPDATE session SET last_context_tokens = ?1, last_context_window = COALESCE(?2, last_context_window), updated_at = ?3 WHERE id = ?4",
                        params![context_tokens, context_window, ts, local_id],
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
                let artifact_reason = terminal_artifact_reason(*stop_reason);
                self.with_session(session_id, |conn, local_id| {
                    finalize_terminal_artifacts(conn, local_id, status, artifact_reason, ts)
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
                        finalize_terminal_artifacts(
                            conn,
                            local_id,
                            "idle",
                            "[turn_failed] 轮次发生不可恢复错误，未完成的工具调用已终止",
                            ts,
                        )
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
                persistent_label,
                matcher_summary,
                ..
            } => {
                let ts = now_millis();
                self.with_session(session_id, |conn, local_id| {
                    upsert_approval_request(
                        conn,
                        local_id,
                        id,
                        action,
                        detail,
                        persistent_label.as_deref(),
                        matcher_summary.as_deref(),
                        None,
                        ts,
                    )
                })
            }
            AgentEvent::Checkpoint {
                id,
                label,
                ts,
                session_id,
                restorable,
                file_count,
                reason,
            } => self.with_session(session_id, |conn, local_id| {
                conn.execute(
                    "INSERT OR IGNORE INTO checkpoint
                     (id, session_id, turn_idx, label, snapshot_ref, ts, turn_id,
                      restorable, file_count, restorable_reason)
                     VALUES (?1, ?2, 0, ?3, '', ?4, NULL, ?5, ?6, ?7)",
                    params![
                        id,
                        local_id,
                        label,
                        ts,
                        i64::from(*restorable),
                        *file_count as i64,
                        reason,
                    ],
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
        self.record_event_for_session_internal(history_session_id, None, true, event)
    }

    pub fn record_event_for_session_in_turn(
        &self,
        history_session_id: &str,
        turn_id: Option<&str>,
        event: &AgentEvent,
    ) -> Result<(), String> {
        self.record_event_for_session_internal(history_session_id, turn_id, false, event)
    }

    fn record_event_for_session_internal(
        &self,
        history_session_id: &str,
        turn_id: Option<&str>,
        legacy_compat: bool,
        event: &AgentEvent,
    ) -> Result<(), String> {
        let event = crate::redaction::sanitize_agent_event(event);
        if let Some(turn_id) = turn_id {
            let conn = self.open()?;
            let local_id = self.resolve_local_id(&conn, history_session_id)?;
            let status = conn
                .query_row(
                    "SELECT status FROM turn WHERE history_session_id = ?1 AND turn_id = ?2",
                    params![local_id, turn_id],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(db_err)?
                .ok_or_else(|| "事件引用了不存在的 turn_id".to_string())?;
            if matches!(status.as_str(), "succeeded" | "failed" | "interrupted") {
                return Ok(());
            }
        }
        match &event {
            AgentEvent::SessionStarted {
                session_id,
                engine,
                model,
                cwd,
                ts,
                capabilities,
            } => {
                if let Some(turn_id) = turn_id {
                    self.observe_latest_turn_attempt(
                        turn_id,
                        session_id,
                        *engine,
                        model,
                        capabilities.as_ref(),
                        *ts,
                    )?;
                }
                self.attach_cli_session_to(
                    history_session_id,
                    session_id,
                    *engine,
                    model,
                    cwd,
                    *ts,
                )?;
                self.persist_runtime_capabilities(history_session_id, capabilities.as_ref())?;
                Ok(())
            }
            AgentEvent::MessageComplete { role, text, .. } => {
                if turn_id.is_none() && !legacy_compat {
                    return Err("新消息缺少 turn_id".to_string());
                }
                // message.ts 用毫秒（与 checkpoint.ts 同单位，回溯截断依赖比较，变更-07）；
                // session.updated_at 维持秒
                let ts = now_millis();
                let updated_at = ts / 1000;
                self.with_local_session(history_session_id, |conn, local_id| {
                    conn.execute(
                        "INSERT INTO message (session_id, role, text, ts, turn_id)
                         VALUES (?1, ?2, ?3, ?4, ?5)",
                        params![local_id, role_to_str(*role), text, ts, turn_id],
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
                if turn_id.is_none() && !legacy_compat {
                    return Err("新工具调用缺少 turn_id".to_string());
                }
                let ts = now_millis();
                let updated_at = ts / 1000;
                self.with_local_session(history_session_id, |conn, local_id| {
                    if let Some(turn_id) = turn_id {
                        reconcile_tool_call(conn, local_id, turn_id, id, name, input, *status, ts)?;
                    } else {
                        insert_tool_call(conn, local_id, id, name, input, *status, ts, None)?;
                    }
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
                outcome,
                started,
                has_output,
                retryable,
                denial_source,
                native_denial_code,
                ..
            } => {
                if turn_id.is_none() && !legacy_compat {
                    return Err("新工具结果缺少 turn_id".to_string());
                }
                let ended_at = now_millis();
                let ts = ended_at / 1000;
                let diff_json = diff
                    .as_ref()
                    .map(serde_json::to_string)
                    .transpose()
                    .map_err(|e| e.to_string())?;
                self.with_local_session(history_session_id, |conn, local_id| {
                    if let Some(turn_id) = turn_id {
                        reconcile_tool_result(
                            conn,
                            local_id,
                            turn_id,
                            id,
                            *status,
                            output.as_deref(),
                            diff_json.as_deref(),
                            ended_at,
                            outcome.as_ref().map(tool_outcome_to_str),
                            *started,
                            *has_output,
                            *retryable,
                            denial_source.as_ref().map(tool_denial_source_to_str),
                            native_denial_code.as_deref(),
                        )?;
                    } else {
                        let bounded_output = output.as_deref().map(bounded_ledger_text);
                        let updated = conn.execute(
                            "UPDATE tool_call
                             SET status = ?1, output = ?2, diff_json = ?3, ended_at = ?4,
                                 outcome = ?5, tool_started = ?6, has_output = ?7,
                                 retryable = ?8, denial_source = ?9, native_denial_code = ?10
                             WHERE id = ?11 AND session_id = ?12 AND turn_id IS NULL",
                            params![
                                history_tool_status(*status),
                                bounded_output,
                                diff_json,
                                ended_at,
                                outcome.as_ref().map(tool_outcome_to_str),
                                started.map(i64::from),
                                has_output.map(i64::from),
                                retryable.map(i64::from),
                                denial_source.as_ref().map(tool_denial_source_to_str),
                                native_denial_code,
                                id,
                                local_id,
                            ],
                        )?;
                        if updated != 1 {
                            return Err(rusqlite::Error::QueryReturnedNoRows);
                        }
                    }
                    conn.execute(
                        "UPDATE session SET updated_at = ?1 WHERE id = ?2",
                        params![ts, local_id],
                    )?;
                    Ok(())
                })
            }
            AgentEvent::TokenUsage {
                input_tokens,
                cached_input_tokens,
                cache_write_input_tokens,
                output_tokens,
                cost_usd,
                service_tier,
                context_window,
                ..
            } => {
                if turn_id.is_none() && !legacy_compat {
                    return Err("新 Usage 缺少 turn_id".to_string());
                }
                let ts = now_seconds();
                self.with_local_session(history_session_id, |conn, local_id| {
                    let frozen_route = turn_id
                        .map(|turn_id| {
                            conn.query_row(
                                "SELECT s.routed_model_id, s.provider_id,
                                        s.pricing_basis_snapshot_json,
                                        (SELECT a.observed_model_id FROM turn_attempt a
                                         WHERE a.turn_id = s.turn_id
                                         ORDER BY a.attempt_no DESC LIMIT 1),
                                        (SELECT a.observed_reasoning_effort FROM turn_attempt a
                                         WHERE a.turn_id = s.turn_id
                                         ORDER BY a.attempt_no DESC LIMIT 1),
                                        s.routed_reasoning_effort
                                 FROM turn_execution_spec s
                                 WHERE s.turn_id = ?1 AND s.history_session_id = ?2",
                                params![turn_id, local_id],
                                |row| {
                                    Ok((
                                        row.get::<_, String>(0)?,
                                        row.get::<_, String>(1)?,
                                        row.get::<_, String>(2)?,
                                        row.get::<_, Option<String>>(3)?,
                                        row.get::<_, Option<String>>(4)?,
                                        row.get::<_, String>(5)?,
                                    ))
                                },
                            )
                            .optional()
                        })
                        .transpose()?
                        .flatten();
                    let (
                        model,
                        provider_id,
                        pricing_basis,
                        pricing_basis_frozen,
                        effective_effort,
                        model_evidence,
                    ) = if let Some((
                        routed_model,
                        provider_id,
                        pricing_json,
                        observed_model,
                        observed_effort,
                        routed_effort,
                    )) = frozen_route
                    {
                        let pricing_basis = serde_json::from_str::<
                            crate::turn_start::PricingBasisSnapshot,
                        >(&pricing_json)
                        .ok()
                        .and_then(|snapshot| snapshot.profile);
                        let model_evidence = if observed_model.is_some() {
                            "runtime_observed"
                        } else {
                            "launch_spec"
                        };
                        (
                            observed_model.unwrap_or(routed_model),
                            provider_id,
                            pricing_basis,
                            true,
                            observed_effort.unwrap_or(routed_effort),
                            model_evidence,
                        )
                    } else {
                        let (model, provider_id): (String, String) = conn.query_row(
                            "SELECT model, provider_id FROM session WHERE id = ?1",
                            params![local_id],
                            |row| Ok((row.get(0)?, row.get(1)?)),
                        )?;
                        (
                            model,
                            provider_id,
                            None,
                            false,
                            String::new(),
                            "legacy_unbound",
                        )
                    };
                    let resolution = self.cost_with_fallback(
                        &provider_id,
                        &model,
                        *input_tokens,
                        *cached_input_tokens,
                        *cache_write_input_tokens,
                        *output_tokens,
                        *cost_usd,
                        service_tier.as_deref(),
                        pricing_basis.as_ref(),
                        pricing_basis_frozen,
                    );
                    conn.execute(
                        "INSERT INTO usage
                         (session_id, model, provider_id, input_tokens, cached_input_tokens,
                          cache_write_input_tokens, output_tokens, cost_usd, reported_cost_usd,
                          cost_kind, price_source, service_tier, pricing_catalog_version,
                           price_snapshot_json, ts, turn_id, effective_reasoning_effort, model_evidence)
                          VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16,
                                  NULLIF(?17, ''), ?18)",
                        params![
                            local_id,
                            model,
                            provider_id,
                            input_tokens,
                            cached_input_tokens.unwrap_or_default(),
                            cache_write_input_tokens.unwrap_or_default(),
                            output_tokens,
                            resolution.cost_usd,
                            resolution.reported_cost_usd,
                            resolution.cost_kind,
                            resolution.price_source,
                            service_tier.as_deref().unwrap_or("standard"),
                            resolution.catalog_version,
                            resolution.price_snapshot_json,
                            ts,
                            turn_id,
                            effective_effort,
                            model_evidence,
                        ],
                    )?;
                    conn.execute(
                        "UPDATE session SET last_context_window = COALESCE(?1, last_context_window), updated_at = ?2 WHERE id = ?3",
                        params![context_window, ts, local_id],
                    )?;
                    Ok(())
                })
            }
            AgentEvent::ContextUsage {
                context_tokens,
                context_window,
                ..
            } => {
                if turn_id.is_none() && !legacy_compat {
                    return Err("新 ContextUsage 缺少 turn_id".to_string());
                }
                let ts = now_seconds();
                self.with_local_session(history_session_id, |conn, local_id| {
                    conn.execute(
                        "UPDATE session SET last_context_tokens = ?1, last_context_window = COALESCE(?2, last_context_window), updated_at = ?3 WHERE id = ?4",
                        params![context_tokens, context_window, ts, local_id],
                    )?;
                    Ok(())
                })
            }
            AgentEvent::TurnComplete { stop_reason, .. } => {
                if !legacy_compat {
                    if let Some(turn_id) = turn_id {
                        return crate::runtime_registry::finish_attempt_from_event(
                            self, turn_id, &event,
                        );
                    }
                    return Ok(());
                }
                let status = match stop_reason {
                    StopReason::End => "done",
                    StopReason::Interrupted | StopReason::Error => "idle",
                };
                let ts = now_seconds();
                let artifact_reason = terminal_artifact_reason(*stop_reason);
                self.with_local_session(history_session_id, |conn, local_id| {
                    finalize_terminal_artifacts(conn, local_id, status, artifact_reason, ts)
                })
            }
            AgentEvent::Error { recoverable, .. } => {
                // 可恢复警告不改会话状态——轮次实际还在跑（变更-12）
                if *recoverable {
                    return Ok(());
                }
                if !legacy_compat {
                    if let Some(turn_id) = turn_id {
                        return crate::runtime_registry::finish_attempt_from_event(
                            self, turn_id, &event,
                        );
                    }
                    return Ok(());
                }
                let ts = now_seconds();
                self.with_local_session(history_session_id, |conn, local_id| {
                    finalize_terminal_artifacts(
                        conn,
                        local_id,
                        "idle",
                        "[turn_failed] 轮次发生不可恢复错误，未完成的工具调用已终止",
                        ts,
                    )
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
                persistent_label,
                matcher_summary,
                ..
            } => {
                if turn_id.is_none() && !legacy_compat {
                    return Err("新审批缺少 turn_id".to_string());
                }
                let ts = now_millis();
                self.with_local_session(history_session_id, |conn, local_id| {
                    upsert_approval_request(
                        conn,
                        local_id,
                        id,
                        action,
                        detail,
                        persistent_label.as_deref(),
                        matcher_summary.as_deref(),
                        turn_id,
                        ts,
                    )
                })
            }
            AgentEvent::Checkpoint {
                id,
                label,
                ts,
                restorable,
                file_count,
                reason,
                ..
            } => {
                if turn_id.is_none() && !legacy_compat {
                    return Err("新检查点缺少 turn_id".to_string());
                }
                self.with_local_session(history_session_id, |conn, local_id| {
                    conn.execute(
                        "INSERT OR IGNORE INTO checkpoint
                     (id, session_id, turn_idx, label, snapshot_ref, ts, turn_id,
                      restorable, file_count, restorable_reason)
                     VALUES (?1, ?2, 0, ?3, '', ?4, ?5, ?6, ?7, ?8)",
                        params![
                            id,
                            local_id,
                            label,
                            ts,
                            turn_id,
                            i64::from(*restorable),
                            *file_count as i64,
                            reason,
                        ],
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

    pub fn set_safe_permission_profile(
        &self,
        session_id: &str,
        profile: &str,
    ) -> Result<(), String> {
        if !matches!(profile, "standard" | "auto") {
            return Err("safe permission profile must be standard or auto".to_string());
        }
        self.with_local_session(session_id, |conn, local_id| {
            conn.execute(
                "UPDATE session SET safe_permission_profile = ?1 WHERE id = ?2",
                params![profile, local_id],
            )?;
            Ok(())
        })
    }

    pub(crate) fn persist_runtime_capabilities(
        &self,
        session_id: &str,
        capabilities: Option<&RuntimeCapabilitySnapshot>,
    ) -> Result<(), String> {
        let json = capabilities
            .map(serde_json::to_string)
            .transpose()
            .map_err(|error| format!("序列化 Runtime 能力快照失败：{error}"))?;
        self.with_local_session(session_id, |conn, local_id| {
            conn.execute(
                "UPDATE session SET runtime_capabilities_json = ?1 WHERE id = ?2",
                params![json, local_id],
            )?;
            Ok(())
        })
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

    pub fn upsert_turn_snapshot(&self, snapshot: TurnSnapshotRecord) -> Result<(), String> {
        retry_locked(|| {
            let _guard = self.write_guard()?;
            let mut conn = self.open()?;
            let tx = conn.transaction().map_err(db_err)?;
            let existing_snapshot = tx
                .query_row(
                    "SELECT turn_epoch, status
                     FROM turn_snapshot WHERE history_session_id = ?1",
                    params![&snapshot.history_session_id],
                    |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
                )
                .optional()
                .map_err(db_err)?;
            if let Some((existing_epoch, existing_status)) = existing_snapshot {
                let incoming_epoch = i64::try_from(snapshot.turn_epoch).unwrap_or(i64::MAX);
                if matches!(
                    existing_status.as_str(),
                    "succeeded" | "failed" | "interrupted"
                ) && incoming_epoch <= existing_epoch
                {
                    return Ok(());
                }
            }
            tx.execute(
                "INSERT INTO turn_snapshot
                 (history_session_id, turn_id, turn_epoch, status, terminal_reason,
                  recoverable, event_seq, updated_at, turn_mode, permission_profile, started_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                 ON CONFLICT(history_session_id) DO UPDATE SET
                   turn_id = excluded.turn_id,
                   turn_epoch = excluded.turn_epoch,
                   status = excluded.status,
                   terminal_reason = excluded.terminal_reason,
                   recoverable = excluded.recoverable,
                   event_seq = excluded.event_seq,
                   updated_at = excluded.updated_at,
                   turn_mode = excluded.turn_mode,
                   permission_profile = excluded.permission_profile,
                   started_at = excluded.started_at
                 WHERE turn_snapshot.status NOT IN ('succeeded', 'failed', 'interrupted')
                    OR excluded.turn_epoch > turn_snapshot.turn_epoch",
                params![
                    &snapshot.history_session_id,
                    &snapshot.turn_id,
                    i64::try_from(snapshot.turn_epoch).unwrap_or(i64::MAX),
                    turn_status_to_str(snapshot.status),
                    &snapshot.terminal_reason,
                    i64::from(snapshot.recoverable),
                    i64::try_from(snapshot.event_seq).unwrap_or(i64::MAX),
                    snapshot.updated_at,
                    &snapshot.mode,
                    &snapshot.permission_profile,
                    snapshot.started_at,
                ],
            )
            .map_err(db_err)?;
            tx.execute(
                "INSERT INTO turn
                 (history_session_id, turn_id, turn_epoch, turn_mode, permission_profile,
                  status, started_at, ended_at, terminal_reason)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                 ON CONFLICT(history_session_id, turn_id) DO UPDATE SET
                   turn_epoch = excluded.turn_epoch,
                   turn_mode = excluded.turn_mode,
                   permission_profile = excluded.permission_profile,
                   status = excluded.status,
                   started_at = excluded.started_at,
                   ended_at = excluded.ended_at,
                   terminal_reason = excluded.terminal_reason
                 WHERE turn.status NOT IN ('succeeded', 'failed', 'interrupted')",
                params![
                    &snapshot.history_session_id,
                    &snapshot.turn_id,
                    i64::try_from(snapshot.turn_epoch).unwrap_or(i64::MAX),
                    &snapshot.mode,
                    &snapshot.permission_profile,
                    turn_status_to_str(snapshot.status),
                    snapshot.started_at,
                    snapshot.status.is_terminal().then_some(snapshot.updated_at),
                    &snapshot.terminal_reason,
                ],
            )
            .map_err(db_err)?;
            if snapshot.status.is_terminal() {
                finalize_turn_artifacts(
                    &tx,
                    &snapshot.history_session_id,
                    &snapshot.turn_id,
                    snapshot.status,
                    snapshot.updated_at,
                )
                .map_err(db_err)?;
            }
            tx.commit().map_err(db_err)?;
            Ok(())
        })
    }

    pub fn begin_supervised_attempt(
        &self,
        snapshot: TurnSnapshotRecord,
        attempt_no: u64,
        runtime_generation_id: &str,
    ) -> Result<(), String> {
        retry_locked(|| {
            let _guard = self.write_guard()?;
            let mut conn = self.open()?;
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(db_err)?;
            tx.execute(
                "INSERT INTO turn_snapshot
                 (history_session_id, turn_id, turn_epoch, status, terminal_reason,
                  recoverable, event_seq, updated_at, turn_mode, permission_profile, started_at,
                  attempt_no, runtime_generation_id, recovery_state)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, 'none')
                 ON CONFLICT(history_session_id) DO UPDATE SET
                   turn_id = excluded.turn_id,
                   turn_epoch = excluded.turn_epoch,
                   status = excluded.status,
                   terminal_reason = excluded.terminal_reason,
                   recoverable = excluded.recoverable,
                   event_seq = excluded.event_seq,
                   updated_at = excluded.updated_at,
                   turn_mode = excluded.turn_mode,
                   permission_profile = excluded.permission_profile,
                   started_at = excluded.started_at,
                   attempt_no = excluded.attempt_no,
                   runtime_generation_id = excluded.runtime_generation_id,
                   recovery_state = excluded.recovery_state
                 WHERE turn_snapshot.status NOT IN ('succeeded', 'failed', 'interrupted')
                    OR excluded.turn_epoch > turn_snapshot.turn_epoch",
                params![
                    &snapshot.history_session_id,
                    &snapshot.turn_id,
                    i64::try_from(snapshot.turn_epoch).unwrap_or(i64::MAX),
                    turn_status_to_str(snapshot.status),
                    &snapshot.terminal_reason,
                    i64::from(snapshot.recoverable),
                    i64::try_from(snapshot.event_seq).unwrap_or(i64::MAX),
                    snapshot.updated_at,
                    &snapshot.mode,
                    &snapshot.permission_profile,
                    snapshot.started_at,
                    i64::try_from(attempt_no).unwrap_or(i64::MAX),
                    runtime_generation_id,
                ],
            )
            .map_err(db_err)?;
            tx.execute(
                "INSERT INTO turn
                 (history_session_id, turn_id, turn_epoch, turn_mode, permission_profile,
                  status, started_at, ended_at, terminal_reason)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL, NULL)
                 ON CONFLICT(history_session_id, turn_id) DO UPDATE SET
                   turn_epoch = excluded.turn_epoch,
                   turn_mode = excluded.turn_mode,
                   permission_profile = excluded.permission_profile,
                   status = excluded.status,
                   started_at = excluded.started_at,
                   ended_at = NULL,
                   terminal_reason = NULL
                 WHERE turn.status NOT IN ('succeeded', 'failed', 'interrupted')",
                params![
                    &snapshot.history_session_id,
                    &snapshot.turn_id,
                    i64::try_from(snapshot.turn_epoch).unwrap_or(i64::MAX),
                    &snapshot.mode,
                    &snapshot.permission_profile,
                    turn_status_to_str(snapshot.status),
                    snapshot.started_at,
                ],
            )
            .map_err(db_err)?;
            tx.execute(
                "INSERT OR IGNORE INTO stream_boundary_event
                 (turn_id, attempt_no, event_seq, history_session_id, runtime_generation_id,
                  event_kind, disposition, event_digest, observed_at)
                 VALUES (?1, ?2, 0, ?3, ?4, 'attempt_started', 'accepted',
                         'attempt_started', ?5)",
                params![
                    &snapshot.turn_id,
                    i64::try_from(attempt_no).unwrap_or(i64::MAX),
                    &snapshot.history_session_id,
                    runtime_generation_id,
                    snapshot.updated_at,
                ],
            )
            .map_err(db_err)?;
            tx.commit().map_err(db_err)
        })
    }

    pub fn load_turn_snapshot(
        &self,
        history_session_id: &str,
    ) -> Result<Option<TurnSnapshotRecord>, String> {
        let conn = self.open()?;
        conn.query_row(
            "SELECT history_session_id, turn_id, turn_epoch, status, terminal_reason,
                    recoverable, event_seq, updated_at, turn_mode, permission_profile, started_at
             FROM turn_snapshot WHERE history_session_id = ?1",
            params![history_session_id],
            |row| {
                let epoch: i64 = row.get(2)?;
                let status: String = row.get(3)?;
                let event_seq: i64 = row.get(6)?;
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    epoch,
                    status,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, i64>(5)?,
                    event_seq,
                    row.get::<_, i64>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, String>(9)?,
                    row.get::<_, i64>(10)?,
                ))
            },
        )
        .optional()
        .map_err(db_err)?
        .map(
            |(
                history_session_id,
                turn_id,
                epoch,
                status,
                terminal_reason,
                recoverable,
                event_seq,
                updated_at,
                mode,
                permission_profile,
                started_at,
            )| {
                Ok(TurnSnapshotRecord {
                    history_session_id,
                    turn_id,
                    turn_epoch: u64::try_from(epoch)
                        .map_err(|_| "invalid negative turn epoch".to_string())?,
                    status: parse_turn_status(&status)?,
                    terminal_reason,
                    recoverable: recoverable != 0,
                    event_seq: u64::try_from(event_seq)
                        .map_err(|_| "invalid negative event sequence".to_string())?,
                    updated_at,
                    mode,
                    permission_profile,
                    started_at,
                })
            },
        )
        .transpose()
    }

    #[allow(clippy::too_many_arguments)]
    pub fn record_stream_boundary(
        &self,
        history_session_id: &str,
        turn_id: &str,
        attempt_no: u64,
        runtime_generation_id: &str,
        event_seq: u64,
        event_kind: &str,
        disposition: &str,
        event_digest: &str,
        observed_at: i64,
    ) -> Result<(), String> {
        retry_locked(|| {
            let _guard = self.write_guard()?;
            let mut conn = self.open()?;
            let tx = conn.transaction().map_err(db_err)?;
            tx.execute(
                "INSERT OR IGNORE INTO stream_boundary_event
                 (turn_id, attempt_no, event_seq, history_session_id, runtime_generation_id,
                  event_kind, disposition, event_digest, observed_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    turn_id,
                    i64::try_from(attempt_no).unwrap_or(i64::MAX),
                    i64::try_from(event_seq).unwrap_or(i64::MAX),
                    history_session_id,
                    runtime_generation_id,
                    event_kind,
                    disposition,
                    event_digest,
                    observed_at,
                ],
            )
            .map_err(db_err)?;
            tx.execute(
                "UPDATE turn_snapshot
                 SET attempt_no = ?1, runtime_generation_id = ?2, recovery_state = 'none'
                 WHERE history_session_id = ?3 AND turn_id = ?4",
                params![
                    i64::try_from(attempt_no).unwrap_or(i64::MAX),
                    runtime_generation_id,
                    history_session_id,
                    turn_id,
                ],
            )
            .map_err(db_err)?;
            tx.commit().map_err(db_err)
        })
    }

    pub fn record_stream_diagnostic(
        &self,
        candidate: Option<&crate::turn_supervisor::EngineEventCandidate>,
        event_kind: &str,
        reason: &str,
        detail: Option<&str>,
    ) -> Result<(), String> {
        let detail = detail.map(|value| {
            crate::redaction::redact_text(value)
                .chars()
                .take(4096)
                .collect::<String>()
        });
        retry_locked(|| {
            let _guard = self.write_guard()?;
            let conn = self.open()?;
            conn.execute(
                "INSERT INTO stream_diagnostic
                 (history_session_id, turn_id, attempt_no, runtime_generation_id, source_seq,
                  event_kind, reason, detail, recorded_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    candidate.map(|value| value.history_session_id.as_str()),
                    candidate.map(|value| value.turn_id.as_str()),
                    candidate.map(|value| i64::try_from(value.attempt_no).unwrap_or(i64::MAX)),
                    candidate.map(|value| value.runtime_generation_id.as_str()),
                    candidate.map(|value| i64::try_from(value.source_seq).unwrap_or(i64::MAX)),
                    event_kind,
                    reason,
                    detail,
                    now_millis(),
                ],
            )
            .map_err(db_err)?;
            Ok(())
        })
    }

    pub fn finalize_supervised_turn(
        &self,
        snapshot: &crate::turn_supervisor::TurnSnapshot,
        attempt_no: u64,
        runtime_generation_id: &str,
        event_kind: &str,
        event_digest: &str,
    ) -> Result<(), String> {
        if !snapshot.status.is_terminal() {
            return Err("Finalizer 只能提交 Turn 终态".to_string());
        }
        retry_locked(|| {
            let _guard = self.write_guard()?;
            let mut conn = self.open()?;
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(db_err)?;
            if attempt_no > 0 {
                let (attempt_state, receipt) = match snapshot.status {
                    crate::turn_supervisor::TurnStatus::Succeeded => (
                        "completed",
                        snapshot.terminal_reason.as_deref().unwrap_or("end"),
                    ),
                    crate::turn_supervisor::TurnStatus::Interrupted => (
                        "interrupted",
                        snapshot.terminal_reason.as_deref().unwrap_or("interrupted"),
                    ),
                    crate::turn_supervisor::TurnStatus::Failed => (
                        "error",
                        snapshot.terminal_reason.as_deref().unwrap_or("error"),
                    ),
                    _ => unreachable!(),
                };
                let receipt = crate::redaction::redact_text(receipt)
                    .chars()
                    .take(4096)
                    .collect::<String>();
                let changed = tx
                    .execute(
                        "UPDATE turn_attempt
                         SET delivery_state = ?1, terminal_receipt = ?2, ended_at = ?3
                         WHERE turn_id = ?4 AND attempt_no = ?5 AND generation_id = ?6
                           AND delivery_state IN ('prepared', 'accepted')",
                        params![
                            attempt_state,
                            receipt,
                            snapshot.updated_at,
                            snapshot.turn_id,
                            i64::try_from(attempt_no).unwrap_or(i64::MAX),
                            runtime_generation_id,
                        ],
                    )
                    .map_err(db_err)?;
                if changed != 1 {
                    return Err(
                        "TurnAttempt 终态 CAS 失败：状态、attempt 或 generation 已变化".to_string(),
                    );
                }
            }
            tx.execute(
                "INSERT INTO turn_snapshot
                 (history_session_id, turn_id, turn_epoch, status, terminal_reason,
                  recoverable, event_seq, updated_at, turn_mode, permission_profile, started_at,
                  attempt_no, runtime_generation_id, recovery_state)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, 'none')
                 ON CONFLICT(history_session_id) DO UPDATE SET
                   status = excluded.status,
                   terminal_reason = excluded.terminal_reason,
                   recoverable = excluded.recoverable,
                   event_seq = excluded.event_seq,
                   updated_at = excluded.updated_at,
                   attempt_no = excluded.attempt_no,
                   runtime_generation_id = excluded.runtime_generation_id,
                   recovery_state = 'none'
                 WHERE turn_snapshot.turn_id = excluded.turn_id
                   AND turn_snapshot.status NOT IN ('succeeded', 'failed', 'interrupted')",
                params![
                    snapshot.history_session_id,
                    snapshot.turn_id,
                    i64::try_from(snapshot.turn_epoch).unwrap_or(i64::MAX),
                    turn_status_to_str(snapshot.status),
                    snapshot.terminal_reason,
                    i64::from(snapshot.recoverable),
                    i64::try_from(snapshot.event_seq).unwrap_or(i64::MAX),
                    snapshot.updated_at,
                    snapshot.mode,
                    snapshot.permission_profile,
                    snapshot.started_at,
                    i64::try_from(attempt_no).unwrap_or(i64::MAX),
                    runtime_generation_id,
                ],
            )
            .map_err(db_err)?;
            let changed = tx
                .execute(
                    "UPDATE turn
                     SET status = ?1, ended_at = ?2, terminal_reason = ?3
                     WHERE history_session_id = ?4 AND turn_id = ?5
                       AND status NOT IN ('succeeded', 'failed', 'interrupted')",
                    params![
                        turn_status_to_str(snapshot.status),
                        snapshot.updated_at,
                        snapshot.terminal_reason,
                        snapshot.history_session_id,
                        snapshot.turn_id,
                    ],
                )
                .map_err(db_err)?;
            if changed != 1 {
                return Err("Turn 唯一终态 CAS 失败".to_string());
            }
            finalize_turn_artifacts(
                &tx,
                &snapshot.history_session_id,
                &snapshot.turn_id,
                snapshot.status,
                snapshot.updated_at,
            )
            .map_err(db_err)?;
            tx.execute(
                "INSERT INTO stream_boundary_event
                 (turn_id, attempt_no, event_seq, history_session_id, runtime_generation_id,
                  event_kind, disposition, event_digest, observed_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'accepted', ?7, ?8)",
                params![
                    snapshot.turn_id,
                    i64::try_from(attempt_no).unwrap_or(i64::MAX),
                    i64::try_from(snapshot.event_seq).unwrap_or(i64::MAX),
                    snapshot.history_session_id,
                    runtime_generation_id,
                    event_kind,
                    event_digest,
                    snapshot.updated_at,
                ],
            )
            .map_err(db_err)?;
            tx.commit().map_err(db_err)
        })
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
        provider_id: &str,
        model: &str,
        input_tokens: u64,
        cached_input_tokens: Option<u64>,
        cache_write_input_tokens: Option<u64>,
        output_tokens: u64,
        cost_usd: f64,
        service_tier: Option<&str>,
        frozen_pricing: Option<&ResolvedPricingProfile>,
        pricing_basis_frozen: bool,
    ) -> CostResolution {
        if cost_usd > 0.0 || (input_tokens == 0 && output_tokens == 0) {
            return CostResolution {
                cost_usd,
                cost_kind: if cost_usd > 0.0 { "actual" } else { "unknown" }.to_string(),
                price_source: if cost_usd > 0.0 { "engine" } else { "unknown" }.to_string(),
                catalog_version: None,
                price_snapshot_json: None,
                reported_cost_usd: (cost_usd > 0.0).then_some(cost_usd),
            };
        }
        let price = if pricing_basis_frozen {
            frozen_pricing.cloned()
        } else {
            frozen_pricing.cloned().or_else(|| {
                let prices = self.model_prices.lock().ok()?;
                prices
                    .get(&model_price_key(provider_id, model))
                    .or_else(|| prices.get(&model_price_key("", model)))
                    .cloned()
            })
        };
        let Some(price) = price else {
            return CostResolution::unknown();
        };
        if price.source == "subscription" {
            return CostResolution {
                cost_usd: 0.0,
                cost_kind: "subscription".to_string(),
                price_source: "subscription".to_string(),
                catalog_version: Some(price.catalog_version.clone()),
                price_snapshot_json: serde_json::to_string(&price).ok(),
                reported_cost_usd: None,
            };
        }
        let tier = parse_service_tier(service_tier);
        let Some(pricing_tier) = price
            .tiers
            .get(&tier)
            .or_else(|| price.tiers.get(&ServiceTier::Standard))
        else {
            return CostResolution::unknown();
        };
        let Some(band) = pricing_tier
            .bands
            .iter()
            .find(|band| pricing_band_matches(band, input_tokens))
            .or_else(|| pricing_tier.bands.first())
        else {
            return CostResolution::unknown();
        };
        let cached = cached_input_tokens.unwrap_or_default().min(input_tokens);
        let cache_write = cache_write_input_tokens
            .unwrap_or_default()
            .min(input_tokens.saturating_sub(cached));
        let uncached = input_tokens
            .saturating_sub(cached)
            .saturating_sub(cache_write);
        let calculated = ((uncached as f64 / 1_000_000.0) * band.input)
            + ((cached as f64 / 1_000_000.0) * band.cached_input.unwrap_or(band.input))
            + ((cache_write as f64 / 1_000_000.0) * band.cache_write.unwrap_or(band.input))
            + ((output_tokens as f64 / 1_000_000.0) * band.output);
        CostResolution {
            cost_usd: calculated,
            cost_kind: "estimated".to_string(),
            price_source: price.source.clone(),
            catalog_version: Some(price.catalog_version.clone()),
            price_snapshot_json: serde_json::to_string(&price).ok(),
            reported_cost_usd: None,
        }
    }

    fn open(&self) -> Result<Connection, String> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("创建会话数据库目录失败：{e}"))?;
        }
        let mut conn = Connection::open(&self.path).map_err(db_err)?;
        configure_connection(&conn)?;
        self.ensure_initialized(&mut conn)?;
        Ok(conn)
    }

    fn write_guard(&self) -> Result<std::sync::MutexGuard<'_, ()>, String> {
        self.write_lock
            .lock()
            .map_err(|_| "会话数据库写锁中毒".to_string())
    }

    fn ensure_initialized(&self, conn: &mut Connection) -> Result<(), String> {
        let mut initialized = self
            .initialized
            .lock()
            .map_err(|_| "会话数据库初始化锁中毒".to_string())?;
        if *initialized {
            return Ok(());
        }
        retry_locked(|| init_schema(conn))?;
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
                "SELECT role, text, ts, reverted, turn_id FROM message WHERE session_id = ?1 ORDER BY id ASC",
            )
            .map_err(db_err)?;
        let mut messages = stmt
            .query_map(params![session_id], |row| {
                Ok(SessionMessage {
                    role: role_from_str(row.get::<_, String>(0)?.as_str())?,
                    text: row.get(1)?,
                    ts: row.get(2)?,
                    reverted: row.get::<_, i64>(3)? != 0,
                    turn_id: row.get(4)?,
                    attachments: Vec::new(),
                })
            })
            .map_err(db_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_err)?;
        for message in &mut messages {
            if message.role != Role::User {
                continue;
            }
            let Some(turn_id) = message.turn_id.as_deref() else {
                continue;
            };
            let mut attachment_stmt = conn
                .prepare(
                    "SELECT source_path FROM message_attachment
                     WHERE session_id = ?1 AND turn_id = ?2 ORDER BY ordinal ASC",
                )
                .map_err(db_err)?;
            message.attachments = attachment_stmt
                .query_map(params![session_id, turn_id], |row| row.get(0))
                .map_err(db_err)?
                .collect::<Result<Vec<String>, _>>()
                .map_err(db_err)?;
        }
        Ok(messages)
    }

    fn session_context_for_conn(
        &self,
        conn: &Connection,
        session_id: &str,
    ) -> Result<Vec<SessionContextRecord>, String> {
        let mut stmt = conn
            .prepare(
                "SELECT id, kind, source_path, canonical_path, display_name, status,
                        status_detail, created_at, updated_at
                 FROM session_context WHERE session_id = ?1 ORDER BY created_at ASC, id ASC",
            )
            .map_err(db_err)?;
        let contexts = stmt
            .query_map(params![session_id], |row| {
                Ok(SessionContextRecord {
                    id: row.get(0)?,
                    kind: row.get(1)?,
                    source_path: row.get(2)?,
                    canonical_path: row.get(3)?,
                    display_name: row.get(4)?,
                    status: row.get(5)?,
                    status_detail: row.get(6)?,
                    created_at: row.get(7)?,
                    updated_at: row.get(8)?,
                })
            })
            .map_err(db_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_err)?;
        Ok(contexts)
    }

    fn tools_for_conn(
        &self,
        conn: &Connection,
        session_id: &str,
    ) -> Result<Vec<SessionToolCall>, String> {
        let mut stmt = conn
            .prepare(
                "SELECT id, name, status, input_json, output, diff_json, ts, ended_at, turn_id,
                        outcome, tool_started, has_output, retryable, denial_source, native_denial_code
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
                    ended_at: row.get(7)?,
                    turn_id: row.get(8)?,
                    outcome: row.get(9)?,
                    started: row.get::<_, Option<i64>>(10)?.map(|value| value != 0),
                    has_output: row.get::<_, Option<i64>>(11)?.map(|value| value != 0),
                    retryable: row.get::<_, Option<i64>>(12)?.map(|value| value != 0),
                    denial_source: row.get(13)?,
                    native_denial_code: row.get(14)?,
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
                "SELECT id, label, ts, turn_id, restorable, file_count, restorable_reason
                 FROM checkpoint WHERE session_id = ?1 ORDER BY ts ASC",
            )
            .map_err(db_err)?;
        let checkpoints = stmt
            .query_map(params![session_id], |row| {
                Ok(SessionCheckpoint {
                    id: row.get(0)?,
                    label: row.get(1)?,
                    ts: row.get(2)?,
                    turn_id: row.get(3)?,
                    restorable: row.get::<_, i64>(4)? != 0,
                    file_count: row.get::<_, i64>(5)?.max(0) as u64,
                    reason: row.get(6)?,
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
                "SELECT id, action, detail, status, ts, decision, rule_id, error, resolved_at,
                        persistent_label, matcher_summary, turn_id
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
                    decision: row.get(5)?,
                    rule_id: row.get(6)?,
                    error: row.get(7)?,
                    resolved_at: row.get(8)?,
                    persistent_label: row.get(9)?,
                    matcher_summary: row.get(10)?,
                    turn_id: row.get(11)?,
                })
            })
            .map_err(db_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_err)?;
        Ok(approvals)
    }

    pub fn save_permission_rule(&self, rule: &PermissionRule) -> Result<(), String> {
        let _guard = self.write_guard()?;
        let mut conn = self.open()?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_err)?;
        let effect = stable_serde_string(&rule.effect)?;
        let scope = stable_serde_string(&rule.scope)?;
        let capability = stable_serde_string(&rule.capability)?;
        tx.execute(
            "INSERT INTO permission_rule
             (id, principal, effect, scope, tool_call_id, turn_id, history_session_id, project_root,
              engine, capability, operation, resource_pattern, created_at, expires_at,
              max_uses, uses)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, 0)
             ON CONFLICT(id) DO UPDATE SET
               principal = excluded.principal,
               effect = excluded.effect,
               scope = excluded.scope,
               tool_call_id = excluded.tool_call_id,
               turn_id = excluded.turn_id,
               history_session_id = excluded.history_session_id,
               project_root = excluded.project_root,
               engine = excluded.engine,
               capability = excluded.capability,
               operation = excluded.operation,
               resource_pattern = excluded.resource_pattern,
               created_at = excluded.created_at,
               expires_at = excluded.expires_at,
               max_uses = excluded.max_uses",
            params![
                rule.id,
                rule.principal,
                effect,
                scope,
                rule.scope_binding.tool_call_id,
                rule.scope_binding.turn_id,
                rule.scope_binding.session_id,
                rule.scope_binding.project_root,
                rule.engine,
                capability,
                rule.operation,
                rule.resource_pattern,
                rule.created_at,
                rule.expires_at,
                rule.max_uses.map(i64::from)
            ],
        )
        .map_err(db_err)?;
        bump_permission_policy_version_on_conn(&tx)?;
        tx.commit().map_err(db_err)?;
        self.invalidate_runtime_grant_cache();
        Ok(())
    }

    /// RuntimeGrant is created from the full normalized action, never from a
    /// display-oriented PermissionRule. This keeps network origins and exact
    /// command inputs from being widened by lossy string reconstruction.
    pub fn save_runtime_grant_for_action(
        &self,
        rule: &PermissionRule,
        action: &ActionDescriptor,
    ) -> Result<(), String> {
        if !matches!(
            rule.scope,
            PermissionScope::Project | PermissionScope::Global
        ) || rule.effect != PermissionEffect::Allow
        {
            return Err("runtime grants require a project or global allow rule".to_string());
        }
        if rule.engine.as_deref() != Some(action.engine.as_str()) {
            return Err("runtime grant engine does not match its permission rule".to_string());
        }
        let matcher = crate::permissions::runtime_grant_matcher(action).ok_or_else(|| {
            "this runtime action is not eligible for persistent authorization".to_string()
        })?;
        let adapter_version = crate::permissions::runtime_approval_adapter_version(&action.engine)
            .ok_or_else(|| "runtime adapter is not registered".to_string())?;
        let provider_id = match self.session_provider_id(&action.session_id) {
            Ok(provider_id) => provider_id,
            Err(error) if error.contains("Query returned no rows") => String::new(),
            Err(error) => return Err(error),
        };
        let _guard = self.write_guard()?;
        let mut conn = self.open()?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_err)?;
        tx.execute(
            "INSERT INTO runtime_grant
             (id, engine, provider_id, project_root, matcher_kind, matcher_value, scope,
              adapter_version, ceiling_version, created_at, revoked_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, NULL)
             ON CONFLICT(id) DO UPDATE SET
               engine = excluded.engine,
               provider_id = excluded.provider_id,
               project_root = excluded.project_root,
               matcher_kind = excluded.matcher_kind,
               matcher_value = excluded.matcher_value,
               scope = excluded.scope,
               adapter_version = excluded.adapter_version,
               ceiling_version = excluded.ceiling_version,
               created_at = excluded.created_at,
               revoked_at = NULL",
            params![
                rule.id,
                action.engine,
                provider_id,
                rule.scope_binding.project_root,
                matcher.kind,
                matcher.value,
                stable_serde_string(&rule.scope)?,
                adapter_version,
                crate::permissions::RUNTIME_GRANT_CEILING_VERSION,
                rule.created_at,
            ],
        )
        .map_err(db_err)?;
        bump_permission_policy_version_on_conn(&tx)?;
        tx.commit().map_err(db_err)?;
        self.invalidate_runtime_grant_cache();
        Ok(())
    }

    pub fn runtime_grant_matches(
        &self,
        rule_id: &str,
        action: &ActionDescriptor,
    ) -> Result<bool, String> {
        let provider_id = self
            .session_provider_id(&action.session_id)
            .unwrap_or_default();
        let cache = self.runtime_grant_snapshot()?;
        Ok(cache
            .get(rule_id)
            .is_some_and(|grant| runtime_grant_record_matches(grant, action, &provider_id)))
    }

    fn runtime_grant_snapshot(&self) -> Result<HashMap<String, RuntimeGrantRecord>, String> {
        if let Ok(cache) = self.runtime_grants.lock() {
            if cache.policy_version != 0 {
                return Ok(cache.entries.clone());
            }
        }
        let policy_version = self.permission_policy_version()?;
        let conn = self.open()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, engine, COALESCE(provider_id, ''), project_root, matcher_kind,
                        matcher_value, scope, adapter_version, ceiling_version
                 FROM runtime_grant WHERE revoked_at IS NULL",
            )
            .map_err(db_err)?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    RuntimeGrantRecord {
                        engine: row.get(1)?,
                        provider_id: row.get(2)?,
                        project_root: row.get(3)?,
                        matcher_kind: row.get(4)?,
                        matcher_value: row.get(5)?,
                        scope: row.get(6)?,
                        adapter_version: row.get(7)?,
                        ceiling_version: row.get(8)?,
                    },
                ))
            })
            .map_err(db_err)?
            .collect::<Result<HashMap<_, _>, _>>()
            .map_err(db_err)?;
        if let Ok(mut cache) = self.runtime_grants.lock() {
            cache.policy_version = policy_version;
            cache.entries = rows.clone();
        }
        Ok(rows)
    }

    fn invalidate_runtime_grant_cache(&self) {
        if let Ok(mut cache) = self.runtime_grants.lock() {
            cache.policy_version = 0;
            cache.entries.clear();
        }
    }

    pub fn save_consumed_permission_rule(&self, rule: &PermissionRule) -> Result<(), String> {
        let _guard = self.write_guard()?;
        let mut conn = self.open()?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_err)?;
        tx.execute(
            "INSERT INTO permission_rule
             (id, principal, effect, scope, tool_call_id, turn_id, history_session_id, project_root,
              engine, capability, operation, resource_pattern, created_at, expires_at,
              max_uses, uses)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, 1)
             ON CONFLICT(id) DO UPDATE SET
               principal = excluded.principal,
               effect = excluded.effect,
               scope = excluded.scope,
               tool_call_id = excluded.tool_call_id,
               turn_id = excluded.turn_id,
               history_session_id = excluded.history_session_id,
               project_root = excluded.project_root,
               engine = excluded.engine,
               capability = excluded.capability,
               operation = excluded.operation,
               resource_pattern = excluded.resource_pattern,
               created_at = excluded.created_at,
               expires_at = excluded.expires_at,
               max_uses = excluded.max_uses,
               uses = MAX(permission_rule.uses, excluded.uses)",
            params![
                rule.id,
                rule.principal,
                stable_serde_string(&rule.effect)?,
                stable_serde_string(&rule.scope)?,
                rule.scope_binding.tool_call_id,
                rule.scope_binding.turn_id,
                rule.scope_binding.session_id,
                rule.scope_binding.project_root,
                rule.engine,
                stable_serde_string(&rule.capability)?,
                rule.operation,
                rule.resource_pattern,
                rule.created_at,
                rule.expires_at,
                rule.max_uses.map(i64::from)
            ],
        )
        .map_err(db_err)?;
        bump_permission_policy_version_on_conn(&tx)?;
        tx.commit().map_err(db_err)
    }

    pub fn list_permission_rules(&self) -> Result<Vec<PermissionRule>, String> {
        let conn = self.open()?;
        permission_rules_for_conn(&conn)
    }

    pub fn permission_policy_version(&self) -> Result<u64, String> {
        let conn = self.open()?;
        permission_policy_version_on_conn(&conn)
    }

    pub fn evaluate_permission_action(
        &self,
        action: &ActionDescriptor,
    ) -> Result<PermissionDecision, String> {
        self.evaluate_permission_action_inner(action)
    }

    fn evaluate_permission_action_inner(
        &self,
        action: &ActionDescriptor,
    ) -> Result<PermissionDecision, String> {
        let runtime_grants = self.runtime_grant_snapshot()?;
        let provider_id = self
            .session_provider_id(&action.session_id)
            .unwrap_or_default();
        let _guard = self.write_guard()?;
        let mut conn = self.open()?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_err)?;
        let fingerprint = action_fingerprint(action)?;
        let policy_version = permission_policy_version_on_conn(&tx)?;
        let mut stmt = tx
            .prepare(
                "SELECT action_fingerprint, effect, reason, rule_id, policy_version
                 FROM permission_audit
                 WHERE history_session_id = ?1 AND turn_id = ?2 AND tool_call_id = ?3
                 ORDER BY id ASC",
            )
            .map_err(db_err)?;
        let prior = stmt
            .query_map(
                params![action.session_id, action.turn_id, action.tool_call_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, i64>(4)?,
                    ))
                },
            )
            .map_err(db_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_err)?;
        drop(stmt);
        let has_identity_collision = prior.iter().any(|(saved, ..)| saved != &fingerprint);
        if has_identity_collision {
            if let Some((_, effect, reason, rule_id, version)) =
                prior.iter().rev().find(|(_, effect, reason, _, _)| {
                    effect == "deny" && reason.contains("identity collision")
                })
            {
                return Ok(PermissionDecision {
                    effect: stable_serde_from_string(effect.clone(), 1).map_err(db_err)?,
                    reason: reason.clone(),
                    rule_id: rule_id.clone(),
                    policy_version: u64::try_from(*version).map_err(|e| e.to_string())?,
                });
            }
        } else if let Some((_, effect, reason, rule_id, version)) =
            prior.iter().find(|(saved, _, _, _, version)| {
                saved == &fingerprint && u64::try_from(*version).ok() == Some(policy_version)
            })
        {
            return Ok(PermissionDecision {
                effect: stable_serde_from_string(effect.clone(), 1).map_err(db_err)?,
                reason: reason.clone(),
                rule_id: rule_id.clone(),
                policy_version: u64::try_from(*version).map_err(|e| e.to_string())?,
            });
        }

        let now = now_millis();
        let settings_effect = if matches!(
            action.capability,
            Capability::FileRead | Capability::DirectoryList
        ) {
            safe_read_effect(action)
        } else if action.capability == Capability::FileWrite {
            let safe_profile = self.session_safe_profile_on_conn(&tx, &action.session_id)?;
            safe_file_write_effect(action, &safe_profile, &tx)?
        } else {
            PermissionEffect::Ask
        };
        let mut decision = if has_identity_collision {
            PermissionDecision {
                effect: PermissionEffect::Deny,
                reason: "tool call identity collision: input changed for an existing id"
                    .to_string(),
                rule_id: None,
                policy_version,
            }
        } else if settings_effect == PermissionEffect::Deny {
            PermissionDecision {
                effect: PermissionEffect::Deny,
                reason: "safe read boundary rejects this target".to_string(),
                rule_id: None,
                policy_version,
            }
        } else {
            let rules = permission_rules_for_conn(&tx)?;
            let mut evaluated =
                crate::permission_kernel::evaluate_action(action, &rules, now, policy_version);
            if evaluated.effect == PermissionEffect::Ask
                && evaluated.rule_id.is_none()
                && settings_effect == PermissionEffect::Allow
            {
                evaluated.effect = PermissionEffect::Allow;
                evaluated.reason = "permission settings allow this capability".to_string();
            }
            evaluated
        };
        if decision.effect == PermissionEffect::Allow {
            if let Some(rule_id) = decision.rule_id.as_deref() {
                let persistent_scope: Option<String> = tx
                    .query_row(
                        "SELECT scope FROM permission_rule WHERE id = ?1",
                        params![rule_id],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(db_err)?;
                if persistent_scope
                    .as_deref()
                    .is_some_and(|scope| scope == "project" || scope == "global")
                    && crate::permissions::runtime_approval_adapter_version(&action.engine)
                        .is_some()
                    && !runtime_grants.get(rule_id).is_some_and(|grant| {
                        runtime_grant_record_matches(grant, action, &provider_id)
                    })
                {
                    decision = PermissionDecision {
                        effect: PermissionEffect::Ask,
                        reason: "runtime grant binding is missing, revoked, or stale".to_string(),
                        rule_id: None,
                        policy_version,
                    };
                }
            }
            if decision.effect == PermissionEffect::Allow {
                if let Some(rule_id) = decision.rule_id.as_deref() {
                    let changed = tx
                        .execute(
                            "UPDATE permission_rule SET uses = uses + 1
                         WHERE id = ?1 AND (max_uses IS NULL OR uses < max_uses)",
                            params![rule_id],
                        )
                        .map_err(db_err)?;
                    if changed != 1 {
                        decision = PermissionDecision {
                            effect: PermissionEffect::Ask,
                            reason: "matching permission rule is exhausted".to_string(),
                            rule_id: None,
                            policy_version,
                        };
                    }
                }
            }
        }
        insert_permission_audit(&tx, action, &fingerprint, &decision, now)?;
        tx.commit().map_err(db_err)?;
        Ok(decision)
    }

    pub fn mark_permission_execution_started(
        &self,
        action: &ActionDescriptor,
    ) -> Result<(), String> {
        self.mark_permission_execution_started_with_authorization(action, "policy_allow", false)
    }

    pub fn mark_user_approved_execution_started(
        &self,
        action: &ActionDescriptor,
    ) -> Result<(), String> {
        self.mark_permission_execution_started_with_authorization(action, "user_approved", true)
    }

    fn mark_permission_execution_started_with_authorization(
        &self,
        action: &ActionDescriptor,
        authorization: &str,
        allow_ask: bool,
    ) -> Result<(), String> {
        let _guard = self.write_guard()?;
        let conn = self.open()?;
        let fingerprint = action_fingerprint(action)?;
        let changed = conn
            .execute(
                "UPDATE permission_audit
                 SET execution_status = 'started', execution_started_at = ?5,
                     execution_authorization = ?6
                 WHERE id = (
                   SELECT id FROM permission_audit
                   WHERE history_session_id = ?1 AND turn_id = ?2 AND tool_call_id = ?3
                     AND action_fingerprint = ?4
                     AND (effect = 'allow' OR (?7 = 1 AND effect = 'ask'))
                   ORDER BY id DESC LIMIT 1
                 ) AND execution_status = 'not_started'",
                params![
                    action.session_id,
                    action.turn_id,
                    action.tool_call_id,
                    fingerprint,
                    now_millis(),
                    authorization,
                    i64::from(allow_ask)
                ],
            )
            .map_err(db_err)?;
        if changed != 1 {
            return Err("permission audit cannot enter execution_started; action may be replayed or unaudited".to_string());
        }
        Ok(())
    }

    pub fn finish_permission_execution(
        &self,
        action: &ActionDescriptor,
        succeeded: bool,
    ) -> Result<(), String> {
        let _guard = self.write_guard()?;
        let conn = self.open()?;
        let fingerprint = action_fingerprint(action)?;
        let status = if succeeded { "completed" } else { "failed" };
        let changed = conn
            .execute(
                "UPDATE permission_audit
                 SET execution_status = ?5, execution_finished_at = ?6
                 WHERE id = (
                   SELECT id FROM permission_audit
                   WHERE history_session_id = ?1 AND turn_id = ?2 AND tool_call_id = ?3
                     AND action_fingerprint = ?4
                   ORDER BY id DESC LIMIT 1
                 ) AND execution_status = 'started'",
                params![
                    action.session_id,
                    action.turn_id,
                    action.tool_call_id,
                    fingerprint,
                    status,
                    now_millis()
                ],
            )
            .map_err(db_err)?;
        if changed != 1 {
            return Err(
                "permission audit cannot finalize an execution that is not started".to_string(),
            );
        }
        Ok(())
    }

    pub fn permission_audit_summary(&self) -> Result<PermissionAuditSummary, String> {
        let conn = self.open()?;
        let (count, oldest_at, newest_at): (i64, Option<i64>, Option<i64>) = conn
            .query_row(
                "SELECT COUNT(*), MIN(created_at), MAX(created_at) FROM permission_audit",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .map_err(db_err)?;
        Ok(PermissionAuditSummary {
            record_count: u64::try_from(count).map_err(|error| error.to_string())?,
            oldest_at,
            newest_at,
            retention_days: PERMISSION_AUDIT_RETENTION_DAYS,
        })
    }

    pub fn prune_permission_audit_before(&self, cutoff_millis: i64) -> Result<usize, String> {
        let _guard = self.write_guard()?;
        let conn = self.open()?;
        conn.execute(
            "DELETE FROM permission_audit
             WHERE created_at < ?1 AND execution_status != 'started'",
            params![cutoff_millis],
        )
        .map_err(db_err)
    }

    pub fn clear_permission_audit(&self) -> Result<usize, String> {
        self.prune_permission_audit_before(i64::MAX)
    }

    pub fn export_permission_audit_json(&self, include_resources: bool) -> Result<String, String> {
        let conn = self.open()?;
        let mut stmt = conn
            .prepare(
                "SELECT created_at, engine, capability, operation, resources_json,
                        effect, reason, policy_version, execution_status, execution_authorization,
                        execution_started_at, execution_finished_at, revocation_too_late_at
                 FROM permission_audit ORDER BY id ASC",
            )
            .map_err(db_err)?;
        let records = stmt
            .query_map([], |row| {
                let resources_json: String = row.get(4)?;
                let resources = serde_json::from_str::<Vec<String>>(&resources_json)
                    .unwrap_or_else(|_| vec!["<invalid-resource-record>".to_string()]);
                let resource_digests = resources
                    .iter()
                    .map(|resource| {
                        format!(
                            "sha256:{}",
                            Sha256::digest(resource.as_bytes())
                                .iter()
                                .map(|byte| format!("{byte:02x}"))
                                .collect::<String>()
                        )
                    })
                    .collect();
                Ok(PermissionAuditExportRecord {
                    created_at: row.get(0)?,
                    engine: row.get(1)?,
                    capability: row.get(2)?,
                    operation: row.get(3)?,
                    effect: row.get(5)?,
                    reason: row.get(6)?,
                    policy_version: row.get(7)?,
                    execution_status: row.get(8)?,
                    execution_authorization: row.get(9)?,
                    execution_started_at: row.get(10)?,
                    execution_finished_at: row.get(11)?,
                    revocation_too_late_at: row.get(12)?,
                    resource_digests,
                    resources: include_resources.then_some(resources),
                })
            })
            .map_err(db_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_err)?;
        serde_json::to_string_pretty(&serde_json::json!({
            "version": 1,
            "exportedAt": now_millis(),
            "includesResourceDetails": include_resources,
            "records": records,
        }))
        .map_err(|error| error.to_string())
    }

    pub fn remove_permission_rule(&self, id: &str) -> Result<usize, String> {
        let _guard = self.write_guard()?;
        let mut conn = self.open()?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_err)?;
        let revocation_too_late = tx
            .execute(
                "UPDATE permission_audit
                 SET revocation_too_late_at = ?2
                 WHERE rule_id = ?1 AND execution_status = 'started'
                   AND revocation_too_late_at IS NULL",
                params![id, now_millis()],
            )
            .map_err(db_err)?;
        let changed = tx
            .execute("DELETE FROM permission_rule WHERE id = ?1", params![id])
            .map_err(db_err)?;
        tx.execute(
            "UPDATE runtime_grant SET revoked_at = ?2 WHERE id = ?1 AND revoked_at IS NULL",
            params![id, now_millis()],
        )
        .map_err(db_err)?;
        if changed > 0 {
            bump_permission_policy_version_on_conn(&tx)?;
        }
        tx.commit().map_err(db_err)?;
        self.invalidate_runtime_grant_cache();
        Ok(revocation_too_late)
    }

    /// 按结构化 rule id 撤销；若该规则来自旧 always-allow 清单，同一事务删除兼容值，
    /// 防止下一次迁移把已撤销授权重新创建。
    pub fn remove_permission_rule_with_legacy_compat(&self, id: &str) -> Result<usize, String> {
        let _guard = self.write_guard()?;
        let mut conn = self.open()?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_err)?;
        let raw: Option<String> = tx
            .query_row(
                "SELECT value_json FROM setting WHERE key = ?1",
                params![ALWAYS_ALLOW_TOOLS_KEY],
                |row| row.get(0),
            )
            .optional()
            .map_err(db_err)?;
        let mut tools = raw
            .map(|value| serde_json::from_str::<Vec<String>>(&value).map_err(|e| e.to_string()))
            .transpose()?
            .unwrap_or_default();
        tools.retain(|tool| legacy_always_allow_rule(tool, 0).id != id);
        let revocation_too_late = tx
            .execute(
                "UPDATE permission_audit
                 SET revocation_too_late_at = ?2
                 WHERE rule_id = ?1 AND execution_status = 'started'
                   AND revocation_too_late_at IS NULL",
                params![id, now_millis()],
            )
            .map_err(db_err)?;
        let changed = tx
            .execute("DELETE FROM permission_rule WHERE id = ?1", params![id])
            .map_err(db_err)?;
        tx.execute(
            "UPDATE runtime_grant SET revoked_at = ?2 WHERE id = ?1 AND revoked_at IS NULL",
            params![id, now_millis()],
        )
        .map_err(db_err)?;
        tx.execute(
            "INSERT INTO setting (key, value_json) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json",
            params![
                ALWAYS_ALLOW_TOOLS_KEY,
                serde_json::to_string(&tools).map_err(|e| e.to_string())?
            ],
        )
        .map_err(db_err)?;
        if changed > 0 {
            bump_permission_policy_version_on_conn(&tx)?;
        }
        tx.commit().map_err(db_err)?;
        self.invalidate_runtime_grant_cache();
        Ok(revocation_too_late)
    }

    /// 把旧版 `approval_always_allow` 字符串清单幂等迁移成结构化规则。
    /// 迁移阶段仍保留旧 setting，供 Phase 1 Claude hook 兼容读取；结构化规则是长期真值。
    pub fn migrate_legacy_always_allow_rules(&self) -> Result<Vec<PermissionRule>, String> {
        let tools = self.get_always_allow_tools()?;
        let existing_ids: std::collections::HashSet<String> = self
            .list_permission_rules()?
            .into_iter()
            .map(|rule| rule.id)
            .collect();
        let created_at = now_millis();
        for tool in tools {
            let rule = legacy_always_allow_rule(&tool, created_at);
            if !existing_ids.contains(&rule.id) {
                self.save_permission_rule(&rule)?;
            }
        }
        self.list_permission_rules()
    }

    /// 审批已处理（变更-07）：用户点了允许/始终允许/拒绝
    pub fn resolve_approval(&self, session_id: &str, approval_id: &str) -> Result<(), String> {
        let _guard = self.write_guard()?;
        let conn = self.open()?;
        let local_id = self.resolve_local_id(&conn, session_id)?;
        let changed = conn
            .execute(
                "UPDATE approval
                 SET status = 'resolved', error = NULL, resolved_at = ?3
                 WHERE session_id = ?1 AND id = ?2 AND status = 'pending'",
                params![local_id, approval_id, now_millis()],
            )
            .map_err(db_err)?;
        ensure_approval_transition(&conn, &local_id, approval_id, changed, "处理", None)?;
        Ok(())
    }

    pub fn mark_approval_applying(
        &self,
        session_id: &str,
        approval_id: &str,
        decision: &str,
    ) -> Result<(), String> {
        let _guard = self.write_guard()?;
        let conn = self.open()?;
        let local_id = self.resolve_local_id(&conn, session_id)?;
        let changed = conn
            .execute(
                "UPDATE approval
                 SET status = 'applying', decision = ?3, rule_id = NULL,
                     error = NULL, resolved_at = NULL
                 WHERE session_id = ?1 AND id = ?2 AND status IN ('pending', 'failed')",
                params![local_id, approval_id, decision],
            )
            .map_err(db_err)?;
        ensure_approval_transition(&conn, &local_id, approval_id, changed, "应用", None)?;
        Ok(())
    }

    pub fn resolve_approval_with_decision(
        &self,
        session_id: &str,
        approval_id: &str,
        decision: &str,
        rule_id: Option<&str>,
    ) -> Result<(), String> {
        let _guard = self.write_guard()?;
        let conn = self.open()?;
        let local_id = self.resolve_local_id(&conn, session_id)?;
        let changed = conn
            .execute(
                "UPDATE approval
                 SET status = 'resolved', rule_id = ?4,
                     error = NULL, resolved_at = ?5
                 WHERE session_id = ?1 AND id = ?2
                   AND status = 'applying' AND decision = ?3",
                params![local_id, approval_id, decision, rule_id, now_millis()],
            )
            .map_err(db_err)?;
        ensure_approval_transition(
            &conn,
            &local_id,
            approval_id,
            changed,
            "处理",
            Some(decision),
        )?;
        Ok(())
    }

    pub fn fail_approval(
        &self,
        session_id: &str,
        approval_id: &str,
        error: &str,
    ) -> Result<(), String> {
        let _guard = self.write_guard()?;
        let conn = self.open()?;
        let local_id = self.resolve_local_id(&conn, session_id)?;
        let changed = conn
            .execute(
                "UPDATE approval
                 SET status = 'failed', error = ?3, resolved_at = ?4
                 WHERE session_id = ?1 AND id = ?2 AND status = 'applying'",
                params![local_id, approval_id, error, now_millis()],
            )
            .map_err(db_err)?;
        ensure_approval_transition(&conn, &local_id, approval_id, changed, "标记失败", None)?;
        Ok(())
    }

    /// 审批恢复执行失败时的补偿：把刚标记 resolved 的记录恢复为 pending，允许用户重试。
    pub fn reopen_approval(&self, session_id: &str, approval_id: &str) -> Result<(), String> {
        let _guard = self.write_guard()?;
        let conn = self.open()?;
        let local_id = self.resolve_local_id(&conn, session_id)?;
        let changed = conn
            .execute(
                "UPDATE approval
                 SET status = 'pending', decision = NULL, rule_id = NULL,
                     error = NULL, resolved_at = NULL
                 WHERE session_id = ?1 AND id = ?2 AND status = 'resolved'",
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

    pub fn list_folders(&self) -> Result<Vec<SessionFolder>, String> {
        let conn = self.open()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, name, sort_order, collapsed, locked, created_at, cwd
                 FROM session_folder ORDER BY sort_order ASC, created_at ASC",
            )
            .map_err(db_err)?;
        let folders = stmt
            .query_map([], |row| {
                Ok(SessionFolder {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    sort_order: row.get(2)?,
                    collapsed: row.get::<_, i64>(3)? != 0,
                    locked: row.get::<_, i64>(4)? != 0,
                    created_at: row.get(5)?,
                    cwd: row.get(6)?,
                })
            })
            .map_err(db_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_err)?;
        Ok(folders)
    }

    pub fn create_folder(&self, name: &str) -> Result<SessionFolder, String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("文件夹名称不能为空".to_string());
        }
        if name.len() > 80 {
            return Err("文件夹名称不能超过 80 个字符".to_string());
        }
        let _guard = self.write_guard()?;
        let conn = self.open()?;
        let id = format!("folder-{}", uuid_like_id());
        let now = now_millis();
        let sort_order: i64 = conn
            .query_row(
                "SELECT COALESCE(MAX(sort_order), -1) + 1 FROM session_folder",
                [],
                |row| row.get(0),
            )
            .map_err(db_err)?;
        conn.execute(
            "INSERT INTO session_folder (id,name,sort_order,collapsed,locked,created_at)
             VALUES (?1,?2,?3,0,0,?4)",
            params![id, name, sort_order, now],
        )
        .map_err(db_err)?;
        self.list_folders()?
            .into_iter()
            .find(|folder| folder.id == id)
            .ok_or_else(|| "创建文件夹后读取失败".to_string())
    }

    pub fn rename_folder(&self, folder_id: &str, name: &str) -> Result<(), String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("文件夹名称不能为空".to_string());
        }
        if name.len() > 80 {
            return Err("文件夹名称不能超过 80 个字符".to_string());
        }
        let _guard = self.write_guard()?;
        let conn = self.open()?;
        let changed = conn
            .execute(
                "UPDATE session_folder SET name = ?1 WHERE id = ?2",
                params![name, folder_id],
            )
            .map_err(db_err)?;
        if changed == 0 {
            return Err("文件夹不存在".to_string());
        }
        Ok(())
    }

    pub fn delete_folder(&self, folder_id: &str) -> Result<(), String> {
        let _guard = self.write_guard()?;
        let conn = self.open()?;
        let tx = conn.unchecked_transaction().map_err(db_err)?;
        let locked: Option<i64> = tx
            .query_row(
                "SELECT locked FROM session_folder WHERE id = ?1",
                params![folder_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(db_err)?;
        match locked {
            None => return Err("文件夹不存在".to_string()),
            Some(1) => return Err("默认文件夹不可删除".to_string()),
            _ => {}
        }
        tx.execute(
            "UPDATE session SET folder_id = 'folder-default' WHERE folder_id = ?1",
            params![folder_id],
        )
        .map_err(db_err)?;
        tx.execute(
            "DELETE FROM session_folder WHERE id = ?1",
            params![folder_id],
        )
        .map_err(db_err)?;
        tx.commit().map_err(db_err)
    }

    pub fn set_session_folder(&self, session_id: &str, folder_id: &str) -> Result<(), String> {
        let _guard = self.write_guard()?;
        let conn = self.open()?;
        let exists: Option<i64> = conn
            .query_row(
                "SELECT 1 FROM session_folder WHERE id = ?1",
                params![folder_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(db_err)?;
        if exists.is_none() {
            return Err("目标文件夹不存在".to_string());
        }
        let local_id = self.resolve_local_id(&conn, session_id)?;
        let changed = conn
            .execute(
                "UPDATE session SET folder_id = ?1 WHERE id = ?2",
                params![folder_id, local_id],
            )
            .map_err(db_err)?;
        if changed == 0 {
            return Err("会话不存在".to_string());
        }
        Ok(())
    }

    pub fn set_folder_collapsed(&self, folder_id: &str, collapsed: bool) -> Result<(), String> {
        let _guard = self.write_guard()?;
        let conn = self.open()?;
        let changed = conn
            .execute(
                "UPDATE session_folder SET collapsed = ?1 WHERE id = ?2",
                params![collapsed as i64, folder_id],
            )
            .map_err(db_err)?;
        if changed == 0 {
            return Err("文件夹不存在".to_string());
        }
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
        let mut conn = self.open()?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_err)?;
        let value_json = serde_json::to_string(value).map_err(|e| e.to_string())?;
        let previous: Option<String> = tx
            .query_row(
                "SELECT value_json FROM setting WHERE key = ?1",
                params![key],
                |row| row.get(0),
            )
            .optional()
            .map_err(db_err)?;
        tx.execute(
            "INSERT INTO setting (key, value_json) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json",
            params![key, value_json],
        )
        .map_err(db_err)?;
        if key == APP_SETTINGS_KEY && previous.as_deref() != Some(value_json.as_str()) {
            bump_permission_policy_version_on_conn(&tx)?;
        }
        tx.commit().map_err(db_err)
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
        let now = now_seconds();
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
        self.add_always_allow_tool_with_outcome(tool)
            .map(|(tools, _)| tools)
    }

    pub fn add_always_allow_tool_with_outcome(
        &self,
        tool: &str,
    ) -> Result<(Vec<String>, bool), String> {
        let _guard = self.write_guard()?;
        let mut conn = self.open()?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_err)?;
        let raw: Option<String> = tx
            .query_row(
                "SELECT value_json FROM setting WHERE key = ?1",
                params![ALWAYS_ALLOW_TOOLS_KEY],
                |row| row.get(0),
            )
            .optional()
            .map_err(db_err)?;
        let mut tools = raw
            .map(|value| serde_json::from_str::<Vec<String>>(&value).map_err(|e| e.to_string()))
            .transpose()?
            .unwrap_or_default();
        let created_by_this_call = !tools.iter().any(|item| item == tool);
        if created_by_this_call {
            tools.push(tool.to_string());
        }
        tx.execute(
            "INSERT INTO setting (key, value_json) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json",
            params![
                ALWAYS_ALLOW_TOOLS_KEY,
                serde_json::to_string(&tools).map_err(|e| e.to_string())?
            ],
        )
        .map_err(db_err)?;
        let rule = legacy_always_allow_rule(tool, now_millis());
        let rule_inserted = tx
            .execute(
                "INSERT INTO permission_rule
             (id, principal, effect, scope, tool_call_id, turn_id, history_session_id, project_root,
              engine, capability, operation, resource_pattern, created_at, expires_at,
              max_uses, uses)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, 0)
             ON CONFLICT(id) DO NOTHING",
                params![
                    rule.id,
                    rule.principal,
                    stable_serde_string(&rule.effect)?,
                    stable_serde_string(&rule.scope)?,
                    rule.scope_binding.tool_call_id,
                    rule.scope_binding.turn_id,
                    rule.scope_binding.session_id,
                    rule.scope_binding.project_root,
                    rule.engine,
                    stable_serde_string(&rule.capability)?,
                    rule.operation,
                    rule.resource_pattern,
                    rule.created_at,
                    rule.expires_at,
                    rule.max_uses.map(i64::from)
                ],
            )
            .map_err(db_err)?;
        if created_by_this_call || rule_inserted > 0 {
            bump_permission_policy_version_on_conn(&tx)?;
        }
        tx.commit().map_err(db_err)?;
        Ok((tools, created_by_this_call))
    }

    pub fn remove_always_allow_tool(&self, tool: &str) -> Result<Vec<String>, String> {
        let _guard = self.write_guard()?;
        let mut conn = self.open()?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_err)?;
        let raw: Option<String> = tx
            .query_row(
                "SELECT value_json FROM setting WHERE key = ?1",
                params![ALWAYS_ALLOW_TOOLS_KEY],
                |row| row.get(0),
            )
            .optional()
            .map_err(db_err)?;
        let mut tools = raw
            .map(|value| serde_json::from_str::<Vec<String>>(&value).map_err(|e| e.to_string()))
            .transpose()?
            .unwrap_or_default();
        let before = tools.len();
        tools.retain(|item| item != tool);
        tx.execute(
            "INSERT INTO setting (key, value_json) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json",
            params![
                ALWAYS_ALLOW_TOOLS_KEY,
                serde_json::to_string(&tools).map_err(|e| e.to_string())?
            ],
        )
        .map_err(db_err)?;
        let rule_id = legacy_always_allow_rule(tool, 0).id;
        let rule_removed = tx
            .execute(
                "DELETE FROM permission_rule WHERE id = ?1",
                params![rule_id],
            )
            .map_err(db_err)?;
        if tools.len() != before || rule_removed > 0 {
            bump_permission_policy_version_on_conn(&tx)?;
        }
        tx.commit().map_err(db_err)?;
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
        turn_id: &str,
        restorable: bool,
        file_count: u64,
        reason: Option<&str>,
    ) -> Result<(), String> {
        let _guard = self.write_guard()?;
        let conn = self.open()?;
        let local_id = self.resolve_local_id(&conn, session_id)?;
        let turn_idx = conn
            .query_row(
                "SELECT turn_epoch FROM turn WHERE history_session_id = ?1 AND turn_id = ?2",
                params![local_id, turn_id],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(db_err)?
            .unwrap_or(turn_idx);
        conn.execute(
            "INSERT INTO checkpoint
             (id, session_id, turn_idx, label, snapshot_ref, ts, turn_id, restorable, file_count, restorable_reason)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                checkpoint_id,
                local_id,
                turn_idx,
                label,
                snapshot_ref,
                ts,
                turn_id,
                i64::from(restorable),
                file_count as i64,
                reason,
            ],
        )
        .map_err(db_err)?;
        Ok(())
    }

    pub fn get_checkpoint(&self, checkpoint_id: &str) -> Result<Option<CheckpointRecord>, String> {
        let conn = self.open()?;
        let result: Option<CheckpointRecord> = conn
            .query_row(
                "SELECT id, session_id, turn_idx, label, snapshot_ref, ts, turn_id,
                        restorable, file_count, restorable_reason
                 FROM checkpoint WHERE id = ?1",
                params![checkpoint_id],
                |row| {
                    Ok(CheckpointRecord {
                        id: row.get(0)?,
                        session_id: row.get(1)?,
                        turn_idx: row.get(2)?,
                        label: row.get(3)?,
                        snapshot_ref: row.get(4)?,
                        ts: row.get(5)?,
                        turn_id: row.get(6)?,
                        restorable: row.get::<_, i64>(7)? != 0,
                        file_count: row.get::<_, i64>(8)?.max(0) as u64,
                        reason: row.get(9)?,
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

    pub fn native_resume_profile_matches(
        &self,
        session_id: &str,
        native_id: &str,
        launch_profile_ref: &str,
        launch_profile_digest: &str,
    ) -> Result<bool, String> {
        let conn = self.open()?;
        let matched: i64 = conn
            .query_row(
                "SELECT EXISTS(
                   SELECT 1
                   FROM native_session_ref native
                   JOIN runtime_generation generation ON generation.id = native.generation_id
                   WHERE native.owner_kind = 'session'
                     AND native.owner_id = ?1
                     AND native.native_id = ?2
                     AND native.invalidated_at IS NULL
                     AND generation.provider_launch_profile_ref = ?3
                     AND generation.provider_launch_profile_digest = ?4
                     AND generation.capability_snapshot_id IS NOT NULL
                 )",
                params![
                    session_id,
                    native_id,
                    launch_profile_ref,
                    launch_profile_digest
                ],
                |row| row.get(0),
            )
            .map_err(db_err)?;
        Ok(matched != 0)
    }

    // 用量统计查询
    pub fn get_usage_stats(&self, days: u32) -> Result<UsageStats, String> {
        let conn = self.open()?;
        let now = now_seconds();
        let cutoff_ts = now - (days as i64 * 86400);
        let previous_cutoff_ts = cutoff_ts - (days as i64 * 86400);

        let mut stmt = conn
            .prepare(
                "SELECT
                    COALESCE(SUM(cost_usd), 0.0),
                    COALESCE(SUM(input_tokens), 0),
                    COALESCE(SUM(output_tokens), 0),
                    COUNT(DISTINCT id),
                    COALESCE(SUM(CASE WHEN cost_kind = 'actual' THEN cost_usd ELSE 0 END), 0.0),
                    COALESCE(SUM(CASE WHEN cost_kind = 'estimated' THEN cost_usd ELSE 0 END), 0.0),
                    COALESCE(SUM(CASE WHEN cost_kind = 'subscription' THEN 1 ELSE 0 END), 0),
                    COALESCE(SUM(CASE WHEN cost_kind = 'unknown' THEN 1 ELSE 0 END), 0),
                    COALESCE(SUM(CASE WHEN cost_kind = 'legacy' THEN cost_usd ELSE 0 END), 0.0),
                    COALESCE(SUM(CASE WHEN cost_kind = 'legacy' THEN 1 ELSE 0 END), 0)
                 FROM usage
                 WHERE ts >= ?1 AND ts <= ?2",
            )
            .map_err(db_err)?;

        let (
            total_cost,
            input_tokens,
            output_tokens,
            request_count,
            actual_cost,
            estimated_cost,
            subscription_count,
            unknown_count,
            legacy_cost,
            legacy_count,
        ): (f64, i64, i64, i64, f64, f64, i64, i64, f64, i64) = stmt
            .query_row(params![cutoff_ts, now], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                    row.get(9)?,
                ))
            })
            .map_err(db_err)?;

        let session_count: i64 = conn
            .query_row(
                "SELECT COUNT(DISTINCT session_id) FROM usage WHERE ts >= ?1 AND ts <= ?2",
                params![cutoff_ts, now],
                |row| row.get(0),
            )
            .map_err(db_err)?;

        let (previous_total_cost, previous_input_tokens, previous_output_tokens, previous_request_count): (f64, i64, i64, i64) = conn
            .query_row(
                "SELECT COALESCE(SUM(cost_usd), 0.0), COALESCE(SUM(input_tokens), 0), COALESCE(SUM(output_tokens), 0), COUNT(DISTINCT id)
                 FROM usage WHERE ts >= ?1 AND ts < ?2",
                params![previous_cutoff_ts, cutoff_ts],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .map_err(db_err)?;
        let previous_session_count: i64 = conn
            .query_row(
                "SELECT COUNT(DISTINCT session_id) FROM usage WHERE ts >= ?1 AND ts < ?2",
                params![previous_cutoff_ts, cutoff_ts],
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
            actual_cost,
            estimated_cost,
            subscription_count: subscription_count as u32,
            unknown_count: unknown_count as u32,
            legacy_cost,
            legacy_count: legacy_count as u32,
            previous_total_cost,
            previous_total_tokens: (previous_input_tokens + previous_output_tokens) as u64,
            previous_request_count: previous_request_count as u32,
            previous_session_count: previous_session_count as u32,
        })
    }

    pub fn get_usage_by_model(&self, days: u32) -> Result<Vec<ModelUsage>, String> {
        let conn = self.open()?;
        let now = now_seconds();
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

    /// 按服务商聚合用量（P3-6）：优先使用 usage 写入时固化的 provider_id，
    /// v12 以前的老记录回退到 session.provider_id；两者都为空时归入空 key。
    pub fn get_usage_by_provider(&self, days: u32) -> Result<Vec<ProviderUsage>, String> {
        let conn = self.open()?;
        let now = now_seconds();
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
                    COALESCE(NULLIF(u.provider_id, ''), s.provider_id, '') as resolved_provider_id,
                    SUM(u.cost_usd) as cost_usd
                 FROM usage u
                 LEFT JOIN session s ON u.session_id = s.id
                 WHERE u.ts >= ?1
                 GROUP BY COALESCE(NULLIF(u.provider_id, ''), s.provider_id, '')
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
        bump_permission_policy_version_on_conn(&conn)?;
        if let Ok(mut providers) = self.session_providers.lock() {
            providers.insert(session_id.to_string(), provider_id.to_string());
            providers.insert(local_id, provider_id.to_string());
        }
        Ok(())
    }

    pub fn set_session_route_projection(
        &self,
        session_id: &str,
        provider_id: &str,
        model_id: &str,
    ) -> Result<(), String> {
        let _guard = self.write_guard()?;
        let conn = self.open()?;
        let local_id = self.resolve_local_id(&conn, session_id)?;
        let changed = conn
            .execute(
                "UPDATE session SET provider_id = ?1, model = ?2 WHERE id = ?3",
                params![provider_id, model_id, local_id],
            )
            .map_err(db_err)?;
        if changed != 1 {
            return Err("更新 Session 路由兼容投影失败".to_string());
        }
        let mut providers = self
            .session_providers
            .lock()
            .map_err(|_| "会话服务商缓存锁中毒".to_string())?;
        providers.insert(session_id.to_string(), provider_id.to_string());
        providers.insert(local_id, provider_id.to_string());
        Ok(())
    }

    pub fn set_session_turn_preference(
        &self,
        session_id: &str,
        model_id: &str,
        reasoning_effort: Option<&str>,
    ) -> Result<(), String> {
        let model_id = model_id.trim();
        if model_id.is_empty() {
            return Err("下一轮模型偏好不能为空".to_string());
        }
        if let Some(effort) = reasoning_effort {
            crate::reasoning::ReasoningEffort::parse(Some(effort))?;
        }
        let _guard = self.write_guard()?;
        let conn = self.open()?;
        let local_id = self.resolve_local_id(&conn, session_id)?;
        let changed = conn
            .execute(
                "UPDATE session
                 SET preferred_model = ?1, preferred_reasoning_effort = ?2, updated_at = ?3
                 WHERE id = ?4",
                params![model_id, reasoning_effort, now_millis(), local_id],
            )
            .map_err(db_err)?;
        if changed != 1 {
            return Err("更新 Session 下一轮偏好失败".to_string());
        }
        Ok(())
    }

    /// 会话记录的服务商 id（P3-5 起标题时定位计费方用）
    pub fn session_provider_id(&self, session_id: &str) -> Result<String, String> {
        if let Ok(providers) = self.session_providers.lock() {
            if let Some(provider_id) = providers.get(session_id) {
                return Ok(provider_id.clone());
            }
        }
        let conn = self.open()?;
        let local_id = self.resolve_local_id(&conn, session_id)?;
        let provider_id: String = conn
            .query_row(
                "SELECT COALESCE(provider_id, '') FROM session WHERE id = ?1",
                params![local_id],
                |row| row.get(0),
            )
            .map_err(db_err)?;
        if let Ok(mut providers) = self.session_providers.lock() {
            providers.insert(session_id.to_string(), provider_id.clone());
            providers.insert(local_id, provider_id.clone());
        }
        Ok(provider_id)
    }

    /// 读取会话的 safe permission profile（不含触发 refresh，审批热路径用）。
    fn session_safe_profile_on_conn(
        &self,
        conn: &Connection,
        session_id: &str,
    ) -> Result<String, String> {
        let local_id = self.resolve_local_id(conn, session_id)?;
        conn.query_row(
            "SELECT safe_permission_profile FROM session WHERE id = ?1",
            params![local_id],
            |row| row.get::<_, String>(0),
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
        let now = now_seconds();
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
        let now = now_seconds();
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
        let now = now_seconds();

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

    pub fn load_capability_snapshot(
        &self,
        cache_key: &str,
    ) -> Result<Option<crate::capability_registry::EngineCapabilitySnapshot>, String> {
        let conn = self.open()?;
        let snapshot_json = conn
            .query_row(
                "SELECT snapshot_json FROM capability_snapshot WHERE cache_key = ?1",
                params![cache_key],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(db_err)?;
        let Some(snapshot_json) = snapshot_json else {
            return Ok(None);
        };
        let snapshot: crate::capability_registry::EngineCapabilitySnapshot =
            serde_json::from_str(&snapshot_json)
                .map_err(|error| format!("CapabilitySnapshot 缓存损坏：{error}"))?;
        if snapshot.identity.cache_key()? != cache_key {
            return Err("CapabilitySnapshot 缓存身份不匹配".to_string());
        }
        Ok(Some(snapshot))
    }

    pub fn save_capability_snapshot(
        &self,
        cache_key: &str,
        snapshot: &crate::capability_registry::EngineCapabilitySnapshot,
    ) -> Result<(), String> {
        if snapshot.identity.cache_key()? != cache_key {
            return Err("拒绝保存身份不匹配的 CapabilitySnapshot".to_string());
        }
        let identity_json = serde_json::to_string(&snapshot.identity)
            .map_err(|error| format!("序列化 Capability identity 失败：{error}"))?;
        let snapshot_json = serde_json::to_string(snapshot)
            .map_err(|error| format!("序列化 CapabilitySnapshot 失败：{error}"))?;
        let _guard = self.write_guard()?;
        let conn = self.open()?;
        conn.execute(
            "INSERT OR IGNORE INTO capability_snapshot
             (id, cache_key, engine_id, model_capability_key, identity_json, snapshot_json, probed_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                snapshot.id,
                cache_key,
                snapshot.identity.engine_id,
                snapshot.identity.model_capability_key,
                identity_json,
                snapshot_json,
                snapshot.probed_at,
            ],
        )
        .map_err(db_err)?;
        Ok(())
    }

    pub fn update_capability_snapshot(
        &self,
        cache_key: &str,
        snapshot: &crate::capability_registry::EngineCapabilitySnapshot,
    ) -> Result<(), String> {
        if snapshot.identity.cache_key()? != cache_key {
            return Err("拒绝更新身份不匹配的 CapabilitySnapshot".to_string());
        }
        let snapshot_json = serde_json::to_string(snapshot)
            .map_err(|error| format!("序列化 CapabilitySnapshot 失败：{error}"))?;
        let _guard = self.write_guard()?;
        let conn = self.open()?;
        let updated = conn
            .execute(
                "UPDATE capability_snapshot SET snapshot_json = ?1, probed_at = ?2
                 WHERE cache_key = ?3 AND id = ?4",
                params![snapshot_json, snapshot.probed_at, cache_key, snapshot.id],
            )
            .map_err(db_err)?;
        if updated != 1 {
            return Err("CapabilitySnapshot 更新目标不存在".to_string());
        }
        Ok(())
    }

    pub fn create_runtime_generation(
        &self,
        generation: &crate::runtime_registry::RuntimeGeneration,
    ) -> Result<(), String> {
        let _guard = self.write_guard()?;
        let mut conn = self.open()?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_err)?;
        if generation.owner.kind() == "session" {
            let exists: i64 = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM session WHERE id = ?1)",
                    params![generation.owner.id()],
                    |row| row.get(0),
                )
                .map_err(db_err)?;
            if exists == 0 {
                return Err("RuntimeGeneration 引用了不存在的 Session owner".to_string());
            }
        }
        tx.execute(
            "UPDATE runtime_generation
             SET status = 'lost_on_restart', ended_at = ?1
             WHERE owner_kind = ?2 AND owner_id = ?3 AND status = 'active'",
            params![
                generation.created_at,
                generation.owner.kind(),
                generation.owner.id()
            ],
        )
        .map_err(db_err)?;
        tx.execute(
            "INSERT INTO runtime_generation
             (id, owner_kind, owner_id, engine_id, compatibility_key,
              engine_profile_digest, provider_launch_profile_ref, provider_launch_profile_digest,
              capability_snapshot_id, canonical_cwd, status, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 'active', ?11)",
            params![
                generation.id,
                generation.owner.kind(),
                generation.owner.id(),
                generation.engine_id,
                generation.compatibility_key,
                generation.engine_profile_digest,
                generation.provider_launch_profile_ref,
                generation.provider_launch_profile_digest,
                generation.capability_snapshot_id,
                generation.canonical_cwd,
                generation.created_at,
            ],
        )
        .map_err(db_err)?;
        tx.commit().map_err(db_err)
    }

    pub fn close_runtime_generation(
        &self,
        generation_id: &str,
        status: &str,
        ended_at: i64,
    ) -> Result<(), String> {
        let _guard = self.write_guard()?;
        let conn = self.open()?;
        let updated = conn
            .execute(
                "UPDATE runtime_generation SET status = ?1, ended_at = ?2
                 WHERE id = ?3 AND status = 'active'",
                params![status, ended_at, generation_id],
            )
            .map_err(db_err)?;
        if updated > 1 {
            return Err("RuntimeGeneration 终止更新影响了多行".to_string());
        }
        Ok(())
    }

    pub fn rotate_runtime_generation(
        &self,
        previous_generation_id: &str,
        generation: &crate::runtime_registry::RuntimeGeneration,
    ) -> Result<(), String> {
        let _guard = self.write_guard()?;
        let mut conn = self.open()?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_err)?;
        let changed = tx
            .execute(
                "UPDATE runtime_generation
                 SET status = ?1, ended_at = ?2
                 WHERE id = ?3 AND owner_kind = ?4 AND owner_id = ?5 AND status = 'active'",
                params![
                    "closed",
                    generation.created_at,
                    previous_generation_id,
                    generation.owner.kind(),
                    generation.owner.id(),
                ],
            )
            .map_err(db_err)?;
        if changed != 1 {
            return Err("待轮换的 RuntimeGeneration 已失效或 owner 不匹配".to_string());
        }
        tx.execute(
            "INSERT INTO runtime_generation
             (id, owner_kind, owner_id, engine_id, compatibility_key,
              engine_profile_digest, provider_launch_profile_ref, provider_launch_profile_digest,
              capability_snapshot_id, canonical_cwd, status, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 'active', ?11)",
            params![
                generation.id,
                generation.owner.kind(),
                generation.owner.id(),
                generation.engine_id,
                generation.compatibility_key,
                generation.engine_profile_digest,
                generation.provider_launch_profile_ref,
                generation.provider_launch_profile_digest,
                generation.capability_snapshot_id,
                generation.canonical_cwd,
                generation.created_at,
            ],
        )
        .map_err(db_err)?;
        tx.commit().map_err(db_err)
    }

    pub fn create_turn_attempt(
        &self,
        spec: &TurnExecutionSpec,
        generation: &crate::runtime_registry::RuntimeGeneration,
        input_native_id: Option<&str>,
    ) -> Result<crate::runtime_registry::TurnAttempt, String> {
        let _guard = self.write_guard()?;
        let mut conn = self.open()?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_err)?;
        let generation_facts: (String, String, String, String) = tx
            .query_row(
                "SELECT owner_kind, owner_id, compatibility_key, status
                 FROM runtime_generation WHERE id = ?1",
                params![generation.id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .map_err(db_err)?;
        if generation_facts.0 != "session"
            || generation_facts.1 != spec.history_session_id
            || generation_facts.2 != generation.compatibility_key
            || generation_facts.3 != "active"
        {
            return Err("TurnAttempt 的 RuntimeGeneration owner 或状态不匹配".to_string());
        }
        let attempt_no: u64 = tx
            .query_row(
                "SELECT COALESCE(MAX(attempt_no), 0) + 1 FROM turn_attempt WHERE turn_id = ?1",
                params![spec.turn_id],
                |row| row.get(0),
            )
            .map_err(db_err)?;
        let input_native_ref_id = input_native_id
            .map(|native_id| attach_native_ref(&tx, generation, native_id, spec.created_at))
            .transpose()?;
        tx.execute(
            "INSERT INTO turn_attempt
             (turn_id, attempt_no, owner_kind, owner_id, generation_id,
              runtime_compatibility_key, input_native_ref_id, delivery_state, created_at)
             VALUES (?1, ?2, 'session', ?3, ?4, ?5, ?6, 'prepared', ?7)",
            params![
                spec.turn_id,
                attempt_no,
                spec.history_session_id,
                generation.id,
                generation.compatibility_key,
                input_native_ref_id,
                spec.created_at,
            ],
        )
        .map_err(db_err)?;
        tx.commit().map_err(db_err)?;
        Ok(crate::runtime_registry::TurnAttempt {
            turn_id: spec.turn_id.clone(),
            attempt_no,
            owner: generation.owner.clone(),
            generation_id: generation.id.clone(),
            runtime_compatibility_key: generation.compatibility_key.clone(),
            input_native_ref_id,
            output_native_ref_id: None,
            observed_model_id: None,
            observed_reasoning_effort: None,
            actual_capability_snapshot: None,
            delivery_state: "prepared".to_string(),
            terminal_receipt: None,
            created_at: spec.created_at,
            accepted_at: None,
            ended_at: None,
        })
    }

    pub fn mark_turn_attempt_accepted(
        &self,
        turn_id: &str,
        attempt_no: u64,
        accepted_at: i64,
    ) -> Result<(), String> {
        let _guard = self.write_guard()?;
        let conn = self.open()?;
        let updated = conn
            .execute(
                "UPDATE turn_attempt SET delivery_state = 'accepted', accepted_at = ?1
                 WHERE turn_id = ?2 AND attempt_no = ?3 AND delivery_state = 'prepared'",
                params![accepted_at, turn_id, attempt_no],
            )
            .map_err(db_err)?;
        if updated != 1 {
            return Err("TurnAttempt 接受回执状态转换无效".to_string());
        }
        Ok(())
    }

    pub fn observe_latest_turn_attempt(
        &self,
        turn_id: &str,
        native_id: &str,
        engine: EngineId,
        observed_model: &str,
        capabilities: Option<&crate::protocol::RuntimeCapabilitySnapshot>,
        observed_at: i64,
    ) -> Result<(), String> {
        let _guard = self.write_guard()?;
        let mut conn = self.open()?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_err)?;
        let (attempt_no, generation_id, owner_kind, owner_id, routed_model): (
            u64,
            String,
            String,
            String,
            String,
        ) = tx
            .query_row(
                "SELECT a.attempt_no, a.generation_id, a.owner_kind, a.owner_id,
                        s.routed_model_id
                 FROM turn_attempt a
                 JOIN turn_execution_spec s ON s.turn_id = a.turn_id
                 WHERE a.turn_id = ?1 ORDER BY a.attempt_no DESC LIMIT 1",
                params![turn_id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .map_err(db_err)?;
        if !observed_model_matches(engine, &routed_model, observed_model) {
            tx.execute(
                "UPDATE turn_attempt
                 SET observed_model_id = ?1, delivery_state = 'error',
                     terminal_receipt = '[runtime_model_mismatch]', ended_at = ?2
                 WHERE turn_id = ?3 AND attempt_no = ?4",
                params![observed_model, observed_at, turn_id, attempt_no],
            )
            .map_err(db_err)?;
            tx.commit().map_err(db_err)?;
            return Err(format!(
                "[runtime_model_mismatch] routed model {routed_model}，Runtime 报告 {observed_model}"
            ));
        }
        let generation = runtime_generation_from_conn(&tx, &generation_id)?;
        if generation.owner.kind() != owner_kind || generation.owner.id() != owner_id {
            return Err("NativeSessionRef owner 与 TurnAttempt 不匹配".to_string());
        }
        let output_native_ref_id = attach_native_ref(&tx, &generation, native_id, observed_at)?;
        let capability_json = capabilities
            .map(serde_json::to_string)
            .transpose()
            .map_err(|error| format!("序列化 Runtime capability 失败：{error}"))?;
        tx.execute(
            "UPDATE turn_attempt
             SET output_native_ref_id = ?1, observed_model_id = ?2,
                 actual_capability_snapshot_json = ?3,
                 delivery_state = 'accepted', accepted_at = COALESCE(accepted_at, ?4)
             WHERE turn_id = ?5 AND attempt_no = ?6
               AND delivery_state IN ('prepared', 'accepted')",
            params![
                output_native_ref_id,
                observed_model,
                capability_json,
                observed_at,
                turn_id,
                attempt_no
            ],
        )
        .map_err(db_err)?;
        tx.commit().map_err(db_err)
    }

    pub fn finish_latest_turn_attempt(
        &self,
        turn_id: &str,
        state: &str,
        receipt: Option<&str>,
        ended_at: i64,
    ) -> Result<(), String> {
        let conn = self.open()?;
        let attempt_no: Option<u64> = conn
            .query_row(
                "SELECT MAX(attempt_no) FROM turn_attempt WHERE turn_id = ?1",
                params![turn_id],
                |row| row.get(0),
            )
            .map_err(db_err)?;
        if let Some(attempt_no) = attempt_no {
            self.finish_turn_attempt(turn_id, attempt_no, state, receipt, ended_at)
        } else {
            Ok(())
        }
    }

    pub fn finish_turn_attempt(
        &self,
        turn_id: &str,
        attempt_no: u64,
        state: &str,
        receipt: Option<&str>,
        ended_at: i64,
    ) -> Result<(), String> {
        if !matches!(
            state,
            "rejected" | "completed" | "interrupted" | "error" | "delivery_unknown"
        ) {
            return Err(format!("未知 TurnAttempt 终态：{state}"));
        }
        let receipt = receipt.map(|value| {
            let redacted = crate::redaction::redact_text(value);
            redacted.chars().take(4096).collect::<String>()
        });
        let _guard = self.write_guard()?;
        let conn = self.open()?;
        conn.execute(
            "UPDATE turn_attempt
             SET delivery_state = ?1, terminal_receipt = ?2, ended_at = ?3
             WHERE turn_id = ?4 AND attempt_no = ?5
               AND delivery_state IN ('prepared', 'accepted')",
            params![state, receipt, ended_at, turn_id, attempt_no],
        )
        .map_err(db_err)?;
        Ok(())
    }

    pub fn load_turn_recovery_inputs(
        &self,
    ) -> Result<Vec<crate::runtime_registry::TurnRecoveryInput>, String> {
        let conn = self.open()?;
        let mut stmt = conn
            .prepare(
                "SELECT turn_id, attempt_no, owner_kind, owner_id, generation_id,
                        delivery_state, input_native_ref_id, output_native_ref_id
                 FROM turn_attempt
                 WHERE delivery_state IN ('prepared', 'accepted', 'delivery_unknown')
                 ORDER BY created_at ASC, turn_id ASC, attempt_no ASC",
            )
            .map_err(db_err)?;
        let rows = stmt
            .query_map([], |row| {
                let kind: String = row.get(2)?;
                let id: String = row.get(3)?;
                let owner = if kind == "session" {
                    crate::runtime_registry::RuntimeOwnerRef::Session(id)
                } else {
                    crate::runtime_registry::RuntimeOwnerRef::Operation(id)
                };
                Ok(crate::runtime_registry::TurnRecoveryInput {
                    turn_id: row.get(0)?,
                    attempt_no: row.get(1)?,
                    owner,
                    generation_id: row.get(4)?,
                    delivery_state: row.get(5)?,
                    input_native_ref_id: row.get(6)?,
                    output_native_ref_id: row.get(7)?,
                })
            })
            .map_err(db_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_err)?;
        Ok(rows)
    }

    /// Startup reconciliation for v25. A restarted process cannot reconnect to
    /// an in-memory RuntimeGeneration, so accepted work is never replayed.
    pub fn reconcile_stream_recovery(&self) -> Result<StreamRecoveryReport, String> {
        retry_locked(|| {
            let _guard = self.write_guard()?;
            let mut conn = self.open()?;
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(db_err)?;
            let now = now_millis();
            let runtime_generations_lost = tx
                .execute(
                    "UPDATE runtime_generation
                     SET status = 'lost_on_restart', ended_at = ?1
                     WHERE status = 'active'",
                    params![now],
                )
                .map_err(db_err)? as u64;
            let pending = {
                let mut stmt = tx
                    .prepare(
                        "SELECT a.turn_id, a.attempt_no, a.owner_id, a.generation_id,
                                a.delivery_state, t.turn_epoch, t.turn_mode, t.permission_profile,
                                t.started_at, COALESCE(s.event_seq, 0), COALESCE(s.status, t.status)
                         FROM turn_attempt a
                         JOIN turn t ON t.turn_id = a.turn_id AND t.history_session_id = a.owner_id
                         LEFT JOIN turn_snapshot s
                           ON s.history_session_id = a.owner_id AND s.turn_id = a.turn_id
                         WHERE a.delivery_state IN ('prepared', 'accepted')
                            OR (a.delivery_state = 'delivery_unknown'
                                AND COALESCE(s.recovery_state, 'none') != 'delivery_unknown')
                         ORDER BY a.created_at ASC, a.turn_id ASC, a.attempt_no ASC",
                    )
                    .map_err(db_err)?;
                let rows = stmt
                    .query_map([], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, u64>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, String>(4)?,
                            row.get::<_, u64>(5)?,
                            row.get::<_, String>(6)?,
                            row.get::<_, String>(7)?,
                            row.get::<_, i64>(8)?,
                            row.get::<_, u64>(9)?,
                            row.get::<_, String>(10)?,
                        ))
                    })
                    .map_err(db_err)?;
                let values = rows.collect::<Result<Vec<_>, _>>().map_err(db_err)?;
                values
            };
            let mut report = StreamRecoveryReport {
                runtime_generations_lost,
                ..Default::default()
            };
            for (
                turn_id,
                attempt_no,
                session_id,
                generation_id,
                delivery_state,
                turn_epoch,
                turn_mode,
                permission_profile,
                started_at,
                previous_event_seq,
                snapshot_status,
            ) in pending
            {
                let waiting_approval = snapshot_status == "waiting_approval";
                let approval_not_executed: bool = if waiting_approval {
                    tx.query_row(
                        "SELECT EXISTS(
                           SELECT 1 FROM approval p
                           WHERE p.session_id = ?1 AND p.turn_id = ?2
                             AND p.status IN ('pending', 'applying')
                         ) AND NOT EXISTS(
                           SELECT 1 FROM approval p
                           JOIN tool_call c
                             ON c.session_id = p.session_id AND c.turn_id = p.turn_id AND c.id = p.id
                           WHERE p.session_id = ?1 AND p.turn_id = ?2
                             AND p.status IN ('pending', 'applying')
                             AND (c.result_count > 0 OR c.status IN ('success', 'error'))
                         )",
                        params![session_id, turn_id],
                        |row| row.get(0),
                    )
                    .map_err(db_err)?
                } else {
                    false
                };
                let (attempt_state, turn_status, recovery_state, reason) =
                    if delivery_state == "prepared" {
                        report.prepared_interrupted += 1;
                        (
                            "rejected",
                            crate::turn_supervisor::TurnStatus::Interrupted,
                            "safe_to_retry",
                            "[recovery_not_accepted] Runtime 未接受本次投递，可由用户重试",
                        )
                    } else if delivery_state == "accepted" && approval_not_executed {
                        report.approval_interrupted += 1;
                        (
                            "interrupted",
                            crate::turn_supervisor::TurnStatus::Interrupted,
                            "approval_runtime_lost",
                            "[recovery_approval_runtime_lost] 原 Runtime 已丢失，旧审批已转为只读",
                        )
                    } else {
                        report.delivery_unknown += 1;
                        (
                            "delivery_unknown",
                            crate::turn_supervisor::TurnStatus::Failed,
                            "delivery_unknown",
                            "[delivery_unknown] Runtime 已接受投递但终态未知，禁止自动重放",
                        )
                    };
                let changed = tx
                    .execute(
                        "UPDATE turn_attempt
                         SET delivery_state = ?1, terminal_receipt = ?2, ended_at = ?3
                         WHERE turn_id = ?4 AND attempt_no = ?5 AND generation_id = ?6
                           AND delivery_state IN ('prepared', 'accepted', 'delivery_unknown')",
                        params![
                            attempt_state,
                            reason,
                            now,
                            turn_id,
                            i64::try_from(attempt_no).unwrap_or(i64::MAX),
                            generation_id,
                        ],
                    )
                    .map_err(db_err)?;
                if changed != 1 {
                    return Err("启动恢复期间 TurnAttempt CAS 失败".to_string());
                }
                let event_seq = previous_event_seq.saturating_add(1);
                tx.execute(
                    "INSERT INTO turn_snapshot
                     (history_session_id, turn_id, turn_epoch, status, terminal_reason,
                      recoverable, event_seq, updated_at, turn_mode, permission_profile, started_at,
                      attempt_no, runtime_generation_id, recovery_state)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
                     ON CONFLICT(history_session_id) DO UPDATE SET
                       turn_id = excluded.turn_id,
                       turn_epoch = excluded.turn_epoch,
                       status = excluded.status,
                       terminal_reason = excluded.terminal_reason,
                       recoverable = excluded.recoverable,
                       event_seq = excluded.event_seq,
                       updated_at = excluded.updated_at,
                       turn_mode = excluded.turn_mode,
                       permission_profile = excluded.permission_profile,
                       started_at = excluded.started_at,
                       attempt_no = excluded.attempt_no,
                       runtime_generation_id = excluded.runtime_generation_id,
                       recovery_state = excluded.recovery_state",
                    params![
                        session_id,
                        turn_id,
                        i64::try_from(turn_epoch).unwrap_or(i64::MAX),
                        turn_status_to_str(turn_status),
                        reason,
                        i64::from(turn_status == crate::turn_supervisor::TurnStatus::Interrupted),
                        i64::try_from(event_seq).unwrap_or(i64::MAX),
                        now,
                        turn_mode,
                        permission_profile,
                        started_at,
                        i64::try_from(attempt_no).unwrap_or(i64::MAX),
                        generation_id,
                        recovery_state,
                    ],
                )
                .map_err(db_err)?;
                tx.execute(
                    "UPDATE turn SET status = ?1, ended_at = ?2, terminal_reason = ?3
                     WHERE history_session_id = ?4 AND turn_id = ?5
                       AND status NOT IN ('succeeded', 'failed', 'interrupted')",
                    params![
                        turn_status_to_str(turn_status),
                        now,
                        reason,
                        session_id,
                        turn_id,
                    ],
                )
                .map_err(db_err)?;
                finalize_turn_artifacts(&tx, &session_id, &turn_id, turn_status, now)
                    .map_err(db_err)?;
                tx.execute(
                    "INSERT OR IGNORE INTO stream_boundary_event
                     (turn_id, attempt_no, event_seq, history_session_id, runtime_generation_id,
                      event_kind, disposition, event_digest, observed_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, 'startup_recovery', 'accepted', ?6, ?7)",
                    params![
                        turn_id,
                        i64::try_from(attempt_no).unwrap_or(i64::MAX),
                        i64::try_from(event_seq).unwrap_or(i64::MAX),
                        session_id,
                        generation_id,
                        recovery_state,
                        now,
                    ],
                )
                .map_err(db_err)?;
            }
            tx.commit().map_err(db_err)?;
            Ok(report)
        })
    }
}

fn attach_native_ref(
    tx: &Transaction<'_>,
    generation: &crate::runtime_registry::RuntimeGeneration,
    native_id: &str,
    created_at: i64,
) -> Result<String, String> {
    if let Some(existing) = tx
        .query_row(
            "SELECT id FROM native_session_ref WHERE generation_id = ?1 AND native_id = ?2",
            params![generation.id, native_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(db_err)?
    {
        return Ok(existing);
    }
    let id = format!("native-ref-{:032x}", rand::random::<u128>());
    let native_kind = if generation.engine_id == "codex" {
        "codex_thread_id"
    } else {
        "claude_session_id"
    };
    tx.execute(
        "INSERT INTO native_session_ref
         (id, generation_id, owner_kind, owner_id, engine_id, native_kind,
          native_id, launch_profile_identity, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            id,
            generation.id,
            generation.owner.kind(),
            generation.owner.id(),
            generation.engine_id,
            native_kind,
            native_id,
            generation.provider_launch_profile_ref,
            created_at,
        ],
    )
    .map_err(db_err)?;
    Ok(id)
}

fn runtime_generation_from_conn(
    conn: &Connection,
    generation_id: &str,
) -> Result<crate::runtime_registry::RuntimeGeneration, String> {
    conn.query_row(
        "SELECT id, owner_kind, owner_id, engine_id, compatibility_key,
                engine_profile_digest, provider_launch_profile_ref, provider_launch_profile_digest,
                COALESCE(capability_snapshot_id, 'legacy_unbound'), canonical_cwd, created_at
         FROM runtime_generation WHERE id = ?1",
        params![generation_id],
        |row| {
            let owner_kind: String = row.get(1)?;
            let owner_id: String = row.get(2)?;
            Ok(crate::runtime_registry::RuntimeGeneration {
                id: row.get(0)?,
                owner: if owner_kind == "session" {
                    crate::runtime_registry::RuntimeOwnerRef::Session(owner_id)
                } else {
                    crate::runtime_registry::RuntimeOwnerRef::Operation(owner_id)
                },
                engine_id: row.get(3)?,
                compatibility_key: row.get(4)?,
                engine_profile_digest: row.get(5)?,
                provider_launch_profile_ref: row.get(6)?,
                provider_launch_profile_digest: row.get(7)?,
                capability_snapshot_id: row.get(8)?,
                canonical_cwd: row.get(9)?,
                created_at: row.get(10)?,
            })
        },
    )
    .map_err(db_err)
}

fn observed_model_matches(engine: EngineId, routed: &str, observed: &str) -> bool {
    if routed == observed {
        return true;
    }
    matches!(engine, EngineId::ClaudeCode)
        && matches!(routed, "default" | "best" | "sonnet" | "opus" | "haiku")
}

#[derive(Debug, Clone)]
pub struct CheckpointRecord {
    pub id: String,
    pub session_id: String,
    pub turn_idx: i64,
    pub label: String,
    pub snapshot_ref: String,
    pub ts: i64,
    pub turn_id: Option<String>,
    pub restorable: bool,
    pub file_count: u64,
    pub reason: Option<String>,
}

fn ensure_context_mutation_allowed(conn: &Connection, session_id: &str) -> Result<(), String> {
    let active: i64 = conn
        .query_row(
            "SELECT EXISTS(
               SELECT 1 FROM turn
               WHERE history_session_id = ?1
                 AND status IN ('committed', 'running', 'waiting_approval', 'stalled')
             )",
            params![session_id],
            |row| row.get(0),
        )
        .map_err(db_err)?;
    if active != 0 {
        return Err("轮次运行或等待审批期间不能修改会话上下文".to_string());
    }
    Ok(())
}

fn verify_frozen_context_set(
    tx: &Transaction<'_>,
    session_id: &str,
    frozen: &[FrozenSessionContext],
) -> Result<(), String> {
    let cwd: String = tx
        .query_row(
            "SELECT cwd FROM session WHERE id = ?1",
            params![session_id],
            |row| row.get(0),
        )
        .map_err(db_err)?;
    let mut stmt = tx
        .prepare(
            "SELECT id, kind, source_path, canonical_path, status
             FROM session_context WHERE session_id = ?1 ORDER BY created_at ASC, id ASC",
        )
        .map_err(db_err)?;
    let current = stmt
        .query_map(params![session_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
            ))
        })
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)?;
    if current.len() != frozen.len() {
        return Err("会话上下文已在 TurnStart 冻结期间变化，请重试".to_string());
    }
    for ((id, kind, source_path, canonical_path, status), expected) in current.iter().zip(frozen) {
        let validated = crate::session_context::validate_session_context_path(&cwd, source_path)?;
        if status != "ready"
            || id != &expected.id
            || kind != &expected.kind
            || canonical_path != &expected.canonical_path
            || validated.canonical_path_digest != expected.canonical_path_digest
            || validated.identity_digest != expected.identity_digest
        {
            return Err("会话上下文已在 TurnStart 冻结期间变化，请重试".to_string());
        }
    }
    Ok(())
}

fn context_path_key(path: &str) -> String {
    #[cfg(windows)]
    {
        path.replace('/', "\\").to_lowercase()
    }
    #[cfg(not(windows))]
    {
        path.to_string()
    }
}

/// 当前数据库 schema 版本。任何加列/改表都必须：把版本 +1，并在
/// `apply_migrations` 中补一段从旧版本到新版本的迁移 SQL。
const SCHEMA_VERSION: i64 = 30;
// 新增迁移必须先使用这个连续版本号，再同步提升 SCHEMA_VERSION。
const NEXT_MIGRATION_VERSION: i64 = 31;
const _: () = assert!(NEXT_MIGRATION_VERSION == SCHEMA_VERSION + 1);

fn init_schema(conn: &mut Connection) -> Result<(), String> {
    conn.execute_batch("PRAGMA journal_mode = WAL;")
        .map_err(db_err)?;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_err)?;
    tx.execute_batch(
        "
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
          pinned INTEGER NOT NULL DEFAULT 0,
          runtime_capabilities_json TEXT,
          safe_permission_profile TEXT NOT NULL DEFAULT 'standard',
          folder_id TEXT NOT NULL DEFAULT 'folder-default',
          last_context_tokens INTEGER,
          last_context_window INTEGER,
          preferred_model TEXT,
          preferred_reasoning_effort TEXT
        );
        CREATE TABLE IF NOT EXISTS session_folder (
          id TEXT PRIMARY KEY,
          name TEXT NOT NULL,
          sort_order INTEGER NOT NULL DEFAULT 0,
          collapsed INTEGER NOT NULL DEFAULT 0,
          locked INTEGER NOT NULL DEFAULT 0,
          created_at INTEGER NOT NULL,
          cwd TEXT,
          cwd_key TEXT
        );
        CREATE TABLE IF NOT EXISTS message (
          id INTEGER PRIMARY KEY AUTOINCREMENT,
          session_id TEXT NOT NULL REFERENCES session(id) ON DELETE CASCADE,
          role TEXT NOT NULL,
          text TEXT NOT NULL,
          ts INTEGER NOT NULL,
          reverted INTEGER NOT NULL DEFAULT 0,
          turn_id TEXT
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
          ended_at INTEGER,
          turn_id TEXT,
          native_id TEXT,
          input_digest TEXT,
          integrity_status TEXT NOT NULL DEFAULT 'legacy_unbound',
          result_count INTEGER NOT NULL DEFAULT 0,
          outcome TEXT,
          tool_started INTEGER,
          has_output INTEGER,
          retryable INTEGER,
          denial_source TEXT,
          native_denial_code TEXT,
          PRIMARY KEY (id, session_id)
        );
        CREATE TABLE IF NOT EXISTS usage (
          id INTEGER PRIMARY KEY AUTOINCREMENT,
          session_id TEXT REFERENCES session(id) ON DELETE CASCADE,
          operation_id TEXT REFERENCES background_operation(id) ON DELETE CASCADE,
          model TEXT NOT NULL,
          provider_id TEXT NOT NULL DEFAULT '',
          input_tokens INTEGER NOT NULL,
          cached_input_tokens INTEGER NOT NULL DEFAULT 0,
          cache_write_input_tokens INTEGER NOT NULL DEFAULT 0,
          output_tokens INTEGER NOT NULL,
          cost_usd REAL NOT NULL,
          reported_cost_usd REAL,
          cost_kind TEXT NOT NULL DEFAULT 'unknown',
          price_source TEXT NOT NULL DEFAULT 'unknown',
          service_tier TEXT NOT NULL DEFAULT 'standard',
          pricing_catalog_version TEXT,
          price_snapshot_json TEXT,
          ts INTEGER NOT NULL,
          turn_id TEXT,
          effective_reasoning_effort TEXT,
          model_evidence TEXT NOT NULL DEFAULT 'legacy_unbound',
          CHECK ((session_id IS NOT NULL) != (operation_id IS NOT NULL))
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
          decision TEXT,
          rule_id TEXT,
          error TEXT,
          resolved_at INTEGER,
          persistent_label TEXT,
          matcher_summary TEXT,
          turn_id TEXT,
          PRIMARY KEY (id, session_id)
        );
        CREATE TABLE IF NOT EXISTS permission_rule (
          id TEXT PRIMARY KEY,
          principal TEXT NOT NULL DEFAULT 'main-agent',
          effect TEXT NOT NULL,
          scope TEXT NOT NULL,
          tool_call_id TEXT,
          turn_id TEXT,
          history_session_id TEXT,
          project_root TEXT,
          engine TEXT,
          capability TEXT NOT NULL,
          operation TEXT,
          resource_pattern TEXT,
          created_at INTEGER NOT NULL,
          expires_at INTEGER,
          max_uses INTEGER,
          uses INTEGER NOT NULL DEFAULT 0
        );
        CREATE TABLE IF NOT EXISTS runtime_grant (
          id TEXT PRIMARY KEY,
          engine TEXT NOT NULL,
          provider_id TEXT,
          project_root TEXT,
          matcher_kind TEXT NOT NULL,
          matcher_value TEXT NOT NULL,
          scope TEXT NOT NULL,
          adapter_version TEXT NOT NULL DEFAULT 'unknown',
          ceiling_version TEXT NOT NULL DEFAULT 'safe-v1',
          created_at INTEGER NOT NULL,
          revoked_at INTEGER
        );
         CREATE TABLE IF NOT EXISTS turn_snapshot (
          history_session_id TEXT PRIMARY KEY REFERENCES session(id) ON DELETE CASCADE,
          turn_id TEXT NOT NULL,
          turn_epoch INTEGER NOT NULL,
          status TEXT NOT NULL,
          terminal_reason TEXT,
           recoverable INTEGER NOT NULL DEFAULT 1,
           event_seq INTEGER NOT NULL DEFAULT 0,
           updated_at INTEGER NOT NULL,
           turn_mode TEXT NOT NULL DEFAULT 'build',
           permission_profile TEXT NOT NULL DEFAULT 'standard',
            started_at INTEGER NOT NULL DEFAULT 0,
            attempt_no INTEGER,
            runtime_generation_id TEXT,
            recovery_state TEXT NOT NULL DEFAULT 'none'
         );
         CREATE TABLE IF NOT EXISTS turn (
           history_session_id TEXT NOT NULL REFERENCES session(id) ON DELETE CASCADE,
           turn_id TEXT NOT NULL,
           turn_epoch INTEGER NOT NULL,
           turn_mode TEXT NOT NULL,
           permission_profile TEXT NOT NULL,
           status TEXT NOT NULL,
           started_at INTEGER NOT NULL,
           ended_at INTEGER,
           terminal_reason TEXT,
           identity_source TEXT NOT NULL DEFAULT 'legacy',
           PRIMARY KEY (history_session_id, turn_id)
         );
         CREATE TABLE IF NOT EXISTS turn_execution_spec (
           turn_id TEXT PRIMARY KEY,
           history_session_id TEXT NOT NULL REFERENCES session(id) ON DELETE CASCADE,
           turn_epoch INTEGER NOT NULL,
           engine_id TEXT NOT NULL,
           provider_id TEXT NOT NULL,
           provider_kind TEXT NOT NULL,
           provider_display_name TEXT NOT NULL,
           route_label_snapshot TEXT NOT NULL,
           requested_model_id TEXT NOT NULL,
           routed_model_id TEXT NOT NULL,
           model_label_snapshot TEXT NOT NULL,
           requested_reasoning_effort TEXT NOT NULL,
           routed_reasoning_effort TEXT NOT NULL,
           turn_mode TEXT NOT NULL,
           permission_profile TEXT NOT NULL,
           binding_id TEXT,
           binding_revision INTEGER,
           engine_profile_digest TEXT NOT NULL,
           provider_launch_profile_ref TEXT NOT NULL,
           launch_config_digest TEXT NOT NULL,
           routing_capability_snapshot_id TEXT,
           resolution_source TEXT NOT NULL,
           legacy_route_snapshot_digest TEXT,
           pricing_basis_snapshot_json TEXT NOT NULL,
           created_at INTEGER NOT NULL,
           UNIQUE (history_session_id, turn_epoch),
           CHECK (
             (resolution_source = 'legacy_session_compat'
              AND binding_id IS NULL
              AND binding_revision IS NULL
              AND routing_capability_snapshot_id IS NULL
              AND legacy_route_snapshot_digest IS NOT NULL)
             OR
             (resolution_source = 'binding_live'
              AND binding_id IS NOT NULL
              AND binding_revision IS NOT NULL
              AND routing_capability_snapshot_id IS NOT NULL)
           )
         );
        CREATE TABLE IF NOT EXISTS turn_budget_snapshot (
          turn_id TEXT PRIMARY KEY REFERENCES turn_execution_spec(turn_id) ON DELETE CASCADE,
          snapshot_json TEXT NOT NULL,
          created_at INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS turn_budget_fact (
          turn_id TEXT NOT NULL REFERENCES turn_execution_spec(turn_id) ON DELETE CASCADE,
          attempt_no INTEGER NOT NULL,
          dimension TEXT NOT NULL,
          observed INTEGER NOT NULL,
          budget_limit INTEGER NOT NULL,
          enforcement_mode TEXT NOT NULL,
          action TEXT NOT NULL,
          observed_at INTEGER NOT NULL,
          PRIMARY KEY (turn_id, attempt_no, dimension, action)
        );
        CREATE TABLE IF NOT EXISTS background_operation (
          id TEXT PRIMARY KEY,
          kind TEXT NOT NULL,
          source_session_id TEXT REFERENCES session(id) ON DELETE SET NULL,
          input_digest TEXT NOT NULL,
          idempotency_key TEXT NOT NULL UNIQUE,
          status TEXT NOT NULL CHECK (status IN ('committed', 'running', 'succeeded', 'failed', 'cancelled', 'delivery_unknown')),
          result_json TEXT,
          error_code TEXT,
          created_at INTEGER NOT NULL,
          started_at INTEGER,
          cancel_requested_at INTEGER,
          ended_at INTEGER
        );
        CREATE TABLE IF NOT EXISTS operation_execution_spec (
          operation_id TEXT PRIMARY KEY REFERENCES background_operation(id) ON DELETE CASCADE,
          engine_id TEXT NOT NULL,
          provider_id TEXT NOT NULL,
          provider_kind TEXT NOT NULL,
          provider_display_name TEXT NOT NULL,
          route_label_snapshot TEXT NOT NULL,
          requested_model_id TEXT NOT NULL,
          routed_model_id TEXT NOT NULL,
          model_label_snapshot TEXT NOT NULL,
          requested_reasoning_effort TEXT NOT NULL,
          routed_reasoning_effort TEXT NOT NULL,
          binding_id TEXT NOT NULL,
          binding_revision INTEGER NOT NULL,
          engine_profile_digest TEXT NOT NULL,
          provider_launch_profile_ref TEXT NOT NULL,
          provider_launch_profile_digest TEXT NOT NULL,
          launch_config_digest TEXT NOT NULL,
          routing_capability_snapshot_id TEXT NOT NULL,
          pricing_basis_snapshot_json TEXT NOT NULL,
          purpose TEXT NOT NULL,
          created_at INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS model_only_operation_policy (
          operation_id TEXT PRIMARY KEY REFERENCES background_operation(id) ON DELETE CASCADE,
          contract_version INTEGER NOT NULL,
          canonical_cwd TEXT NOT NULL CHECK (canonical_cwd = ''),
          sandbox_mode TEXT NOT NULL CHECK (sandbox_mode = 'read_only'),
          tools_disabled INTEGER NOT NULL CHECK (tools_disabled = 1),
          extensions_disabled INTEGER NOT NULL CHECK (extensions_disabled = 1),
          persistent_grants_disabled INTEGER NOT NULL CHECK (persistent_grants_disabled = 1),
          capability_snapshot_id TEXT NOT NULL,
          launch_evidence TEXT NOT NULL,
          created_at INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS operation_budget_snapshot (
          operation_id TEXT PRIMARY KEY REFERENCES background_operation(id) ON DELETE CASCADE,
          snapshot_json TEXT NOT NULL,
          created_at INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS operation_attempt (
          operation_id TEXT NOT NULL REFERENCES background_operation(id) ON DELETE CASCADE,
          attempt_no INTEGER NOT NULL,
          owner_kind TEXT NOT NULL CHECK (owner_kind = 'operation'),
          owner_id TEXT NOT NULL,
          generation_id TEXT REFERENCES runtime_generation(id),
          runtime_compatibility_key TEXT NOT NULL,
          observed_model_id TEXT,
          observed_reasoning_effort TEXT,
          actual_capability_snapshot_json TEXT,
          delivery_state TEXT NOT NULL CHECK (delivery_state IN ('prepared', 'accepted', 'rejected', 'completed', 'interrupted', 'error', 'delivery_unknown')),
          terminal_receipt TEXT,
          created_at INTEGER NOT NULL,
          accepted_at INTEGER,
          ended_at INTEGER,
          PRIMARY KEY (operation_id, attempt_no),
          CHECK (owner_id = operation_id)
        );
        CREATE INDEX IF NOT EXISTS idx_operation_attempt_recovery
          ON operation_attempt(delivery_state, created_at);
        CREATE TABLE IF NOT EXISTS operation_progress_fact (
          operation_id TEXT NOT NULL REFERENCES background_operation(id) ON DELETE CASCADE,
          attempt_no INTEGER NOT NULL,
          seq INTEGER NOT NULL,
          kind TEXT NOT NULL,
          value INTEGER,
          detail_json TEXT,
          observed_at INTEGER NOT NULL,
          PRIMARY KEY (operation_id, attempt_no, seq)
        );
        CREATE TABLE IF NOT EXISTS message_attachment (
          id TEXT PRIMARY KEY,
          message_id INTEGER NOT NULL REFERENCES message(id) ON DELETE CASCADE,
          session_id TEXT NOT NULL REFERENCES session(id) ON DELETE CASCADE,
          turn_id TEXT NOT NULL,
          ordinal INTEGER NOT NULL,
          source_path TEXT NOT NULL,
          path_digest TEXT NOT NULL,
          UNIQUE (message_id, ordinal)
        );
        CREATE TABLE IF NOT EXISTS session_context (
          id TEXT PRIMARY KEY,
          session_id TEXT NOT NULL REFERENCES session(id) ON DELETE CASCADE,
          kind TEXT NOT NULL CHECK (kind IN ('file', 'directory')),
          source_path TEXT NOT NULL,
          canonical_path TEXT NOT NULL,
          canonical_key TEXT NOT NULL,
          display_name TEXT NOT NULL,
          status TEXT NOT NULL CHECK (status IN ('ready', 'missing', 'blocked')),
          status_detail TEXT,
          created_at INTEGER NOT NULL,
          updated_at INTEGER NOT NULL,
          UNIQUE (session_id, canonical_key)
        );
        CREATE TABLE IF NOT EXISTS turn_context_snapshot (
          turn_id TEXT NOT NULL REFERENCES turn_execution_spec(turn_id) ON DELETE CASCADE,
          session_id TEXT NOT NULL REFERENCES session(id) ON DELETE CASCADE,
          context_id TEXT NOT NULL,
          ordinal INTEGER NOT NULL,
          kind TEXT NOT NULL,
          canonical_path_digest TEXT NOT NULL,
          identity_digest TEXT NOT NULL,
          validation_status TEXT NOT NULL CHECK (validation_status = 'ready'),
          PRIMARY KEY (turn_id, context_id)
        );
        CREATE TABLE IF NOT EXISTS runtime_generation (
          id TEXT PRIMARY KEY,
          owner_kind TEXT NOT NULL CHECK (owner_kind IN ('session', 'operation')),
          owner_id TEXT NOT NULL,
          engine_id TEXT NOT NULL,
          compatibility_key TEXT NOT NULL,
          engine_profile_digest TEXT NOT NULL,
          provider_launch_profile_ref TEXT NOT NULL,
          provider_launch_profile_digest TEXT NOT NULL,
          capability_snapshot_id TEXT NOT NULL REFERENCES capability_snapshot(id),
          canonical_cwd TEXT NOT NULL,
          status TEXT NOT NULL CHECK (status IN ('active', 'closed', 'application_exit', 'lost_on_restart', 'crashed')),
          created_at INTEGER NOT NULL,
          ended_at INTEGER
        );
        CREATE UNIQUE INDEX IF NOT EXISTS idx_runtime_owner_active
          ON runtime_generation(owner_kind, owner_id) WHERE status = 'active';
        CREATE TABLE IF NOT EXISTS capability_snapshot (
          id TEXT PRIMARY KEY,
          cache_key TEXT NOT NULL UNIQUE,
          engine_id TEXT NOT NULL,
          model_capability_key TEXT NOT NULL,
          identity_json TEXT NOT NULL,
          snapshot_json TEXT NOT NULL,
          probed_at INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_capability_snapshot_engine_model
          ON capability_snapshot(engine_id, model_capability_key, probed_at);
        CREATE TABLE IF NOT EXISTS native_session_ref (
          id TEXT PRIMARY KEY,
          generation_id TEXT NOT NULL REFERENCES runtime_generation(id),
          owner_kind TEXT NOT NULL CHECK (owner_kind IN ('session', 'operation')),
          owner_id TEXT NOT NULL,
          engine_id TEXT NOT NULL,
          native_kind TEXT NOT NULL CHECK (native_kind IN ('claude_session_id', 'codex_thread_id')),
          native_id TEXT NOT NULL,
          launch_profile_identity TEXT NOT NULL,
          created_at INTEGER NOT NULL,
          invalidated_at INTEGER,
          UNIQUE (generation_id, native_id)
        );
        CREATE TABLE IF NOT EXISTS turn_attempt (
          turn_id TEXT NOT NULL REFERENCES turn_execution_spec(turn_id) ON DELETE CASCADE,
          attempt_no INTEGER NOT NULL,
          owner_kind TEXT NOT NULL CHECK (owner_kind = 'session'),
          owner_id TEXT NOT NULL REFERENCES session(id) ON DELETE CASCADE,
          generation_id TEXT NOT NULL REFERENCES runtime_generation(id),
          runtime_compatibility_key TEXT NOT NULL,
          input_native_ref_id TEXT REFERENCES native_session_ref(id),
          output_native_ref_id TEXT REFERENCES native_session_ref(id),
          observed_model_id TEXT,
          observed_reasoning_effort TEXT,
          actual_capability_snapshot_json TEXT,
          delivery_state TEXT NOT NULL CHECK (delivery_state IN ('prepared', 'accepted', 'rejected', 'completed', 'interrupted', 'error', 'delivery_unknown')),
          terminal_receipt TEXT,
          created_at INTEGER NOT NULL,
          accepted_at INTEGER,
          ended_at INTEGER,
          PRIMARY KEY (turn_id, attempt_no)
        );
        CREATE INDEX IF NOT EXISTS idx_turn_attempt_recovery
          ON turn_attempt(delivery_state, created_at);
        CREATE TABLE IF NOT EXISTS stream_boundary_event (
          turn_id TEXT NOT NULL,
          attempt_no INTEGER NOT NULL,
          event_seq INTEGER NOT NULL,
          history_session_id TEXT NOT NULL REFERENCES session(id) ON DELETE CASCADE,
          runtime_generation_id TEXT NOT NULL,
          event_kind TEXT NOT NULL,
          disposition TEXT NOT NULL,
          event_digest TEXT NOT NULL,
          observed_at INTEGER NOT NULL,
          PRIMARY KEY (turn_id, attempt_no, event_seq)
        );
        CREATE INDEX IF NOT EXISTS idx_stream_boundary_session
          ON stream_boundary_event(history_session_id, observed_at);
        CREATE TABLE IF NOT EXISTS stream_diagnostic (
          id INTEGER PRIMARY KEY AUTOINCREMENT,
          history_session_id TEXT,
          turn_id TEXT,
          attempt_no INTEGER,
          runtime_generation_id TEXT,
          source_seq INTEGER,
          event_kind TEXT NOT NULL,
          reason TEXT NOT NULL,
          detail TEXT,
          recorded_at INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS permission_audit (
          id INTEGER PRIMARY KEY AUTOINCREMENT,
          history_session_id TEXT NOT NULL,
          turn_id TEXT NOT NULL,
          tool_call_id TEXT NOT NULL,
          action_fingerprint TEXT NOT NULL,
          principal TEXT NOT NULL,
          engine TEXT NOT NULL,
          capability TEXT NOT NULL,
          operation TEXT NOT NULL,
          resources_json TEXT NOT NULL,
          effect TEXT NOT NULL,
          reason TEXT NOT NULL,
          rule_id TEXT,
          policy_version INTEGER NOT NULL,
          created_at INTEGER NOT NULL,
          execution_status TEXT NOT NULL DEFAULT 'not_started',
          execution_authorization TEXT,
          execution_started_at INTEGER,
          execution_finished_at INTEGER,
          revocation_too_late_at INTEGER
        );
        CREATE UNIQUE INDEX IF NOT EXISTS permission_audit_identity_fingerprint
          ON permission_audit (
            history_session_id, turn_id, tool_call_id, action_fingerprint, policy_version
          );
        CREATE TABLE IF NOT EXISTS checkpoint (
          id TEXT PRIMARY KEY,
          session_id TEXT NOT NULL REFERENCES session(id) ON DELETE CASCADE,
          turn_idx INTEGER NOT NULL,
          label TEXT NOT NULL,
          snapshot_ref TEXT NOT NULL,
          ts INTEGER NOT NULL,
          turn_id TEXT,
          restorable INTEGER NOT NULL DEFAULT 0,
          file_count INTEGER NOT NULL DEFAULT 0,
          restorable_reason TEXT
        );
        ",
    )
    .map_err(db_err)?;
    apply_migrations(&tx)?;
    reconcile_terminal_artifacts(&tx)?;
    let retention_cutoff = now_millis()
        .saturating_sub(PERMISSION_AUDIT_RETENTION_DAYS.saturating_mul(24 * 60 * 60 * 1000));
    tx.execute(
        "DELETE FROM permission_audit
         WHERE created_at < ?1 AND execution_status != 'started'",
        params![retention_cutoff],
    )
    .map_err(db_err)?;
    tx.commit().map_err(db_err)
}

fn terminal_artifact_reason(stop_reason: StopReason) -> &'static str {
    match stop_reason {
        StopReason::End => {
            "[tool_result_missing] 轮次已结束，但 Runtime 未返回该工具调用的最终结果"
        }
        StopReason::Interrupted => "[turn_interrupted] 轮次已中断，工具调用未完成",
        StopReason::Error => "[turn_failed] 轮次失败，工具调用未完成",
    }
}

fn insert_tool_call(
    conn: &Connection,
    session_id: &str,
    id: &str,
    name: &str,
    input: &serde_json::Value,
    status: CallStatus,
    ts: i64,
    turn_id: Option<&str>,
) -> rusqlite::Result<()> {
    let input_json = serde_json::to_string(input).unwrap_or_else(|_| "null".to_string());
    let inserted = conn.execute(
        "INSERT OR IGNORE INTO tool_call
         (id, session_id, name, input_json, status, output, ts, turn_id)
         VALUES (?1, ?2, ?3, ?4, ?5, NULL, ?6, ?7)",
        params![
            id,
            session_id,
            name,
            &input_json,
            history_call_status(status),
            ts,
            turn_id
        ],
    )?;
    if inserted == 1 {
        return Ok(());
    }
    let (existing_name, existing_input): (String, String) = conn.query_row(
        "SELECT name, input_json FROM tool_call WHERE id = ?1 AND session_id = ?2",
        params![id, session_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    if existing_name == name && existing_input == input_json {
        return Ok(());
    }
    Err(rusqlite::Error::ToSqlConversionFailure(Box::new(
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("tool call identity collision for {id}"),
        ),
    )))
}

fn reconcile_tool_call(
    conn: &Connection,
    session_id: &str,
    turn_id: &str,
    native_id: &str,
    name: &str,
    input: &serde_json::Value,
    status: CallStatus,
    ts: i64,
) -> rusqlite::Result<()> {
    if turn_is_terminal(conn, session_id, turn_id)? {
        return Ok(());
    }
    let input_json = serde_json::to_string(input).unwrap_or_else(|_| "null".to_string());
    let input_digest = format!("sha256:{:x}", Sha256::digest(input_json.as_bytes()));
    let existing = conn
        .query_row(
            "SELECT id, name, input_digest, integrity_status
             FROM tool_call
             WHERE session_id = ?1 AND turn_id = ?2 AND native_id = ?3",
            params![session_id, turn_id, native_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )
        .optional()?;
    if let Some((id, existing_name, existing_digest, integrity_status)) = existing {
        if existing_name == name
            && existing_digest.as_deref() == Some(&input_digest)
            && integrity_status != "orphan_result"
        {
            return Ok(());
        }
        conn.execute(
            "UPDATE tool_call SET integrity_status = 'collision' WHERE id = ?1 AND session_id = ?2",
            params![id, session_id],
        )?;
        return Err(rusqlite::Error::ToSqlConversionFailure(Box::new(
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("tool call identity collision for {native_id}"),
            ),
        )));
    }
    let id = ledger_tool_id(turn_id, native_id);
    conn.execute(
        "INSERT INTO tool_call
         (id, session_id, name, input_json, status, output, ts, turn_id, native_id,
          input_digest, integrity_status, result_count)
         VALUES (?1, ?2, ?3, ?4, ?5, NULL, ?6, ?7, ?8, ?9, 'started', 0)",
        params![
            id,
            session_id,
            name,
            input_json,
            history_call_status(status),
            ts,
            turn_id,
            native_id,
            input_digest,
        ],
    )?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn reconcile_tool_result(
    conn: &Connection,
    session_id: &str,
    turn_id: &str,
    native_id: &str,
    status: ToolStatus,
    output: Option<&str>,
    diff_json: Option<&str>,
    ended_at: i64,
    outcome: Option<&str>,
    started: Option<bool>,
    has_output: Option<bool>,
    retryable: Option<bool>,
    denial_source: Option<&str>,
    native_denial_code: Option<&str>,
) -> rusqlite::Result<()> {
    if turn_is_terminal(conn, session_id, turn_id)? {
        return Ok(());
    }
    let output = output.map(bounded_ledger_text);
    let existing = conn
        .query_row(
            "SELECT id, status, result_count FROM tool_call
             WHERE session_id = ?1 AND turn_id = ?2 AND native_id = ?3",
            params![session_id, turn_id, native_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            },
        )
        .optional()?;
    if let Some((id, existing_status, result_count)) = existing {
        if existing_status != "pending" {
            conn.execute(
                "UPDATE tool_call
                 SET integrity_status = 'duplicate_result', result_count = ?1
                 WHERE id = ?2 AND session_id = ?3",
                params![result_count.saturating_add(1), id, session_id],
            )?;
            return Ok(());
        }
        conn.execute(
            "UPDATE tool_call
                 SET status = ?1, output = ?2, diff_json = ?3, ended_at = ?4,
                 integrity_status = 'complete', result_count = 1, outcome = ?5,
                 tool_started = ?6, has_output = ?7, retryable = ?8,
                 denial_source = ?9, native_denial_code = ?10
             WHERE id = ?11 AND session_id = ?12 AND status = 'pending'",
            params![
                history_tool_status(status),
                output,
                diff_json,
                ended_at,
                outcome,
                started.map(i64::from),
                has_output.map(i64::from),
                retryable.map(i64::from),
                denial_source,
                native_denial_code,
                id,
                session_id,
            ],
        )?;
        return Ok(());
    }
    let id = ledger_tool_id(turn_id, native_id);
    conn.execute(
        "INSERT INTO tool_call
         (id, session_id, name, input_json, status, output, diff_json, ts, ended_at,
          turn_id, native_id, input_digest, integrity_status, result_count, outcome,
          tool_started, has_output, retryable, denial_source, native_denial_code)
         VALUES (?1, ?2, '[orphan]', 'null', ?3, ?4, ?5, ?6, ?6, ?7, ?8,
                 'sha256:orphan', 'orphan_result', 1, ?9, ?10, ?11, ?12, ?13, ?14)",
        params![
            id,
            session_id,
            history_tool_status(status),
            output,
            diff_json,
            ended_at,
            turn_id,
            native_id,
            outcome,
            started.map(i64::from),
            has_output.map(i64::from),
            retryable.map(i64::from),
            denial_source,
            native_denial_code,
        ],
    )?;
    Ok(())
}

fn ledger_tool_id(turn_id: &str, native_id: &str) -> String {
    format!(
        "tool-{:x}",
        Sha256::digest(format!("{turn_id}\0{native_id}").as_bytes())
    )
}

fn bounded_ledger_text(value: &str) -> String {
    const MAX_BYTES: usize = 65_536;
    const SUFFIX: &str = "\n[ledger_output_truncated]";
    if value.len() <= MAX_BYTES {
        return value.to_string();
    }
    let mut boundary = MAX_BYTES.saturating_sub(SUFFIX.len());
    while !value.is_char_boundary(boundary) {
        boundary = boundary.saturating_sub(1);
    }
    let mut bounded = value[..boundary].to_string();
    bounded.push_str(SUFFIX);
    bounded
}

fn turn_is_terminal(conn: &Connection, session_id: &str, turn_id: &str) -> rusqlite::Result<bool> {
    conn.query_row(
        "SELECT status IN ('succeeded', 'failed', 'interrupted')
         FROM turn WHERE history_session_id = ?1 AND turn_id = ?2",
        params![session_id, turn_id],
        |row| row.get::<_, i64>(0),
    )
    .optional()
    .map(|value| value.unwrap_or(0) != 0)
}

fn finalize_terminal_artifacts(
    conn: &Connection,
    session_id: &str,
    session_status: &str,
    artifact_reason: &str,
    updated_at_seconds: i64,
) -> rusqlite::Result<()> {
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "UPDATE tool_call
         SET status = 'error', output = COALESCE(output, ?1), ended_at = COALESCE(ended_at, ?2)
         WHERE session_id = ?3 AND status = 'pending'",
        params![artifact_reason, updated_at_seconds * 1000, session_id],
    )?;
    tx.execute(
        "UPDATE approval
         SET status = 'expired', error = COALESCE(error, ?1), resolved_at = COALESCE(resolved_at, ?2)
         WHERE session_id = ?3 AND status = 'pending'",
        params![artifact_reason, updated_at_seconds * 1000, session_id],
    )?;
    tx.execute(
        "UPDATE approval
         SET status = 'failed', error = COALESCE(error, ?1), resolved_at = COALESCE(resolved_at, ?2)
         WHERE session_id = ?3 AND status = 'applying'",
        params![artifact_reason, updated_at_seconds * 1000, session_id],
    )?;
    tx.execute(
        "UPDATE session SET status = ?1, updated_at = ?2 WHERE id = ?3",
        params![session_status, updated_at_seconds, session_id],
    )?;
    tx.commit()
}

fn finalize_turn_artifacts(
    tx: &Transaction<'_>,
    session_id: &str,
    turn_id: &str,
    status: crate::turn_supervisor::TurnStatus,
    updated_at_millis: i64,
) -> rusqlite::Result<()> {
    let (session_status, artifact_reason) = match status {
        crate::turn_supervisor::TurnStatus::Succeeded => (
            "done",
            "[tool_result_missing] 轮次已结束，但 Runtime 未返回该工具调用的最终结果",
        ),
        crate::turn_supervisor::TurnStatus::Interrupted => {
            ("idle", "[turn_interrupted] 轮次已中断，工具调用未完成")
        }
        crate::turn_supervisor::TurnStatus::Failed => {
            ("idle", "[turn_failed] 轮次失败，工具调用未完成")
        }
        _ => return Ok(()),
    };
    tx.execute(
        "UPDATE tool_call
         SET status = 'error', output = COALESCE(output, ?1), ended_at = COALESCE(ended_at, ?2),
             integrity_status = CASE
               WHEN integrity_status = 'started' THEN 'pending_closed'
               ELSE integrity_status
             END
         WHERE session_id = ?3 AND turn_id = ?4 AND status = 'pending'",
        params![artifact_reason, updated_at_millis, session_id, turn_id],
    )?;
    tx.execute(
        "UPDATE approval
         SET status = 'expired', error = COALESCE(error, ?1), resolved_at = COALESCE(resolved_at, ?2)
         WHERE session_id = ?3 AND turn_id = ?4 AND status = 'pending'",
        params![artifact_reason, updated_at_millis, session_id, turn_id],
    )?;
    // approval_response owns applying -> resolved/failed. A deny acknowledgement can emit the
    // terminal TurnComplete before that command returns, so finalizing it here races the ledger
    // commit. Startup reconciliation still closes genuinely abandoned applying approvals.
    tx.execute(
        "UPDATE session SET status = ?1, updated_at = ?2 WHERE id = ?3",
        params![session_status, updated_at_millis / 1000, session_id],
    )?;
    Ok(())
}

fn reconcile_terminal_artifacts(tx: &Transaction<'_>) -> Result<(), String> {
    let terminal_sessions = "SELECT history_session_id FROM turn_snapshot
                             WHERE status IN ('succeeded', 'failed', 'interrupted')";
    tx.execute(
        &format!(
            "UPDATE tool_call
             SET status = 'error', output = COALESCE(output, '[turn_reconciled] 应用恢复时发现轮次已结束，工具调用未完成')
             WHERE status = 'pending' AND session_id IN ({terminal_sessions})"
        ),
        [],
    )
    .map_err(db_err)?;
    tx.execute(
        &format!(
            "UPDATE approval
             SET status = 'expired', error = COALESCE(error, '[turn_reconciled] 应用恢复时发现轮次已结束')
             WHERE status = 'pending' AND session_id IN ({terminal_sessions})"
        ),
        [],
    )
    .map_err(db_err)?;
    tx.execute(
        &format!(
            "UPDATE approval
             SET status = 'failed', error = COALESCE(error, '[turn_reconciled] 应用恢复时发现审批投递未完成')
             WHERE status = 'applying' AND session_id IN ({terminal_sessions})"
        ),
        [],
    )
    .map_err(db_err)?;
    Ok(())
}

/// 基于 PRAGMA user_version 的迁移框架：老库按版本逐级升级，新库直接盖章当前版本。
fn apply_migrations(tx: &Transaction<'_>) -> Result<(), String> {
    let current: i64 = tx
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(db_err)?;
    if current > SCHEMA_VERSION {
        return Err(format!(
            "数据库版本（{current}）比当前应用支持的版本（{SCHEMA_VERSION}）更新，请升级 Helm"
        ));
    }
    // 迁移块必须继续按版本升序排列。
    if current < 2 && !column_exists(tx, "message", "reverted")? {
        // v2（P2-5 回溯语义）：message 表加 reverted 标记。
        // 新库的 CREATE TABLE 已含该列，这里只补老库。
        tx.execute_batch("ALTER TABLE message ADD COLUMN reverted INTEGER NOT NULL DEFAULT 0;")
            .map_err(db_err)?;
    }
    if current < 3 && !column_exists(tx, "session", "provider_id")? {
        // v3（P3-6 用量归属）：session 表记录创建/恢复时实际使用的服务商 id，
        // 用量按服务商聚合不再靠模型名猜。老会话保持空串，前端显示「未标注」。
        tx.execute_batch("ALTER TABLE session ADD COLUMN provider_id TEXT NOT NULL DEFAULT '';")
            .map_err(db_err)?;
    }
    if current < 4 {
        // v4（变更-07 回溯时间戳修复）：message/tool_call 的 ts 从秒统一为毫秒，
        // 与 checkpoint.ts（一直是毫秒）同单位——否则 revert_messages_after 的
        // `ts > 检查点毫秒` 永远不成立，回溯的消息截断完全失效。
        // 阈值 1e11：秒级时间戳（~1.7e9）远小于它，毫秒级（~1.7e12）远大于它，幂等安全。
        // 同时删除旧版从未写入过的 turn 死表。v17 会在后续迁移中以新的
        // 权限审计结构重新创建同名表，不能保留旧表形状混用。
        tx.execute_batch(
            "UPDATE message SET ts = ts * 1000 WHERE ts > 0 AND ts < 100000000000;
             UPDATE tool_call SET ts = ts * 1000 WHERE ts > 0 AND ts < 100000000000;
             DROP TABLE IF EXISTS turn;",
        )
        .map_err(db_err)?;
    }
    if current < 5 && !column_exists(tx, "session", "pinned")? {
        // v5（变更-12 会话管理）：置顶标记。新库的 CREATE TABLE 已含该列，这里只补老库。
        tx.execute_batch("ALTER TABLE session ADD COLUMN pinned INTEGER NOT NULL DEFAULT 0;")
            .map_err(db_err)?;
    }
    if current < 6 {
        // v6（Permission Ledger）：结构化权限规则与审批事务元数据。
        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS permission_rule (
               id TEXT PRIMARY KEY,
               effect TEXT NOT NULL,
               scope TEXT NOT NULL,
               engine TEXT,
               capability TEXT NOT NULL,
               operation TEXT,
               resource_pattern TEXT,
               created_at INTEGER NOT NULL,
               expires_at INTEGER,
               max_uses INTEGER,
               uses INTEGER NOT NULL DEFAULT 0
             );",
        )
        .map_err(db_err)?;
        for (column, sql) in [
            ("decision", "ALTER TABLE approval ADD COLUMN decision TEXT;"),
            ("rule_id", "ALTER TABLE approval ADD COLUMN rule_id TEXT;"),
            ("error", "ALTER TABLE approval ADD COLUMN error TEXT;"),
            (
                "resolved_at",
                "ALTER TABLE approval ADD COLUMN resolved_at INTEGER;",
            ),
        ] {
            if !column_exists(tx, "approval", column)? {
                tx.execute_batch(sql).map_err(db_err)?;
            }
        }
    }
    if current < 7 {
        for (column, sql) in [
            (
                "principal",
                "ALTER TABLE permission_rule ADD COLUMN principal TEXT NOT NULL DEFAULT 'main-agent';",
            ),
            ("turn_id", "ALTER TABLE permission_rule ADD COLUMN turn_id TEXT;"),
            (
                "history_session_id",
                "ALTER TABLE permission_rule ADD COLUMN history_session_id TEXT;",
            ),
            (
                "project_root",
                "ALTER TABLE permission_rule ADD COLUMN project_root TEXT;",
            ),
        ] {
            if !column_exists(tx, "permission_rule", column)? {
                tx.execute_batch(sql).map_err(db_err)?;
            }
        }
        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS permission_audit (
               id INTEGER PRIMARY KEY AUTOINCREMENT,
               history_session_id TEXT NOT NULL,
               turn_id TEXT NOT NULL,
               tool_call_id TEXT NOT NULL,
               action_fingerprint TEXT NOT NULL,
               principal TEXT NOT NULL,
               engine TEXT NOT NULL,
               capability TEXT NOT NULL,
               operation TEXT NOT NULL,
               resources_json TEXT NOT NULL,
               effect TEXT NOT NULL,
               reason TEXT NOT NULL,
               rule_id TEXT,
               policy_version INTEGER NOT NULL,
               created_at INTEGER NOT NULL
             );
             CREATE UNIQUE INDEX IF NOT EXISTS permission_audit_identity_fingerprint
               ON permission_audit (
                 history_session_id, turn_id, tool_call_id, action_fingerprint, policy_version
               );",
        )
        .map_err(db_err)?;
    }
    if current < 8 {
        if !column_exists(tx, "permission_audit", "action_fingerprint")? {
            tx.execute_batch(
                "ALTER TABLE permission_audit
                 ADD COLUMN action_fingerprint TEXT NOT NULL DEFAULT '';",
            )
            .map_err(db_err)?;
        }
        tx.execute_batch(
            "CREATE UNIQUE INDEX IF NOT EXISTS permission_audit_identity_fingerprint
             ON permission_audit (
               history_session_id, turn_id, tool_call_id, action_fingerprint, policy_version
             );",
        )
        .map_err(db_err)?;
    }
    if current < 9 && !column_exists(tx, "permission_rule", "tool_call_id")? {
        tx.execute_batch("ALTER TABLE permission_rule ADD COLUMN tool_call_id TEXT;")
            .map_err(db_err)?;
    }
    if current < 10 {
        for (column, sql) in [
            (
                "execution_status",
                "ALTER TABLE permission_audit ADD COLUMN execution_status TEXT NOT NULL DEFAULT 'not_started';",
            ),
            (
                "execution_started_at",
                "ALTER TABLE permission_audit ADD COLUMN execution_started_at INTEGER;",
            ),
            (
                "execution_finished_at",
                "ALTER TABLE permission_audit ADD COLUMN execution_finished_at INTEGER;",
            ),
            (
                "revocation_too_late_at",
                "ALTER TABLE permission_audit ADD COLUMN revocation_too_late_at INTEGER;",
            ),
        ] {
            if !column_exists(tx, "permission_audit", column)? {
                tx.execute_batch(sql).map_err(db_err)?;
            }
        }
    }
    if current < 11 && !column_exists(tx, "permission_audit", "execution_authorization")? {
        tx.execute_batch("ALTER TABLE permission_audit ADD COLUMN execution_authorization TEXT;")
            .map_err(db_err)?;
    }
    if current < 12 {
        // v12：固化每条 Usage 的 Provider、缓存 token、层级与价格快照。
        for (column, sql) in [
            (
                "provider_id",
                "ALTER TABLE usage ADD COLUMN provider_id TEXT NOT NULL DEFAULT '';",
            ),
            (
                "cached_input_tokens",
                "ALTER TABLE usage ADD COLUMN cached_input_tokens INTEGER NOT NULL DEFAULT 0;",
            ),
            (
                "cache_write_input_tokens",
                "ALTER TABLE usage ADD COLUMN cache_write_input_tokens INTEGER NOT NULL DEFAULT 0;",
            ),
            (
                "reported_cost_usd",
                "ALTER TABLE usage ADD COLUMN reported_cost_usd REAL;",
            ),
            (
                "cost_kind",
                "ALTER TABLE usage ADD COLUMN cost_kind TEXT NOT NULL DEFAULT 'unknown';",
            ),
            (
                "price_source",
                "ALTER TABLE usage ADD COLUMN price_source TEXT NOT NULL DEFAULT 'unknown';",
            ),
            (
                "service_tier",
                "ALTER TABLE usage ADD COLUMN service_tier TEXT NOT NULL DEFAULT 'standard';",
            ),
            (
                "pricing_catalog_version",
                "ALTER TABLE usage ADD COLUMN pricing_catalog_version TEXT;",
            ),
            (
                "price_snapshot_json",
                "ALTER TABLE usage ADD COLUMN price_snapshot_json TEXT;",
            ),
        ] {
            if !column_exists(tx, "usage", column)? {
                tx.execute_batch(sql).map_err(db_err)?;
            }
        }
        tx.execute_batch(
            "UPDATE usage
             SET provider_id = COALESCE(
               (SELECT provider_id FROM session WHERE session.id = usage.session_id), ''
             )
             WHERE provider_id = '';
             UPDATE usage
             SET cost_kind = CASE WHEN cost_usd > 0 THEN 'legacy' ELSE 'unknown' END,
                 price_source = CASE WHEN cost_usd > 0 THEN 'legacy' ELSE 'unknown' END
             WHERE cost_kind = 'unknown';",
        )
        .map_err(db_err)?;
    }
    if current < 13 {
        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS runtime_grant (
               id TEXT PRIMARY KEY,
               engine TEXT NOT NULL,
               provider_id TEXT,
               project_root TEXT,
               matcher_kind TEXT NOT NULL,
               matcher_value TEXT NOT NULL,
               scope TEXT NOT NULL,
               adapter_version TEXT NOT NULL DEFAULT 'unknown',
               ceiling_version TEXT NOT NULL DEFAULT 'safe-v1',
               created_at INTEGER NOT NULL,
               revoked_at INTEGER
             );",
        )
        .map_err(db_err)?;
    }
    if current < 14 {
        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS turn_snapshot (
               history_session_id TEXT PRIMARY KEY REFERENCES session(id) ON DELETE CASCADE,
               turn_id TEXT NOT NULL,
               turn_epoch INTEGER NOT NULL,
               status TEXT NOT NULL,
               terminal_reason TEXT,
               recoverable INTEGER NOT NULL DEFAULT 1,
               event_seq INTEGER NOT NULL DEFAULT 0,
               updated_at INTEGER NOT NULL
             );",
        )
        .map_err(db_err)?;
    }
    if current < 15 && !column_exists(tx, "session", "runtime_capabilities_json")? {
        tx.execute_batch("ALTER TABLE session ADD COLUMN runtime_capabilities_json TEXT;")
            .map_err(db_err)?;
    }
    if current < 16 && !column_exists(tx, "session", "safe_permission_profile")? {
        tx.execute_batch(
            "ALTER TABLE session ADD COLUMN safe_permission_profile TEXT NOT NULL DEFAULT 'standard';",
        )
        .map_err(db_err)?;
    }
    if current < 17 {
        if !column_exists(tx, "turn_snapshot", "turn_mode")? {
            tx.execute_batch(
                "ALTER TABLE turn_snapshot ADD COLUMN turn_mode TEXT NOT NULL DEFAULT 'build';",
            )
            .map_err(db_err)?;
        }
        if !column_exists(tx, "turn_snapshot", "permission_profile")? {
            tx.execute_batch("ALTER TABLE turn_snapshot ADD COLUMN permission_profile TEXT NOT NULL DEFAULT 'standard';")
                .map_err(db_err)?;
        }
        if !column_exists(tx, "turn_snapshot", "started_at")? {
            tx.execute_batch(
                "ALTER TABLE turn_snapshot ADD COLUMN started_at INTEGER NOT NULL DEFAULT 0;",
            )
            .map_err(db_err)?;
        }
        if !column_exists(tx, "message", "turn_id")? {
            tx.execute_batch("ALTER TABLE message ADD COLUMN turn_id TEXT;")
                .map_err(db_err)?;
        }
        if !column_exists(tx, "approval", "persistent_label")? {
            tx.execute_batch("ALTER TABLE approval ADD COLUMN persistent_label TEXT;")
                .map_err(db_err)?;
        }
        if !column_exists(tx, "approval", "matcher_summary")? {
            tx.execute_batch("ALTER TABLE approval ADD COLUMN matcher_summary TEXT;")
                .map_err(db_err)?;
        }
        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS turn (
               history_session_id TEXT NOT NULL REFERENCES session(id) ON DELETE CASCADE,
               turn_id TEXT NOT NULL,
               turn_epoch INTEGER NOT NULL,
               turn_mode TEXT NOT NULL,
               permission_profile TEXT NOT NULL,
               status TEXT NOT NULL,
               started_at INTEGER NOT NULL,
               ended_at INTEGER,
               terminal_reason TEXT,
               PRIMARY KEY (history_session_id, turn_id)
             );",
        )
        .map_err(db_err)?;
    }
    if current < 18 && !column_exists(tx, "turn", "turn_id")? {
        // v18：v4 的 DROP turn 死表是后补进代码的——在它进代码前就已升到 ≥4 的老库
        // 从未执行过该 DROP，残留的旧 turn 表（id/session_id/idx 形状，从未写入过）
        // 会让 v17 的 CREATE TABLE IF NOT EXISTS 变成空操作，读侧随即报
        // "no such column: turn_id"。这里按 turn_id 列是否存在识别旧形状并重建。
        tx.execute_batch(
            "DROP TABLE IF EXISTS turn;
             CREATE TABLE turn (
               history_session_id TEXT NOT NULL REFERENCES session(id) ON DELETE CASCADE,
               turn_id TEXT NOT NULL,
               turn_epoch INTEGER NOT NULL,
               turn_mode TEXT NOT NULL,
               permission_profile TEXT NOT NULL,
               status TEXT NOT NULL,
               started_at INTEGER NOT NULL,
               ended_at INTEGER,
               terminal_reason TEXT,
               PRIMARY KEY (history_session_id, turn_id)
             );",
        )
        .map_err(db_err)?;
    }
    if current < 19 {
        tx.execute(
            "INSERT OR IGNORE INTO session_folder
             (id, name, sort_order, collapsed, locked, created_at)
             VALUES ('folder-default', '默认', 0, 0, 1, ?1)",
            params![now_millis()],
        )
        .map_err(db_err)?;
        if !column_exists(tx, "session", "folder_id")? {
            tx.execute_batch(
                "ALTER TABLE session ADD COLUMN folder_id TEXT NOT NULL DEFAULT 'folder-default';",
            )
            .map_err(db_err)?;
        }
        tx.execute_batch(
            "UPDATE session SET folder_id = 'folder-default'
             WHERE folder_id = '' OR folder_id IS NULL;",
        )
        .map_err(db_err)?;
    }
    if current < 20 {
        for (table, column, sql) in [
            (
                "session",
                "last_context_tokens",
                "ALTER TABLE session ADD COLUMN last_context_tokens INTEGER;",
            ),
            (
                "session",
                "last_context_window",
                "ALTER TABLE session ADD COLUMN last_context_window INTEGER;",
            ),
            (
                "tool_call",
                "ended_at",
                "ALTER TABLE tool_call ADD COLUMN ended_at INTEGER;",
            ),
            (
                "tool_call",
                "turn_id",
                "ALTER TABLE tool_call ADD COLUMN turn_id TEXT;",
            ),
            (
                "approval",
                "turn_id",
                "ALTER TABLE approval ADD COLUMN turn_id TEXT;",
            ),
            (
                "checkpoint",
                "turn_id",
                "ALTER TABLE checkpoint ADD COLUMN turn_id TEXT;",
            ),
        ] {
            if !column_exists(tx, table, column)? {
                tx.execute_batch(sql).map_err(db_err)?;
            }
        }
    }
    if current < 21 {
        if !column_exists(tx, "session_folder", "cwd")? {
            tx.execute_batch("ALTER TABLE session_folder ADD COLUMN cwd TEXT;")
                .map_err(db_err)?;
        }
        if !column_exists(tx, "session_folder", "cwd_key")? {
            tx.execute_batch("ALTER TABLE session_folder ADD COLUMN cwd_key TEXT;")
                .map_err(db_err)?;
        }
        tx.execute_batch(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_session_folder_cwd_key
               ON session_folder(cwd_key) WHERE cwd_key IS NOT NULL;",
        )
        .map_err(db_err)?;
    }
    if current < 22 {
        if !column_exists(tx, "turn", "identity_source")? {
            tx.execute_batch(
                "ALTER TABLE turn ADD COLUMN identity_source TEXT NOT NULL DEFAULT 'legacy';",
            )
            .map_err(db_err)?;
        }
        if !column_exists(tx, "usage", "turn_id")? {
            tx.execute_batch("ALTER TABLE usage ADD COLUMN turn_id TEXT;")
                .map_err(db_err)?;
        }
        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS turn_execution_spec (
               turn_id TEXT PRIMARY KEY,
               history_session_id TEXT NOT NULL REFERENCES session(id) ON DELETE CASCADE,
               turn_epoch INTEGER NOT NULL,
               engine_id TEXT NOT NULL,
               provider_id TEXT NOT NULL,
               provider_kind TEXT NOT NULL,
               provider_display_name TEXT NOT NULL,
               route_label_snapshot TEXT NOT NULL,
               requested_model_id TEXT NOT NULL,
               routed_model_id TEXT NOT NULL,
               model_label_snapshot TEXT NOT NULL,
               requested_reasoning_effort TEXT NOT NULL,
               routed_reasoning_effort TEXT NOT NULL,
               turn_mode TEXT NOT NULL,
               permission_profile TEXT NOT NULL,
               binding_id TEXT,
               binding_revision INTEGER,
               engine_profile_digest TEXT NOT NULL,
               provider_launch_profile_ref TEXT NOT NULL,
               launch_config_digest TEXT NOT NULL,
               routing_capability_snapshot_id TEXT,
               resolution_source TEXT NOT NULL,
               legacy_route_snapshot_digest TEXT,
               pricing_basis_snapshot_json TEXT NOT NULL,
               created_at INTEGER NOT NULL,
               UNIQUE (history_session_id, turn_epoch),
               CHECK (
                 (resolution_source = 'legacy_session_compat'
                  AND binding_id IS NULL
                  AND binding_revision IS NULL
                  AND routing_capability_snapshot_id IS NULL
                  AND legacy_route_snapshot_digest IS NOT NULL)
                 OR
                 (resolution_source = 'binding_live'
                  AND binding_id IS NOT NULL
                  AND binding_revision IS NOT NULL
                  AND routing_capability_snapshot_id IS NOT NULL)
               )
             );",
        )
        .map_err(db_err)?;
    }
    if current < 23 {
        for (column, sql) in [
            (
                "native_id",
                "ALTER TABLE tool_call ADD COLUMN native_id TEXT;",
            ),
            (
                "input_digest",
                "ALTER TABLE tool_call ADD COLUMN input_digest TEXT;",
            ),
            (
                "integrity_status",
                "ALTER TABLE tool_call ADD COLUMN integrity_status TEXT NOT NULL DEFAULT 'legacy_unbound';",
            ),
            (
                "result_count",
                "ALTER TABLE tool_call ADD COLUMN result_count INTEGER NOT NULL DEFAULT 0;",
            ),
        ] {
            if !column_exists(tx, "tool_call", column)? {
                tx.execute_batch(sql).map_err(db_err)?;
            }
        }
        if !column_exists(tx, "usage", "effective_reasoning_effort")? {
            tx.execute_batch("ALTER TABLE usage ADD COLUMN effective_reasoning_effort TEXT;")
                .map_err(db_err)?;
        }
        if !column_exists(tx, "usage", "model_evidence")? {
            tx.execute_batch(
                "ALTER TABLE usage ADD COLUMN model_evidence TEXT NOT NULL DEFAULT 'legacy_unbound';",
            )
            .map_err(db_err)?;
        }
        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS message_attachment (
               id TEXT PRIMARY KEY,
               message_id INTEGER NOT NULL REFERENCES message(id) ON DELETE CASCADE,
               session_id TEXT NOT NULL REFERENCES session(id) ON DELETE CASCADE,
               turn_id TEXT NOT NULL,
               ordinal INTEGER NOT NULL,
               source_path TEXT NOT NULL,
               path_digest TEXT NOT NULL,
               UNIQUE (message_id, ordinal)
             );
             CREATE TABLE IF NOT EXISTS session_context (
               id TEXT PRIMARY KEY,
               session_id TEXT NOT NULL REFERENCES session(id) ON DELETE CASCADE,
               kind TEXT NOT NULL CHECK (kind IN ('file', 'directory')),
               source_path TEXT NOT NULL,
               canonical_path TEXT NOT NULL,
               canonical_key TEXT NOT NULL,
               display_name TEXT NOT NULL,
               status TEXT NOT NULL CHECK (status IN ('ready', 'missing', 'blocked')),
               status_detail TEXT,
               created_at INTEGER NOT NULL,
               updated_at INTEGER NOT NULL,
               UNIQUE (session_id, canonical_key)
             );
             CREATE TABLE IF NOT EXISTS turn_context_snapshot (
               turn_id TEXT NOT NULL REFERENCES turn_execution_spec(turn_id) ON DELETE CASCADE,
               session_id TEXT NOT NULL REFERENCES session(id) ON DELETE CASCADE,
               context_id TEXT NOT NULL,
               ordinal INTEGER NOT NULL,
               kind TEXT NOT NULL,
               canonical_path_digest TEXT NOT NULL,
               identity_digest TEXT NOT NULL,
               validation_status TEXT NOT NULL CHECK (validation_status = 'ready'),
               PRIMARY KEY (turn_id, context_id)
             );
             CREATE INDEX IF NOT EXISTS idx_message_turn ON message(session_id, turn_id);
             CREATE INDEX IF NOT EXISTS idx_tool_call_turn ON tool_call(session_id, turn_id);
             CREATE INDEX IF NOT EXISTS idx_approval_turn ON approval(session_id, turn_id);
             CREATE INDEX IF NOT EXISTS idx_usage_turn ON usage(session_id, turn_id);
             CREATE INDEX IF NOT EXISTS idx_checkpoint_turn ON checkpoint(session_id, turn_id);
             CREATE UNIQUE INDEX IF NOT EXISTS idx_tool_native_turn
               ON tool_call(session_id, turn_id, native_id)
               WHERE turn_id IS NOT NULL AND native_id IS NOT NULL;",
        )
        .map_err(db_err)?;
    }
    if current < 24 {
        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS runtime_generation (
               id TEXT PRIMARY KEY,
               owner_kind TEXT NOT NULL CHECK (owner_kind IN ('session', 'operation')),
               owner_id TEXT NOT NULL,
               engine_id TEXT NOT NULL,
               compatibility_key TEXT NOT NULL,
               engine_profile_digest TEXT NOT NULL,
               provider_launch_profile_ref TEXT NOT NULL,
               provider_launch_profile_digest TEXT NOT NULL,
               canonical_cwd TEXT NOT NULL,
               status TEXT NOT NULL CHECK (status IN ('active', 'closed', 'application_exit', 'lost_on_restart', 'crashed')),
               created_at INTEGER NOT NULL,
               ended_at INTEGER
             );
             CREATE UNIQUE INDEX IF NOT EXISTS idx_runtime_owner_active
               ON runtime_generation(owner_kind, owner_id) WHERE status = 'active';
             CREATE TABLE IF NOT EXISTS native_session_ref (
               id TEXT PRIMARY KEY,
               generation_id TEXT NOT NULL REFERENCES runtime_generation(id),
               owner_kind TEXT NOT NULL CHECK (owner_kind IN ('session', 'operation')),
               owner_id TEXT NOT NULL,
               engine_id TEXT NOT NULL,
               native_kind TEXT NOT NULL CHECK (native_kind IN ('claude_session_id', 'codex_thread_id')),
               native_id TEXT NOT NULL,
               launch_profile_identity TEXT NOT NULL,
               created_at INTEGER NOT NULL,
               invalidated_at INTEGER,
               UNIQUE (generation_id, native_id)
             );
             CREATE TABLE IF NOT EXISTS turn_attempt (
               turn_id TEXT NOT NULL REFERENCES turn_execution_spec(turn_id) ON DELETE CASCADE,
               attempt_no INTEGER NOT NULL,
               owner_kind TEXT NOT NULL CHECK (owner_kind = 'session'),
               owner_id TEXT NOT NULL REFERENCES session(id) ON DELETE CASCADE,
               generation_id TEXT NOT NULL REFERENCES runtime_generation(id),
               runtime_compatibility_key TEXT NOT NULL,
               input_native_ref_id TEXT REFERENCES native_session_ref(id),
               output_native_ref_id TEXT REFERENCES native_session_ref(id),
               observed_model_id TEXT,
               observed_reasoning_effort TEXT,
               actual_capability_snapshot_json TEXT,
               delivery_state TEXT NOT NULL CHECK (delivery_state IN ('prepared', 'accepted', 'rejected', 'completed', 'interrupted', 'error', 'delivery_unknown')),
               terminal_receipt TEXT,
               created_at INTEGER NOT NULL,
               accepted_at INTEGER,
               ended_at INTEGER,
               PRIMARY KEY (turn_id, attempt_no)
             );
             CREATE INDEX IF NOT EXISTS idx_turn_attempt_recovery
               ON turn_attempt(delivery_state, created_at);",
        )
        .map_err(db_err)?;
    }
    if current < 25 {
        if !column_exists(tx, "turn_snapshot", "attempt_no")? {
            tx.execute_batch("ALTER TABLE turn_snapshot ADD COLUMN attempt_no INTEGER;")
                .map_err(db_err)?;
        }
        if !column_exists(tx, "turn_snapshot", "runtime_generation_id")? {
            tx.execute_batch("ALTER TABLE turn_snapshot ADD COLUMN runtime_generation_id TEXT;")
                .map_err(db_err)?;
        }
        if !column_exists(tx, "turn_snapshot", "recovery_state")? {
            tx.execute_batch(
                "ALTER TABLE turn_snapshot ADD COLUMN recovery_state TEXT NOT NULL DEFAULT 'none';",
            )
            .map_err(db_err)?;
        }
        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS stream_boundary_event (
               turn_id TEXT NOT NULL,
               attempt_no INTEGER NOT NULL,
               event_seq INTEGER NOT NULL,
               history_session_id TEXT NOT NULL REFERENCES session(id) ON DELETE CASCADE,
               runtime_generation_id TEXT NOT NULL,
               event_kind TEXT NOT NULL,
               disposition TEXT NOT NULL,
               event_digest TEXT NOT NULL,
               observed_at INTEGER NOT NULL,
               PRIMARY KEY (turn_id, attempt_no, event_seq)
             );
             CREATE INDEX IF NOT EXISTS idx_stream_boundary_session
               ON stream_boundary_event(history_session_id, observed_at);
             CREATE TABLE IF NOT EXISTS stream_diagnostic (
               id INTEGER PRIMARY KEY AUTOINCREMENT,
               history_session_id TEXT,
               turn_id TEXT,
               attempt_no INTEGER,
               runtime_generation_id TEXT,
               source_seq INTEGER,
               event_kind TEXT NOT NULL,
               reason TEXT NOT NULL,
               detail TEXT,
               recorded_at INTEGER NOT NULL
             );",
        )
        .map_err(db_err)?;
    }
    if current < 26 {
        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS capability_snapshot (
               id TEXT PRIMARY KEY,
               cache_key TEXT NOT NULL UNIQUE,
               engine_id TEXT NOT NULL,
               model_capability_key TEXT NOT NULL,
               identity_json TEXT NOT NULL,
               snapshot_json TEXT NOT NULL,
               probed_at INTEGER NOT NULL
             );
             CREATE INDEX IF NOT EXISTS idx_capability_snapshot_engine_model
               ON capability_snapshot(engine_id, model_capability_key, probed_at);",
        )
        .map_err(db_err)?;
        if !column_exists(tx, "runtime_generation", "capability_snapshot_id")? {
            // v25 active generations are process-local and are reconciled to lost_on_restart.
            // Their NULL value is a legacy projection; every v26 generation writes a real snapshot id.
            tx.execute_batch(
                "ALTER TABLE runtime_generation ADD COLUMN capability_snapshot_id TEXT REFERENCES capability_snapshot(id);",
            )
            .map_err(db_err)?;
        }
    }
    if current < 27 {
        if !column_exists(tx, "session", "preferred_model")? {
            tx.execute_batch("ALTER TABLE session ADD COLUMN preferred_model TEXT;")
                .map_err(db_err)?;
        }
        if !column_exists(tx, "session", "preferred_reasoning_effort")? {
            tx.execute_batch("ALTER TABLE session ADD COLUMN preferred_reasoning_effort TEXT;")
                .map_err(db_err)?;
        }
        tx.execute_batch(
            "UPDATE session
             SET preferred_model = model
             WHERE preferred_model IS NULL OR preferred_model = '';",
        )
        .map_err(db_err)?;
    }
    if current < 28 {
        tx.execute_batch(
            "ALTER TABLE usage RENAME TO usage_v27;
             CREATE TABLE usage (
               id INTEGER PRIMARY KEY AUTOINCREMENT,
               session_id TEXT REFERENCES session(id) ON DELETE CASCADE,
               operation_id TEXT REFERENCES background_operation(id) ON DELETE CASCADE,
               model TEXT NOT NULL,
               provider_id TEXT NOT NULL DEFAULT '',
               input_tokens INTEGER NOT NULL,
               cached_input_tokens INTEGER NOT NULL DEFAULT 0,
               cache_write_input_tokens INTEGER NOT NULL DEFAULT 0,
               output_tokens INTEGER NOT NULL,
               cost_usd REAL NOT NULL,
               reported_cost_usd REAL,
               cost_kind TEXT NOT NULL DEFAULT 'unknown',
               price_source TEXT NOT NULL DEFAULT 'unknown',
               service_tier TEXT NOT NULL DEFAULT 'standard',
               pricing_catalog_version TEXT,
               price_snapshot_json TEXT,
               ts INTEGER NOT NULL,
               turn_id TEXT,
               effective_reasoning_effort TEXT,
               model_evidence TEXT NOT NULL DEFAULT 'legacy_unbound',
               CHECK ((session_id IS NOT NULL) != (operation_id IS NOT NULL))
             );
             INSERT INTO usage
               (id, session_id, operation_id, model, provider_id, input_tokens,
                cached_input_tokens, cache_write_input_tokens, output_tokens, cost_usd,
                reported_cost_usd, cost_kind, price_source, service_tier,
                pricing_catalog_version, price_snapshot_json, ts, turn_id,
                effective_reasoning_effort, model_evidence)
             SELECT id, session_id, NULL, model, provider_id, input_tokens,
                    cached_input_tokens, cache_write_input_tokens, output_tokens, cost_usd,
                    reported_cost_usd, cost_kind, price_source, service_tier,
                    pricing_catalog_version, price_snapshot_json, ts, turn_id,
                    effective_reasoning_effort, model_evidence
             FROM usage_v27;
             DROP TABLE usage_v27;
             CREATE INDEX IF NOT EXISTS idx_usage_turn ON usage(session_id, turn_id);
             CREATE INDEX IF NOT EXISTS idx_usage_operation ON usage(operation_id, ts);
             CREATE TABLE IF NOT EXISTS turn_budget_snapshot (
               turn_id TEXT PRIMARY KEY REFERENCES turn_execution_spec(turn_id) ON DELETE CASCADE,
               snapshot_json TEXT NOT NULL,
               created_at INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS turn_budget_fact (
               turn_id TEXT NOT NULL REFERENCES turn_execution_spec(turn_id) ON DELETE CASCADE,
               attempt_no INTEGER NOT NULL,
               dimension TEXT NOT NULL,
               observed INTEGER NOT NULL,
               budget_limit INTEGER NOT NULL,
               enforcement_mode TEXT NOT NULL,
               action TEXT NOT NULL,
               observed_at INTEGER NOT NULL,
               PRIMARY KEY (turn_id, attempt_no, dimension, action)
             );
             INSERT OR IGNORE INTO turn_budget_snapshot (turn_id, snapshot_json, created_at)
             SELECT turn_id,
                    '{\"contractVersion\":1,\"limits\":[{\"dimension\":\"input_bytes\",\"limit\":2097152,\"enforcementMode\":\"hard_preflight\"},{\"dimension\":\"token\",\"limit\":200000,\"enforcementMode\":\"post_facto\"},{\"dimension\":\"cost_microusd\",\"limit\":20000000,\"enforcementMode\":\"post_facto\"},{\"dimension\":\"tool_count\",\"limit\":128,\"enforcementMode\":\"streaming\"},{\"dimension\":\"repeat_digest\",\"limit\":8,\"enforcementMode\":\"streaming\"},{\"dimension\":\"output_bytes\",\"limit\":16777216,\"enforcementMode\":\"streaming\"},{\"dimension\":\"wall_clock_ms\",\"limit\":3600000,\"enforcementMode\":\"streaming\"},{\"dimension\":\"idle_ms\",\"limit\":300000,\"enforcementMode\":\"streaming\"},{\"dimension\":\"context_ratio_permille\",\"limit\":950,\"enforcementMode\":\"post_facto\"}],\"createdAt\":0}',
                    created_at
             FROM turn_execution_spec;
             CREATE TABLE IF NOT EXISTS background_operation (
               id TEXT PRIMARY KEY,
               kind TEXT NOT NULL,
               source_session_id TEXT REFERENCES session(id) ON DELETE SET NULL,
               input_digest TEXT NOT NULL,
               idempotency_key TEXT NOT NULL UNIQUE,
               status TEXT NOT NULL CHECK (status IN ('committed', 'running', 'succeeded', 'failed', 'cancelled', 'delivery_unknown')),
               result_json TEXT,
               error_code TEXT,
               created_at INTEGER NOT NULL,
               started_at INTEGER,
               cancel_requested_at INTEGER,
               ended_at INTEGER
             );
             CREATE TABLE IF NOT EXISTS operation_execution_spec (
               operation_id TEXT PRIMARY KEY REFERENCES background_operation(id) ON DELETE CASCADE,
               engine_id TEXT NOT NULL,
               provider_id TEXT NOT NULL,
               provider_kind TEXT NOT NULL,
               provider_display_name TEXT NOT NULL,
               route_label_snapshot TEXT NOT NULL,
               requested_model_id TEXT NOT NULL,
               routed_model_id TEXT NOT NULL,
               model_label_snapshot TEXT NOT NULL,
               requested_reasoning_effort TEXT NOT NULL,
               routed_reasoning_effort TEXT NOT NULL,
               binding_id TEXT NOT NULL,
               binding_revision INTEGER NOT NULL,
               engine_profile_digest TEXT NOT NULL,
               provider_launch_profile_ref TEXT NOT NULL,
               provider_launch_profile_digest TEXT NOT NULL,
               launch_config_digest TEXT NOT NULL,
               routing_capability_snapshot_id TEXT NOT NULL,
               pricing_basis_snapshot_json TEXT NOT NULL,
               purpose TEXT NOT NULL,
               created_at INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS model_only_operation_policy (
               operation_id TEXT PRIMARY KEY REFERENCES background_operation(id) ON DELETE CASCADE,
               contract_version INTEGER NOT NULL,
               canonical_cwd TEXT NOT NULL CHECK (canonical_cwd = ''),
               sandbox_mode TEXT NOT NULL CHECK (sandbox_mode = 'read_only'),
               tools_disabled INTEGER NOT NULL CHECK (tools_disabled = 1),
               extensions_disabled INTEGER NOT NULL CHECK (extensions_disabled = 1),
               persistent_grants_disabled INTEGER NOT NULL CHECK (persistent_grants_disabled = 1),
               capability_snapshot_id TEXT NOT NULL,
               launch_evidence TEXT NOT NULL,
               created_at INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS operation_budget_snapshot (
               operation_id TEXT PRIMARY KEY REFERENCES background_operation(id) ON DELETE CASCADE,
               snapshot_json TEXT NOT NULL,
               created_at INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS operation_attempt (
               operation_id TEXT NOT NULL REFERENCES background_operation(id) ON DELETE CASCADE,
               attempt_no INTEGER NOT NULL,
               owner_kind TEXT NOT NULL CHECK (owner_kind = 'operation'),
               owner_id TEXT NOT NULL,
               generation_id TEXT REFERENCES runtime_generation(id),
               runtime_compatibility_key TEXT NOT NULL,
               observed_model_id TEXT,
               observed_reasoning_effort TEXT,
               actual_capability_snapshot_json TEXT,
               delivery_state TEXT NOT NULL CHECK (delivery_state IN ('prepared', 'accepted', 'rejected', 'completed', 'interrupted', 'error', 'delivery_unknown')),
               terminal_receipt TEXT,
               created_at INTEGER NOT NULL,
               accepted_at INTEGER,
               ended_at INTEGER,
               PRIMARY KEY (operation_id, attempt_no),
               CHECK (owner_id = operation_id)
             );
             CREATE INDEX IF NOT EXISTS idx_operation_attempt_recovery
               ON operation_attempt(delivery_state, created_at);
             CREATE TABLE IF NOT EXISTS operation_progress_fact (
               operation_id TEXT NOT NULL REFERENCES background_operation(id) ON DELETE CASCADE,
               attempt_no INTEGER NOT NULL,
               seq INTEGER NOT NULL,
               kind TEXT NOT NULL,
               value INTEGER,
               detail_json TEXT,
               observed_at INTEGER NOT NULL,
               PRIMARY KEY (operation_id, attempt_no, seq)
             );",
        )
        .map_err(db_err)?;
    }
    if current < 29 {
        if !column_exists(tx, "background_operation", "input_json")? {
            tx.execute_batch("ALTER TABLE background_operation ADD COLUMN input_json TEXT;")
                .map_err(db_err)?;
        }
        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS handoff (
               id TEXT PRIMARY KEY,
               operation_id TEXT NOT NULL UNIQUE REFERENCES background_operation(id) ON DELETE RESTRICT,
               source_session_id TEXT REFERENCES session(id) ON DELETE SET NULL,
               source_title_snapshot TEXT NOT NULL,
               source_engine TEXT NOT NULL,
               source_cwd_snapshot TEXT NOT NULL,
               target_engine TEXT NOT NULL,
               boundary_turn_id TEXT NOT NULL,
               boundary_turn_epoch INTEGER NOT NULL,
               content_json TEXT NOT NULL,
               created_at INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS session_fork (
               id TEXT PRIMARY KEY,
               operation_id TEXT NOT NULL UNIQUE REFERENCES background_operation(id) ON DELETE RESTRICT,
               source_session_id TEXT REFERENCES session(id) ON DELETE SET NULL,
               target_session_id TEXT NOT NULL UNIQUE REFERENCES session(id) ON DELETE CASCADE,
               handoff_id TEXT NOT NULL UNIQUE REFERENCES handoff(id) ON DELETE RESTRICT,
               target_engine TEXT NOT NULL,
               boundary_turn_id TEXT NOT NULL,
               boundary_turn_epoch INTEGER NOT NULL,
               created_at INTEGER NOT NULL
             );
             CREATE INDEX IF NOT EXISTS idx_session_fork_source
               ON session_fork(source_session_id, created_at);",
        )
        .map_err(db_err)?;
    }
    if current < 30 {
        for (column, sql) in [
            ("outcome", "ALTER TABLE tool_call ADD COLUMN outcome TEXT;"),
            (
                "tool_started",
                "ALTER TABLE tool_call ADD COLUMN tool_started INTEGER;",
            ),
            (
                "has_output",
                "ALTER TABLE tool_call ADD COLUMN has_output INTEGER;",
            ),
            (
                "retryable",
                "ALTER TABLE tool_call ADD COLUMN retryable INTEGER;",
            ),
            (
                "denial_source",
                "ALTER TABLE tool_call ADD COLUMN denial_source TEXT;",
            ),
            (
                "native_denial_code",
                "ALTER TABLE tool_call ADD COLUMN native_denial_code TEXT;",
            ),
        ] {
            if !column_exists(tx, "tool_call", column)? {
                tx.execute_batch(sql).map_err(db_err)?;
            }
        }
        for (column, sql) in [
            (
                "restorable",
                "ALTER TABLE checkpoint ADD COLUMN restorable INTEGER NOT NULL DEFAULT 0;",
            ),
            (
                "file_count",
                "ALTER TABLE checkpoint ADD COLUMN file_count INTEGER NOT NULL DEFAULT 0;",
            ),
            (
                "restorable_reason",
                "ALTER TABLE checkpoint ADD COLUMN restorable_reason TEXT;",
            ),
        ] {
            if !column_exists(tx, "checkpoint", column)? {
                tx.execute_batch(sql).map_err(db_err)?;
            }
        }
        // 旧记录没有结构化恢复事实。只把显然有效的快照引用保留为待验证 legacy，
        // 空引用、null 和设备目标始终不可恢复。
        tx.execute(
            "UPDATE checkpoint
             SET restorable = CASE
                 WHEN snapshot_ref != ''
                  AND lower(label) NOT LIKE '%null%'
                  AND lower(label) NOT LIKE '%/dev/null%'
                  AND lower(label) NOT LIKE '%\\dev\\null%'
                 THEN 1 ELSE 0 END,
                 file_count = CASE WHEN snapshot_ref != '' THEN 1 ELSE 0 END,
                 restorable_reason = CASE WHEN snapshot_ref = '' THEN 'legacy_empty_snapshot' ELSE restorable_reason END",
            [],
        )
        .map_err(db_err)?;
    }
    // Keep the mandatory Folder invariant intact even after manual DB edits or older write paths.
    tx.execute(
        "UPDATE session
         SET folder_id = 'folder-default'
         WHERE folder_id IS NULL
            OR folder_id = ''
            OR NOT EXISTS (
                SELECT 1 FROM session_folder
                WHERE session_folder.id = session.folder_id
            )",
        [],
    )
    .map_err(db_err)?;
    if current < SCHEMA_VERSION {
        tx.execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION};"))
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
        runtime_capabilities: row
            .get::<_, Option<String>>(15)?
            .and_then(|raw| serde_json::from_str(&raw).ok()),
        safe_permission_profile: row
            .get::<_, Option<String>>(16)?
            .unwrap_or_else(|| "standard".to_string()),
        folder_id: row
            .get::<_, Option<String>>(17)?
            .unwrap_or_else(|| "folder-default".to_string()),
        cached_input_tokens: row.get::<_, i64>(18)? as u64,
        cache_write_input_tokens: row.get::<_, i64>(19)? as u64,
        last_context_tokens: row.get::<_, Option<i64>>(20)?.map(|value| value as u64),
        last_context_window: row.get::<_, Option<i64>>(21)?.map(|value| value as u64),
        preferred_model: row.get(22)?,
        preferred_reasoning_effort: row.get(23)?,
    })
}

fn uuid_like_id() -> String {
    format!("{}-{:016x}", now_millis(), rand::random::<u64>())
}

fn default_safe_permission_profile() -> String {
    "standard".to_string()
}

fn load_background_operation_on_conn(
    conn: &Connection,
    column: &str,
    value: &str,
) -> Result<Option<BackgroundOperation>, String> {
    if !matches!(column, "id" | "idempotency_key") {
        return Err("BackgroundOperation 查询字段不受支持".to_string());
    }
    conn.query_row(
        &format!(
            "SELECT id, kind, source_session_id, input_digest, input_json, idempotency_key, status,
                    result_json, error_code, created_at, started_at, cancel_requested_at, ended_at
             FROM background_operation WHERE {column} = ?1"
        ),
        params![value],
        |row| {
            let input_json: Option<String> = row.get(4)?;
            let result_json: Option<String> = row.get(7)?;
            Ok(BackgroundOperation {
                id: row.get(0)?,
                kind: row.get(1)?,
                source_session_id: row.get(2)?,
                input_digest: row.get(3)?,
                input: input_json
                    .as_deref()
                    .map(serde_json::from_str)
                    .transpose()
                    .map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            4,
                            rusqlite::types::Type::Text,
                            Box::new(error),
                        )
                    })?,
                idempotency_key: row.get(5)?,
                status: row.get(6)?,
                result: result_json
                    .as_deref()
                    .map(serde_json::from_str)
                    .transpose()
                    .map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            7,
                            rusqlite::types::Type::Text,
                            Box::new(error),
                        )
                    })?,
                error_code: row.get(8)?,
                created_at: row.get(9)?,
                started_at: row.get(10)?,
                cancel_requested_at: row.get(11)?,
                ended_at: row.get(12)?,
            })
        },
    )
    .optional()
    .map_err(db_err)
}

fn db_err(error: rusqlite::Error) -> String {
    format!("会话数据库错误：{error}")
}

fn sql_text_conversion_error(index: usize, error: String) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        index,
        rusqlite::types::Type::Text,
        Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, error)),
    )
}

fn resolve_or_create_cwd_folder(
    conn: &Connection,
    canonical_cwd: &str,
    cwd_key: &str,
) -> Result<(String, bool), String> {
    if let Some(id) = conn
        .query_row(
            "SELECT id FROM session_folder WHERE cwd_key = ?1",
            params![cwd_key],
            |row| row.get(0),
        )
        .optional()
        .map_err(db_err)?
    {
        return Ok((id, false));
    }

    let id = format!("folder-{}", uuid_like_id());
    let name = project_folder_name(&canonical_cwd);
    let sort_order: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(sort_order), -1) + 1 FROM session_folder",
            [],
            |row| row.get(0),
        )
        .map_err(db_err)?;
    let inserted = conn
        .execute(
            "INSERT OR IGNORE INTO session_folder
         (id, name, sort_order, collapsed, locked, created_at, cwd, cwd_key)
         VALUES (?1, ?2, ?3, 0, 0, ?4, ?5, ?6)",
            params![id, name, sort_order, now_millis(), canonical_cwd, cwd_key],
        )
        .map_err(db_err)?;
    let folder_id = conn
        .query_row(
            "SELECT id FROM session_folder WHERE cwd_key = ?1",
            params![cwd_key],
            |row| row.get(0),
        )
        .map_err(db_err)?;
    Ok((folder_id, inserted == 1))
}

fn canonical_folder_cwd(cwd: &str) -> Result<(String, String), String> {
    let canonical = Path::new(cwd).canonicalize().map_err(|error| {
        format!(
            "工作目录不存在：{}。请重新选择一个有效目录（{error}）",
            Path::new(cwd).display()
        )
    })?;
    if !canonical.is_dir() {
        return Err(format!(
            "工作目录不存在或不是文件夹：{}。请重新选择一个有效目录",
            canonical.display()
        ));
    }
    let display = strip_extended_path_prefix(&canonical.to_string_lossy());
    let mut key = display.replace('\\', "/").trim_end_matches('/').to_string();
    #[cfg(windows)]
    {
        key = key.to_lowercase();
    }
    Ok((display, key))
}

pub(crate) fn strip_extended_path_prefix(value: &str) -> String {
    let normalized = value.replace('\\', "/");
    if let Some(rest) = normalized.strip_prefix("//?/") {
        if rest
            .get(..4)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("UNC/"))
        {
            return format!("//{}", &rest[4..]).replace('/', "\\");
        }
        return rest.replace('/', "\\");
    }
    value.to_string()
}

fn project_folder_name(canonical_cwd: &str) -> String {
    let candidate = Path::new(canonical_cwd)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.trim().is_empty())
        .unwrap_or(canonical_cwd);
    candidate.chars().take(80).collect()
}

fn normalized_path_within(path: &str, root: &str) -> bool {
    let path = normalize_scope_path(path);
    let root = normalize_scope_path(root);
    path == root || path.starts_with(&format!("{root}/"))
}

fn normalize_scope_path(value: &str) -> String {
    let mut path = value.replace('\\', "/");
    // Windows may report the same directory as `C:/...` or `//?/C:/...`.
    // Strip only the extended-path namespace; device paths (`//./...`) remain
    // distinct and are rejected by the filesystem boundary checks elsewhere.
    if let Some(rest) = path.strip_prefix("//?/") {
        path = if rest
            .get(..4)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("UNC/"))
        {
            format!("//{}", &rest[4..])
        } else {
            rest.to_string()
        };
    }
    path.trim_end_matches('/').to_ascii_lowercase()
}

fn runtime_grant_record_matches(
    grant: &RuntimeGrantRecord,
    action: &ActionDescriptor,
    provider_id: &str,
) -> bool {
    let Some(matcher) = crate::permissions::runtime_grant_matcher(action) else {
        return false;
    };
    let Some(adapter_version) =
        crate::permissions::runtime_approval_adapter_version(&action.engine)
    else {
        return false;
    };
    let scope_matches = if grant.scope == "project" {
        match (grant.project_root.as_deref(), action.cwd.as_deref()) {
            (Some(root), Some(cwd)) => normalized_path_within(cwd, root),
            _ => false,
        }
    } else {
        grant.scope == "global"
    };
    grant.engine == action.engine
        && grant.provider_id == provider_id
        && grant.adapter_version == adapter_version
        && grant.ceiling_version == crate::permissions::RUNTIME_GRANT_CEILING_VERSION
        && grant.matcher_kind == matcher.kind
        && grant.matcher_value == matcher.value
        && scope_matches
}

fn turn_status_to_str(status: crate::turn_supervisor::TurnStatus) -> &'static str {
    use crate::turn_supervisor::TurnStatus;
    match status {
        TurnStatus::Running => "running",
        TurnStatus::WaitingApproval => "waiting_approval",
        TurnStatus::Stalled => "stalled",
        TurnStatus::Succeeded => "succeeded",
        TurnStatus::Failed => "failed",
        TurnStatus::Interrupted => "interrupted",
    }
}

fn parse_turn_status(value: &str) -> Result<crate::turn_supervisor::TurnStatus, String> {
    use crate::turn_supervisor::TurnStatus;
    match value {
        "running" => Ok(TurnStatus::Running),
        "waiting_approval" => Ok(TurnStatus::WaitingApproval),
        "stalled" => Ok(TurnStatus::Stalled),
        "succeeded" => Ok(TurnStatus::Succeeded),
        "failed" => Ok(TurnStatus::Failed),
        "interrupted" => Ok(TurnStatus::Interrupted),
        other => Err(format!("invalid persisted turn status: {other}")),
    }
}

fn stable_serde_string<T: Serialize>(value: &T) -> Result<String, String> {
    match serde_json::to_value(value).map_err(|error| error.to_string())? {
        serde_json::Value::String(value) => Ok(value),
        value => Err(format!("权限枚举未序列化为稳定字符串：{value}")),
    }
}

fn stable_serde_from_string<T: serde::de::DeserializeOwned>(
    value: String,
    column: usize,
) -> rusqlite::Result<T> {
    serde_json::from_value(serde_json::Value::String(value)).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            column,
            rusqlite::types::Type::Text,
            Box::new(error),
        )
    })
}

fn permission_rules_for_conn(conn: &Connection) -> Result<Vec<PermissionRule>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id, principal, effect, scope, tool_call_id, turn_id, history_session_id, project_root,
                    engine, capability, operation, resource_pattern, created_at,
                    expires_at, max_uses, uses
             FROM permission_rule
             ORDER BY created_at ASC, id ASC",
        )
        .map_err(db_err)?;
    let rules = stmt
        .query_map([], |row| {
            let max_uses = row
                .get::<_, Option<i64>>(14)?
                .map(|value| {
                    u32::try_from(value).map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            14,
                            rusqlite::types::Type::Integer,
                            Box::new(error),
                        )
                    })
                })
                .transpose()?;
            let uses = u32::try_from(row.get::<_, i64>(15)?).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    15,
                    rusqlite::types::Type::Integer,
                    Box::new(error),
                )
            })?;
            Ok(PermissionRule {
                id: row.get(0)?,
                principal: row.get(1)?,
                effect: stable_serde_from_string(row.get(2)?, 2)?,
                scope: stable_serde_from_string(row.get(3)?, 3)?,
                scope_binding: crate::permissions::PermissionScopeBinding {
                    tool_call_id: row.get(4)?,
                    turn_id: row.get(5)?,
                    session_id: row.get(6)?,
                    project_root: row.get(7)?,
                },
                engine: row.get(8)?,
                capability: stable_serde_from_string(row.get(9)?, 9)?,
                operation: row.get(10)?,
                resource_pattern: row.get(11)?,
                created_at: row.get(12)?,
                expires_at: row.get(13)?,
                max_uses,
                uses,
            })
        })
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)?;
    Ok(rules)
}

fn action_fingerprint(action: &ActionDescriptor) -> Result<String, String> {
    let bytes = serde_json::to_vec(action).map_err(|e| e.to_string())?;
    let digest = Sha256::digest(bytes);
    Ok(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn safe_read_effect(action: &ActionDescriptor) -> PermissionEffect {
    // 结构化读取先经过统一的动作与工作区边界判定，再走固定系统策略；
    // 不读取或反序列化用户设置。符合边界的动作仍进入 Kernel，使显式 Deny 保持优先。
    if matches!(
        action.capability,
        Capability::FileRead | Capability::DirectoryList
    ) {
        return if crate::permissions::safe_read_action_is_eligible(action) {
            PermissionEffect::Allow
        } else {
            PermissionEffect::Deny
        };
    }
    PermissionEffect::Deny
}

/// auto 档对工作区内结构化写（fileChange）的固定自动放行（术语表「自动执行」）。
/// 仅当会话 safe profile 为 `auto`、且所有写目标可证明落在 session workspace 根内
/// 且非敏感时返回 Allow；越界/敏感 fail-closed 返回 Deny；其余档位统一 Ask（保持现状）。
/// 显式 Deny 由 Kernel 优先于本固定 Allow（参见 evaluate_permission_action_inner 顺序）。
fn safe_file_write_effect(
    action: &ActionDescriptor,
    safe_profile: &str,
    conn: &Connection,
) -> Result<PermissionEffect, String> {
    if action.capability != Capability::FileWrite || safe_profile != "auto" {
        return Ok(PermissionEffect::Ask);
    }
    let session_cwd = conn
        .query_row("SELECT cwd FROM session WHERE id = ?1", params![action.session_id], |row| {
            row.get::<_, String>(0)
        })
        .optional()
        .map_err(db_err)?;
    let Some(session_cwd) = session_cwd.filter(|root| !root.is_empty()) else {
        return Ok(PermissionEffect::Ask);
    };
    let eligible = crate::permissions::safe_file_write_resources_within(action, &session_cwd);
    Ok(if eligible {
        PermissionEffect::Allow
    } else {
        PermissionEffect::Deny
    })
}

fn permission_policy_version_on_conn(conn: &Connection) -> Result<u64, String> {
    let raw: Option<String> = conn
        .query_row(
            "SELECT value_json FROM setting WHERE key = ?1",
            params![PERMISSION_POLICY_VERSION_KEY],
            |row| row.get(0),
        )
        .optional()
        .map_err(db_err)?;
    raw.map(|value| serde_json::from_str::<u64>(&value).map_err(|e| e.to_string()))
        .transpose()
        .map(|version| version.unwrap_or(1))
}

fn bump_permission_policy_version_on_conn(conn: &Connection) -> Result<u64, String> {
    let next = permission_policy_version_on_conn(conn)?.saturating_add(1);
    conn.execute(
        "INSERT INTO setting (key, value_json) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json",
        params![
            PERMISSION_POLICY_VERSION_KEY,
            serde_json::to_string(&next).map_err(|e| e.to_string())?
        ],
    )
    .map_err(db_err)?;
    Ok(next)
}

fn insert_permission_audit(
    conn: &Connection,
    action: &ActionDescriptor,
    fingerprint: &str,
    decision: &PermissionDecision,
    created_at: i64,
) -> Result<(), String> {
    conn.execute(
        "INSERT INTO permission_audit
         (history_session_id, turn_id, tool_call_id, action_fingerprint, principal,
          engine, capability, operation, resources_json, effect, reason, rule_id,
          policy_version, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
        params![
            action.session_id,
            action.turn_id,
            action.tool_call_id,
            fingerprint,
            action.principal,
            action.engine,
            stable_serde_string(&action.capability)?,
            action.operation,
            serde_json::to_string(&action.resources).map_err(|e| e.to_string())?,
            stable_serde_string(&decision.effect)?,
            decision.reason,
            decision.rule_id,
            i64::try_from(decision.policy_version).map_err(|e| e.to_string())?,
            created_at,
        ],
    )
    .map_err(db_err)?;
    Ok(())
}

fn legacy_always_allow_rule(tool_key: &str, created_at: i64) -> PermissionRule {
    let (tool_name, bash_operation) = tool_key
        .split_once(':')
        .map_or((tool_key, None), |(name, operation)| {
            (name, (!operation.is_empty()).then_some(operation))
        });
    let (capability, operation) = match tool_name {
        "Read" | "Glob" | "Grep" | "LS" => (Capability::FileRead, Some(tool_name.to_string())),
        "Write" | "Edit" | "MultiEdit" | "NotebookEdit" => {
            (Capability::FileWrite, Some(tool_name.to_string()))
        }
        "Bash" => (
            Capability::ProcessExec,
            bash_operation.map(ToString::to_string),
        ),
        "WebFetch" | "WebSearch" => (Capability::NetworkRequest, Some(tool_name.to_string())),
        name if name.starts_with("mcp__") => (Capability::McpInvoke, Some(name.to_string())),
        name => (
            Capability::Unknown(name.to_string()),
            Some(name.to_string()),
        ),
    };
    let encoded_key = tool_key
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    PermissionRule {
        id: format!("legacy-always-allow:{encoded_key}"),
        principal: "main-agent".to_string(),
        effect: PermissionEffect::Allow,
        scope: PermissionScope::Global,
        scope_binding: Default::default(),
        // 旧规则只在 Claude hook 生效；迁移不得静默扩大到其他 Engine。
        engine: Some("claude-code".to_string()),
        capability,
        operation,
        resource_pattern: None,
        created_at,
        expires_at: None,
        max_uses: None,
        uses: 0,
    }
}

fn upsert_approval_request(
    conn: &Connection,
    session_id: &str,
    approval_id: &str,
    action: &str,
    detail: &str,
    persistent_label: Option<&str>,
    matcher_summary: Option<&str>,
    turn_id: Option<&str>,
    ts: i64,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO approval
         (id, session_id, action, detail, status, ts, persistent_label, matcher_summary, turn_id)
         VALUES (?1, ?2, ?3, ?4, 'pending', ?5, ?6, ?7, ?8)
         ON CONFLICT(id, session_id) DO UPDATE SET
           action = excluded.action,
           detail = excluded.detail,
           ts = excluded.ts,
           persistent_label = excluded.persistent_label,
           matcher_summary = excluded.matcher_summary,
           turn_id = excluded.turn_id
         WHERE approval.status = 'pending'",
        params![
            approval_id,
            session_id,
            action,
            detail,
            ts,
            persistent_label,
            matcher_summary,
            turn_id
        ],
    )?;
    Ok(())
}

fn ensure_approval_transition(
    conn: &Connection,
    session_id: &str,
    approval_id: &str,
    changed: usize,
    action: &str,
    expected_decision: Option<&str>,
) -> Result<(), String> {
    if changed == 1 {
        return Ok(());
    }
    let approval: Option<(String, Option<String>)> = conn
        .query_row(
            "SELECT status, decision FROM approval WHERE session_id = ?1 AND id = ?2",
            params![session_id, approval_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(db_err)?;
    match approval {
        Some((status, decision))
            if status == "applying"
                && expected_decision.is_some()
                && decision.as_deref() != expected_decision =>
        {
            Err(format!(
                "审批决定不一致：{approval_id}（账本：{}，请求：{}）",
                decision.as_deref().unwrap_or("<none>"),
                expected_decision.unwrap_or("<none>")
            ))
        }
        Some((status, _)) => Err(format!(
            "审批状态不允许{action}：{approval_id}（当前状态：{status}）"
        )),
        None => Err(format!("审批不存在：{approval_id}")),
    }
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
    pub actual_cost: f64,
    pub estimated_cost: f64,
    pub subscription_count: u32,
    pub unknown_count: u32,
    /// v12 以前只保存金额、没有来源证据的历史花费；不能冒充实际或估算。
    pub legacy_cost: f64,
    pub legacy_count: u32,
    pub previous_total_cost: f64,
    pub previous_total_tokens: u64,
    pub previous_request_count: u32,
    pub previous_session_count: u32,
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

fn tool_outcome_to_str(outcome: &ToolOutcomeKind) -> &'static str {
    match outcome {
        ToolOutcomeKind::ToolSucceeded => "tool_succeeded",
        ToolOutcomeKind::AutoReviewUnavailable => "auto_review_unavailable",
        ToolOutcomeKind::AutoReviewParseError => "auto_review_parse_error",
        ToolOutcomeKind::AutoReviewBlocked => "auto_review_blocked",
        ToolOutcomeKind::RuntimeDenied => "runtime_denied",
        ToolOutcomeKind::ToolFailed => "tool_failed",
    }
}

fn tool_denial_source_to_str(source: &ToolDenialSource) -> &'static str {
    match source {
        ToolDenialSource::AutoReviewer => "auto_reviewer",
        ToolDenialSource::Runtime => "runtime",
        ToolDenialSource::Tool => "tool",
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

// message/tool_call 的 ts 单位（变更-07）：毫秒（now_millis），与 checkpoint.ts 同单位。
// session.created_at/updated_at 与 usage.ts 维持秒（now_seconds，用量按 date(ts,'unixepoch') 聚合）。

#[cfg(test)]
mod path_prefix_tests {
    use super::strip_extended_path_prefix;

    #[test]
    fn strips_windows_verbatim_drive_prefix() {
        assert_eq!(
            strip_extended_path_prefix(r"\\?\D:\projects\迁移工作区"),
            r"D:\projects\迁移工作区"
        );
    }

    #[test]
    fn leaves_plain_windows_path_untouched() {
        assert_eq!(
            strip_extended_path_prefix(r"D:\other\projects\workspace"),
            r"D:\other\projects\workspace"
        );
    }

    #[test]
    fn restores_unc_from_verbatim_form() {
        assert_eq!(
            strip_extended_path_prefix(r"\\?\UNC\server\share\dir"),
            r"\\server\share\dir"
        );
    }
}

