use helm_lib::budget::TurnBudgetSnapshot;
use helm_lib::capability_registry::{CapabilityIdentity, CapabilitySet, EngineCapabilitySnapshot};
use helm_lib::operations::{
    BackgroundOperation, ModelOnlyOperationOutput, ModelOnlyOperationPolicy,
    NewBackgroundOperation, OperationExecutionSpec,
};
use helm_lib::permissions::normalize_tool_action;
use helm_lib::permissions::{
    Capability, PermissionEffect, PermissionRule, PermissionScope, PermissionScopeBinding,
};
use helm_lib::pricing::{PricingBand, PricingTier, ResolvedPricingProfile, ServiceTier};
use helm_lib::protocol::{
    AgentEvent, CallStatus, Diff, DiffHunk, DiffKind, DiffLine, EngineId, Role,
    RuntimeCapabilitySnapshot, StopReason, ToolStatus,
};
use helm_lib::providers::BindingConfig;
use helm_lib::reasoning::ReasoningEffort;
use helm_lib::runtime_registry::{RuntimeGeneration, RuntimeOwnerRef, RuntimeRegistry};
use helm_lib::sessions::{
    HistoryToolStatus, NewSessionRecord, SessionHistoryStore, SessionStatus,
    UsageBreakdownDimension, SCHEMA_VERSION,
};
use helm_lib::turn_start::{
    BindingLiveRouteResolver, PricingBasisSnapshot, RuntimeRoute, TurnExecutionSpec,
    TurnStartCommand,
};
use helm_lib::turn_supervisor::{CandidateDisposition, EngineEventCandidate, TurnSupervisor};
use std::collections::HashMap;
use std::fs;
use std::sync::{Arc, Barrier};
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

fn seed_test_capability(store: &SessionHistoryStore, model: &str) -> String {
    let identity = CapabilityIdentity {
        engine_id: "codex".into(),
        adapter_version: "test".into(),
        binary_identity: "test-binary".into(),
        engine_profile_digest: "sha256:engine".into(),
        provider_launch_profile_ref: "provider:provider-legacy:api".into(),
        provider_launch_profile_digest: "sha256:provider-launch".into(),
        launch_profile_identity: "sha256:launch".into(),
        model_capability_key: model.into(),
    };
    let cache_key = identity.cache_key().unwrap();
    let snapshot = EngineCapabilitySnapshot {
        id: format!("capability-{model}"),
        identity,
        capabilities: CapabilitySet::unknown("test_fixture"),
        probe_kind: "test_fixture".into(),
        probed_at: 1,
    };
    store
        .save_capability_snapshot(&cache_key, &snapshot)
        .unwrap();
    snapshot.id
}

#[test]
fn change_27i_turn_and_operation_budgets_are_atomic_and_idempotent() {
    let path = temp_history_path("change-27i-atomic-budgets");
    let store = SessionHistoryStore::new(path.clone());
    store
        .create_session(NewSessionRecord {
            id: "session-27i".into(),
            engine: EngineId::ClaudeCode,
            model: "claude-fixture".into(),
            cwd: std::env::temp_dir().to_string_lossy().to_string(),
            created_at: 1,
        })
        .unwrap();
    let turn = start_change_27d_turn(&store, "session-27i", "budget", Vec::new());
    let conn = rusqlite::Connection::open(&path).unwrap();
    let raw_budget: String = conn
        .query_row(
            "SELECT snapshot_json FROM turn_budget_snapshot WHERE turn_id = ?1",
            [&turn.turn_id],
            |row| row.get(0),
        )
        .unwrap();
    let turn_budget: TurnBudgetSnapshot = serde_json::from_str(&raw_budget).unwrap();
    turn_budget.validate().unwrap();
    drop(conn);

    let created_at = 2;
    let capability_id = seed_test_capability(&store, "claude-fixture");
    let operation_id = "operation-27i".to_string();
    let spec = OperationExecutionSpec {
        operation_id: operation_id.clone(),
        owner: RuntimeOwnerRef::Operation(operation_id.clone()),
        purpose: "auto_title".into(),
        engine_id: "claude-code".into(),
        provider_id: "provider-fixture".into(),
        provider_kind: "api".into(),
        provider_display_name: "Fixture".into(),
        route_label_snapshot: "Fixture / Claude".into(),
        requested_model_id: "claude-fixture".into(),
        routed_model_id: "claude-fixture".into(),
        model_label_snapshot: "Claude Fixture".into(),
        requested_reasoning_effort: ReasoningEffort::Auto,
        routed_reasoning_effort: ReasoningEffort::Auto,
        binding_id: "claude-code".into(),
        binding_revision: 7,
        engine_profile_digest: "sha256:engine".into(),
        provider_launch_profile_ref: "provider:provider-fixture:api".into(),
        provider_launch_profile_digest: "sha256:profile".into(),
        launch_config_digest: "sha256:launch".into(),
        routing_capability_snapshot_id: capability_id.clone(),
        pricing_basis_snapshot: PricingBasisSnapshot { profile: None },
        created_at,
    };
    let policy = ModelOnlyOperationPolicy {
        contract_version: 1,
        canonical_cwd: String::new(),
        sandbox_mode: "read_only".into(),
        tools_disabled: true,
        extensions_disabled: true,
        persistent_grants_disabled: true,
        capability_snapshot_id: spec.routing_capability_snapshot_id.clone(),
        launch_evidence: "claude_help_contract:claude_empty_tools_launch_verified".into(),
        created_at,
    };
    let operation = BackgroundOperation {
        id: operation_id,
        kind: "auto_title".into(),
        source_session_id: Some("session-27i".into()),
        input_digest: "sha256:input".into(),
        input: None,
        idempotency_key: "auto-title:session-27i:first-turn".into(),
        status: "committed".into(),
        result: None,
        error_code: None,
        created_at,
        started_at: None,
        cancel_requested_at: None,
        ended_at: None,
    };
    let new_operation = NewBackgroundOperation {
        operation,
        spec,
        policy,
        budget: TurnBudgetSnapshot::standard(created_at),
    };
    let (first, inserted) = store.create_background_operation(&new_operation).unwrap();
    assert!(inserted);
    let (second, inserted) = store.create_background_operation(&new_operation).unwrap();
    assert!(!inserted);
    assert_eq!(first.id, second.id);

    let conn = rusqlite::Connection::open(path).unwrap();
    for table in [
        "background_operation",
        "operation_execution_spec",
        "model_only_operation_policy",
        "operation_budget_snapshot",
    ] {
        let count: i64 = conn
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 1, "{table} 必须与 operation 原子且幂等创建");
    }
    assert!(conn
        .execute(
            "INSERT INTO usage
             (session_id, operation_id, model, input_tokens, output_tokens, cost_usd, ts)
             VALUES (NULL, NULL, 'invalid', 0, 0, 0, 1)",
            [],
        )
        .is_err());
    assert!(conn
        .execute(
            "INSERT INTO usage
             (session_id, operation_id, model, input_tokens, output_tokens, cost_usd, ts)
             VALUES (?1, ?2, 'invalid', 0, 0, 0, 1)",
            rusqlite::params!["session-27i", first.id],
        )
        .is_err());
    drop(conn);

    let restored = store
        .load_background_operation_execution(&first.id)
        .unwrap()
        .unwrap();
    assert_eq!(restored.spec.binding_revision, 7);
    assert_eq!(restored.spec.routed_model_id, "claude-fixture");
    assert_eq!(
        restored.policy.capability_snapshot_id,
        restored.spec.routing_capability_snapshot_id
    );
    restored.budget.validate().unwrap();

    assert!(store
        .request_background_operation_cancel(&first.id)
        .unwrap());
    assert_eq!(
        store
            .load_background_operation(&first.id)
            .unwrap()
            .unwrap()
            .status,
        "cancelled"
    );
    store.prepare_background_operation_retry(&first.id).unwrap();
    let generation = RuntimeGeneration {
        id: "operation-generation-27i".into(),
        owner: RuntimeOwnerRef::Operation(first.id.clone()),
        engine_id: "claude-code".into(),
        compatibility_key: "sha256:compatibility".into(),
        engine_profile_digest: "sha256:engine".into(),
        provider_launch_profile_ref: "provider:provider-fixture:api".into(),
        provider_launch_profile_digest: "sha256:profile".into(),
        capability_snapshot_id: capability_id,
        canonical_cwd: String::new(),
        created_at: 3,
    };
    store.create_runtime_generation(&generation).unwrap();
    let attempt_no = store
        .create_operation_attempt(&first.id, &generation)
        .unwrap();
    store
        .mark_operation_attempt_accepted(&first.id, attempt_no)
        .unwrap();
    assert_eq!(store.reconcile_background_operations().unwrap(), 1);
    assert_eq!(
        store
            .load_background_operation(&first.id)
            .unwrap()
            .unwrap()
            .status,
        "delivery_unknown"
    );
    assert_eq!(store.reconcile_background_operations().unwrap(), 0);
    store.prepare_background_operation_retry(&first.id).unwrap();
    assert_eq!(
        store
            .load_background_operation(&first.id)
            .unwrap()
            .unwrap()
            .status,
        "committed"
    );
}

#[test]
fn change_27j_fork_completion_is_atomic_and_survives_source_deletion() {
    let path = temp_history_path("change-27j-fork-atomic");
    let store = SessionHistoryStore::new(path.clone());
    store
        .create_session(NewSessionRecord {
            id: "source-27j".into(),
            engine: EngineId::ClaudeCode,
            model: "claude-source".into(),
            cwd: std::env::temp_dir().to_string_lossy().to_string(),
            created_at: 1,
        })
        .unwrap();
    let capability_id = seed_test_capability(&store, "codex-target");
    let operation_id = "fork-operation-27j".to_string();
    let frozen = serde_json::json!({
        "sourceSessionId": "source-27j",
        "sourceTitle": "源会话",
        "sourceEngine": "claude-code",
        "sourceCwd": std::env::temp_dir().to_string_lossy(),
        "sourceFolderId": "folder-default",
        "targetEngine": "codex",
        "boundaryTurnId": "turn-boundary-27j",
        "boundaryTurnEpoch": 3,
        "ledgerJson": "[]"
    });
    let new_operation = NewBackgroundOperation {
        operation: BackgroundOperation {
            id: operation_id.clone(),
            kind: "fork_job".into(),
            source_session_id: Some("source-27j".into()),
            input_digest: helm_lib::turn_start::digest_json(
                &serde_json::from_value::<helm_lib::handoff::FrozenForkInput>(frozen.clone())
                    .unwrap(),
            )
            .unwrap(),
            input: Some(frozen.clone()),
            idempotency_key: "fork:source-27j:turn-boundary-27j:codex".into(),
            status: "committed".into(),
            result: None,
            error_code: None,
            created_at: 2,
            started_at: None,
            cancel_requested_at: None,
            ended_at: None,
        },
        spec: OperationExecutionSpec {
            operation_id: operation_id.clone(),
            owner: RuntimeOwnerRef::Operation(operation_id.clone()),
            purpose: "fork_job".into(),
            engine_id: "codex".into(),
            provider_id: "provider-target".into(),
            provider_kind: "api".into(),
            provider_display_name: "Target".into(),
            route_label_snapshot: "Target / Codex".into(),
            requested_model_id: "codex-target".into(),
            routed_model_id: "codex-target".into(),
            model_label_snapshot: "Codex Target".into(),
            requested_reasoning_effort: ReasoningEffort::Auto,
            routed_reasoning_effort: ReasoningEffort::Auto,
            binding_id: "binding:codex".into(),
            binding_revision: 9,
            engine_profile_digest: "sha256:engine".into(),
            provider_launch_profile_ref: "provider:provider-target:api".into(),
            provider_launch_profile_digest: "sha256:profile".into(),
            launch_config_digest: "sha256:launch".into(),
            routing_capability_snapshot_id: capability_id.clone(),
            pricing_basis_snapshot: PricingBasisSnapshot { profile: None },
            created_at: 2,
        },
        policy: ModelOnlyOperationPolicy {
            contract_version: 1,
            canonical_cwd: String::new(),
            sandbox_mode: "read_only".into(),
            tools_disabled: true,
            extensions_disabled: true,
            persistent_grants_disabled: true,
            capability_snapshot_id: capability_id.clone(),
            launch_evidence: "fixture:no-tools".into(),
            created_at: 2,
        },
        budget: TurnBudgetSnapshot::standard(2),
    };
    store.create_background_operation(&new_operation).unwrap();
    assert!(store.get_session("target-27j").is_err());
    let generation = RuntimeGeneration {
        id: "fork-generation-27j".into(),
        owner: RuntimeOwnerRef::Operation(operation_id.clone()),
        engine_id: "codex".into(),
        compatibility_key: "sha256:compatibility".into(),
        engine_profile_digest: "sha256:engine".into(),
        provider_launch_profile_ref: "provider:provider-target:api".into(),
        provider_launch_profile_digest: "sha256:profile".into(),
        capability_snapshot_id: capability_id,
        canonical_cwd: String::new(),
        created_at: 3,
    };
    store.create_runtime_generation(&generation).unwrap();
    let attempt = store
        .create_operation_attempt(&operation_id, &generation)
        .unwrap();
    store
        .mark_operation_attempt_accepted(&operation_id, attempt)
        .unwrap();
    let handoff = serde_json::json!({
        "contractVersion": 1,
        "goal": "继续实现 27J",
        "completed": ["冻结边界"],
        "currentState": "等待后续 Turn",
        "decisionsAndFiles": ["src-tauri/src/handoff.rs"],
        "remaining": ["人工验收"],
        "constraints": ["细节可能有损"]
    });
    store
        .complete_model_only_operation(
            &operation_id,
            attempt,
            &ModelOnlyOperationOutput {
                text: handoff.to_string(),
                input_tokens: 10,
                cached_input_tokens: 0,
                cache_write_input_tokens: 0,
                output_tokens: 5,
                reported_cost_usd: None,
                service_tier: None,
                observed_model_id: Some("codex-target".into()),
            },
            &serde_json::json!({
                "handoff": handoff,
                "frozenInput": frozen,
                "targetSessionId": "target-27j"
            }),
        )
        .unwrap();

    let target = store.get_session("target-27j").unwrap();
    let fork = target.fork.unwrap();
    assert_eq!(fork.source_session_id.as_deref(), Some("source-27j"));
    assert!(store
        .session_handoff_context("target-27j")
        .unwrap()
        .unwrap()
        .contains("细节可能有损"));
    let conn = rusqlite::Connection::open(&path).unwrap();
    let owners: (i64, i64) = conn
        .query_row(
            "SELECT COUNT(operation_id), COUNT(session_id) FROM usage WHERE operation_id = ?1",
            [&operation_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(owners, (1, 0));
    drop(conn);

    store.delete_session("source-27j").unwrap();
    let target = store.get_session("target-27j").unwrap();
    let fork = target.fork.unwrap();
    assert!(fork.source_session_id.is_none());
    assert_eq!(fork.source_title_snapshot, "源会话");
    assert!(store
        .session_handoff_context("target-27j")
        .unwrap()
        .is_some());
}

#[test]
fn change_27g_migrates_v25_capability_cache_identity_to_v26() {
    let path = temp_history_path("change-27g-v25-v26");
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch(
        "CREATE TABLE runtime_generation (
           id TEXT PRIMARY KEY,
           owner_kind TEXT NOT NULL,
           owner_id TEXT NOT NULL,
           engine_id TEXT NOT NULL,
           compatibility_key TEXT NOT NULL,
           engine_profile_digest TEXT NOT NULL,
           provider_launch_profile_ref TEXT NOT NULL,
           provider_launch_profile_digest TEXT NOT NULL,
           canonical_cwd TEXT NOT NULL,
           status TEXT NOT NULL,
           created_at INTEGER NOT NULL,
           ended_at INTEGER
         );
         PRAGMA user_version = 25;",
    )
    .unwrap();
    drop(conn);

    let store = SessionHistoryStore::new(path.clone());
    store.list_sessions().unwrap();
    let conn = rusqlite::Connection::open(&path).unwrap();
    assert_eq!(
        conn.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        SCHEMA_VERSION
    );
    let capability_column: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('runtime_generation')
             WHERE name = 'capability_snapshot_id'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(capability_column, 1);
    let capability_table: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master
             WHERE type = 'table' AND name = 'capability_snapshot'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(capability_table, 1);
    drop(conn);
    let _ = fs::remove_file(path);
}

#[test]
fn change_27h_migrates_session_turn_preferences_through_v28() {
    let path = temp_history_path("change-27h-v26-v27");
    let store = SessionHistoryStore::new(path.clone());
    store
        .create_session(NewSessionRecord {
            id: "session-27h-pref".into(),
            engine: EngineId::Codex,
            model: "gpt-old".into(),
            cwd: temp_project_dir("change-27h-pref")
                .to_string_lossy()
                .to_string(),
            created_at: 1,
        })
        .unwrap();
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(
            "ALTER TABLE session DROP COLUMN preferred_reasoning_effort;
             ALTER TABLE session DROP COLUMN preferred_model;
             PRAGMA user_version = 26;",
        )
        .unwrap();
    }
    let reopened = SessionHistoryStore::new(path.clone());
    let detail = reopened.get_session("session-27h-pref").unwrap();
    assert_eq!(detail.summary.preferred_model.as_deref(), Some("gpt-old"));
    assert_eq!(detail.summary.preferred_reasoning_effort, None);
    reopened
        .set_session_turn_preference("session-27h-pref", "gpt-next", Some("high"))
        .unwrap();
    let detail = reopened.get_session("session-27h-pref").unwrap();
    assert_eq!(detail.summary.preferred_model.as_deref(), Some("gpt-next"));
    assert_eq!(
        detail.summary.preferred_reasoning_effort.as_deref(),
        Some("high")
    );
    let conn = rusqlite::Connection::open(path).unwrap();
    assert_eq!(
        conn.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        SCHEMA_VERSION
    );
}

#[test]
fn change_27i_migrates_27h_v27_to_v28_without_losing_preferences_or_usage() {
    let path = temp_history_path("change-27i-v27-v28");
    let store = SessionHistoryStore::new(path.clone());
    store
        .create_session(NewSessionRecord {
            id: "session-v27-v28".into(),
            engine: EngineId::Codex,
            model: "gpt-old".into(),
            cwd: temp_project_dir("change-27i-v27-v28")
                .to_string_lossy()
                .to_string(),
            created_at: 1,
        })
        .unwrap();
    store
        .set_session_turn_preference("session-v27-v28", "gpt-next", Some("high"))
        .unwrap();
    drop(store);

    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch(
        "PRAGMA foreign_keys = OFF;
         DROP TABLE operation_progress_fact;
         DROP TABLE operation_attempt;
         DROP TABLE operation_budget_snapshot;
         DROP TABLE model_only_operation_policy;
         DROP TABLE operation_execution_spec;
         DROP TABLE background_operation;
         DROP TABLE turn_budget_fact;
         DROP TABLE turn_budget_snapshot;
         ALTER TABLE usage RENAME TO usage_v28;
         CREATE TABLE usage (
           id INTEGER PRIMARY KEY AUTOINCREMENT,
           session_id TEXT NOT NULL REFERENCES session(id) ON DELETE CASCADE,
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
           model_evidence TEXT NOT NULL DEFAULT 'legacy_unbound'
         );
         INSERT INTO usage
           (session_id, model, provider_id, input_tokens, output_tokens, cost_usd, ts)
         VALUES ('session-v27-v28', 'gpt-old', 'provider-old', 11, 7, 0.25, 2);
         DROP TABLE usage_v28;
         CREATE INDEX idx_usage_turn ON usage(session_id, turn_id);
         PRAGMA user_version = 27;
         PRAGMA foreign_keys = ON;",
    )
    .unwrap();
    drop(conn);

    for _ in 0..2 {
        let reopened = SessionHistoryStore::new(path.clone());
        let detail = reopened.get_session("session-v27-v28").unwrap();
        assert_eq!(detail.summary.preferred_model.as_deref(), Some("gpt-next"));
        assert_eq!(
            detail.summary.preferred_reasoning_effort.as_deref(),
            Some("high")
        );
    }

    let conn = rusqlite::Connection::open(path).unwrap();
    assert_eq!(
        conn.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        SCHEMA_VERSION
    );
    let usage: (i64, i64, Option<String>) = conn
        .query_row(
            "SELECT input_tokens, output_tokens, operation_id FROM usage
             WHERE session_id = 'session-v27-v28'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(usage, (11, 7, None));
    for table in [
        "turn_budget_snapshot",
        "background_operation",
        "operation_execution_spec",
        "operation_attempt",
        "operation_progress_fact",
        "handoff",
        "session_fork",
    ] {
        let exists: i64 = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
                [table],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(exists, 1, "缺少 v28~v29 表 {table}");
    }
    let foreign_key_errors = conn
        .prepare("PRAGMA foreign_key_check")
        .unwrap()
        .query_map([], |_| Ok(()))
        .unwrap()
        .count();
    assert_eq!(foreign_key_errors, 0);
}

#[test]
fn change_29_migrates_tool_outcome_and_checkpoint_recovery_facts_from_v29_to_v30() {
    let path = temp_history_path("change-29-v29-v30");
    let store = SessionHistoryStore::new(path.clone());
    store
        .create_session(NewSessionRecord {
            id: "session-v29-v30".into(),
            engine: EngineId::ClaudeCode,
            model: "mimo-v2.5-pro".into(),
            cwd: temp_project_dir("change-29-v29-v30")
                .to_string_lossy()
                .to_string(),
            created_at: 1,
        })
        .unwrap();
    drop(store);

    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute(
        "INSERT INTO checkpoint (id, session_id, turn_idx, label, snapshot_ref, ts, turn_id)
         VALUES ('legacy-empty', 'session-v29-v30', 0, '改动前：null', '', 2, NULL)",
        [],
    )
    .unwrap();
    conn.execute_batch(
        "ALTER TABLE tool_call DROP COLUMN outcome;
         ALTER TABLE tool_call DROP COLUMN tool_started;
         ALTER TABLE tool_call DROP COLUMN has_output;
         ALTER TABLE tool_call DROP COLUMN retryable;
         ALTER TABLE tool_call DROP COLUMN denial_source;
         ALTER TABLE tool_call DROP COLUMN native_denial_code;
         ALTER TABLE checkpoint DROP COLUMN restorable;
         ALTER TABLE checkpoint DROP COLUMN file_count;
         ALTER TABLE checkpoint DROP COLUMN restorable_reason;
          PRAGMA user_version = 29;",
    )
    .unwrap();
    drop(conn);

    let reopened = SessionHistoryStore::new(path.clone());
    let detail = reopened.get_session("session-v29-v30").unwrap();
    assert_eq!(detail.checkpoints.len(), 1);
    assert!(!detail.checkpoints[0].restorable);
    assert_eq!(detail.checkpoints[0].file_count, 0);
    assert_eq!(
        detail.checkpoints[0].reason.as_deref(),
        Some("legacy_empty_snapshot")
    );
    let conn = rusqlite::Connection::open(path).unwrap();
    assert_eq!(
        conn.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        SCHEMA_VERSION
    );
    let tool_fact_columns: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('tool_call')
             WHERE name IN ('outcome', 'tool_started', 'has_output', 'retryable', 'denial_source', 'native_denial_code')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(tool_fact_columns, 6);
}

#[test]
fn change_27d_context_freeze_and_message_attachments_have_separate_lifetimes() {
    let path = temp_history_path("change-27d-context");
    let project = temp_project_dir("change-27d-context");
    let context_file = project.join("context.txt");
    let attachment = project.join("one-shot.txt");
    fs::write(&context_file, "persistent context").unwrap();
    fs::write(&attachment, "one shot").unwrap();
    let store = SessionHistoryStore::new(path.clone());
    store
        .create_session(NewSessionRecord {
            id: "session-27d-context".into(),
            engine: EngineId::Codex,
            model: "gpt-fixture".into(),
            cwd: project.to_string_lossy().to_string(),
            created_at: 1,
        })
        .unwrap();
    let context = store
        .add_session_context("session-27d-context", &context_file.to_string_lossy())
        .unwrap();
    let spec = start_change_27d_turn(
        &store,
        "session-27d-context",
        "use context",
        vec![attachment.to_string_lossy().to_string()],
    );
    assert_eq!(spec.session_context.len(), 1);
    assert!(store
        .remove_session_context("session-27d-context", &context.id)
        .unwrap_err()
        .contains("运行或等待审批"));

    let supervisor = TurnSupervisor::new(store.clone());
    supervisor.begin(
        "session-27d-context",
        &spec.turn_id,
        spec.turn_epoch,
        "build",
        "standard",
    );
    assert!(supervisor.accept_event(
        "session-27d-context",
        Some(&spec.turn_id),
        Some(spec.turn_epoch),
        1,
        &AgentEvent::TurnComplete {
            session_id: "native-27d".into(),
            stop_reason: StopReason::End,
        },
    ));
    store
        .remove_session_context("session-27d-context", &context.id)
        .unwrap();
    let detail = store.get_session("session-27d-context").unwrap();
    assert!(detail.session_context.is_empty());
    let user = detail
        .messages
        .iter()
        .find(|message| message.role == Role::User)
        .unwrap();
    assert_eq!(user.attachments, vec![attachment.to_string_lossy()]);
    let ledger = store.get_turn_ledger("session-27d-context").unwrap();
    assert_eq!(ledger.len(), 1);
    assert_eq!(ledger[0].attachments.len(), 1);
    assert_eq!(ledger[0].session_context.len(), 1);
    assert_eq!(ledger[0].messages.len(), 1);

    let conn = rusqlite::Connection::open(path).unwrap();
    let evidence: (i64, String, String) = conn
        .query_row(
            "SELECT COUNT(*), canonical_path_digest, identity_digest
             FROM turn_context_snapshot WHERE turn_id = ?1",
            [&spec.turn_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(evidence.0, 1);
    assert!(evidence.1.starts_with("sha256:"));
    assert!(evidence.2.starts_with("sha256:"));
    let _ = fs::remove_dir_all(project);
}

#[test]
fn change_27d_context_revalidation_is_fail_closed() {
    let path = temp_history_path("change-27d-context-missing");
    let project = temp_project_dir("change-27d-context-missing");
    let context_file = project.join("context.txt");
    fs::write(&context_file, "context").unwrap();
    let store = SessionHistoryStore::new(path);
    store
        .create_session(NewSessionRecord {
            id: "session-27d-missing".into(),
            engine: EngineId::ClaudeCode,
            model: "claude-fixture".into(),
            cwd: project.to_string_lossy().to_string(),
            created_at: 1,
        })
        .unwrap();
    store
        .add_session_context("session-27d-missing", &context_file.to_string_lossy())
        .unwrap();
    fs::remove_file(context_file).unwrap();
    let contexts = store.list_session_contexts("session-27d-missing").unwrap();
    assert_eq!(contexts[0].status, "missing");
    assert!(store
        .freeze_session_contexts("session-27d-missing")
        .unwrap_err()
        .contains("不可用"));
    let _ = fs::remove_dir_all(project);
}

#[test]
fn change_27d_turn_start_rechecks_frozen_context_identity() {
    let path = temp_history_path("change-27d-context-race");
    let project = temp_project_dir("change-27d-context-race");
    let context_file = project.join("context.txt");
    fs::write(&context_file, "before").unwrap();
    let store = SessionHistoryStore::new(path);
    store
        .create_session(NewSessionRecord {
            id: "session-27d-context-race".into(),
            engine: EngineId::Codex,
            model: "gpt-fixture".into(),
            cwd: project.to_string_lossy().to_string(),
            created_at: 1,
        })
        .unwrap();
    store
        .add_session_context("session-27d-context-race", &context_file.to_string_lossy())
        .unwrap();

    let command = change_27c_command("session-27d-context-race", "use context", 10_000);
    let mut spec = resolve_test_route(&change_27c_route(), &command);
    spec.session_context = store
        .freeze_session_contexts("session-27d-context-race")
        .unwrap();
    fs::write(&context_file, "after with a different length").unwrap();

    assert!(store
        .start_turn(&command, spec)
        .unwrap_err()
        .contains("冻结期间变化"));
    assert_eq!(
        store
            .get_session("session-27d-context-race")
            .unwrap()
            .messages
            .len(),
        0
    );
    let _ = fs::remove_dir_all(project);
}

#[test]
fn change_27d_tool_integrity_reconciles_duplicates_orphans_and_pending() {
    let path = temp_history_path("change-27d-tools");
    let project = temp_project_dir("change-27d-tools");
    let store = SessionHistoryStore::new(path.clone());
    store
        .create_session(NewSessionRecord {
            id: "session-27d-tools".into(),
            engine: EngineId::Codex,
            model: "gpt-fixture".into(),
            cwd: project.to_string_lossy().to_string(),
            created_at: 1,
        })
        .unwrap();
    let spec = start_change_27d_turn(&store, "session-27d-tools", "tools", Vec::new());
    let supervisor = TurnSupervisor::new(store.clone());
    supervisor.begin(
        "session-27d-tools",
        &spec.turn_id,
        spec.turn_epoch,
        "build",
        "standard",
    );
    let call = AgentEvent::ToolCall {
        session_id: "native".into(),
        id: "native-call".into(),
        name: "Read".into(),
        input: serde_json::json!({"path":"context.txt"}),
        status: CallStatus::Pending,
    };
    store
        .record_event_for_session_in_turn("session-27d-tools", Some(&spec.turn_id), &call)
        .unwrap();
    store
        .record_event_for_session_in_turn("session-27d-tools", Some(&spec.turn_id), &call)
        .unwrap();
    let result = AgentEvent::ToolResult {
        session_id: "native".into(),
        id: "native-call".into(),
        status: ToolStatus::Success,
        output: Some("界".repeat(30_000)),
        diff: None,
        outcome: Some(helm_lib::protocol::ToolOutcomeKind::ToolSucceeded),
        started: Some(true),
        has_output: Some(true),
        retryable: Some(false),
        denial_source: None,
        native_denial_code: None,
    };
    store
        .record_event_for_session_in_turn("session-27d-tools", Some(&spec.turn_id), &result)
        .unwrap();
    store
        .record_event_for_session_in_turn("session-27d-tools", Some(&spec.turn_id), &result)
        .unwrap();
    store
        .record_event_for_session_in_turn(
            "session-27d-tools",
            Some(&spec.turn_id),
            &AgentEvent::ToolResult {
                session_id: "native".into(),
                id: "orphan".into(),
                status: ToolStatus::Error,
                output: Some("missing start".into()),
                diff: None,
                outcome: None,
                started: None,
                has_output: None,
                retryable: None,
                denial_source: None,
                native_denial_code: None,
            },
        )
        .unwrap();
    store
        .record_event_for_session_in_turn(
            "session-27d-tools",
            Some(&spec.turn_id),
            &AgentEvent::ApprovalRequest {
                session_id: "native".into(),
                id: "approval".into(),
                action: "Write".into(),
                detail: "write output".into(),
                input: None,
                available_decisions: vec![],
                persistent_label: None,
                matcher_summary: None,
            },
        )
        .unwrap();
    store
        .record_event_for_session_in_turn(
            "session-27d-tools",
            Some(&spec.turn_id),
            &AgentEvent::Checkpoint {
                session_id: "native".into(),
                id: "checkpoint".into(),
                label: "before write".into(),
                ts: 10_100,
                restorable: true,
                file_count: 1,
                reason: None,
            },
        )
        .unwrap();
    store
        .record_event_for_session_in_turn(
            "session-27d-tools",
            Some(&spec.turn_id),
            &AgentEvent::TokenUsage {
                session_id: "native".into(),
                input_tokens: 10,
                cached_input_tokens: Some(2),
                cache_write_input_tokens: None,
                output_tokens: 5,
                cost_usd: 0.0,
                service_tier: Some("standard".into()),
                context_window: Some(100),
            },
        )
        .unwrap();
    store
        .record_event_for_session_in_turn(
            "session-27d-tools",
            Some(&spec.turn_id),
            &AgentEvent::ToolCall {
                session_id: "native".into(),
                id: "pending".into(),
                name: "Write".into(),
                input: serde_json::json!({"path":"out.txt"}),
                status: CallStatus::Pending,
            },
        )
        .unwrap();
    assert!(store
        .record_event_for_session_in_turn("session-27d-tools", None, &call)
        .unwrap_err()
        .contains("turn_id"));
    assert!(supervisor.accept_event(
        "session-27d-tools",
        Some(&spec.turn_id),
        Some(spec.turn_epoch),
        9,
        &AgentEvent::TurnComplete {
            session_id: "native".into(),
            stop_reason: StopReason::End,
        },
    ));

    let conn = rusqlite::Connection::open(path).unwrap();
    let duplicate: (String, i64) = conn
        .query_row(
            "SELECT integrity_status, result_count FROM tool_call
             WHERE turn_id = ?1 AND native_id = 'native-call'",
            [&spec.turn_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(duplicate, ("duplicate_result".into(), 2));
    let facts: (String, i64, i64, i64) = conn
        .query_row(
            "SELECT outcome, tool_started, has_output, retryable FROM tool_call
             WHERE turn_id = ?1 AND native_id = 'native-call'",
            [&spec.turn_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!(facts, ("tool_succeeded".into(), 1, 1, 0));
    let bounded_output: (i64, String) = conn
        .query_row(
            "SELECT LENGTH(CAST(output AS BLOB)), output FROM tool_call
             WHERE turn_id = ?1 AND native_id = 'native-call'",
            [&spec.turn_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert!(bounded_output.0 <= 65_536);
    assert!(bounded_output.1.ends_with("[ledger_output_truncated]"));
    let orphan: String = conn
        .query_row(
            "SELECT integrity_status FROM tool_call
             WHERE turn_id = ?1 AND native_id = 'orphan'",
            [&spec.turn_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(orphan, "orphan_result");
    let pending: (String, String) = conn
        .query_row(
            "SELECT status, integrity_status FROM tool_call
             WHERE turn_id = ?1 AND native_id = 'pending'",
            [&spec.turn_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(pending, ("error".into(), "pending_closed".into()));
    let ledger = store.get_turn_ledger("session-27d-tools").unwrap();
    assert_eq!(ledger.len(), 1);
    assert_eq!(ledger[0].tool_calls.len(), 3);
    assert_eq!(ledger[0].approvals.len(), 1);
    assert_eq!(ledger[0].checkpoints.len(), 1);
    assert_eq!(ledger[0].usage.len(), 1);
    assert_eq!(ledger[0].usage[0].model_evidence, "launch_spec");
    assert_eq!(
        ledger[0].usage[0].effective_reasoning_effort.as_deref(),
        Some("auto")
    );
    let _ = fs::remove_dir_all(project);
}

fn temp_project_dir(name: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "helm-session-project-{}-{name}-{}",
        std::process::id(),
        rand::random::<u64>()
    ));
    fs::create_dir_all(&path).unwrap();
    path
}

fn start_change_27d_turn(
    store: &SessionHistoryStore,
    session_id: &str,
    text: &str,
    attachments: Vec<String>,
) -> helm_lib::turn_start::TurnExecutionSpec {
    let mut command = change_27c_command(session_id, text, 10_000);
    command.attachments = attachments;
    let mut spec = resolve_test_route(&change_27c_route(), &command);
    spec.session_context = store.freeze_session_contexts(session_id).unwrap();
    store.start_turn(&command, spec).unwrap().1
}

fn change_27c_route() -> RuntimeRoute {
    RuntimeRoute {
        engine_id: "codex".into(),
        provider_id: "provider-legacy".into(),
        provider_kind: "api".into(),
        provider_display_name: "Legacy Provider".into(),
        route_label_snapshot: "Legacy Provider / gpt-fixture".into(),
        model_id: "gpt-fixture".into(),
        model_label_snapshot: "GPT Fixture".into(),
        default_reasoning_effort: ReasoningEffort::Auto,
        engine_profile_digest: "sha256:engine".into(),
        provider_launch_profile_ref: "provider:provider-legacy:api".into(),
        provider_launch_profile_digest: "sha256:provider-launch".into(),
        launch_config_digest: "sha256:launch".into(),
        pricing_basis_snapshot: PricingBasisSnapshot { profile: None },
    }
}

fn change_27c_command(session_id: &str, text: &str, created_at: i64) -> TurnStartCommand {
    TurnStartCommand {
        history_session_id: session_id.into(),
        display_text: text.into(),
        turn_mode: "build".into(),
        permission_profile: "standard".into(),
        requested_reasoning_effort: None,
        requested_model_id: Some("gpt-fixture".into()),
        attachments: Vec::new(),
        created_at,
    }
}

fn resolve_test_route(route: &RuntimeRoute, command: &TurnStartCommand) -> TurnExecutionSpec {
    let binding = BindingConfig {
        engine_id: route.engine_id.clone(),
        provider_id: route.provider_id.clone(),
        primary_model: route.model_id.clone(),
        fast_model: None,
        assistant_model_id: None,
        reasoning_effort: None,
        thinking_enabled: None,
        context_1m: None,
        revision: 1,
    };
    BindingLiveRouteResolver::resolve(route, &binding, "capability-test", command).unwrap()
}

#[test]
fn change_29_compatibility_retry_rotates_attempt_without_terminating_turn() {
    let path = temp_history_path("change-29-compatibility-retry");
    let store = SessionHistoryStore::new(path.clone());
    store
        .create_session(NewSessionRecord {
            id: "session-29-retry".into(),
            engine: EngineId::Codex,
            model: "gpt-fixture".into(),
            cwd: "D:/work/fixture".into(),
            created_at: 1,
        })
        .unwrap();
    let command = change_27c_command("session-29-retry", "hello", 1_000);
    let spec = resolve_test_route(&change_27c_route(), &command);
    let (_, spec) = store.start_turn(&command, spec).unwrap();
    let generation = RuntimeGeneration {
        id: "runtime-29-retry".into(),
        owner: RuntimeOwnerRef::Session("session-29-retry".into()),
        engine_id: "codex".into(),
        compatibility_key: "sha256:compat".into(),
        engine_profile_digest: "sha256:engine".into(),
        provider_launch_profile_ref: "provider:provider-legacy:api".into(),
        provider_launch_profile_digest: "sha256:provider-launch".into(),
        capability_snapshot_id: seed_test_capability(&store, "gpt-fixture"),
        canonical_cwd: "d:\\work\\fixture".into(),
        created_at: 1_001,
    };
    store.create_runtime_generation(&generation).unwrap();
    let first = store.create_turn_attempt(&spec, &generation, None).unwrap();
    let supervisor = TurnSupervisor::new(store.clone());
    supervisor
        .begin_attempt(
            &spec.history_session_id,
            &spec.turn_id,
            spec.turn_epoch,
            &spec.turn_mode,
            &spec.permission_profile,
            generation.owner.clone(),
            first.attempt_no,
            &generation.id,
        )
        .unwrap();
    store
        .mark_turn_attempt_accepted(&spec.turn_id, first.attempt_no, 1_002)
        .unwrap();

    let second = store.create_turn_attempt(&spec, &generation, None).unwrap();
    supervisor
        .retry_attempt(
            &spec.history_session_id,
            &spec.turn_id,
            second.attempt_no,
            &generation.id,
            "[auto_review_compatibility_retry] not started",
        )
        .unwrap();
    assert_eq!(
        supervisor
            .snapshot(&spec.history_session_id)
            .unwrap()
            .unwrap()
            .status,
        helm_lib::turn_supervisor::TurnStatus::Running
    );
    let conn = rusqlite::Connection::open(path).unwrap();
    let snapshot_attempt: u64 = conn
        .query_row(
            "SELECT attempt_no FROM turn_snapshot WHERE history_session_id = ?1",
            [&spec.history_session_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(snapshot_attempt, second.attempt_no);
    let states: Vec<(u64, String)> = {
        let mut stmt = conn
            .prepare(
                "SELECT attempt_no, delivery_state FROM turn_attempt
                 WHERE turn_id = ?1 ORDER BY attempt_no",
            )
            .unwrap();
        stmt.query_map([&spec.turn_id], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    assert_eq!(states, vec![(1, "error".into()), (2, "prepared".into())]);
}

#[test]
fn change_27e_attempt_tracks_generation_native_refs_capabilities_and_terminal_receipt() {
    let path = temp_history_path("change-27e-attempt");
    let store = SessionHistoryStore::new(path.clone());
    store
        .create_session(NewSessionRecord {
            id: "session-27e".into(),
            engine: EngineId::Codex,
            model: "gpt-fixture".into(),
            cwd: "D:/work/fixture".into(),
            created_at: 1,
        })
        .unwrap();
    let command = change_27c_command("session-27e", "hello", 1_000);
    let spec = resolve_test_route(&change_27c_route(), &command);
    let (_, spec) = store.start_turn(&command, spec).unwrap();
    let generation = RuntimeGeneration {
        id: "runtime-27e".into(),
        owner: RuntimeOwnerRef::Session("session-27e".into()),
        engine_id: "codex".into(),
        compatibility_key: "sha256:compat".into(),
        engine_profile_digest: "sha256:engine".into(),
        provider_launch_profile_ref: "provider:provider-legacy:api".into(),
        provider_launch_profile_digest: "sha256:provider-launch".into(),
        capability_snapshot_id: seed_test_capability(&store, "gpt-fixture"),
        canonical_cwd: "d:\\work\\fixture".into(),
        created_at: 1_001,
    };
    store.create_runtime_generation(&generation).unwrap();
    let attempt = store
        .create_turn_attempt(&spec, &generation, Some("thread-input"))
        .unwrap();
    assert_eq!(attempt.attempt_no, 1);
    assert_eq!(store.load_turn_recovery_inputs().unwrap().len(), 1);
    store
        .mark_turn_attempt_accepted(&spec.turn_id, attempt.attempt_no, 1_002)
        .unwrap();
    let recovered = RuntimeRegistry::new(store.clone()).unwrap();
    assert_eq!(recovered.recovery_inputs().len(), 1);
    assert_eq!(recovered.recovery_inputs()[0].turn_id, spec.turn_id);
    store
        .record_event_for_session_in_turn(
            "session-27e",
            Some(&spec.turn_id),
            &AgentEvent::SessionStarted {
                session_id: "thread-output".into(),
                engine: EngineId::Codex,
                model: "gpt-fixture".into(),
                cwd: "D:/work/fixture".into(),
                ts: 1_003,
                capabilities: Some(RuntimeCapabilitySnapshot::unknown()),
            },
        )
        .unwrap();
    store
        .record_event_for_session_in_turn(
            "session-27e",
            Some(&spec.turn_id),
            &AgentEvent::TokenUsage {
                session_id: "thread-output".into(),
                input_tokens: 10,
                cached_input_tokens: None,
                cache_write_input_tokens: None,
                output_tokens: 5,
                cost_usd: 0.01,
                service_tier: None,
                context_window: None,
            },
        )
        .unwrap();
    store
        .record_event_for_session_in_turn(
            "session-27e",
            Some(&spec.turn_id),
            &AgentEvent::TurnComplete {
                session_id: "thread-output".into(),
                stop_reason: StopReason::End,
            },
        )
        .unwrap();
    assert!(store.load_turn_recovery_inputs().unwrap().is_empty());

    let conn = rusqlite::Connection::open(path).unwrap();
    let facts: (String, String, String, String, String) = conn
        .query_row(
            "SELECT a.delivery_state, a.observed_model_id,
                    input_ref.native_id, output_ref.native_id,
                    a.actual_capability_snapshot_json
             FROM turn_attempt a
             JOIN native_session_ref input_ref ON input_ref.id = a.input_native_ref_id
             JOIN native_session_ref output_ref ON output_ref.id = a.output_native_ref_id
             WHERE a.turn_id = ?1 AND a.attempt_no = 1",
            [&spec.turn_id],
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
        .unwrap();
    assert_eq!(facts.0, "completed");
    assert_eq!(facts.1, "gpt-fixture");
    assert_eq!(facts.2, "thread-input");
    assert_eq!(facts.3, "thread-output");
    assert!(facts.4.contains("approvalContractVersion"));
    let usage_facts: (String, String, String) = conn
        .query_row(
            "SELECT model, effective_reasoning_effort, model_evidence
             FROM usage WHERE turn_id = ?1",
            [&spec.turn_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(usage_facts.0, "gpt-fixture");
    assert_eq!(usage_facts.1, "auto");
    assert_eq!(usage_facts.2, "runtime_observed");
    let spec_columns = conn
        .prepare("PRAGMA table_info(turn_execution_spec)")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(1))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert!(!spec_columns.iter().any(|name| {
        matches!(
            name.as_str(),
            "generation_id" | "observed_model_id" | "output_native_ref_id" | "delivery_state"
        )
    }));
}

#[test]
fn change_27f_supervisor_owns_sequence_boundaries_and_the_only_terminal() {
    let path = temp_history_path("change-27f-supervisor");
    let store = SessionHistoryStore::new(path.clone());
    store
        .create_session(NewSessionRecord {
            id: "session-27f".into(),
            engine: EngineId::Codex,
            model: "gpt-fixture".into(),
            cwd: "D:/work/fixture".into(),
            created_at: 1,
        })
        .unwrap();
    let command = change_27c_command("session-27f", "hello", 10_000);
    let spec = resolve_test_route(&change_27c_route(), &command);
    let (_, spec) = store.start_turn(&command, spec).unwrap();
    let generation = RuntimeGeneration {
        id: "runtime-27f".into(),
        owner: RuntimeOwnerRef::Session("session-27f".into()),
        engine_id: "codex".into(),
        compatibility_key: "sha256:compat".into(),
        engine_profile_digest: "sha256:engine".into(),
        provider_launch_profile_ref: "provider:provider-legacy:api".into(),
        provider_launch_profile_digest: "sha256:provider-launch".into(),
        capability_snapshot_id: seed_test_capability(&store, "gpt-fixture"),
        canonical_cwd: "d:\\work\\fixture".into(),
        created_at: 10_001,
    };
    store.create_runtime_generation(&generation).unwrap();
    let attempt = store.create_turn_attempt(&spec, &generation, None).unwrap();
    let supervisor = TurnSupervisor::new(store.clone());
    supervisor
        .begin_attempt(
            "session-27f",
            &spec.turn_id,
            spec.turn_epoch,
            &spec.turn_mode,
            &spec.permission_profile,
            generation.owner.clone(),
            attempt.attempt_no,
            &generation.id,
        )
        .unwrap();
    let candidate = |source_seq, native_event_id, event| EngineEventCandidate {
        owner: generation.owner.clone(),
        history_session_id: "session-27f".into(),
        turn_id: spec.turn_id.clone(),
        turn_epoch: spec.turn_epoch,
        attempt_no: attempt.attempt_no,
        runtime_generation_id: generation.id.clone(),
        source_seq,
        native_event_id,
        observed_at: 10_001 + source_seq as i64,
        event,
    };
    assert_eq!(
        supervisor.process_candidate(candidate(
            1,
            Some("native-session:thread-27f".into()),
            AgentEvent::SessionStarted {
                session_id: "thread-27f".into(),
                engine: EngineId::Codex,
                model: "gpt-fixture".into(),
                cwd: "D:/work/fixture".into(),
                ts: 10_002,
                capabilities: Some(RuntimeCapabilitySnapshot::unknown()),
            },
        )),
        CandidateDisposition::Accepted
    );
    assert_eq!(
        supervisor.process_candidate(candidate(
            2,
            None,
            AgentEvent::MessageDelta {
                session_id: "thread-27f".into(),
                role: Role::Assistant,
                text: "streamed text".into(),
            },
        )),
        CandidateDisposition::Accepted
    );
    assert_eq!(
        supervisor.process_candidate(candidate(
            3,
            Some("native-terminal".into()),
            AgentEvent::TurnComplete {
                session_id: "thread-27f".into(),
                stop_reason: StopReason::End,
            },
        )),
        CandidateDisposition::Accepted
    );
    assert_eq!(
        supervisor.process_candidate(candidate(
            4,
            Some("native-terminal".into()),
            AgentEvent::TurnComplete {
                session_id: "thread-27f".into(),
                stop_reason: StopReason::End,
            },
        )),
        CandidateDisposition::Duplicate
    );

    let conn = rusqlite::Connection::open(path).unwrap();
    let attempt_state: String = conn
        .query_row(
            "SELECT delivery_state FROM turn_attempt WHERE turn_id = ?1 AND attempt_no = ?2",
            rusqlite::params![spec.turn_id, attempt.attempt_no],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(attempt_state, "completed");
    let snapshot: (String, i64, String, String) = conn
        .query_row(
            "SELECT status, event_seq, runtime_generation_id, recovery_state
             FROM turn_snapshot WHERE history_session_id = 'session-27f'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!(
        snapshot,
        ("succeeded".into(), 3, generation.id, "none".into())
    );
    let boundary_kinds = conn
        .prepare(
            "SELECT event_kind FROM stream_boundary_event
             WHERE turn_id = ?1 ORDER BY event_seq",
        )
        .unwrap()
        .query_map([&spec.turn_id], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(
        boundary_kinds,
        vec!["attempt_started", "session_started", "turn_complete"]
    );
}

#[test]
fn change_27f_startup_recovery_is_deterministic_read_only_and_idempotent() {
    let path = temp_history_path("change-27f-recovery");
    let store = SessionHistoryStore::new(path.clone());
    let make_attempt = |session_id: &str, created_at: i64| {
        store
            .create_session(NewSessionRecord {
                id: session_id.into(),
                engine: EngineId::Codex,
                model: "gpt-fixture".into(),
                cwd: "D:/work/fixture".into(),
                created_at,
            })
            .unwrap();
        let command = change_27c_command(session_id, "recover", created_at + 1);
        let spec = resolve_test_route(&change_27c_route(), &command);
        let (_, spec) = store.start_turn(&command, spec).unwrap();
        let generation = RuntimeGeneration {
            id: format!("runtime-{session_id}"),
            owner: RuntimeOwnerRef::Session(session_id.into()),
            engine_id: "codex".into(),
            compatibility_key: format!("sha256:compat-{session_id}"),
            engine_profile_digest: "sha256:engine".into(),
            provider_launch_profile_ref: "provider:provider-legacy:api".into(),
            provider_launch_profile_digest: "sha256:provider-launch".into(),
            capability_snapshot_id: seed_test_capability(&store, "gpt-fixture"),
            canonical_cwd: "d:\\work\\fixture".into(),
            created_at: created_at + 2,
        };
        store.create_runtime_generation(&generation).unwrap();
        let attempt = store.create_turn_attempt(&spec, &generation, None).unwrap();
        let supervisor = TurnSupervisor::new(store.clone());
        supervisor
            .begin_attempt(
                session_id,
                &spec.turn_id,
                spec.turn_epoch,
                &spec.turn_mode,
                &spec.permission_profile,
                generation.owner.clone(),
                attempt.attempt_no,
                &generation.id,
            )
            .unwrap();
        (spec, generation, attempt, supervisor)
    };

    let (prepared_spec, _, _, _) = make_attempt("prepared", 20_000);
    let (approval_spec, _, approval_attempt, approval_supervisor) =
        make_attempt("approval", 21_000);
    store
        .mark_turn_attempt_accepted(&approval_spec.turn_id, approval_attempt.attempt_no, 21_003)
        .unwrap();
    assert!(approval_supervisor.submit_event(
        "approval",
        Some(&approval_spec.turn_id),
        Some(approval_spec.turn_epoch),
        AgentEvent::ApprovalRequest {
            session_id: "thread-approval".into(),
            id: "request-approval".into(),
            action: "Bash".into(),
            detail: "echo pending".into(),
            input: None,
            available_decisions: Vec::new(),
            persistent_label: None,
            matcher_summary: None,
        },
    ));
    let (unknown_spec, _, unknown_attempt, _) = make_attempt("unknown", 22_000);
    store
        .mark_turn_attempt_accepted(&unknown_spec.turn_id, unknown_attempt.attempt_no, 22_003)
        .unwrap();

    let report = store.reconcile_stream_recovery().unwrap();
    assert_eq!(report.prepared_interrupted, 1);
    assert_eq!(report.approval_interrupted, 1);
    assert_eq!(report.delivery_unknown, 1);
    assert_eq!(report.runtime_generations_lost, 3);
    assert_eq!(
        store.reconcile_stream_recovery().unwrap(),
        Default::default()
    );

    let conn = rusqlite::Connection::open(path).unwrap();
    let recovery_fact = |turn_id: &str| -> (String, String, String) {
        conn.query_row(
            "SELECT a.delivery_state, s.status, s.recovery_state
             FROM turn_attempt a JOIN turn_snapshot s ON s.turn_id = a.turn_id
             WHERE a.turn_id = ?1",
            [turn_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap()
    };
    assert_eq!(
        recovery_fact(&prepared_spec.turn_id),
        (
            "rejected".into(),
            "interrupted".into(),
            "safe_to_retry".into()
        )
    );
    assert_eq!(
        recovery_fact(&approval_spec.turn_id),
        (
            "interrupted".into(),
            "interrupted".into(),
            "approval_runtime_lost".into()
        )
    );
    assert_eq!(
        recovery_fact(&unknown_spec.turn_id),
        (
            "delivery_unknown".into(),
            "failed".into(),
            "delivery_unknown".into()
        )
    );
    let approval_status: String = conn
        .query_row(
            "SELECT status FROM approval WHERE session_id = 'approval' AND id = 'request-approval'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(approval_status, "expired", "重启后的旧审批必须只读");
}

#[test]
fn change_27i_v23_migrates_through_v28_additively_and_reopens_idempotently() {
    let path = temp_history_path("change-27e-v23-migration");
    let store = SessionHistoryStore::new(path.clone());
    store.list_sessions().unwrap();
    drop(store);
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch(
        "DROP TABLE turn_attempt;
         DROP TABLE native_session_ref;
         DROP TABLE runtime_generation;
         PRAGMA user_version = 23;",
    )
    .unwrap();
    drop(conn);

    for _ in 0..2 {
        SessionHistoryStore::new(path.clone())
            .list_sessions()
            .unwrap();
    }
    let conn = rusqlite::Connection::open(path).unwrap();
    assert_eq!(
        conn.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        SCHEMA_VERSION
    );
    for table in [
        "runtime_generation",
        "native_session_ref",
        "turn_attempt",
        "stream_boundary_event",
        "stream_diagnostic",
        "turn_budget_snapshot",
        "turn_budget_fact",
        "background_operation",
        "operation_execution_spec",
        "model_only_operation_policy",
        "operation_budget_snapshot",
        "operation_attempt",
        "operation_progress_fact",
        "handoff",
        "session_fork",
    ] {
        let exists: i64 = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
                [table],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(exists, 1, "缺少 v24~v29 表 {table}");
    }
    let snapshot_columns = conn
        .prepare("PRAGMA table_info(turn_snapshot)")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(1))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    for column in ["attempt_no", "runtime_generation_id", "recovery_state"] {
        assert!(snapshot_columns.iter().any(|name| name == column));
    }
    let usage_columns = conn
        .prepare("PRAGMA table_info(usage)")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(1))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert!(usage_columns.iter().any(|name| name == "operation_id"));
    let foreign_key_errors = conn
        .prepare("PRAGMA foreign_key_check")
        .unwrap()
        .query_map([], |_| Ok(()))
        .unwrap()
        .count();
    assert_eq!(foreign_key_errors, 0);
}

#[test]
fn change_27e_generation_rotation_is_atomic_and_owner_scoped() {
    let path = temp_history_path("change-27e-generation-rotation");
    let store = SessionHistoryStore::new(path.clone());
    store
        .create_session(NewSessionRecord {
            id: "session-rotation".into(),
            engine: EngineId::Codex,
            model: "gpt-fixture".into(),
            cwd: "D:/work/fixture".into(),
            created_at: 1,
        })
        .unwrap();
    let first = RuntimeGeneration {
        id: "runtime-first".into(),
        owner: RuntimeOwnerRef::Session("session-rotation".into()),
        engine_id: "codex".into(),
        compatibility_key: "sha256:compat-first".into(),
        engine_profile_digest: "sha256:engine".into(),
        provider_launch_profile_ref: "provider:provider-legacy:api".into(),
        provider_launch_profile_digest: "sha256:provider-launch".into(),
        capability_snapshot_id: seed_test_capability(&store, "gpt-fixture"),
        canonical_cwd: "d:\\work\\fixture".into(),
        created_at: 10,
    };
    store.create_runtime_generation(&first).unwrap();
    let second = RuntimeGeneration {
        id: "runtime-second".into(),
        compatibility_key: "sha256:compat-process-config-changed".into(),
        created_at: 20,
        ..first.clone()
    };
    store.rotate_runtime_generation(&first.id, &second).unwrap();
    assert!(store
        .rotate_runtime_generation(&first.id, &second)
        .unwrap_err()
        .contains("失效"));

    let conn = rusqlite::Connection::open(path).unwrap();
    let rows = conn
        .prepare(
            "SELECT id, status, ended_at FROM runtime_generation
             WHERE owner_kind = 'session' AND owner_id = 'session-rotation'
             ORDER BY created_at",
        )
        .unwrap()
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<i64>>(2)?,
            ))
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(
        rows,
        vec![
            ("runtime-first".into(), "closed".into(), Some(20),),
            ("runtime-second".into(), "active".into(), None),
        ]
    );
}

#[test]
fn change_27e_attempt_rejects_owner_and_observed_model_mismatches() {
    let path = temp_history_path("change-27e-mismatch");
    let store = SessionHistoryStore::new(path.clone());
    for session_id in ["session-owner", "session-other"] {
        store
            .create_session(NewSessionRecord {
                id: session_id.into(),
                engine: EngineId::Codex,
                model: "gpt-fixture".into(),
                cwd: "D:/work/fixture".into(),
                created_at: 1,
            })
            .unwrap();
    }
    let command = change_27c_command("session-other", "hello", 2_000);
    let spec = resolve_test_route(&change_27c_route(), &command);
    let (_, spec) = store.start_turn(&command, spec).unwrap();
    let wrong_generation = RuntimeGeneration {
        id: "runtime-wrong-owner".into(),
        owner: RuntimeOwnerRef::Session("session-owner".into()),
        engine_id: "codex".into(),
        compatibility_key: "sha256:compat".into(),
        engine_profile_digest: "sha256:engine".into(),
        provider_launch_profile_ref: "provider:provider-legacy:api".into(),
        provider_launch_profile_digest: "sha256:provider-launch".into(),
        capability_snapshot_id: seed_test_capability(&store, "gpt-fixture"),
        canonical_cwd: "d:\\work\\fixture".into(),
        created_at: 2_001,
    };
    store.create_runtime_generation(&wrong_generation).unwrap();
    assert!(store
        .create_turn_attempt(&spec, &wrong_generation, None)
        .unwrap_err()
        .contains("owner"));

    let generation = RuntimeGeneration {
        id: "runtime-correct-owner".into(),
        owner: RuntimeOwnerRef::Session("session-other".into()),
        created_at: 2_002,
        ..wrong_generation
    };
    store.create_runtime_generation(&generation).unwrap();
    let attempt = store.create_turn_attempt(&spec, &generation, None).unwrap();
    store
        .mark_turn_attempt_accepted(&spec.turn_id, attempt.attempt_no, 2_003)
        .unwrap();
    let error = store
        .record_event_for_session_in_turn(
            "session-other",
            Some(&spec.turn_id),
            &AgentEvent::SessionStarted {
                session_id: "thread-wrong-model".into(),
                engine: EngineId::Codex,
                model: "different-model".into(),
                cwd: "D:/work/fixture".into(),
                ts: 2_004,
                capabilities: None,
            },
        )
        .unwrap_err();
    assert!(error.contains("runtime_model_mismatch"));
    let conn = rusqlite::Connection::open(path).unwrap();
    let state: String = conn
        .query_row(
            "SELECT delivery_state FROM turn_attempt WHERE turn_id = ?1",
            [&spec.turn_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(state, "error");
    let cli_session_id: Option<String> = conn
        .query_row(
            "SELECT cli_session_id FROM session WHERE id = 'session-other'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(cli_session_id, None);
    let native_ref_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM native_session_ref WHERE native_id = 'thread-wrong-model'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(native_ref_count, 0);
}

#[test]
fn change_27c_turn_start_is_atomic_and_epochs_survive_restart() {
    let path = temp_history_path("change-27c-turn-start");
    let store = SessionHistoryStore::new(path.clone());
    store
        .create_session(NewSessionRecord {
            id: "session-27c".into(),
            engine: EngineId::Codex,
            model: "gpt-fixture".into(),
            cwd: "D:/work/fixture".into(),
            created_at: 1,
        })
        .unwrap();
    let first_command = change_27c_command("session-27c", "first", 1_000);
    let mut first_route = change_27c_route();
    first_route.pricing_basis_snapshot.profile = Some(ResolvedPricingProfile {
        catalog_version: "frozen-v1".into(),
        source: "manual".into(),
        currency: "USD".into(),
        source_url: String::new(),
        observed_at: "1".into(),
        tiers: HashMap::from([(
            ServiceTier::Standard,
            PricingTier {
                bands: vec![PricingBand {
                    min_input_tokens: None,
                    max_input_tokens: None,
                    input: 1.0,
                    cached_input: None,
                    cache_write: None,
                    output: 2.0,
                }],
            },
        )]),
    });
    let first_spec = resolve_test_route(&first_route, &first_command);
    let (_, first_spec) = store.start_turn(&first_command, first_spec).unwrap();
    assert_eq!(first_spec.turn_epoch, 1);
    drop(store);

    let reopened = SessionHistoryStore::new(path.clone());
    let second_command = change_27c_command("session-27c", "second", 2_000);
    let second_spec = resolve_test_route(&change_27c_route(), &second_command);
    let (_, second_spec) = reopened.start_turn(&second_command, second_spec).unwrap();
    assert_eq!(second_spec.turn_epoch, 2);
    assert_ne!(first_spec.turn_id, second_spec.turn_id);

    let mut mutable_price = first_route.pricing_basis_snapshot.profile.clone().unwrap();
    mutable_price.catalog_version = "mutable-v2".into();
    reopened.set_model_pricing_profile("provider-legacy", "gpt-fixture", mutable_price);

    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute(
        "UPDATE session SET model = 'changed-model', provider_id = 'changed-provider'
         WHERE id = 'session-27c'",
        [],
    )
    .unwrap();
    drop(conn);
    reopened
        .record_event_for_session_in_turn(
            "session-27c",
            Some(&first_spec.turn_id),
            &AgentEvent::TokenUsage {
                session_id: "native-27c".into(),
                input_tokens: 1_000_000,
                cached_input_tokens: None,
                cache_write_input_tokens: None,
                output_tokens: 1_000_000,
                cost_usd: 0.0,
                service_tier: None,
                context_window: None,
            },
        )
        .unwrap();
    reopened
        .record_event_for_session_in_turn(
            "session-27c",
            Some(&second_spec.turn_id),
            &AgentEvent::TokenUsage {
                session_id: "native-27c".into(),
                input_tokens: 1_000_000,
                cached_input_tokens: None,
                cache_write_input_tokens: None,
                output_tokens: 1_000_000,
                cost_usd: 0.0,
                service_tier: None,
                context_window: None,
            },
        )
        .unwrap();

    let conn = rusqlite::Connection::open(path).unwrap();
    let atomic_rows: (i64, i64, i64) = conn
        .query_row(
            "SELECT
               (SELECT COUNT(*) FROM turn WHERE identity_source = 'control_plane'),
               (SELECT COUNT(*) FROM message WHERE turn_id IS NOT NULL),
               (SELECT COUNT(*) FROM turn_execution_spec)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(atomic_rows, (2, 2, 2));
    let sources: (
        String,
        Option<String>,
        Option<i64>,
        Option<String>,
        Option<String>,
    ) = conn
        .query_row(
            "SELECT resolution_source, binding_id, binding_revision,
                    routing_capability_snapshot_id, legacy_route_snapshot_digest
             FROM turn_execution_spec WHERE turn_id = ?1",
            [&first_spec.turn_id],
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
        .unwrap();
    assert_eq!(sources.0, "binding_live");
    assert_eq!(sources.1.as_deref(), Some("binding:codex"));
    assert_eq!(sources.2, Some(1));
    assert_eq!(sources.3.as_deref(), Some("capability-test"));
    assert_eq!(sources.4, None);
    let usage_route: (String, String, String, f64, String) = conn
        .query_row(
            "SELECT model, provider_id, turn_id, cost_usd, pricing_catalog_version
             FROM usage WHERE turn_id = ?1",
            [&first_spec.turn_id],
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
        .unwrap();
    assert_eq!(usage_route.0, "gpt-fixture");
    assert_eq!(usage_route.1, "provider-legacy");
    assert_eq!(usage_route.2, first_spec.turn_id);
    assert!((usage_route.3 - 3.0).abs() < 1e-9);
    assert_eq!(usage_route.4, "frozen-v1");
    let unknown_usage: (f64, String, Option<String>) = conn
        .query_row(
            "SELECT cost_usd, cost_kind, pricing_catalog_version
             FROM usage WHERE turn_id = ?1",
            [&second_spec.turn_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(unknown_usage.0, 0.0);
    assert_eq!(unknown_usage.1, "unknown");
    assert_eq!(unknown_usage.2, None);
}

#[test]
fn change_27c_invalid_spec_and_dispatch_rollback_leave_no_partial_turn() {
    let path = temp_history_path("change-27c-atomic-failure");
    let store = SessionHistoryStore::new(path.clone());
    store
        .create_session(NewSessionRecord {
            id: "session-27c-failure".into(),
            engine: EngineId::Codex,
            model: "gpt-fixture".into(),
            cwd: "D:/work/fixture".into(),
            created_at: 1,
        })
        .unwrap();
    let command = change_27c_command("session-27c-failure", "must rollback", 1_000);
    let mut invalid = resolve_test_route(&change_27c_route(), &command);
    invalid.binding_revision = None;
    assert!(store.start_turn(&command, invalid).is_err());

    let mut inconsistent = resolve_test_route(&change_27c_route(), &command);
    inconsistent.permission_profile = "auto".into();
    assert!(store.start_turn(&command, inconsistent).is_err());

    let mut preassigned_epoch = resolve_test_route(&change_27c_route(), &command);
    preassigned_epoch.turn_epoch = 7;
    assert!(store.start_turn(&command, preassigned_epoch).is_err());

    let valid = resolve_test_route(&change_27c_route(), &command);
    let (prepared, _) = store.start_turn(&command, valid).unwrap();
    store.rollback_prepared_user_turn(prepared).unwrap();
    let conn = rusqlite::Connection::open(path).unwrap();
    for table in ["turn", "message", "turn_execution_spec"] {
        let count: i64 = conn
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 0, "{table} must roll back with TurnStart");
    }
}

#[test]
fn change_27c_turn_start_rolls_back_each_persistent_write_fault() {
    let failpoints = [
        (
            "turn",
            "CREATE TRIGGER fail_turn BEFORE INSERT ON turn
             BEGIN SELECT RAISE(ABORT, 'turn fault'); END;",
        ),
        (
            "spec",
            "CREATE TRIGGER fail_spec BEFORE INSERT ON turn_execution_spec
             BEGIN SELECT RAISE(ABORT, 'spec fault'); END;",
        ),
        (
            "message",
            "CREATE TRIGGER fail_message BEFORE INSERT ON message
             WHEN NEW.turn_id IS NOT NULL
             BEGIN SELECT RAISE(ABORT, 'message fault'); END;",
        ),
        (
            "session",
            "CREATE TRIGGER fail_session BEFORE UPDATE ON session
             WHEN NEW.status = 'active'
             BEGIN SELECT RAISE(ABORT, 'session fault'); END;",
        ),
        (
            "setting",
            "CREATE TRIGGER fail_setting BEFORE INSERT ON setting
             WHEN NEW.key = 'active_session_id'
             BEGIN SELECT RAISE(ABORT, 'setting fault'); END;",
        ),
    ];

    for (name, trigger) in failpoints {
        let path = temp_history_path(&format!("change-27c-fault-{name}"));
        let store = SessionHistoryStore::new(path.clone());
        store
            .create_session(NewSessionRecord {
                id: "session-27c-fault".into(),
                engine: EngineId::Codex,
                model: "gpt-fixture".into(),
                cwd: "D:/work/fixture".into(),
                created_at: 1,
            })
            .unwrap();
        let conn = rusqlite::Connection::open(&path).unwrap();
        let initial_session_state: (String, i64) = conn
            .query_row(
                "SELECT status, updated_at FROM session WHERE id = 'session-27c-fault'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        let initial_active_setting: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM setting WHERE key = 'active_session_id'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        conn.execute_batch(trigger).unwrap();
        drop(conn);

        let command = change_27c_command("session-27c-fault", "atomic", 1_000);
        let spec = resolve_test_route(&change_27c_route(), &command);
        assert!(store.start_turn(&command, spec).is_err(), "{name}");

        let conn = rusqlite::Connection::open(path).unwrap();
        for table in ["turn", "message", "turn_execution_spec"] {
            let count: i64 = conn
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(count, 0, "{name}: {table} must roll back");
        }
        let session_state: (String, i64) = conn
            .query_row(
                "SELECT status, updated_at FROM session WHERE id = 'session-27c-fault'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(session_state, initial_session_state, "{name}");
        let active_setting: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM setting WHERE key = 'active_session_id'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(active_setting, initial_active_setting, "{name}");
    }
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
fn session_folders_persist_and_deleting_one_moves_sessions_to_default() {
    let path = temp_history_path("session-folders");
    let store = SessionHistoryStore::new(path);
    let defaults = store.list_folders().unwrap();
    assert_eq!(defaults.len(), 1);
    assert_eq!(defaults[0].id, "folder-default");
    assert!(defaults[0].locked);

    let project = store.create_folder("acme-web").unwrap();
    store
        .create_session_in_folder(
            NewSessionRecord {
                id: "folder-session".to_string(),
                engine: EngineId::Codex,
                model: "gpt-5".to_string(),
                cwd: r"D:\work\acme".to_string(),
                created_at: 1_717_171_700,
            },
            Some(&project.id),
        )
        .unwrap();
    assert_eq!(store.list_sessions().unwrap()[0].folder_id, project.id);

    store.set_folder_collapsed(&project.id, true).unwrap();
    assert!(
        store
            .list_folders()
            .unwrap()
            .into_iter()
            .find(|folder| folder.id == project.id)
            .unwrap()
            .collapsed
    );

    store.delete_folder(&project.id).unwrap();
    assert_eq!(
        store.list_sessions().unwrap()[0].folder_id,
        "folder-default"
    );
    store.rename_folder("folder-default", "收件箱").unwrap();
    assert_eq!(store.list_folders().unwrap()[0].name, "收件箱");
    assert!(store
        .rename_folder("folder-default", &"x".repeat(81))
        .is_err());
    assert!(store.set_folder_collapsed("folder-missing", true).is_err());
    assert!(store.delete_folder("folder-default").is_err());
}

#[test]
fn production_sessions_are_grouped_by_canonical_cwd() {
    let db_path = temp_history_path("cwd-folder-reuse");
    let project = temp_project_dir("reuse");
    let store = SessionHistoryStore::new(db_path);

    for id in ["cwd-session-1", "cwd-session-2"] {
        store
            .create_session_for_cwd(
                NewSessionRecord {
                    id: id.to_string(),
                    engine: EngineId::Codex,
                    model: "gpt-5".to_string(),
                    cwd: project.join(".").to_string_lossy().to_string(),
                    created_at: 1_717_171_700,
                },
                None,
            )
            .unwrap();
    }

    let folders = store.list_folders().unwrap();
    assert_eq!(folders.len(), 2);
    let project_folder = folders.iter().find(|folder| folder.cwd.is_some()).unwrap();
    assert_eq!(
        project_folder.name,
        project.file_name().unwrap().to_string_lossy()
    );
    assert_eq!(
        std::path::PathBuf::from(project_folder.cwd.as_deref().unwrap())
            .canonicalize()
            .unwrap(),
        project.canonicalize().unwrap()
    );
    let sessions = store.list_sessions().unwrap();
    assert_eq!(sessions.len(), 2);
    assert!(sessions
        .iter()
        .all(|session| session.folder_id == project_folder.id));

    let _ = fs::remove_dir_all(project);
}

#[cfg(windows)]
#[test]
fn windows_cwd_variants_reuse_the_same_project_folder() {
    let db_path = temp_history_path("cwd-folder-windows-variants");
    let project = temp_project_dir("windows-variants");
    let canonical = project
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .to_string();
    let display = canonical.strip_prefix(r"\\?\").unwrap_or(&canonical);
    let variants = [
        display.to_string(),
        display.to_uppercase().replace('\\', "/"),
        format!(r"\\?\{display}"),
    ];
    let store = SessionHistoryStore::new(db_path);

    for (index, cwd) in variants.into_iter().enumerate() {
        store
            .create_session_for_cwd(
                NewSessionRecord {
                    id: format!("variant-{index}"),
                    engine: EngineId::Codex,
                    model: "gpt-5".to_string(),
                    cwd,
                    created_at: 1_717_171_700 + index as i64,
                },
                None,
            )
            .unwrap();
    }

    let project_folders = store
        .list_folders()
        .unwrap()
        .into_iter()
        .filter(|folder| folder.cwd.is_some())
        .collect::<Vec<_>>();
    assert_eq!(project_folders.len(), 1);
    assert!(store
        .list_sessions()
        .unwrap()
        .iter()
        .all(|session| session.folder_id == project_folders[0].id));

    let _ = fs::remove_dir_all(project);
}

#[test]
fn same_named_directories_get_distinct_project_folders() {
    let db_path = temp_history_path("cwd-folder-same-name");
    let left_root = temp_project_dir("left");
    let right_root = temp_project_dir("right");
    let left = left_root.join("demo");
    let right = right_root.join("demo");
    fs::create_dir_all(&left).unwrap();
    fs::create_dir_all(&right).unwrap();
    let store = SessionHistoryStore::new(db_path);

    for (id, cwd) in [("left-session", &left), ("right-session", &right)] {
        store
            .create_session_for_cwd(
                NewSessionRecord {
                    id: id.to_string(),
                    engine: EngineId::ClaudeCode,
                    model: "claude-test".to_string(),
                    cwd: cwd.to_string_lossy().to_string(),
                    created_at: 1_717_171_700,
                },
                None,
            )
            .unwrap();
    }

    let project_folders = store
        .list_folders()
        .unwrap()
        .into_iter()
        .filter(|folder| folder.cwd.is_some())
        .collect::<Vec<_>>();
    assert_eq!(project_folders.len(), 2);
    assert!(project_folders.iter().all(|folder| folder.name == "demo"));
    assert_ne!(
        store.get_session("left-session").unwrap().summary.folder_id,
        store
            .get_session("right-session")
            .unwrap()
            .summary
            .folder_id
    );

    let _ = fs::remove_dir_all(left_root);
    let _ = fs::remove_dir_all(right_root);
}

#[test]
fn explicit_folder_overrides_cwd_grouping_and_invalid_cwd_creates_nothing() {
    let db_path = temp_history_path("cwd-folder-explicit");
    let project = temp_project_dir("explicit");
    let store = SessionHistoryStore::new(db_path);
    let manual = store.create_folder("手工分类").unwrap();
    store
        .create_session_for_cwd(
            NewSessionRecord {
                id: "manual-session".to_string(),
                engine: EngineId::Codex,
                model: "gpt-5".to_string(),
                cwd: project.to_string_lossy().to_string(),
                created_at: 1_717_171_700,
            },
            Some(&manual.id),
        )
        .unwrap();
    assert_eq!(
        store
            .get_session("manual-session")
            .unwrap()
            .summary
            .folder_id,
        manual.id
    );
    assert_eq!(store.list_folders().unwrap().len(), 2);

    let missing = project.join("missing");
    let error = store
        .create_session_for_cwd(
            NewSessionRecord {
                id: "missing-session".to_string(),
                engine: EngineId::Codex,
                model: "gpt-5".to_string(),
                cwd: missing.to_string_lossy().to_string(),
                created_at: 1_717_171_700,
            },
            None,
        )
        .unwrap_err();
    assert!(error.contains("工作目录不存在"));
    assert!(store.get_session("missing-session").is_err());
    assert_eq!(store.list_folders().unwrap().len(), 2);

    let file = project.join("not-a-directory.txt");
    fs::write(&file, "not a directory").unwrap();
    let error = store
        .create_session_for_cwd(
            NewSessionRecord {
                id: "file-session".to_string(),
                engine: EngineId::Codex,
                model: "gpt-5".to_string(),
                cwd: file.to_string_lossy().to_string(),
                created_at: 1_717_171_701,
            },
            None,
        )
        .unwrap_err();
    assert!(error.contains("工作目录不存在"));
    assert!(store.get_session("file-session").is_err());
    assert_eq!(store.list_folders().unwrap().len(), 2);

    let _ = fs::remove_dir_all(project);
}

#[test]
fn deleted_project_folder_is_recreated_for_the_next_session() {
    let db_path = temp_history_path("cwd-folder-recreate");
    let project = temp_project_dir("recreate");
    let store = SessionHistoryStore::new(db_path);
    store
        .create_session_for_cwd(
            NewSessionRecord {
                id: "before-delete".to_string(),
                engine: EngineId::Codex,
                model: "gpt-5".to_string(),
                cwd: project.to_string_lossy().to_string(),
                created_at: 1_717_171_700,
            },
            None,
        )
        .unwrap();
    let first_folder = store
        .list_folders()
        .unwrap()
        .into_iter()
        .find(|folder| folder.cwd.is_some())
        .unwrap();
    store.delete_folder(&first_folder.id).unwrap();
    assert_eq!(
        store
            .get_session("before-delete")
            .unwrap()
            .summary
            .folder_id,
        "folder-default"
    );

    store
        .create_session_for_cwd(
            NewSessionRecord {
                id: "after-delete".to_string(),
                engine: EngineId::Codex,
                model: "gpt-5".to_string(),
                cwd: project.to_string_lossy().to_string(),
                created_at: 1_717_171_701,
            },
            None,
        )
        .unwrap();
    let second_folder_id = store.get_session("after-delete").unwrap().summary.folder_id;
    assert_ne!(second_folder_id, first_folder.id);
    assert_ne!(second_folder_id, "folder-default");

    let _ = fs::remove_dir_all(project);
}

#[test]
fn session_folder_integrity_repair_moves_orphans_to_default() {
    let path = temp_history_path("folder-integrity-repair");
    let store = SessionHistoryStore::new(path.clone());
    store
        .create_session(NewSessionRecord {
            id: "orphan-session".to_string(),
            engine: EngineId::ClaudeCode,
            model: "claude-sonnet-4.6".to_string(),
            cwd: r"D:\work\orphan".to_string(),
            created_at: 1_717_171_700,
        })
        .unwrap();
    drop(store);

    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute(
        "UPDATE session SET folder_id = 'folder-missing' WHERE id = 'orphan-session'",
        [],
    )
    .unwrap();
    drop(conn);

    let repaired = SessionHistoryStore::new(path);
    assert_eq!(
        repaired.list_sessions().unwrap()[0].folder_id,
        "folder-default"
    );
}

#[test]
fn schema_v18_migrates_existing_sessions_into_default_folder() {
    let path = temp_history_path("folders-v18-migration");
    let store = SessionHistoryStore::new(path.clone());
    store
        .create_session(NewSessionRecord {
            id: "legacy-session".to_string(),
            engine: EngineId::ClaudeCode,
            model: "claude-sonnet-4.6".to_string(),
            cwd: r"D:\work\legacy".to_string(),
            created_at: 1_717_171_700,
        })
        .unwrap();
    drop(store);

    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch(
        "DROP TABLE session_folder;
         ALTER TABLE session DROP COLUMN folder_id;
         PRAGMA user_version = 18;",
    )
    .unwrap();
    drop(conn);

    let migrated = SessionHistoryStore::new(path.clone());
    assert_eq!(migrated.list_folders().unwrap()[0].id, "folder-default");
    assert_eq!(
        migrated.list_sessions().unwrap()[0].folder_id,
        "folder-default"
    );
    let conn = rusqlite::Connection::open(path).unwrap();
    assert_eq!(
        conn.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        SCHEMA_VERSION
    );
}

#[test]
fn session_runtime_capability_snapshot_survives_history_restore() {
    let path = temp_history_path("runtime-capability-snapshot");
    let store = SessionHistoryStore::new(path);
    store
        .create_session(NewSessionRecord {
            id: "history-capability".to_string(),
            engine: EngineId::ClaudeCode,
            model: "claude-sonnet-4.6".to_string(),
            cwd: "D:/repo".to_string(),
            created_at: 1_717_171_700,
        })
        .unwrap();
    store
        .record_event_for_session(
            "history-capability",
            &AgentEvent::SessionStarted {
                session_id: "cli-capability".to_string(),
                engine: EngineId::ClaudeCode,
                model: "claude-sonnet-4.6".to_string(),
                cwd: "D:/repo".to_string(),
                ts: 1_717_171_701_000,
                capabilities: Some(helm_lib::protocol::RuntimeCapabilitySnapshot {
                    web_search: helm_lib::protocol::RuntimeCapabilityAvailability::Available,
                    web_fetch: helm_lib::protocol::RuntimeCapabilityAvailability::Unavailable,
                    approval_contract_version: "claude-hook-bridge-v1".to_string(),
                    capability_snapshot_id: None,
                    auto_review_strategy: None,
                }),
            },
        )
        .unwrap();
    let restored = store.get_session("history-capability").unwrap();
    let capabilities = restored.summary.runtime_capabilities.unwrap();
    assert_eq!(
        capabilities.web_search,
        helm_lib::protocol::RuntimeCapabilityAvailability::Available
    );
    assert_eq!(
        capabilities.approval_contract_version,
        "claude-hook-bridge-v1"
    );
    store
        .set_safe_permission_profile("history-capability", "auto")
        .unwrap();
    assert_eq!(
        store
            .get_session("history-capability")
            .unwrap()
            .summary
            .safe_permission_profile,
        "auto"
    );
    assert!(store
        .set_safe_permission_profile("history-capability", "full_access")
        .is_err());
}

#[test]
fn schema_v16_migrates_turn_context_and_history_table() {
    let path = temp_history_path("turn-v17-migration");
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE session (
               id TEXT PRIMARY KEY, cli_session_id TEXT UNIQUE, title TEXT NOT NULL,
               engine TEXT NOT NULL, model TEXT NOT NULL, cwd TEXT NOT NULL,
               status TEXT NOT NULL, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL,
               summary TEXT, provider_id TEXT NOT NULL DEFAULT '', pinned INTEGER NOT NULL DEFAULT 0,
               runtime_capabilities_json TEXT, safe_permission_profile TEXT NOT NULL DEFAULT 'standard'
             );
             CREATE TABLE turn_snapshot (
               history_session_id TEXT PRIMARY KEY, turn_id TEXT NOT NULL,
               turn_epoch INTEGER NOT NULL, status TEXT NOT NULL, terminal_reason TEXT,
               recoverable INTEGER NOT NULL DEFAULT 1, event_seq INTEGER NOT NULL DEFAULT 0,
               updated_at INTEGER NOT NULL
             );
             CREATE TABLE turn (
               id TEXT PRIMARY KEY, session_id TEXT NOT NULL, turn_idx INTEGER NOT NULL
             );
             INSERT INTO session (id, title, engine, model, cwd, status, created_at, updated_at)
             VALUES ('history-v16', '旧会话', 'claude-code', 'model', 'D:/repo', 'idle', 1, 1);
             INSERT INTO turn_snapshot
               (history_session_id, turn_id, turn_epoch, status, recoverable, event_seq, updated_at)
             VALUES ('history-v16', 'turn-1', 1, 'succeeded', 0, 3, 2);
             PRAGMA user_version = 16;",
        )
        .unwrap();
    }
    let store = SessionHistoryStore::new(path.clone());
    let detail = store.get_session("history-v16").unwrap();
    assert!(detail.turns.is_empty());
    let conn = rusqlite::Connection::open(path).unwrap();
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, SCHEMA_VERSION);
    for column in ["turn_mode", "permission_profile", "started_at"] {
        let exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('turn_snapshot') WHERE name = ?1",
                [column],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(exists, 1, "turn_snapshot 缺少 {column}");
    }
    for column in ["history_session_id", "turn_id", "turn_epoch", "turn_mode"] {
        let exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('turn') WHERE name = ?1",
                [column],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(exists, 1, "旧 turn 表迁移后缺少 {column}");
    }
}

#[test]
fn approval_matcher_copy_survives_history_restore() {
    let path = temp_history_path("approval-matcher-copy");
    let store = SessionHistoryStore::new(path);
    store
        .create_session(NewSessionRecord {
            id: "history-approval".to_string(),
            engine: EngineId::ClaudeCode,
            model: "model".to_string(),
            cwd: "D:/repo".to_string(),
            created_at: 1,
        })
        .unwrap();
    store
        .record_event_for_session(
            "history-approval",
            &AgentEvent::ApprovalRequest {
                session_id: "cli-approval".to_string(),
                id: "approval-1".to_string(),
                action: "WebFetch".to_string(),
                detail: "https://example.com/docs".to_string(),
                input: Some(serde_json::json!({"url":"https://example.com/docs"})),
                available_decisions: vec![],
                persistent_label: Some("此项目始终允许读取 https://example.com:443".to_string()),
                matcher_summary: Some(
                    "当前引擎 + 当前项目 + GET/HEAD + https://example.com:443".to_string(),
                ),
            },
        )
        .unwrap();
    let approval = store
        .get_session("history-approval")
        .unwrap()
        .approvals
        .into_iter()
        .next()
        .unwrap();
    assert_eq!(
        approval.persistent_label.as_deref(),
        Some("此项目始终允许读取 https://example.com:443")
    );
    assert!(approval.matcher_summary.unwrap().contains("GET/HEAD"));
}

#[test]
fn persistent_runtime_allow_is_mirrored_and_revoked_in_schema_v13() {
    let path = temp_history_path("runtime-grant-v13");
    let store = SessionHistoryStore::new(path.clone());
    store
        .create_session(NewSessionRecord {
            id: "history-1".to_string(),
            engine: EngineId::ClaudeCode,
            model: "claude-sonnet-4.6".to_string(),
            cwd: "D:/repo".to_string(),
            created_at: 1_717_171_700,
        })
        .unwrap();
    store
        .set_session_provider("history-1", "provider-a")
        .unwrap();
    let rule = PermissionRule {
        id: "approval-project:web-search".to_string(),
        principal: "main-agent".to_string(),
        effect: PermissionEffect::Allow,
        scope: PermissionScope::Project,
        scope_binding: PermissionScopeBinding {
            project_root: Some("D:/repo".to_string()),
            ..PermissionScopeBinding::default()
        },
        engine: Some("claude-code".to_string()),
        capability: Capability::NetworkRequest,
        operation: Some("WebSearch".to_string()),
        resource_pattern: None,
        created_at: 100,
        expires_at: None,
        max_uses: None,
        uses: 0,
    };
    store.save_permission_rule(&rule).unwrap();
    let action = normalize_tool_action(
        "claude-code",
        "history-1",
        "turn-1",
        "tool-1",
        "WebSearch",
        &serde_json::json!({"query":"rust docs"}),
        Some("D:/repo"),
    );
    store.save_runtime_grant_for_action(&rule, &action).unwrap();
    let later_search = normalize_tool_action(
        "claude-code",
        "history-1",
        "turn-2",
        "tool-2",
        "WebSearch",
        &serde_json::json!({"query":"tokio docs"}),
        Some(r"\\?\D:\repo"),
    );
    assert!(store
        .runtime_grant_matches(&rule.id, &later_search)
        .unwrap());
    let mut samples = Vec::with_capacity(2_000);
    for _ in 0..2_000 {
        let started = std::time::Instant::now();
        assert!(store
            .runtime_grant_matches(&rule.id, &later_search)
            .unwrap());
        samples.push(started.elapsed());
    }
    samples.sort_unstable();
    let p95 = samples[samples.len() * 95 / 100];
    eprintln!("runtime-grant-warm-p95-us={}", p95.as_micros());
    assert!(p95 < std::time::Duration::from_millis(5));
    let moved = path.with_extension("sqlite.moved");
    let _ = fs::remove_file(&moved);
    fs::rename(&path, &moved).unwrap();
    assert!(
        store
            .runtime_grant_matches(&rule.id, &later_search)
            .unwrap(),
        "a warm RuntimeGrant match must not reopen SQLite"
    );
    fs::rename(&moved, &path).unwrap();
    let conn = rusqlite::Connection::open(&path).unwrap();
    let grant: (String, String, Option<i64>) = conn
        .query_row(
            "SELECT matcher_kind, matcher_value, revoked_at FROM runtime_grant WHERE id = ?1",
            [&rule.id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(grant.0, "tool_family");
    assert_eq!(grant.1, "WebSearch");
    assert_eq!(grant.2, None);
    store
        .set_session_provider("history-1", "provider-b")
        .unwrap();
    assert!(
        !store
            .runtime_grant_matches(&rule.id, &later_search)
            .unwrap(),
        "RuntimeGrant must not cross provider bindings"
    );
    store
        .set_session_provider("history-1", "provider-a")
        .unwrap();
    store.remove_permission_rule(&rule.id).unwrap();
    let revoked: Option<i64> = conn
        .query_row(
            "SELECT revoked_at FROM runtime_grant WHERE id = ?1",
            [&rule.id],
            |row| row.get(0),
        )
        .unwrap();
    assert!(revoked.is_some());
    assert!(!store
        .runtime_grant_matches(&rule.id, &later_search)
        .unwrap());
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
            capabilities: None,
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
            cached_input_tokens: None,
            cache_write_input_tokens: None,
            output_tokens: 25,
            cost_usd: 0.03,
            service_tier: None,
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
                capabilities: None,
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
                capabilities: None,
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
                cached_input_tokens: None,
                cache_write_input_tokens: None,
                output_tokens: 1_000_000,
                cost_usd: 0.0,
                service_tier: None,
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
                cached_input_tokens: None,
                cache_write_input_tokens: None,
                output_tokens: 1_000_000,
                cost_usd: 0.42,
                service_tier: None,
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
            capabilities: None,
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
                cached_input_tokens: None,
                cache_write_input_tokens: None,
                output_tokens,
                cost_usd,
                service_tier: None,
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
            capabilities: None,
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
            outcome: None,
            started: None,
            has_output: None,
            retryable: None,
            denial_source: None,
            native_denial_code: None,
        })
        .unwrap();

    let detail = store.get_session("local-1").unwrap();
    let diff = detail.tool_calls[0].diff.as_ref().expect("应恢复 diff");
    assert_eq!(diff.path, "demo.ts");
    assert_eq!(diff.hunks[0].lines[0].kind, DiffKind::Del);
    assert_eq!(diff.hunks[0].lines[1].kind, DiffKind::Add);
}

#[test]
fn restored_store_continues_after_the_latest_persisted_turn_epoch() {
    let path = temp_history_path("turn-epoch-restart");
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
    store
        .upsert_turn_snapshot(helm_lib::sessions::TurnSnapshotRecord {
            history_session_id: "local-1".to_string(),
            turn_id: "turn-7".to_string(),
            turn_epoch: 7,
            status: helm_lib::turn_supervisor::TurnStatus::Succeeded,
            terminal_reason: Some("end".to_string()),
            recoverable: false,
            event_seq: 4,
            updated_at: 1_717_171_704_000,
            mode: "build".to_string(),
            permission_profile: "standard".to_string(),
            started_at: 1_717_171_703_000,
        })
        .unwrap();

    drop(store);
    let restored = SessionHistoryStore::new(path);
    assert_eq!(restored.latest_turn_epoch("local-1").unwrap(), 7);
}

#[test]
fn terminal_turn_closes_pending_tools_and_approvals_atomically() {
    let path = temp_history_path("terminal-artifacts");
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
        .record_event_for_session(
            "local-1",
            &AgentEvent::ToolCall {
                session_id: "cli-1".to_string(),
                id: "tool-pending".to_string(),
                name: "Bash".to_string(),
                input: serde_json::json!({"command":"exit 1"}),
                status: CallStatus::Pending,
            },
        )
        .unwrap();
    store
        .record_event_for_session(
            "local-1",
            &AgentEvent::ApprovalRequest {
                session_id: "cli-1".to_string(),
                id: "approval-pending".to_string(),
                action: "Bash".to_string(),
                detail: "exit 1".to_string(),
                input: None,
                available_decisions: vec![],
                persistent_label: None,
                matcher_summary: None,
            },
        )
        .unwrap();
    store
        .record_event_for_session(
            "local-1",
            &AgentEvent::TurnComplete {
                session_id: "cli-1".to_string(),
                stop_reason: StopReason::Error,
            },
        )
        .unwrap();

    let detail = store.get_session("local-1").unwrap();
    assert_eq!(detail.tool_calls[0].status, HistoryToolStatus::Error);
    assert!(detail.tool_calls[0]
        .output
        .as_deref()
        .unwrap()
        .starts_with("[turn_failed]"));
    assert_eq!(detail.approvals[0].status, "expired");
    assert!(detail.approvals[0]
        .error
        .as_deref()
        .unwrap()
        .starts_with("[turn_failed]"));
}

#[test]
fn terminal_turn_does_not_race_an_applying_approval_commit() {
    let path = temp_history_path("terminal-applying-approval");
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
    let spec = start_change_27d_turn(&store, "local-1", "deny tool", Vec::new());
    let supervisor = TurnSupervisor::new(store.clone());
    supervisor.begin(
        "local-1",
        &spec.turn_id,
        spec.turn_epoch,
        "build",
        "standard",
    );
    store
        .record_event_for_session_in_turn(
            "local-1",
            Some(&spec.turn_id),
            &AgentEvent::ApprovalRequest {
                session_id: "cli-1".to_string(),
                id: "approval-deny".to_string(),
                action: "Bash".to_string(),
                detail: "echo denied".to_string(),
                input: None,
                available_decisions: vec![],
                persistent_label: None,
                matcher_summary: None,
            },
        )
        .unwrap();
    store
        .mark_approval_applying("local-1", "approval-deny", "deny")
        .unwrap();

    assert!(supervisor.accept_event(
        "local-1",
        Some(&spec.turn_id),
        Some(spec.turn_epoch),
        1,
        &AgentEvent::TurnComplete {
            session_id: "cli-1".to_string(),
            stop_reason: StopReason::Interrupted,
        },
    ));

    let applying = &store.get_session("local-1").unwrap().approvals[0];
    assert_eq!(applying.status, "applying");
    assert_eq!(applying.decision.as_deref(), Some("deny"));
    store
        .resolve_approval_with_decision("local-1", "approval-deny", "deny", None)
        .unwrap();
    let resolved = &store.get_session("local-1").unwrap().approvals[0];
    assert_eq!(resolved.status, "resolved");
    assert_eq!(resolved.decision.as_deref(), Some("deny"));
    assert_eq!(resolved.error, None);
}

#[test]
fn reopening_terminal_session_reconciles_legacy_pending_artifacts() {
    let path = temp_history_path("terminal-reconcile");
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
    store
        .upsert_turn_snapshot(helm_lib::sessions::TurnSnapshotRecord {
            history_session_id: "local-1".to_string(),
            turn_id: "turn-1".to_string(),
            turn_epoch: 1,
            status: helm_lib::turn_supervisor::TurnStatus::Succeeded,
            terminal_reason: Some("end".to_string()),
            recoverable: false,
            event_seq: 2,
            updated_at: 1_717_171_704_000,
            mode: "build".to_string(),
            permission_profile: "standard".to_string(),
            started_at: 1_717_171_703_000,
        })
        .unwrap();
    drop(store);

    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute(
        "INSERT INTO tool_call (id, session_id, name, input_json, status, output, ts)
         VALUES ('legacy-tool', 'local-1', 'Bash', '{}', 'pending', NULL, 1717171704000)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO approval (id, session_id, action, detail, status, ts)
         VALUES ('legacy-approval', 'local-1', 'Bash', 'legacy', 'pending', 1717171704000)",
        [],
    )
    .unwrap();
    drop(conn);

    let restored = SessionHistoryStore::new(path);
    let detail = restored.get_session("local-1").unwrap();
    assert_eq!(detail.tool_calls[0].status, HistoryToolStatus::Error);
    assert_eq!(detail.approvals[0].status, "expired");
}

#[test]
fn session_history_never_persists_inline_or_structured_credentials() {
    let path = temp_history_path("credential-redaction");
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
    let sentinel = "HELM_TEST_PERSISTED_SECRET_SENTINEL";
    store
        .record_user_message(
            "local-1",
            &format!("Authorization: Bearer {sentinel}"),
            1_717_171_701_000,
        )
        .unwrap();
    store
        .record_event_for_session(
            "local-1",
            &AgentEvent::ToolCall {
                session_id: "cli-1".to_string(),
                id: "tool-secret".to_string(),
                name: "Read".to_string(),
                input: serde_json::json!({"file_path":"settings.json"}),
                status: CallStatus::Pending,
            },
        )
        .unwrap();
    store
        .record_event_for_session(
            "local-1",
            &AgentEvent::ToolResult {
                session_id: "cli-1".to_string(),
                id: "tool-secret".to_string(),
                status: ToolStatus::Success,
                output: Some(format!(r#"{{"ANTHROPIC_AUTH_TOKEN":"{sentinel}"}}"#)),
                diff: None,
                outcome: None,
                started: None,
                has_output: None,
                retryable: None,
                denial_source: None,
                native_denial_code: None,
            },
        )
        .unwrap();

    let encoded = serde_json::to_string(&store.get_session("local-1").unwrap()).unwrap();
    assert!(!encoded.contains(sentinel));
    assert!(encoded.contains("REDACTED"));
}

#[test]
fn orphan_tool_result_is_rejected_instead_of_becoming_a_silent_success() {
    let path = temp_history_path("orphan-tool-result");
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

    let error = store
        .record_event_for_session(
            "local-1",
            &AgentEvent::ToolResult {
                session_id: "cli-1".to_string(),
                id: "missing-tool".to_string(),
                status: ToolStatus::Success,
                output: Some("should not persist".to_string()),
                diff: None,
                outcome: None,
                started: None,
                has_output: None,
                retryable: None,
                denial_source: None,
                native_denial_code: None,
            },
        )
        .unwrap_err();
    assert!(error.contains("Query returned no rows"), "{error}");
    assert!(store.get_session("local-1").unwrap().tool_calls.is_empty());
}

#[test]
fn tool_call_replay_is_idempotent_but_input_collision_is_rejected() {
    let path = temp_history_path("tool-call-collision");
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
    let original = AgentEvent::ToolCall {
        session_id: "cli-1".to_string(),
        id: "tool-1".to_string(),
        name: "Read".to_string(),
        input: serde_json::json!({"file_path":"one.txt"}),
        status: CallStatus::Pending,
    };
    store
        .record_event_for_session("local-1", &original)
        .unwrap();
    store
        .record_event_for_session(
            "local-1",
            &AgentEvent::ToolResult {
                session_id: "cli-1".to_string(),
                id: "tool-1".to_string(),
                status: ToolStatus::Success,
                output: Some("done".to_string()),
                diff: None,
                outcome: None,
                started: None,
                has_output: None,
                retryable: None,
                denial_source: None,
                native_denial_code: None,
            },
        )
        .unwrap();
    store
        .record_event_for_session("local-1", &original)
        .unwrap();
    let replayed = store.get_session("local-1").unwrap();
    assert_eq!(replayed.tool_calls.len(), 1);
    assert_eq!(replayed.tool_calls[0].status, HistoryToolStatus::Success);
    assert_eq!(replayed.tool_calls[0].output.as_deref(), Some("done"));

    let collision = store
        .record_event_for_session(
            "local-1",
            &AgentEvent::ToolCall {
                session_id: "cli-1".to_string(),
                id: "tool-1".to_string(),
                name: "Read".to_string(),
                input: serde_json::json!({"file_path":"two.txt"}),
                status: CallStatus::Pending,
            },
        )
        .unwrap_err();
    assert!(
        collision.contains("tool call identity collision"),
        "{collision}"
    );
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
            capabilities: None,
        })
        .unwrap();
    store
        .record_event(&AgentEvent::Checkpoint {
            session_id: "claude-real-1".to_string(),
            id: "ckpt-1".to_string(),
            label: "改动前：demo.ts".to_string(),
            ts: 1_717_171_703_000,
            restorable: false,
            file_count: 0,
            reason: Some("legacy_empty_snapshot".to_string()),
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
        .save_checkpoint(
            "ckpt-1",
            "local-1",
            0,
            "写文件前",
            "snap-1",
            2,
            "turn-1",
            true,
            1,
            None,
        )
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
        .save_checkpoint(
            "ckpt-1",
            "local-1",
            0,
            "写文件前",
            "snap-1",
            before_millis,
            "turn-1",
            true,
            1,
            None,
        )
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
    let turn_columns = ["turn_id", "turn_mode", "permission_profile", "status"];
    for column in turn_columns {
        let exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('turn') WHERE name = ?1",
                [column],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            exists, 1,
            "旧 turn 死表应被替换为 v17 审计结构，缺少 {column}"
        );
    }

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
fn permission_schema_v5_migrates_to_current_without_losing_approval_rows() {
    let path = temp_history_path("permission-v6-migration");
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE session (
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
             CREATE TABLE approval (
               id TEXT NOT NULL,
               session_id TEXT NOT NULL REFERENCES session(id) ON DELETE CASCADE,
               action TEXT NOT NULL,
               detail TEXT NOT NULL,
               status TEXT NOT NULL DEFAULT 'pending',
               ts INTEGER NOT NULL,
               PRIMARY KEY (id, session_id)
             );
             INSERT INTO session
               (id, cli_session_id, title, engine, model, cwd, status, created_at, updated_at)
             VALUES
               ('local-1', NULL, '旧会话', 'claude-code', 'claude-sonnet-4.6',
                'D:/work/demo', 'idle', 1717171700, 1717171700);
             INSERT INTO approval (id, session_id, action, detail, status, ts)
             VALUES ('appr-old', 'local-1', 'Write', 'legacy.txt', 'pending', 1717171700000);
             PRAGMA user_version = 5;",
        )
        .unwrap();
    }

    let store = SessionHistoryStore::new(path.clone());
    let detail = store.get_session("local-1").unwrap();
    assert_eq!(detail.approvals.len(), 1);
    assert_eq!(detail.approvals[0].id, "appr-old");

    let conn = rusqlite::Connection::open(path).unwrap();
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, SCHEMA_VERSION);
    let turn_snapshot_exists: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='turn_snapshot'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(turn_snapshot_exists, 1);
    let permission_rule_table: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='permission_rule'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(permission_rule_table, 1);

    let mut stmt = conn.prepare("PRAGMA table_info(approval)").unwrap();
    let columns: Vec<String> = stmt
        .query_map([], |row| row.get(1))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    for column in ["decision", "rule_id", "error", "resolved_at"] {
        assert!(columns.iter().any(|item| item == column), "缺少列 {column}");
    }
}

#[test]
fn current_schema_includes_permission_context_audit_and_usage_price_snapshot() {
    let path = temp_history_path("permission-schema-v9");
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

    let conn = rusqlite::Connection::open(path).unwrap();
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, SCHEMA_VERSION);
    for column in [
        "principal",
        "tool_call_id",
        "turn_id",
        "history_session_id",
        "project_root",
    ] {
        let exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('permission_rule') WHERE name = ?1",
                [column],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(exists, 1, "permission_rule 缺少 {column}");
    }
    let audit_exists: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='permission_audit'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(audit_exists, 1);
    for column in [
        "provider_id",
        "cached_input_tokens",
        "cache_write_input_tokens",
        "reported_cost_usd",
        "cost_kind",
        "price_source",
        "service_tier",
        "pricing_catalog_version",
        "price_snapshot_json",
    ] {
        let exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('usage') WHERE name = ?1",
                [column],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(exists, 1, "usage 缺少 {column}");
    }
    for column in [
        "execution_status",
        "execution_authorization",
        "execution_started_at",
        "execution_finished_at",
        "revocation_too_late_at",
    ] {
        let exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('permission_audit') WHERE name = ?1",
                [column],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(exists, 1, "permission_audit 缺少 {column}");
    }
}

#[test]
fn schema_v11_migrates_existing_cost_to_honest_legacy_bucket() {
    let path = temp_history_path("pricing-v11-legacy-cost");
    let store = SessionHistoryStore::new(path.clone());
    store
        .create_session(NewSessionRecord {
            id: "legacy-session".to_string(),
            engine: EngineId::Codex,
            model: "legacy-model".to_string(),
            cwd: r"D:\work\legacy".to_string(),
            created_at: 1_717_171_700,
        })
        .unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute(
            "INSERT INTO usage
               (session_id, model, input_tokens, output_tokens, cost_usd, ts)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params!["legacy-session", "legacy-model", 100, 10, 1.23, now],
        )
        .unwrap();
        conn.execute_batch(
            "UPDATE usage SET cost_kind = 'unknown', price_source = 'unknown';
             PRAGMA user_version = 11;",
        )
        .unwrap();
    }

    let reopened = SessionHistoryStore::new(path.clone());
    let stats = reopened.get_usage_stats(30).unwrap();
    assert!((stats.total_cost - 1.23).abs() < 1e-9);
    assert!((stats.legacy_cost - 1.23).abs() < 1e-9);
    assert_eq!(stats.legacy_count, 1);
    assert_eq!(stats.unknown_count, 0);
    let conn = rusqlite::Connection::open(path).unwrap();
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, SCHEMA_VERSION);
}

#[test]
fn permission_schema_v6_migration_rolls_back_all_changes_on_midway_failure() {
    let path = temp_history_path("permission-v6-rollback");
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE VIRTUAL TABLE approval USING fts5(id, session_id, action, detail, status, ts);
             INSERT INTO approval (id, session_id, action, detail, status, ts)
             VALUES ('legacy', 'local-1', 'Write', 'legacy.txt', 'pending', '1234');
             PRAGMA user_version = 5;",
        )
        .unwrap();
    }

    let store = SessionHistoryStore::new(path.clone());
    let error = store.list_permission_rules().unwrap_err();
    assert!(error.contains("会话数据库错误"));

    let conn = rusqlite::Connection::open(path).unwrap();
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, 5, "失败迁移不得提前更新 user_version");
    let permission_rule_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='permission_rule'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(permission_rule_count, 0, "失败迁移创建的表必须回滚");
    let mut stmt = conn.prepare("PRAGMA table_info(approval)").unwrap();
    let columns: Vec<String> = stmt
        .query_map([], |row| row.get(1))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    for column in ["decision", "rule_id", "error", "resolved_at"] {
        assert!(!columns.iter().any(|item| item == column));
    }
}

#[test]
fn permission_schema_concurrent_store_initialization_is_safe_and_complete() {
    let path = temp_history_path("permission-concurrent-init");
    let barrier = Arc::new(Barrier::new(3));
    let mut handles = Vec::new();
    for _ in 0..2 {
        let path = path.clone();
        let barrier = Arc::clone(&barrier);
        handles.push(std::thread::spawn(move || {
            let store = SessionHistoryStore::new(path);
            barrier.wait();
            store.list_permission_rules()
        }));
    }
    barrier.wait();
    for handle in handles {
        let result = handle.join().unwrap();
        assert!(result.is_ok(), "并发初始化失败：{result:?}");
    }

    let conn = rusqlite::Connection::open(path).unwrap();
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, SCHEMA_VERSION);
    let permission_rule_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='permission_rule'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(permission_rule_count, 1);
    let permission_audit_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='permission_audit'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(permission_audit_count, 1);
    let mut stmt = conn.prepare("PRAGMA table_info(approval)").unwrap();
    let columns: Vec<String> = stmt
        .query_map([], |row| row.get(1))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    for column in ["decision", "rule_id", "error", "resolved_at"] {
        assert!(columns.iter().any(|item| item == column));
    }
}

#[test]
fn permission_rule_crud_round_trips_structured_values() {
    let path = temp_history_path("permission-rule-crud");
    let store = SessionHistoryStore::new(path.clone());
    let rule = PermissionRule {
        id: "rule-project-write".to_string(),
        principal: "main-agent".to_string(),
        effect: PermissionEffect::Allow,
        scope: PermissionScope::Project,
        scope_binding: Default::default(),
        engine: Some("claude-code".to_string()),
        capability: Capability::FileWrite,
        operation: Some("Edit".to_string()),
        resource_pattern: Some("D:/work/demo/**".to_string()),
        created_at: 1_752_314_400_000,
        expires_at: Some(1_752_318_000_000),
        max_uses: Some(10),
        uses: 0,
    };

    store.save_permission_rule(&rule).unwrap();
    assert_eq!(store.list_permission_rules().unwrap(), vec![rule.clone()]);

    let conn = rusqlite::Connection::open(path).unwrap();
    let stored: (String, String, String, i64) = conn
        .query_row(
            "SELECT effect, scope, capability, uses FROM permission_rule WHERE id = ?1",
            [rule.id.as_str()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!(
        stored,
        ("allow".into(), "project".into(), "file_write".into(), 0)
    );

    store.remove_permission_rule(&rule.id).unwrap();
    assert!(store.list_permission_rules().unwrap().is_empty());
}

#[test]
fn permission_evaluation_is_idempotent_consumes_once_and_audits_every_distinct_attempt() {
    let path = temp_history_path("permission-evaluate-audit");
    let store = SessionHistoryStore::new(path.clone());
    let mut rule = PermissionRule {
        id: "once-ls".to_string(),
        principal: "main-agent".to_string(),
        effect: PermissionEffect::Allow,
        scope: PermissionScope::Once,
        scope_binding: helm_lib::permissions::PermissionScopeBinding {
            tool_call_id: Some("tool-1".to_string()),
            turn_id: Some("turn-1".to_string()),
            session_id: Some("history-1".to_string()),
            project_root: None,
        },
        engine: Some("claude-code".to_string()),
        capability: Capability::ProcessExec,
        operation: Some("ls".to_string()),
        resource_pattern: None,
        created_at: 1,
        expires_at: None,
        max_uses: Some(1),
        uses: 0,
    };
    let action = normalize_tool_action(
        "claude-code",
        "history-1",
        "turn-1",
        "tool-1",
        "Bash",
        &serde_json::json!({"command": "ls -la"}),
        Some("D:/repo"),
    );

    let before_approval = store.evaluate_permission_action(&action).unwrap();
    assert_eq!(before_approval.effect, PermissionEffect::Ask);

    store.save_permission_rule(&rule).unwrap();
    let first = store.evaluate_permission_action(&action).unwrap();
    let replay = store.evaluate_permission_action(&action).unwrap();
    assert_eq!(
        first, replay,
        "同一策略版本的 ToolCall 重放必须返回同一决定"
    );
    assert_eq!(first.effect, PermissionEffect::Allow);
    assert!(first.policy_version > before_approval.policy_version);

    rule = store.list_permission_rules().unwrap().remove(0);
    assert_eq!(rule.uses, 1, "幂等重放不能重复消耗 once 规则");

    let changed = normalize_tool_action(
        "claude-code",
        "history-1",
        "turn-1",
        "tool-1",
        "Bash",
        &serde_json::json!({"command": "ls /secret"}),
        Some("D:/repo"),
    );
    let collision = store.evaluate_permission_action(&changed).unwrap();
    assert_eq!(collision.effect, PermissionEffect::Deny);
    assert!(collision.reason.contains("identity collision"));

    let replay_after_collision = store.evaluate_permission_action(&action).unwrap();
    assert_eq!(
        replay_after_collision.effect,
        PermissionEffect::Deny,
        "ToolCall id 一旦发现输入碰撞，之后不得重放旧输入绕回 allow"
    );
    assert!(replay_after_collision.reason.contains("identity collision"));

    let conn = rusqlite::Connection::open(path).unwrap();
    let audit_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM permission_audit", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(
        audit_count, 3,
        "旧策略 ask、新策略 allow、输入冲突各有一条审计"
    );
}

#[test]
fn permission_audit_tracks_execution_revocation_retention_and_redacted_export() {
    let path = temp_history_path("permission-audit-lifecycle");
    let store = SessionHistoryStore::new(path.clone());
    let secret_path = "D:/private/customer-secret.txt";
    let action = normalize_tool_action(
        "claude-code",
        "history-sensitive",
        "turn-sensitive",
        "tool-sensitive",
        "Read",
        &serde_json::json!({"file_path": secret_path}),
        Some("D:/private"),
    );
    let rule = PermissionRule {
        id: "rule-sensitive-read".to_string(),
        principal: "main-agent".to_string(),
        effect: PermissionEffect::Allow,
        scope: PermissionScope::Session,
        scope_binding: helm_lib::permissions::PermissionScopeBinding {
            session_id: Some("history-sensitive".to_string()),
            ..Default::default()
        },
        engine: Some("claude-code".to_string()),
        capability: Capability::FileRead,
        operation: Some("Read".to_string()),
        resource_pattern: Some(secret_path.to_string()),
        created_at: 1,
        expires_at: None,
        max_uses: None,
        uses: 0,
    };
    store.save_permission_rule(&rule).unwrap();
    assert_eq!(
        store.evaluate_permission_action(&action).unwrap().effect,
        PermissionEffect::Allow
    );
    store.mark_permission_execution_started(&action).unwrap();

    let too_late = store
        .remove_permission_rule_with_legacy_compat(&rule.id)
        .unwrap();
    assert_eq!(too_late, 1, "正在执行的动作必须留下撤销过晚事实");
    store.finish_permission_execution(&action, true).unwrap();

    let redacted = store.export_permission_audit_json(false).unwrap();
    assert!(!redacted.contains(secret_path));
    assert!(!redacted.contains("history-sensitive"));
    assert!(redacted.contains("resourceDigests"));
    assert!(redacted.contains("revocationTooLateAt"));
    let detailed = store.export_permission_audit_json(true).unwrap();
    assert!(detailed.contains(secret_path));

    let conn = rusqlite::Connection::open(&path).unwrap();
    let status: (String, Option<i64>) = conn
        .query_row(
            "SELECT execution_status, revocation_too_late_at FROM permission_audit LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(status.0, "completed");
    assert!(status.1.is_some());
    conn.execute("UPDATE permission_audit SET created_at = 1", [])
        .unwrap();
    assert_eq!(store.prune_permission_audit_before(2).unwrap(), 1);
    assert_eq!(store.permission_audit_summary().unwrap().record_count, 0);
}

#[test]
fn runtime_managed_permission_evaluation_keeps_safe_read_and_kernel_boundaries() {
    let path = temp_history_path("permission-settings-defaults");
    let store = SessionHistoryStore::new(path);

    // 旧版本可能保存了 readFiles=deny；该字段现在只是被忽略，不能改变固定安全读取策略。
    let mut legacy_settings =
        serde_json::to_value(helm_lib::settings::AppSettings::default()).unwrap();
    legacy_settings["permissions"]["readFiles"] = serde_json::json!("deny");
    store
        .set_json_setting("app_settings", &legacy_settings)
        .unwrap();

    let read = normalize_tool_action(
        "claude-code",
        "history-1",
        "turn-1",
        "read-1",
        "LS",
        &serde_json::json!({"path": "D:/repo"}),
        Some("D:/repo"),
    );
    assert_eq!(
        store.evaluate_permission_action(&read).unwrap().effect,
        PermissionEffect::Allow,
        "旧 readFiles=deny 不能关闭固定安全读取"
    );

    let grep = normalize_tool_action(
        "claude-code",
        "history-1",
        "turn-1",
        "grep-1",
        "Grep",
        &serde_json::json!({"path": "src/main.rs", "pattern": "fn main"}),
        Some("D:/repo"),
    );
    assert_eq!(
        store.evaluate_permission_action(&grep).unwrap().effect,
        PermissionEffect::Allow,
        "结构化 Grep 应与 Read/LS 使用同一固定安全读取策略"
    );

    for (tool_id, file_path) in [
        ("read-outside-1", "D:/outside.txt"),
        ("read-sensitive-1", "D:/repo/.env.local"),
    ] {
        let unsafe_read = normalize_tool_action(
            "claude-code",
            "history-1",
            "turn-1",
            tool_id,
            "Read",
            &serde_json::json!({"file_path": file_path}),
            Some("D:/repo"),
        );
        let decision = store.evaluate_permission_action(&unsafe_read).unwrap();
        assert_eq!(decision.effect, PermissionEffect::Deny);
        assert!(decision.reason.contains("safe read boundary"));
    }

    let shell_ls = normalize_tool_action(
        "claude-code",
        "history-1",
        "turn-1",
        "shell-ls-1",
        "Bash",
        &serde_json::json!({"command": "ls -la"}),
        Some("D:/repo"),
    );
    assert_eq!(shell_ls.capability, Capability::ProcessExec);
    assert_eq!(
        store.evaluate_permission_action(&shell_ls).unwrap().effect,
        PermissionEffect::Ask,
        "Shell 中的 ls 仍是 ProcessExec，不能按首词自动放行"
    );

    let explicit_deny = PermissionRule {
        id: "safe-read-hard-deny".to_string(),
        principal: "main-agent".to_string(),
        effect: PermissionEffect::Deny,
        scope: PermissionScope::Global,
        scope_binding: Default::default(),
        engine: Some("claude-code".to_string()),
        capability: Capability::FileRead,
        operation: None,
        resource_pattern: Some("D:/repo/**".to_string()),
        created_at: 1,
        expires_at: None,
        max_uses: None,
        uses: 0,
    };
    store.save_permission_rule(&explicit_deny).unwrap();
    let denied_read = normalize_tool_action(
        "claude-code",
        "history-1",
        "turn-1",
        "read-denied-1",
        "Read",
        &serde_json::json!({"file_path": "D:/repo/secret.txt"}),
        Some("D:/repo"),
    );
    assert_eq!(
        store
            .evaluate_permission_action(&denied_read)
            .unwrap()
            .effect,
        PermissionEffect::Deny,
        "显式 Helm Deny 仍必须覆盖固定安全读取 Allow"
    );

    let network = normalize_tool_action(
        "claude-code",
        "history-1",
        "turn-1",
        "web-1",
        "WebFetch",
        &serde_json::json!({"url": "https://example.com"}),
        Some("D:/repo"),
    );
    assert_eq!(
        store.evaluate_permission_action(&network).unwrap().effect,
        PermissionEffect::Ask,
        "普通 RuntimeManaged 网络动作等待 Runtime 审批"
    );
    let command = normalize_tool_action(
        "claude-code",
        "history-1",
        "turn-1",
        "bash-1",
        "Bash",
        &serde_json::json!({"command": "cargo test"}),
        Some("D:/repo"),
    );
    let before = store.evaluate_permission_action(&command).unwrap();
    assert_eq!(before.effect, PermissionEffect::Ask);
    store.add_always_allow_tool("Bash:cargo").unwrap();
    let always_allowed = store.evaluate_permission_action(&command).unwrap();
    assert_eq!(
        always_allowed.effect,
        PermissionEffect::Ask,
        "遗留裸工具名授权没有 RuntimeGrant 身份，不能让普通 Runtime 静默 Allow"
    );
    store.remove_always_allow_tool("Bash:cargo").unwrap();

    let denied_command = normalize_tool_action(
        "claude-code",
        "history-1",
        "turn-1",
        "bash-deny",
        "Bash",
        &serde_json::json!({"command": "cargo test"}),
        Some("D:/repo"),
    );
    assert_eq!(
        store
            .evaluate_permission_action(&denied_command)
            .unwrap()
            .effect,
        PermissionEffect::Ask,
        "普通 RuntimeManaged 命令只由 Runtime 审批与 Kernel 裁决"
    );
}

#[test]
fn legacy_always_allow_values_migrate_once_into_structured_global_rules() {
    let path = temp_history_path("legacy-always-allow-migration");
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
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute(
            "INSERT OR REPLACE INTO setting (key, value_json) VALUES (?1, ?2)",
            rusqlite::params![
                "approval_always_allow",
                serde_json::json!(["Bash:ls", "Write", "Bash:ls"]).to_string()
            ],
        )
        .unwrap();

    store.migrate_legacy_always_allow_rules().unwrap();
    store.migrate_legacy_always_allow_rules().unwrap();

    let rules = store.list_permission_rules().unwrap();
    assert_eq!(rules.len(), 2, "重复迁移或重复 legacy 值都不能产生重复规则");
    assert!(rules.iter().any(|rule| {
        rule.effect == PermissionEffect::Allow
            && rule.scope == PermissionScope::Global
            && rule.engine.as_deref() == Some("claude-code")
            && rule.capability == Capability::ProcessExec
            && rule.operation.as_deref() == Some("ls")
    }));
    assert!(rules.iter().any(|rule| {
        rule.effect == PermissionEffect::Allow
            && rule.scope == PermissionScope::Global
            && rule.engine.as_deref() == Some("claude-code")
            && rule.capability == Capability::FileWrite
            && rule.operation.as_deref() == Some("Write")
    }));
}

#[test]
fn revoking_a_migrated_rule_by_id_also_removes_the_legacy_compat_value() {
    let path = temp_history_path("legacy-always-allow-revoke");
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
    store.add_always_allow_tool("Bash:ls").unwrap();
    let rule = store
        .list_permission_rules()
        .unwrap()
        .into_iter()
        .find(|rule| rule.operation.as_deref() == Some("ls"))
        .unwrap();

    store
        .remove_permission_rule_with_legacy_compat(&rule.id)
        .unwrap();
    store.migrate_legacy_always_allow_rules().unwrap();

    assert!(store.list_permission_rules().unwrap().is_empty());
    assert!(store.get_always_allow_tools().unwrap().is_empty());
}

#[test]
fn permission_rules_cover_all_effects_scopes_unknown_capabilities_and_preserve_uses() {
    let path = temp_history_path("permission-rule-variants");
    let store = SessionHistoryStore::new(path.clone());
    let mut expected = Vec::new();
    let mut index = 0_i64;
    for effect in [
        PermissionEffect::Allow,
        PermissionEffect::Ask,
        PermissionEffect::Deny,
    ] {
        for scope in [
            PermissionScope::Once,
            PermissionScope::Turn,
            PermissionScope::Session,
            PermissionScope::Project,
            PermissionScope::Global,
        ] {
            index += 1;
            let rule = PermissionRule {
                id: format!("rule-{index:02}"),
                principal: "main-agent".to_string(),
                effect,
                scope,
                scope_binding: Default::default(),
                engine: None,
                capability: if index == 15 {
                    Capability::Unknown("CustomTool".to_string())
                } else {
                    Capability::ProcessExec
                },
                operation: Some(format!("operation-{index}")),
                resource_pattern: None,
                created_at: index,
                expires_at: None,
                max_uses: Some(index as u32),
                uses: 0,
            };
            store.save_permission_rule(&rule).unwrap();
            expected.push(rule);
        }
    }
    assert_eq!(store.list_permission_rules().unwrap(), expected);

    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute(
        "UPDATE permission_rule SET uses = 7 WHERE id = 'rule-01'",
        [],
    )
    .unwrap();
    let mut updated = expected[0].clone();
    updated.operation = Some("updated-operation".to_string());
    store.save_permission_rule(&updated).unwrap();
    let uses: i64 = conn
        .query_row(
            "SELECT uses FROM permission_rule WHERE id = 'rule-01'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(uses, 7, "重复保存规则不得重置使用次数");
}

#[test]
fn permission_rule_listing_rejects_invalid_persisted_enum_values() {
    let path = temp_history_path("permission-rule-invalid-enum");
    let store = SessionHistoryStore::new(path.clone());
    let rule = PermissionRule {
        id: "rule-invalid".to_string(),
        principal: "main-agent".to_string(),
        effect: PermissionEffect::Allow,
        scope: PermissionScope::Session,
        scope_binding: Default::default(),
        engine: None,
        capability: Capability::FileRead,
        operation: None,
        resource_pattern: None,
        created_at: 1,
        expires_at: None,
        max_uses: None,
        uses: 0,
    };
    store.save_permission_rule(&rule).unwrap();

    let conn = rusqlite::Connection::open(path).unwrap();
    for (column, invalid) in [
        ("effect", "invalid-effect"),
        ("scope", "invalid-scope"),
        ("capability", "invalid-capability"),
    ] {
        conn.execute(
            &format!("UPDATE permission_rule SET {column} = ?1 WHERE id = 'rule-invalid'"),
            [invalid],
        )
        .unwrap();
        assert!(
            store.list_permission_rules().is_err(),
            "{column} 应拒绝坏枚举"
        );
        store.save_permission_rule(&rule).unwrap();
    }
}

#[test]
fn permission_resolved_approval_replay_does_not_return_to_pending() {
    let path = temp_history_path("permission-approval-replay");
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

    let request = AgentEvent::ApprovalRequest {
        session_id: "cli-1".to_string(),
        id: "appr-replay".to_string(),
        action: "Write".to_string(),
        detail: "first.txt".to_string(),
        input: None,
        available_decisions: vec![],
        persistent_label: None,
        matcher_summary: None,
    };
    store.record_event_for_session("local-1", &request).unwrap();
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute(
            "UPDATE approval SET ts = 1234 WHERE session_id = 'local-1' AND id = 'appr-replay'",
            [],
        )
        .unwrap();
    store
        .mark_approval_applying("local-1", "appr-replay", "allow_project")
        .unwrap();
    store
        .resolve_approval_with_decision(
            "local-1",
            "appr-replay",
            "allow_project",
            Some("rule-project-write"),
        )
        .unwrap();

    store
        .record_event_for_session(
            "local-1",
            &AgentEvent::ApprovalRequest {
                session_id: "cli-1".to_string(),
                id: "appr-replay".to_string(),
                action: "Bash".to_string(),
                detail: "replayed.txt".to_string(),
                input: None,
                available_decisions: vec![],
                persistent_label: None,
                matcher_summary: None,
            },
        )
        .unwrap();

    let approval = &store.get_session("local-1").unwrap().approvals[0];
    assert_eq!(approval.action, "Write");
    assert_eq!(approval.detail, "first.txt");
    assert_eq!(approval.ts, 1234);
    assert_eq!(approval.status, "resolved");
    assert_eq!(approval.decision.as_deref(), Some("allow_project"));
    assert_eq!(approval.rule_id.as_deref(), Some("rule-project-write"));
    assert_eq!(approval.error, None);
    assert!(approval.resolved_at.is_some());
}

#[test]
fn permission_approval_transitions_persist_decision_rule_error_and_resolution_time() {
    let path = temp_history_path("permission-approval-transitions");
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
    for id in ["appr-applying", "appr-resolved", "appr-failed"] {
        store
            .record_event_for_session(
                "local-1",
                &AgentEvent::ApprovalRequest {
                    session_id: "cli-1".to_string(),
                    id: id.to_string(),
                    action: "Bash".to_string(),
                    detail: "cargo test".to_string(),
                    input: None,
                    available_decisions: vec![],
                    persistent_label: None,
                    matcher_summary: None,
                },
            )
            .unwrap();
    }

    store
        .mark_approval_applying("local-1", "appr-applying", "allow_once")
        .unwrap();
    store
        .mark_approval_applying("local-1", "appr-resolved", "allow_session")
        .unwrap();
    store
        .resolve_approval_with_decision(
            "local-1",
            "appr-resolved",
            "allow_session",
            Some("rule-session-exec"),
        )
        .unwrap();
    store
        .mark_approval_applying("local-1", "appr-failed", "allow_once")
        .unwrap();
    store
        .fail_approval("local-1", "appr-failed", "CLI 恢复失败")
        .unwrap();

    let detail = store.get_session("local-1").unwrap();
    let applying = detail
        .approvals
        .iter()
        .find(|approval| approval.id == "appr-applying")
        .unwrap();
    assert_eq!(applying.status, "applying");
    assert_eq!(applying.decision.as_deref(), Some("allow_once"));
    assert_eq!(applying.rule_id, None);
    assert_eq!(applying.error, None);
    assert_eq!(applying.resolved_at, None);

    let resolved = detail
        .approvals
        .iter()
        .find(|approval| approval.id == "appr-resolved")
        .unwrap();
    assert_eq!(resolved.status, "resolved");
    assert_eq!(resolved.decision.as_deref(), Some("allow_session"));
    assert_eq!(resolved.rule_id.as_deref(), Some("rule-session-exec"));
    assert_eq!(resolved.error, None);
    assert!(resolved.resolved_at.is_some());

    let failed = detail
        .approvals
        .iter()
        .find(|approval| approval.id == "appr-failed")
        .unwrap();
    assert_eq!(failed.status, "failed");
    assert_eq!(failed.decision.as_deref(), Some("allow_once"));
    assert_eq!(failed.rule_id, None);
    assert_eq!(failed.error.as_deref(), Some("CLI 恢复失败"));
    assert!(failed.resolved_at.is_some());

    let missing_error = store
        .mark_approval_applying("local-1", "missing", "allow_once")
        .unwrap_err();
    assert!(missing_error.contains("不存在"));
}

#[test]
fn permission_failed_approval_can_be_explicitly_retried() {
    let path = temp_history_path("permission-failed-retry");
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
                id: "retryable".to_string(),
                action: "Bash".to_string(),
                detail: "cargo test".to_string(),
                input: None,
                available_decisions: vec![],
                persistent_label: None,
                matcher_summary: None,
            },
        )
        .unwrap();
    store
        .mark_approval_applying("local-1", "retryable", "allow")
        .unwrap();
    store
        .fail_approval("local-1", "retryable", "第一次恢复失败")
        .unwrap();

    store
        .mark_approval_applying("local-1", "retryable", "deny")
        .expect("failed 状态必须允许用户显式重试");

    let approval = &store.get_session("local-1").unwrap().approvals[0];
    assert_eq!(approval.status, "applying");
    assert_eq!(approval.decision.as_deref(), Some("deny"));
    assert_eq!(approval.error, None);
    assert_eq!(approval.resolved_at, None);
}

#[test]
fn permission_resolution_requires_the_decision_persisted_while_applying() {
    let path = temp_history_path("permission-decision-mismatch");
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
                id: "decision-check".to_string(),
                action: "Bash".to_string(),
                detail: "cargo test".to_string(),
                input: None,
                available_decisions: vec![],
                persistent_label: None,
                matcher_summary: None,
            },
        )
        .unwrap();
    store
        .mark_approval_applying("local-1", "decision-check", "allow_session")
        .unwrap();
    let before = store.get_session("local-1").unwrap().approvals[0].clone();

    let error = store
        .resolve_approval_with_decision("local-1", "decision-check", "deny", Some("rule-overwrite"))
        .unwrap_err();
    assert!(error.contains("决定不一致"));
    assert_eq!(store.get_session("local-1").unwrap().approvals[0], before);

    store
        .resolve_approval_with_decision(
            "local-1",
            "decision-check",
            "allow_session",
            Some("rule-session"),
        )
        .unwrap();
    let resolved = &store.get_session("local-1").unwrap().approvals[0];
    assert_eq!(resolved.status, "resolved");
    assert_eq!(resolved.decision.as_deref(), Some("allow_session"));
    assert_eq!(resolved.rule_id.as_deref(), Some("rule-session"));
}

#[test]
fn permission_approval_replay_via_cli_session_preserves_resolved_ledger_row() {
    let path = temp_history_path("permission-cli-replay");
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
        .record_event(&AgentEvent::SessionStarted {
            session_id: "cli-1".to_string(),
            engine: EngineId::ClaudeCode,
            model: "claude-sonnet-4.6".to_string(),
            cwd: r"D:\work\demo".to_string(),
            ts: 1_717_171_701_000,
            capabilities: None,
        })
        .unwrap();
    store
        .record_event(&AgentEvent::ApprovalRequest {
            session_id: "cli-1".to_string(),
            id: "cli-replay".to_string(),
            action: "Write".to_string(),
            detail: "original.txt".to_string(),
            input: None,
            available_decisions: vec![],
            persistent_label: None,
            matcher_summary: None,
        })
        .unwrap();
    store
        .mark_approval_applying("local-1", "cli-replay", "allow_once")
        .unwrap();
    store
        .resolve_approval_with_decision("local-1", "cli-replay", "allow_once", None)
        .unwrap();
    let before = store.get_session("local-1").unwrap().approvals[0].clone();

    store
        .record_event(&AgentEvent::ApprovalRequest {
            session_id: "cli-1".to_string(),
            id: "cli-replay".to_string(),
            action: "Bash".to_string(),
            detail: "overwritten".to_string(),
            input: None,
            available_decisions: vec![],
            persistent_label: None,
            matcher_summary: None,
        })
        .unwrap();
    assert_eq!(store.get_session("local-1").unwrap().approvals[0], before);
}

#[test]
fn permission_approval_transitions_reject_illegal_source_states_and_preserve_terminals() {
    let path = temp_history_path("permission-illegal-transitions");
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
    for id in ["pending", "resolved", "expired"] {
        store
            .record_event_for_session(
                "local-1",
                &AgentEvent::ApprovalRequest {
                    session_id: "cli-1".to_string(),
                    id: id.to_string(),
                    action: "Write".to_string(),
                    detail: format!("{id}.txt"),
                    input: None,
                    available_decisions: vec![],
                    persistent_label: None,
                    matcher_summary: None,
                },
            )
            .unwrap();
    }

    let pending_resolve_error = store
        .resolve_approval_with_decision("local-1", "pending", "allow_once", None)
        .unwrap_err();
    assert!(pending_resolve_error.contains("状态不允许"));
    let pending_fail_error = store
        .fail_approval("local-1", "pending", "不应失败")
        .unwrap_err();
    assert!(pending_fail_error.contains("状态不允许"));

    store
        .mark_approval_applying("local-1", "resolved", "allow_session")
        .unwrap();
    let repeated_applying_error = store
        .mark_approval_applying("local-1", "resolved", "deny")
        .unwrap_err();
    assert!(repeated_applying_error.contains("状态不允许"));
    store
        .resolve_approval_with_decision(
            "local-1",
            "resolved",
            "allow_session",
            Some("rule-original"),
        )
        .unwrap();

    store.expire_pending_approvals("local-1").unwrap();

    store
        .record_event_for_session(
            "local-1",
            &AgentEvent::ApprovalRequest {
                session_id: "cli-1".to_string(),
                id: "failed".to_string(),
                action: "Write".to_string(),
                detail: "failed.txt".to_string(),
                input: None,
                available_decisions: vec![],
                persistent_label: None,
                matcher_summary: None,
            },
        )
        .unwrap();
    store
        .mark_approval_applying("local-1", "failed", "allow_once")
        .unwrap();
    store
        .fail_approval("local-1", "failed", "原始错误")
        .unwrap();

    for terminal_id in ["resolved", "expired"] {
        let before = store
            .get_session("local-1")
            .unwrap()
            .approvals
            .into_iter()
            .find(|approval| approval.id == terminal_id)
            .unwrap();
        for error in [
            store
                .mark_approval_applying("local-1", terminal_id, "deny")
                .unwrap_err(),
            store
                .resolve_approval_with_decision(
                    "local-1",
                    terminal_id,
                    "deny",
                    Some("rule-overwrite"),
                )
                .unwrap_err(),
            store
                .fail_approval("local-1", terminal_id, "覆盖错误")
                .unwrap_err(),
        ] {
            assert!(error.contains("状态不允许"), "{terminal_id}: {error}");
        }
        let after = store
            .get_session("local-1")
            .unwrap()
            .approvals
            .into_iter()
            .find(|approval| approval.id == terminal_id)
            .unwrap();
        assert_eq!(after, before, "终态 {terminal_id} 不得被非法转换改写");
    }

    store
        .mark_approval_applying("local-1", "failed", "deny")
        .expect("用户显式重试必须能把 failed 重新带入 applying");
    let retried = store
        .get_session("local-1")
        .unwrap()
        .approvals
        .into_iter()
        .find(|approval| approval.id == "failed")
        .unwrap();
    assert_eq!(retried.status, "applying");
    assert_eq!(retried.decision.as_deref(), Some("deny"));
    assert_eq!(retried.error, None);
    assert_eq!(retried.resolved_at, None);
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
                available_decisions: vec![],
                persistent_label: None,
                matcher_summary: None,
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
                available_decisions: vec![],
                persistent_label: None,
                matcher_summary: None,
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
                available_decisions: vec![],
                persistent_label: None,
                matcher_summary: None,
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
fn usage_breakdown_provider_attributes_costs_by_real_provider_id() {
    // P3-6：用量按 session.provider_id 真实归属，不再按模型名推断；
    // 未标注的旧会话归入空 key。S4 起统一走 get_usage_breakdown(days, dimension)。
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
                    capabilities: None,
                },
            )
            .unwrap();
        store
            .record_event_for_session(
                session,
                &AgentEvent::TokenUsage {
                    session_id: cli.to_string(),
                    input_tokens: 100,
                    cached_input_tokens: Some(40),
                    cache_write_input_tokens: Some(10),
                    output_tokens: 10,
                    cost_usd: cost,
                    service_tier: None,
                    context_window: None,
                },
            )
            .unwrap();
    }

    let rows = store
        .get_usage_breakdown(30, UsageBreakdownDimension::Provider)
        .unwrap();
    assert_eq!(rows.len(), 3, "两个真实服务商 + 一个未标注：{rows:?}");
    assert_eq!(rows[0].key, "gateway-x");
    assert_eq!(rows[0].engine, "claude-code");
    assert_eq!(rows[0].request_count, 1);
    assert_eq!(rows[0].input_tokens, Some(100));
    assert_eq!(rows[0].cached_input_tokens, Some(40));
    assert_eq!(rows[0].cache_write_input_tokens, Some(10));
    assert_eq!(rows[0].output_tokens, Some(10));
    assert!((rows[0].cost_usd - 3.0).abs() < 1e-9);
    assert!((rows[0].share - 0.6).abs() < 1e-9);
    assert_eq!(rows[0].cost_kinds.actual, 1);
    assert!(
        rows.iter().any(|row| row.key.is_empty()),
        "未标注会话归入空 key"
    );
}

#[test]
fn usage_window_rejects_days_outside_frozen_ranges() {
    // S4 冻结：用量查询只接受 7/30/90/365 天，其余 fail-closed。
    let path = temp_history_path("usage-window-guard");
    let store = SessionHistoryStore::new(path);
    for days in [0u32, 1, 8, 364, 366] {
        assert!(store.get_daily_usage(days).is_err(), "days={days} 应被拒绝");
        assert!(store.get_usage_stats(days).is_err(), "days={days} 应被拒绝");
        assert!(
            store
                .get_usage_breakdown(days, UsageBreakdownDimension::Engine)
                .is_err(),
            "days={days} 应被拒绝"
        );
    }
    for days in [7u32, 30, 90, 365] {
        assert!(store.get_daily_usage(days).is_ok(), "days={days} 应合法");
    }
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

#[test]
fn auto_title_respects_manual_rename_before_first_turn_ends() {
    // 变更-12 承诺：手动改名后不再被自动起标题覆盖。首轮结束前（summary 仍为 NULL）
    // 手动改名，守卫必须返回 false，否则自动标题会覆盖用户的手动标题。
    let path = temp_history_path("auto-title-manual-rename");
    let store = SessionHistoryStore::new(path);
    store
        .create_session(NewSessionRecord {
            id: "local-rename".to_string(),
            engine: EngineId::ClaudeCode,
            model: "claude-sonnet-4.6".to_string(),
            cwd: r"D:\work\demo".to_string(),
            created_at: 1_717_171_700,
        })
        .unwrap();
    store
        .record_user_message("local-rename", "帮我修登录超时", 1_717_171_701)
        .unwrap();
    store
        .record_event_for_session(
            "local-rename",
            &AgentEvent::MessageComplete {
                session_id: "cli-1".to_string(),
                role: Role::Assistant,
                text: "已定位到超时原因并修复".to_string(),
            },
        )
        .unwrap();
    // 默认标题待起标题
    assert!(store.session_needs_auto_title("local-rename").unwrap());
    // 手动改名后不再需要自动标题
    store
        .rename_session("local-rename", "我的自定义标题")
        .unwrap();
    assert!(!store.session_needs_auto_title("local-rename").unwrap());
    // 标题保持手动值，不被覆盖
    let sessions = store.list_sessions().unwrap();
    assert_eq!(sessions[0].title, "我的自定义标题");
}

fn pricing_profile(source: &str, bands: Vec<PricingBand>) -> ResolvedPricingProfile {
    ResolvedPricingProfile {
        catalog_version: "test-v1".to_string(),
        source: source.to_string(),
        currency: "USD".to_string(),
        source_url: "https://example.test/pricing".to_string(),
        observed_at: "2026-07-17T00:00:00Z".to_string(),
        tiers: HashMap::from([(ServiceTier::Standard, PricingTier { bands })]),
    }
}

#[test]
fn usage_cost_uses_cached_write_and_long_context_rates() {
    let path = temp_history_path("pricing-cache-long-context");
    let store = SessionHistoryStore::new(path);
    store
        .create_session(NewSessionRecord {
            id: "local-pricing".to_string(),
            engine: EngineId::Codex,
            model: "gpt-5.6-sol".to_string(),
            cwd: r"D:\work\pricing".to_string(),
            created_at: 1_717_171_700,
        })
        .unwrap();
    store
        .set_session_provider("local-pricing", "provider-a")
        .unwrap();
    store.set_model_pricing_profile(
        "provider-a",
        "gpt-5.6-sol",
        pricing_profile(
            "official-reference",
            vec![
                PricingBand {
                    min_input_tokens: None,
                    max_input_tokens: Some(272_000),
                    input: 5.0,
                    cached_input: Some(0.5),
                    cache_write: Some(6.25),
                    output: 30.0,
                },
                PricingBand {
                    min_input_tokens: Some(272_001),
                    max_input_tokens: None,
                    input: 10.0,
                    cached_input: Some(1.0),
                    cache_write: Some(12.5),
                    output: 45.0,
                },
            ],
        ),
    );
    store
        .record_event_for_session(
            "local-pricing",
            &AgentEvent::TokenUsage {
                session_id: "cli-pricing".to_string(),
                input_tokens: 1_000_000,
                cached_input_tokens: Some(800_000),
                cache_write_input_tokens: Some(100_000),
                output_tokens: 100_000,
                cost_usd: 0.0,
                service_tier: Some("standard".to_string()),
                context_window: Some(1_050_000),
            },
        )
        .unwrap();

    let session = store.get_session("local-pricing").unwrap();
    // 长上下文档：0.1M*10 + 0.8M*1 + 0.1M*12.5 + 0.1M*45 = 7.55
    assert!((session.summary.cost_usd - 7.55).abs() < 1e-9);
    let stats = store.get_usage_stats(30).unwrap();
    assert_eq!(stats.unknown_count, 0);
    assert!((stats.estimated_cost - 7.55).abs() < 1e-9);
}

#[test]
fn usage_pricing_isolated_by_provider_and_history_does_not_reprice() {
    let path = temp_history_path("pricing-provider-isolation");
    let store = SessionHistoryStore::new(path);
    for (session_id, provider_id) in [("session-a", "provider-a"), ("session-b", "provider-b")] {
        store
            .create_session(NewSessionRecord {
                id: session_id.to_string(),
                engine: EngineId::Codex,
                model: "same-model".to_string(),
                cwd: r"D:\work\pricing".to_string(),
                created_at: 1_717_171_700,
            })
            .unwrap();
        store.set_session_provider(session_id, provider_id).unwrap();
    }
    for (provider_id, input_price) in [("provider-a", 1.0), ("provider-b", 9.0)] {
        store.set_model_pricing_profile(
            provider_id,
            "same-model",
            pricing_profile(
                "manual",
                vec![PricingBand {
                    min_input_tokens: None,
                    max_input_tokens: None,
                    input: input_price,
                    cached_input: None,
                    cache_write: None,
                    output: 0.0,
                }],
            ),
        );
    }
    for session_id in ["session-a", "session-b"] {
        store
            .record_event_for_session(
                session_id,
                &AgentEvent::TokenUsage {
                    session_id: format!("cli-{session_id}"),
                    input_tokens: 1_000_000,
                    cached_input_tokens: None,
                    cache_write_input_tokens: None,
                    output_tokens: 0,
                    cost_usd: 0.0,
                    service_tier: None,
                    context_window: None,
                },
            )
            .unwrap();
    }
    assert!((store.get_session("session-a").unwrap().summary.cost_usd - 1.0).abs() < 1e-9);
    assert!((store.get_session("session-b").unwrap().summary.cost_usd - 9.0).abs() < 1e-9);

    store.set_model_pricing_profile(
        "provider-a",
        "same-model",
        pricing_profile(
            "manual",
            vec![PricingBand {
                min_input_tokens: None,
                max_input_tokens: None,
                input: 100.0,
                cached_input: None,
                cache_write: None,
                output: 0.0,
            }],
        ),
    );
    assert!((store.get_session("session-a").unwrap().summary.cost_usd - 1.0).abs() < 1e-9);
}

#[test]
fn usage_stats_separates_actual_estimated_subscription_and_unknown_costs() {
    let path = temp_history_path("pricing-cost-kinds");
    let store = SessionHistoryStore::new(path);
    for (session_id, provider_id, model_id) in [
        ("actual", "provider-actual", "model-actual"),
        ("estimated", "provider-estimated", "model-estimated"),
        (
            "subscription",
            "provider-subscription",
            "model-subscription",
        ),
        ("unknown", "provider-unknown", "model-unknown"),
    ] {
        store
            .create_session(NewSessionRecord {
                id: session_id.to_string(),
                engine: EngineId::Codex,
                model: model_id.to_string(),
                cwd: r"D:\work\pricing".to_string(),
                created_at: 1_717_171_700,
            })
            .unwrap();
        store.set_session_provider(session_id, provider_id).unwrap();
    }
    let simple_band = |input| PricingBand {
        min_input_tokens: None,
        max_input_tokens: None,
        input,
        cached_input: None,
        cache_write: None,
        output: 0.0,
    };
    store.set_model_pricing_profile(
        "provider-estimated",
        "model-estimated",
        pricing_profile("official-reference", vec![simple_band(2.0)]),
    );
    store.set_model_pricing_profile(
        "provider-subscription",
        "model-subscription",
        pricing_profile("subscription", vec![simple_band(0.0)]),
    );

    for (session_id, reported_cost) in [
        ("actual", 0.4),
        ("estimated", 0.0),
        ("subscription", 0.0),
        ("unknown", 0.0),
    ] {
        store
            .record_event_for_session(
                session_id,
                &AgentEvent::TokenUsage {
                    session_id: format!("cli-{session_id}"),
                    input_tokens: 1_000_000,
                    cached_input_tokens: None,
                    cache_write_input_tokens: None,
                    output_tokens: 0,
                    cost_usd: reported_cost,
                    service_tier: None,
                    context_window: None,
                },
            )
            .unwrap();
    }

    let stats = store.get_usage_stats(30).unwrap();
    assert!((stats.total_cost - 2.4).abs() < 1e-9);
    assert!((stats.actual_cost - 0.4).abs() < 1e-9);
    assert!((stats.estimated_cost - 2.0).abs() < 1e-9);
    assert_eq!(stats.subscription_count, 1);
    assert_eq!(stats.unknown_count, 1);
    assert_eq!(stats.legacy_count, 0);
}

#[test]
fn usage_stats_queries_the_immediately_previous_equal_length_period() {
    let path = temp_history_path("usage-period-comparison");
    let store = SessionHistoryStore::new(path.clone());
    for session_id in ["current", "previous"] {
        store
            .create_session(NewSessionRecord {
                id: session_id.to_string(),
                engine: EngineId::Codex,
                model: "gpt-test".to_string(),
                cwd: r"D:\work\usage".to_string(),
                created_at: 1_717_171_700,
            })
            .unwrap();
        store
            .record_event_for_session(
                session_id,
                &AgentEvent::TokenUsage {
                    session_id: format!("cli-{session_id}"),
                    input_tokens: if session_id == "current" { 200 } else { 100 },
                    cached_input_tokens: None,
                    cache_write_input_tokens: None,
                    output_tokens: 10,
                    cost_usd: if session_id == "current" { 4.0 } else { 2.0 },
                    service_tier: None,
                    context_window: None,
                },
            )
            .unwrap();
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute(
            "UPDATE usage SET ts = ?1 WHERE session_id = 'previous'",
            rusqlite::params![now - 8 * 86_400],
        )
        .unwrap();

    let stats = store.get_usage_stats(7).unwrap();
    assert!((stats.total_cost - 4.0).abs() < 1e-9);
    assert_eq!(stats.total_tokens, 210);
    assert_eq!(stats.request_count, 1);
    assert_eq!(stats.session_count, 1);
    assert!((stats.previous_total_cost - 2.0).abs() < 1e-9);
    assert_eq!(stats.previous_total_tokens, 110);
    assert_eq!(stats.previous_request_count, 1);
    assert_eq!(stats.previous_session_count, 1);
}

#[test]
fn schema_v20_persists_context_usage_token_split_and_activity_turn_links() {
    let path = temp_history_path("schema-v20-context-activity");
    let store = SessionHistoryStore::new(path);
    store
        .create_session(NewSessionRecord {
            id: "session-v20".to_string(),
            engine: EngineId::ClaudeCode,
            model: "claude-sonnet-4.6".to_string(),
            cwd: r"D:\work\v20".to_string(),
            created_at: 1_717_171_700,
        })
        .unwrap();
    let spec = start_change_27d_turn(&store, "session-v20", "测试 v20 归属", Vec::new());
    let turn_id = Some(spec.turn_id.as_str());
    store
        .record_event_for_session_in_turn(
            "session-v20",
            turn_id,
            &AgentEvent::ContextUsage {
                session_id: "cli-v20".to_string(),
                context_tokens: 80,
                context_window: Some(200),
            },
        )
        .unwrap();
    store
        .record_event_for_session_in_turn(
            "session-v20",
            turn_id,
            &AgentEvent::TokenUsage {
                session_id: "cli-v20".to_string(),
                input_tokens: 100,
                cached_input_tokens: Some(70),
                cache_write_input_tokens: Some(20),
                output_tokens: 10,
                cost_usd: 0.01,
                service_tier: None,
                context_window: Some(200),
            },
        )
        .unwrap();
    store
        .record_event_for_session_in_turn(
            "session-v20",
            turn_id,
            &AgentEvent::ToolCall {
                session_id: "cli-v20".to_string(),
                id: "tool-v20".to_string(),
                name: "Read".to_string(),
                input: serde_json::json!({"path":"README.md"}),
                status: CallStatus::Pending,
            },
        )
        .unwrap();
    store
        .record_event_for_session_in_turn(
            "session-v20",
            turn_id,
            &AgentEvent::ToolResult {
                session_id: "cli-v20".to_string(),
                id: "tool-v20".to_string(),
                status: ToolStatus::Success,
                output: Some("ok".to_string()),
                diff: None,
                outcome: None,
                started: None,
                has_output: None,
                retryable: None,
                denial_source: None,
                native_denial_code: None,
            },
        )
        .unwrap();
    store
        .record_event_for_session_in_turn(
            "session-v20",
            turn_id,
            &AgentEvent::MessageComplete {
                session_id: "cli-v20".to_string(),
                role: Role::Assistant,
                text: "完成".to_string(),
            },
        )
        .unwrap();
    store
        .record_event_for_session_in_turn(
            "session-v20",
            turn_id,
            &AgentEvent::Checkpoint {
                session_id: "cli-v20".to_string(),
                id: "checkpoint-v20".to_string(),
                label: "读取前".to_string(),
                ts: 1_717_171_700_000,
                restorable: false,
                file_count: 0,
                reason: Some("legacy_empty_snapshot".to_string()),
            },
        )
        .unwrap();

    let detail = store.get_session("session-v20").unwrap();
    assert_eq!(detail.summary.cached_input_tokens, 70);
    assert_eq!(detail.summary.cache_write_input_tokens, 20);
    assert_eq!(detail.summary.last_context_tokens, Some(80));
    assert_eq!(detail.summary.last_context_window, Some(200));
    assert_eq!(detail.messages[0].turn_id.as_deref(), turn_id);
    assert_eq!(detail.tool_calls[0].turn_id.as_deref(), turn_id);
    assert!(detail.tool_calls[0].ended_at.is_some());
    assert_eq!(detail.checkpoints[0].turn_id.as_deref(), turn_id);
}

#[test]
fn schema_v19_migrates_to_v21_without_losing_sessions() {
    let path = temp_history_path("schema-v19-to-v20");
    let store = SessionHistoryStore::new(path.clone());
    store
        .create_session(NewSessionRecord {
            id: "legacy-v19".to_string(),
            engine: EngineId::ClaudeCode,
            model: "claude-sonnet-4.6".to_string(),
            cwd: r"D:\work\legacy-v19".to_string(),
            created_at: 1_717_171_700,
        })
        .unwrap();
    drop(store);

    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch(
        "DROP INDEX IF EXISTS idx_tool_native_turn;
         DROP INDEX IF EXISTS idx_tool_call_turn;
         DROP INDEX IF EXISTS idx_approval_turn;
         DROP INDEX IF EXISTS idx_checkpoint_turn;
         ALTER TABLE session DROP COLUMN last_context_tokens;
         ALTER TABLE session DROP COLUMN last_context_window;
         ALTER TABLE tool_call DROP COLUMN ended_at;
         ALTER TABLE tool_call DROP COLUMN turn_id;
         ALTER TABLE approval DROP COLUMN turn_id;
         ALTER TABLE checkpoint DROP COLUMN turn_id;
         PRAGMA user_version = 19;",
    )
    .unwrap();
    drop(conn);

    let migrated = SessionHistoryStore::new(path.clone());
    assert_eq!(migrated.list_sessions().unwrap()[0].id, "legacy-v19");
    let conn = rusqlite::Connection::open(path).unwrap();
    assert_eq!(
        conn.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        SCHEMA_VERSION
    );
    for (table, column) in [
        ("session", "last_context_tokens"),
        ("session", "last_context_window"),
        ("tool_call", "ended_at"),
        ("tool_call", "turn_id"),
        ("approval", "turn_id"),
        ("checkpoint", "turn_id"),
    ] {
        let exists: i64 = conn
            .query_row(
                &format!("SELECT COUNT(*) FROM pragma_table_info('{table}') WHERE name = ?1"),
                [column],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(exists, 1, "missing {table}.{column}");
    }
}

#[test]
fn schema_v20_migrates_project_folder_columns_without_reclassifying_history() {
    let path = temp_history_path("schema-v20-to-v21");
    let store = SessionHistoryStore::new(path.clone());
    store
        .create_session(NewSessionRecord {
            id: "legacy-v20".to_string(),
            engine: EngineId::ClaudeCode,
            model: "claude-test".to_string(),
            cwd: r"D:\work\legacy-v20".to_string(),
            created_at: 1_717_171_700,
        })
        .unwrap();
    drop(store);

    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch(
        "DROP INDEX idx_session_folder_cwd_key;
         ALTER TABLE session_folder DROP COLUMN cwd;
         ALTER TABLE session_folder DROP COLUMN cwd_key;
         PRAGMA user_version = 20;",
    )
    .unwrap();
    drop(conn);

    let migrated = SessionHistoryStore::new(path.clone());
    assert_eq!(
        migrated
            .get_session("legacy-v20")
            .unwrap()
            .summary
            .folder_id,
        "folder-default"
    );
    assert!(migrated
        .list_folders()
        .unwrap()
        .iter()
        .all(|folder| folder.cwd.is_none()));
    let conn = rusqlite::Connection::open(path).unwrap();
    assert_eq!(
        conn.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        SCHEMA_VERSION
    );
    for column in ["cwd", "cwd_key"] {
        let exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('session_folder') WHERE name = ?1",
                [column],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(exists, 1, "missing session_folder.{column}");
    }
}

fn seed_change_27a_fixture(name: &str, sql: &str) -> std::path::PathBuf {
    let path = temp_history_path(name);
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute_batch(sql)
        .unwrap();
    path
}

fn assert_change_27a_fixture_health(path: &std::path::Path) {
    let conn = rusqlite::Connection::open(path).unwrap();
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, SCHEMA_VERSION);

    let mut statement = conn.prepare("PRAGMA foreign_key_check").unwrap();
    let mut rows = statement.query([]).unwrap();
    assert!(
        rows.next().unwrap().is_none(),
        "fixture must not contain foreign key violations"
    );
    let non_legacy_turns: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM turn WHERE identity_source <> 'legacy'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        non_legacy_turns, 0,
        "migrated Turn identity must not be guessed"
    );
    let inferred_specs: i64 = conn
        .query_row("SELECT COUNT(*) FROM turn_execution_spec", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(inferred_specs, 0, "legacy Turn specs must not be inferred");
}

#[test]
fn change_27l_fresh_install_and_v21_upgrade_reopen_at_v30() {
    let fresh_path = temp_history_path("change-27l-fresh-v30");
    for _ in 0..2 {
        SessionHistoryStore::new(fresh_path.clone())
            .list_sessions()
            .unwrap();
    }
    let fresh = rusqlite::Connection::open(&fresh_path).unwrap();
    assert_eq!(
        fresh
            .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        SCHEMA_VERSION
    );
    drop(fresh);

    let upgraded_path = seed_change_27a_fixture(
        "change-27l-v21-v30",
        include_str!("fixtures/change-27a/v21-fresh.sql"),
    );
    for _ in 0..2 {
        let store = SessionHistoryStore::new(upgraded_path.clone());
        let detail = store.get_session("session-v21").unwrap();
        assert_eq!(detail.messages.len(), 2);
        assert_eq!(detail.summary.model, "gpt-fixture");
    }
    assert_change_27a_fixture_health(&upgraded_path);
    let upgraded = rusqlite::Connection::open(&upgraded_path).unwrap();
    for table in [
        "turn_execution_spec",
        "turn_attempt",
        "session_context",
        "capability_snapshot",
        "background_operation",
        "handoff",
        "session_fork",
    ] {
        let exists: i64 = upgraded
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
                [table],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(exists, 1, "v21→v30 缺少表 {table}");
    }
}

#[test]
fn change_27l_v21_upgrade_waits_for_temporary_write_lock() {
    let path = seed_change_27a_fixture(
        "change-27l-v21-lock",
        include_str!("fixtures/change-27a/v21-fresh.sql"),
    );
    let blocker = rusqlite::Connection::open(&path).unwrap();
    blocker.execute_batch("BEGIN EXCLUSIVE").unwrap();

    let upgrade_path = path.clone();
    let handle = std::thread::spawn(move || SessionHistoryStore::new(upgrade_path).list_sessions());
    std::thread::sleep(Duration::from_millis(200));
    blocker.execute_batch("COMMIT").unwrap();

    assert_eq!(handle.join().unwrap().unwrap().len(), 1);
    assert_change_27a_fixture_health(&path);
}

#[test]
fn change_27l_failed_v21_upgrade_rolls_back_schema_and_version() {
    let path = seed_change_27a_fixture(
        "change-27l-v21-interrupted",
        include_str!("fixtures/change-27a/v21-fresh.sql"),
    );
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch(
        "CREATE TABLE checkpoint (
           id TEXT PRIMARY KEY,
           session_id TEXT NOT NULL REFERENCES session(id) ON DELETE CASCADE,
           turn_idx INTEGER NOT NULL,
           label TEXT NOT NULL,
           snapshot_ref TEXT NOT NULL,
           ts INTEGER NOT NULL,
           turn_id TEXT
         );
         INSERT INTO checkpoint
           (id, session_id, turn_idx, label, snapshot_ref, ts, turn_id)
         VALUES ('checkpoint-legacy', 'session-v21', 1, 'legacy', 'snapshot', 1, 'turn-v21-1');
         CREATE TRIGGER inject_v30_migration_failure
         BEFORE UPDATE ON checkpoint
         BEGIN
           SELECT RAISE(ABORT, 'injected migration failure');
         END;",
    )
    .unwrap();
    drop(conn);

    let error = SessionHistoryStore::new(path.clone())
        .list_sessions()
        .unwrap_err();
    assert!(error.contains("injected migration failure"), "{error}");

    let conn = rusqlite::Connection::open(path).unwrap();
    assert_eq!(
        conn.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        21,
        "失败迁移不得提前升级 user_version"
    );
    let v30_columns: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('checkpoint')
             WHERE name IN ('restorable', 'file_count', 'restorable_reason')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(v30_columns, 0, "失败迁移必须回滚已执行的 DDL");
}

#[test]
fn change_27a_v21_fixture_reopens_idempotently() {
    let path = seed_change_27a_fixture(
        "change-27a-v21-fresh",
        include_str!("fixtures/change-27a/v21-fresh.sql"),
    );

    for _ in 0..2 {
        let store = SessionHistoryStore::new(path.clone());
        let detail = store.get_session("session-v21").unwrap();
        assert_eq!(detail.summary.model, "gpt-fixture");
        assert_eq!(detail.messages.len(), 2);
    }
    assert_change_27a_fixture_health(&path);
}

#[test]
fn change_27a_v19_fixture_migrates_to_v21_and_reopens() {
    let path = seed_change_27a_fixture(
        "change-27a-v19-upgrade",
        include_str!("fixtures/change-27a/v19-sequential-upgrade.sql"),
    );

    for _ in 0..2 {
        let store = SessionHistoryStore::new(path.clone());
        assert_eq!(
            store.get_session("session-v19").unwrap().summary.model,
            "claude-fixture"
        );
    }
    assert_change_27a_fixture_health(&path);
}

#[test]
fn change_27a_legacy_fixture_preserves_missing_attribution() {
    let path = seed_change_27a_fixture(
        "change-27a-legacy-attribution",
        include_str!("fixtures/change-27a/legacy-missing-attribution.sql"),
    );

    for _ in 0..2 {
        let store = SessionHistoryStore::new(path.clone());
        assert_eq!(
            store.get_session("session-legacy").unwrap().messages.len(),
            2
        );
    }
    assert_change_27a_fixture_health(&path);

    let conn = rusqlite::Connection::open(path).unwrap();
    let session_attribution: (String, String) = conn
        .query_row(
            "SELECT provider_id, model FROM session WHERE id = 'session-legacy'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(session_attribution, (String::new(), String::new()));

    let usage_attribution: (String, String) = conn
        .query_row(
            "SELECT provider_id, model FROM usage WHERE session_id = 'session-legacy'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(usage_attribution, (String::new(), String::new()));

    for table in ["message", "tool_call", "checkpoint", "usage"] {
        let populated: i64 = conn
            .query_row(
                &format!("SELECT COUNT(*) FROM {table} WHERE turn_id IS NOT NULL"),
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(populated, 0, "{table}.turn_id must not be guessed");
    }
}
