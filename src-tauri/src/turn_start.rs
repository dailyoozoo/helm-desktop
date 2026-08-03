use crate::pricing::ResolvedPricingProfile;
use crate::providers::{AppConfig, BindingConfig, ProviderConfig, ProviderKind};
use crate::reasoning::ReasoningEffort;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const BINDING_LIVE: &str = "binding_live";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PricingBasisSnapshot {
    pub profile: Option<ResolvedPricingProfile>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeRoute {
    pub engine_id: String,
    pub provider_id: String,
    pub provider_kind: String,
    pub provider_display_name: String,
    pub route_label_snapshot: String,
    pub model_id: String,
    pub model_label_snapshot: String,
    pub default_reasoning_effort: ReasoningEffort,
    pub engine_profile_digest: String,
    pub provider_launch_profile_ref: String,
    pub provider_launch_profile_digest: String,
    pub launch_config_digest: String,
    pub pricing_basis_snapshot: PricingBasisSnapshot,
}

#[derive(Debug, Clone)]
pub struct TurnStartCommand {
    pub history_session_id: String,
    pub display_text: String,
    pub turn_mode: String,
    pub permission_profile: String,
    pub requested_reasoning_effort: Option<ReasoningEffort>,
    pub requested_model_id: Option<String>,
    pub attachments: Vec<String>,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FrozenSessionContext {
    pub id: String,
    pub kind: String,
    #[serde(skip_serializing)]
    pub canonical_path: String,
    pub canonical_path_digest: String,
    pub identity_digest: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TurnExecutionSpec {
    pub turn_id: String,
    pub history_session_id: String,
    pub turn_epoch: u64,
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
    pub turn_mode: String,
    pub permission_profile: String,
    pub binding_id: Option<String>,
    pub binding_revision: Option<u64>,
    pub engine_profile_digest: String,
    pub provider_launch_profile_ref: String,
    pub launch_config_digest: String,
    pub routing_capability_snapshot_id: Option<String>,
    pub resolution_source: String,
    pub legacy_route_snapshot_digest: Option<String>,
    pub pricing_basis_snapshot: PricingBasisSnapshot,
    #[serde(default)]
    pub session_context: Vec<FrozenSessionContext>,
    pub created_at: i64,
}

pub struct BindingLiveRouteResolver;

impl BindingLiveRouteResolver {
    pub fn resolve(
        route: &RuntimeRoute,
        binding: &BindingConfig,
        routing_capability_snapshot_id: &str,
        command: &TurnStartCommand,
    ) -> Result<TurnExecutionSpec, String> {
        if route.engine_id.trim().is_empty()
            || route.provider_id.trim().is_empty()
            || route.model_id.trim().is_empty()
        {
            return Err("Binding 路由缺少 Engine、Provider 或 Model".to_string());
        }
        if binding.engine_id != route.engine_id || binding.provider_id != route.provider_id {
            return Err("Binding 与已解析 Runtime 路由身份不一致".to_string());
        }
        if routing_capability_snapshot_id.trim().is_empty() {
            return Err("binding_live 路由缺少 CapabilitySnapshot".to_string());
        }
        let routed_effort = command
            .requested_reasoning_effort
            .unwrap_or(route.default_reasoning_effort);
        let requested_model_id = command
            .requested_model_id
            .clone()
            .filter(|model| !model.trim().is_empty())
            .unwrap_or_else(|| route.model_id.clone());
        Ok(TurnExecutionSpec {
            turn_id: new_turn_id(),
            history_session_id: command.history_session_id.clone(),
            turn_epoch: 0,
            engine_id: route.engine_id.clone(),
            provider_id: route.provider_id.clone(),
            provider_kind: route.provider_kind.clone(),
            provider_display_name: route.provider_display_name.clone(),
            route_label_snapshot: route.route_label_snapshot.clone(),
            requested_model_id,
            routed_model_id: route.model_id.clone(),
            model_label_snapshot: route.model_label_snapshot.clone(),
            requested_reasoning_effort: command
                .requested_reasoning_effort
                .unwrap_or(route.default_reasoning_effort),
            routed_reasoning_effort: routed_effort,
            turn_mode: command.turn_mode.clone(),
            permission_profile: command.permission_profile.clone(),
            binding_id: Some(format!("binding:{}", binding.engine_id)),
            binding_revision: Some(binding.revision),
            engine_profile_digest: route.engine_profile_digest.clone(),
            provider_launch_profile_ref: route.provider_launch_profile_ref.clone(),
            launch_config_digest: route.launch_config_digest.clone(),
            routing_capability_snapshot_id: Some(routing_capability_snapshot_id.to_string()),
            resolution_source: BINDING_LIVE.to_string(),
            legacy_route_snapshot_digest: None,
            pricing_basis_snapshot: route.pricing_basis_snapshot.clone(),
            session_context: Vec::new(),
            created_at: command.created_at,
        })
    }
}

pub fn build_runtime_route(
    config: &AppConfig,
    binding: &BindingConfig,
    model_id: &str,
    engine_bin: &str,
    launch_env: &[(String, String)],
    default_reasoning_effort: ReasoningEffort,
    pricing_profile: Option<ResolvedPricingProfile>,
) -> Result<RuntimeRoute, String> {
    let provider = config
        .providers
        .iter()
        .find(|provider| provider.id == binding.provider_id)
        .ok_or_else(|| format!("找不到绑定服务商：{}", binding.provider_id))?;
    let model_label = config
        .models
        .iter()
        .find(|model| model.provider_id == binding.provider_id && model.id == model_id)
        .map(|model| model.display_name.clone())
        .unwrap_or_else(|| model_id.to_string());
    let engine = config
        .engines
        .iter()
        .find(|engine| engine.id == binding.engine_id)
        .ok_or_else(|| format!("找不到绑定引擎：{}", binding.engine_id))?;
    let engine_profile_digest = digest_json(&(engine, engine_bin))?;
    let launch_config_digest = digest_json(&(
        &binding.engine_id,
        &binding.provider_id,
        model_id,
        engine_bin,
        digest_launch_env(launch_env),
    ))?;
    // ProviderLaunchProfile 只取配置身份与非密钥配置；真实 Key/token 不参与摘要或持久化。
    let provider_launch_profile_digest = provider_launch_profile_digest(binding, provider)?;
    Ok(RuntimeRoute {
        engine_id: binding.engine_id.clone(),
        provider_id: binding.provider_id.clone(),
        provider_kind: provider_kind_str(&provider.kind).to_string(),
        provider_display_name: provider.name.clone(),
        route_label_snapshot: format!("{} / {}", provider.name, model_label),
        model_id: model_id.to_string(),
        model_label_snapshot: model_label,
        default_reasoning_effort,
        engine_profile_digest,
        provider_launch_profile_ref: format!(
            "provider:{}:{}",
            provider.id,
            provider_kind_str(&provider.kind)
        ),
        provider_launch_profile_digest,
        launch_config_digest,
        pricing_basis_snapshot: PricingBasisSnapshot {
            profile: pricing_profile,
        },
    })
}

fn provider_launch_profile_digest(
    binding: &BindingConfig,
    provider: &ProviderConfig,
) -> Result<String, String> {
    let provider_specific = match provider.kind {
        ProviderKind::Subscription => serde_json::json!({}),
        ProviderKind::Api | ProviderKind::Local => serde_json::json!({
            "baseUrl": provider.base_url,
            "keyRef": provider.key_ref,
        }),
    };
    digest_json(&serde_json::json!({
        "engineId": binding.engine_id,
        "providerId": binding.provider_id,
        "providerKind": provider.kind,
        "protocol": provider.protocol,
        "authMethod": provider.auth_method,
        "providerSpecific": provider_specific,
    }))
}

pub fn digest_json<T: Serialize + ?Sized>(value: &T) -> Result<String, String> {
    let bytes = serde_json::to_vec(value).map_err(|error| format!("生成配置摘要失败：{error}"))?;
    Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
}

fn digest_launch_env(env: &[(String, String)]) -> String {
    let mut entries = env
        .iter()
        .map(|(key, value)| {
            (
                key.clone(),
                format!("{:x}", Sha256::digest(value.as_bytes())),
            )
        })
        .collect::<Vec<_>>();
    entries.sort();
    digest_json(&entries).unwrap_or_else(|_| "sha256:unavailable".to_string())
}

fn provider_kind_str(kind: &ProviderKind) -> &'static str {
    match kind {
        ProviderKind::Subscription => "subscription",
        ProviderKind::Api => "api",
        ProviderKind::Local => "local",
    }
}

fn new_turn_id() -> String {
    format!("turn-{:032x}", rand::random::<u128>())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::{AuthMethod, Protocol};

    #[test]
    fn binding_live_resolver_freezes_binding_and_capability_sources() {
        let route = RuntimeRoute {
            engine_id: "codex".into(),
            provider_id: "provider-openai".into(),
            provider_kind: "api".into(),
            provider_display_name: "OpenAI".into(),
            route_label_snapshot: "OpenAI / GPT".into(),
            model_id: "gpt-test".into(),
            model_label_snapshot: "GPT".into(),
            default_reasoning_effort: ReasoningEffort::Auto,
            engine_profile_digest: "sha256:engine".into(),
            provider_launch_profile_ref: "provider:provider-openai:api".into(),
            provider_launch_profile_digest: "sha256:provider-launch".into(),
            launch_config_digest: "sha256:launch".into(),
            pricing_basis_snapshot: PricingBasisSnapshot { profile: None },
        };
        let binding = BindingConfig {
            engine_id: "codex".into(),
            provider_id: "provider-openai".into(),
            primary_model: "gpt-test".into(),
            fast_model: None,
            assistant_model_id: None,
            reasoning_effort: None,
            revision: 7,
        };
        let spec = BindingLiveRouteResolver::resolve(
            &route,
            &binding,
            "capability-7",
            &TurnStartCommand {
                history_session_id: "session-1".into(),
                display_text: "hello".into(),
                turn_mode: "build".into(),
                permission_profile: "standard".into(),
                requested_reasoning_effort: Some(ReasoningEffort::High),
                requested_model_id: Some("requested-model".into()),
                attachments: Vec::new(),
                created_at: 1,
            },
        )
        .unwrap();
        assert_eq!(spec.resolution_source, BINDING_LIVE);
        assert_eq!(spec.binding_id.as_deref(), Some("binding:codex"));
        assert_eq!(spec.binding_revision, Some(7));
        assert_eq!(
            spec.routing_capability_snapshot_id.as_deref(),
            Some("capability-7")
        );
        assert_eq!(spec.requested_model_id, "requested-model");
        assert!(spec.legacy_route_snapshot_digest.is_none());
        assert_eq!(spec.routed_reasoning_effort, ReasoningEffort::High);
    }

    #[test]
    fn turn_ids_are_not_session_local_epochs() {
        let first = new_turn_id();
        let second = new_turn_id();
        assert_ne!(first, second);
        assert_eq!(first.len(), "turn-".len() + 32);
    }

    #[test]
    fn provider_launch_digest_ignores_display_and_probe_state() {
        let binding = BindingConfig {
            engine_id: "codex".into(),
            provider_id: "provider-openai".into(),
            primary_model: "gpt-test".into(),
            fast_model: None,
            assistant_model_id: None,
            reasoning_effort: None,
            revision: 1,
        };
        let provider = ProviderConfig {
            id: "provider-openai".into(),
            name: "OpenAI".into(),
            kind: ProviderKind::Api,
            base_url: "https://api.example.test".into(),
            key_ref: Some("keyring:provider-openai".into()),
            ready: false,
            last_test: None,
            protocol: Protocol::OpenAiResponses,
            auth_method: AuthMethod::ApiKey,
        };
        let first = provider_launch_profile_digest(&binding, &provider).unwrap();
        let mut changed = provider.clone();
        changed.name = "Renamed".into();
        changed.ready = true;
        assert_eq!(
            first,
            provider_launch_profile_digest(&binding, &changed).unwrap()
        );
        changed.base_url = "https://other.example.test".into();
        assert_ne!(
            first,
            provider_launch_profile_digest(&binding, &changed).unwrap()
        );
    }

    #[test]
    fn subscription_launch_digest_ignores_stale_api_fields() {
        let binding = BindingConfig {
            engine_id: "codex".into(),
            provider_id: "openai-subscription".into(),
            primary_model: "default".into(),
            fast_model: None,
            assistant_model_id: None,
            reasoning_effort: None,
            revision: 1,
        };
        let provider = ProviderConfig {
            id: "openai-subscription".into(),
            name: "OpenAI 订阅".into(),
            kind: ProviderKind::Subscription,
            base_url: "stale-value-must-not-route".into(),
            key_ref: Some("stale-key-ref".into()),
            ready: true,
            last_test: None,
            protocol: Protocol::OpenAiResponses,
            auth_method: AuthMethod::OAuth,
        };
        let first = provider_launch_profile_digest(&binding, &provider).unwrap();
        let mut changed = provider.clone();
        changed.base_url = "another-stale-value".into();
        changed.key_ref = Some("another-stale-key-ref".into());
        assert_eq!(
            first,
            provider_launch_profile_digest(&binding, &changed).unwrap()
        );
    }
}
