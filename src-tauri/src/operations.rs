use crate::budget::TurnBudgetSnapshot;
use crate::capability_registry::{CapabilitySupport, EngineCapabilitySnapshot};
use crate::reasoning::ReasoningEffort;
use crate::runtime_registry::RuntimeOwnerRef;
use crate::turn_start::{PricingBasisSnapshot, RuntimeRoute};
use serde::{Deserialize, Serialize};

pub const MODEL_ONLY_POLICY_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct OperationExecutionSpec {
    pub operation_id: String,
    pub owner: RuntimeOwnerRef,
    pub purpose: String,
    pub engine_id: String,
    pub provider_id: String,
    pub provider_kind: String,
    pub provider_display_name: String,
    pub route_label_snapshot: String,
    pub requested_model_id: String,
    pub routed_model_id: String,
    pub model_label_snapshot: String,
    pub requested_reasoning_effort: ReasoningEffort,
    pub routed_reasoning_effort: ReasoningEffort,
    pub binding_id: String,
    pub binding_revision: u64,
    pub engine_profile_digest: String,
    pub provider_launch_profile_ref: String,
    pub provider_launch_profile_digest: String,
    pub launch_config_digest: String,
    pub routing_capability_snapshot_id: String,
    pub pricing_basis_snapshot: PricingBasisSnapshot,
    pub created_at: i64,
}

impl OperationExecutionSpec {
    pub fn from_binding_route(
        operation_id: String,
        purpose: impl Into<String>,
        binding_id: impl Into<String>,
        binding_revision: u64,
        route: &RuntimeRoute,
        capability: &EngineCapabilitySnapshot,
        requested_reasoning_effort: ReasoningEffort,
        routed_reasoning_effort: ReasoningEffort,
        created_at: i64,
    ) -> Result<Self, String> {
        if capability.identity.engine_id != route.engine_id
            || capability.identity.engine_profile_digest != route.engine_profile_digest
            || capability.identity.provider_launch_profile_digest
                != route.provider_launch_profile_digest
            || capability.identity.model_capability_key != route.model_id
        {
            return Err("Operation capability snapshot 与冻结路由不匹配".to_string());
        }
        let owner = RuntimeOwnerRef::Operation(operation_id.clone());
        Ok(Self {
            operation_id,
            owner,
            purpose: purpose.into(),
            engine_id: route.engine_id.clone(),
            provider_id: route.provider_id.clone(),
            provider_kind: route.provider_kind.clone(),
            provider_display_name: route.provider_display_name.clone(),
            route_label_snapshot: route.route_label_snapshot.clone(),
            requested_model_id: route.model_id.clone(),
            routed_model_id: route.model_id.clone(),
            model_label_snapshot: route.model_label_snapshot.clone(),
            requested_reasoning_effort,
            routed_reasoning_effort,
            binding_id: binding_id.into(),
            binding_revision,
            engine_profile_digest: route.engine_profile_digest.clone(),
            provider_launch_profile_ref: route.provider_launch_profile_ref.clone(),
            provider_launch_profile_digest: route.provider_launch_profile_digest.clone(),
            launch_config_digest: route.launch_config_digest.clone(),
            routing_capability_snapshot_id: capability.id.clone(),
            pricing_basis_snapshot: route.pricing_basis_snapshot.clone(),
            created_at,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ModelOnlyOperationPolicy {
    pub contract_version: u32,
    pub canonical_cwd: String,
    pub sandbox_mode: String,
    pub tools_disabled: bool,
    pub extensions_disabled: bool,
    pub persistent_grants_disabled: bool,
    pub capability_snapshot_id: String,
    pub launch_evidence: String,
    pub created_at: i64,
}

impl ModelOnlyOperationPolicy {
    pub fn freeze_from_capability(capability: &EngineCapabilitySnapshot, created_at: i64) -> Self {
        let evidence = &capability.capabilities.model_only_operation;
        Self {
            contract_version: MODEL_ONLY_POLICY_VERSION,
            canonical_cwd: String::new(),
            sandbox_mode: "read_only".to_string(),
            tools_disabled: true,
            extensions_disabled: true,
            persistent_grants_disabled: true,
            capability_snapshot_id: capability.id.clone(),
            launch_evidence: format!(
                "{:?}:{}:{}",
                evidence.support, evidence.source, evidence.diagnostic
            ),
            created_at,
        }
    }

    pub fn from_capability(
        capability: &EngineCapabilitySnapshot,
        created_at: i64,
    ) -> Result<Self, String> {
        let evidence = &capability.capabilities.model_only_operation;
        if evidence.support != CapabilitySupport::Supported {
            return Err(format!(
                "[operation_tools_not_disableable] {}: {}",
                evidence.source, evidence.diagnostic
            ));
        }
        Ok(Self::freeze_from_capability(capability, created_at))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BackgroundOperation {
    pub id: String,
    pub kind: String,
    pub source_session_id: Option<String>,
    pub input_digest: String,
    #[serde(default, skip_serializing)]
    pub input: Option<serde_json::Value>,
    pub idempotency_key: String,
    pub status: String,
    pub result: Option<serde_json::Value>,
    pub error_code: Option<String>,
    pub created_at: i64,
    pub started_at: Option<i64>,
    pub cancel_requested_at: Option<i64>,
    pub ended_at: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct NewBackgroundOperation {
    pub operation: BackgroundOperation,
    pub spec: OperationExecutionSpec,
    pub policy: ModelOnlyOperationPolicy,
    pub budget: TurnBudgetSnapshot,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ModelOnlyOperationOutput {
    pub text: String,
    pub input_tokens: u64,
    pub cached_input_tokens: u64,
    pub cache_write_input_tokens: u64,
    pub output_tokens: u64,
    pub reported_cost_usd: Option<f64>,
    pub service_tier: Option<String>,
    pub observed_model_id: Option<String>,
}

impl NewBackgroundOperation {
    pub fn validate(&self) -> Result<(), String> {
        if self.operation.id != self.spec.operation_id
            || self.spec.owner != RuntimeOwnerRef::Operation(self.operation.id.clone())
        {
            return Err("BackgroundOperation row/spec/owner 身份不一致".to_string());
        }
        if self.policy.capability_snapshot_id != self.spec.routing_capability_snapshot_id {
            return Err("Operation spec 与 ModelOnlyOperationPolicy capability 不一致".to_string());
        }
        if !self.policy.tools_disabled
            || !self.policy.extensions_disabled
            || !self.policy.persistent_grants_disabled
            || !self.policy.canonical_cwd.is_empty()
        {
            return Err("ModelOnlyOperationPolicy 不满足无工具隔离契约".to_string());
        }
        self.budget.validate()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability_registry::{CapabilityEvidence, CapabilitySet, CapabilitySupport};

    #[test]
    fn policy_rejects_unknown_or_unsupported_no_tools_contract() {
        let mut snapshot = fixture_snapshot();
        snapshot.capabilities.model_only_operation =
            CapabilityEvidence::new(CapabilitySupport::Unknown, "fixture", "not_proven");
        let error = ModelOnlyOperationPolicy::from_capability(&snapshot, 1).unwrap_err();
        assert!(error.contains("[operation_tools_not_disableable]"));
    }

    fn fixture_snapshot() -> EngineCapabilitySnapshot {
        EngineCapabilitySnapshot {
            id: "capability-fixture".into(),
            identity: crate::capability_registry::CapabilityIdentity {
                engine_id: "claude-code".into(),
                adapter_version: "fixture".into(),
                binary_identity: "fixture".into(),
                engine_profile_digest: "engine".into(),
                provider_launch_profile_ref: "provider".into(),
                provider_launch_profile_digest: "launch".into(),
                launch_profile_identity: "profile".into(),
                model_capability_key: "model".into(),
            },
            capabilities: CapabilitySet::unknown("fixture"),
            probe_kind: "fixture".into(),
            probed_at: 1,
        }
    }
}
