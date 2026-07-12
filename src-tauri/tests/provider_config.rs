use helm_lib::providers::{
    test_provider_connection, AppConfig, AuthMethod, BindingConfig, EngineConfig, EngineStatus,
    MemorySecretStore, ModelConfig, PriceSource, Protocol, ProviderConfig, ProviderStore,
    ProviderTest, SecretStore, TestOutcome,
};
#[cfg(target_os = "windows")]
use keyring::credential::CredentialPersistence;
use std::fs;
use std::sync::{Arc, Mutex};

fn temp_config_path(name: &str) -> std::path::PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "helm-provider-config-{}-{name}.json",
        std::process::id()
    ));
    let _ = fs::remove_file(&path);
    path
}

fn anthropic_provider() -> ProviderConfig {
    ProviderConfig {
        id: "anthropic".to_string(),
        name: "Anthropic".to_string(),
        base_url: "https://api.anthropic.com".to_string(),
        key_ref: None,
        ready: true,
        last_test: None,
        protocol: Protocol::Anthropic,
        auth_method: AuthMethod::ApiKey,
    }
}

fn openai_provider() -> ProviderConfig {
    ProviderConfig {
        id: "openai".to_string(),
        name: "OpenAI".to_string(),
        base_url: "https://api.openai.com/v1".to_string(),
        key_ref: None,
        ready: true,
        last_test: None,
        protocol: Protocol::OpenAiResponses,
        auth_method: AuthMethod::ApiKey,
    }
}

fn claude_model() -> ModelConfig {
    ModelConfig {
        id: "claude-sonnet-4.6".to_string(),
        provider_id: "anthropic".to_string(),
        display_name: "claude-sonnet-4.6".to_string(),
        input_price_per_mtok: 3.0,
        output_price_per_mtok: 15.0,
        price_source: Some(PriceSource::Manual),
        enabled: true,
    }
}

fn claude_fast_model() -> ModelConfig {
    ModelConfig {
        id: "claude-haiku-4.6".to_string(),
        provider_id: "anthropic".to_string(),
        display_name: "claude-haiku-4.6".to_string(),
        input_price_per_mtok: 1.0,
        output_price_per_mtok: 5.0,
        price_source: Some(PriceSource::Manual),
        enabled: true,
    }
}

fn codex_model() -> ModelConfig {
    ModelConfig {
        id: "gpt-5-codex".to_string(),
        provider_id: "openai".to_string(),
        display_name: "gpt-5-codex".to_string(),
        input_price_per_mtok: 1.25,
        output_price_per_mtok: 10.0,
        price_source: Some(PriceSource::Builtin),
        enabled: true,
    }
}

#[cfg(target_os = "windows")]
#[test]
fn windows_keyring_uses_native_credential_store() {
    assert!(
        matches!(
            keyring::default::default_credential_builder().persistence(),
            CredentialPersistence::UntilDelete
        ),
        "Windows builds must enable keyring/windows-native; mock keyring loses secrets"
    );
}

#[test]
fn provider_api_key_is_stored_as_key_ref_not_plaintext() {
    let path = temp_config_path("provider-key-ref");
    let secrets = MemorySecretStore::default();
    let store = ProviderStore::new(path.clone(), secrets.clone());

    let provider = ProviderConfig {
        id: "anthropic".to_string(),
        name: "Anthropic".to_string(),
        base_url: "https://api.anthropic.com".to_string(),
        key_ref: None,
        ready: false,
        last_test: None,
        protocol: Protocol::Anthropic,
        auth_method: AuthMethod::ApiKey,
    };

    store
        .save_provider(provider, Some("sk-ant-secret-value"))
        .unwrap();

    let raw = fs::read_to_string(&path).unwrap();
    assert!(!raw.contains("sk-ant-secret-value"));
    assert!(raw.contains("helm:provider:anthropic:api-key"));
    assert_eq!(
        secrets.get("helm:provider:anthropic:api-key").unwrap(),
        Some("sk-ant-secret-value".to_string())
    );
}

#[test]
fn saving_provider_without_new_key_preserves_existing_key_ref() {
    let path = temp_config_path("preserve-key-ref");
    let secrets = MemorySecretStore::default();
    let store = ProviderStore::new(path.clone(), secrets.clone());

    store
        .save_provider(
            ProviderConfig {
                id: "anthropic".to_string(),
                name: "Anthropic".to_string(),
                base_url: "https://api.anthropic.com".to_string(),
                key_ref: None,
                ready: false,
                last_test: None,
                protocol: Protocol::Anthropic,
                auth_method: AuthMethod::ApiKey,
            },
            Some("sk-ant-secret-value"),
        )
        .unwrap();

    store
        .save_provider(
            ProviderConfig {
                id: "anthropic".to_string(),
                name: "Anthropic".to_string(),
                base_url: "https://api.anthropic.com".to_string(),
                key_ref: None,
                ready: false,
                last_test: None,
                protocol: Protocol::Anthropic,
                auth_method: AuthMethod::ApiKey,
            },
            None,
        )
        .unwrap();

    let loaded = store.load().unwrap();
    let provider = loaded
        .providers
        .iter()
        .find(|provider| provider.id == "anthropic")
        .unwrap();
    assert_eq!(
        provider.key_ref.as_deref(),
        Some("helm:provider:anthropic:api-key")
    );
    assert_eq!(
        secrets.get("helm:provider:anthropic:api-key").unwrap(),
        Some("sk-ant-secret-value".to_string())
    );
}

#[test]
fn provider_secret_can_be_revealed_from_secret_store_by_provider_id() {
    let path = temp_config_path("reveal-provider-secret");
    let secrets = MemorySecretStore::default();
    let store = ProviderStore::new(path, secrets);

    store
        .save_provider(
            ProviderConfig {
                id: "anthropic".to_string(),
                name: "Anthropic".to_string(),
                base_url: "https://api.anthropic.com".to_string(),
                key_ref: None,
                ready: false,
                last_test: None,
                protocol: Protocol::Anthropic,
                auth_method: AuthMethod::ApiKey,
            },
            Some("sk-ant-secret-value"),
        )
        .unwrap();

    assert_eq!(
        store.provider_secret("anthropic").unwrap(),
        "sk-ant-secret-value"
    );
}

#[derive(Clone, Default)]
struct WriteOnlySecretStore {
    values: Arc<Mutex<Vec<String>>>,
}

impl SecretStore for WriteOnlySecretStore {
    fn set(&self, key_ref: &str, _secret: &str) -> Result<(), String> {
        self.values.lock().unwrap().push(key_ref.to_string());
        Ok(())
    }

    fn get(&self, _key_ref: &str) -> Result<Option<String>, String> {
        Ok(None)
    }

    fn delete(&self, _key_ref: &str) -> Result<(), String> {
        Ok(())
    }
}

#[test]
fn saving_provider_does_not_persist_key_ref_when_secret_cannot_be_read_back() {
    let path = temp_config_path("dangling-key-ref");
    let store = ProviderStore::new(path.clone(), WriteOnlySecretStore::default());

    let result = store.save_provider(
        ProviderConfig {
            id: "anthropic".to_string(),
            name: "Anthropic".to_string(),
            base_url: "https://api.anthropic.com".to_string(),
            key_ref: None,
            ready: false,
            last_test: None,
            protocol: Protocol::Anthropic,
            auth_method: AuthMethod::ApiKey,
        },
        Some("sk-ant-secret-value"),
    );

    assert!(result.is_err());
    assert!(
        !path.exists(),
        "config must not persist keyRef when keyring readback fails"
    );
}

#[test]
fn provider_models_endpoint_accepts_base_or_models_url() {
    assert_eq!(
        helm_lib::providers::provider_models_endpoint("anthropic", "https://api.anthropic.com"),
        "https://api.anthropic.com/v1/models"
    );
    assert_eq!(
        helm_lib::providers::provider_models_endpoint(
            "anthropic",
            "https://api.anthropic.com/v1/models"
        ),
        "https://api.anthropic.com/v1/models"
    );
    assert_eq!(
        helm_lib::providers::provider_models_endpoint("openai", "https://api.openai.com/v1"),
        "https://api.openai.com/v1/models"
    );
    assert_eq!(
        helm_lib::providers::provider_models_endpoint("openai", "https://api.openai.com/v1/models"),
        "https://api.openai.com/v1/models"
    );
}

#[test]
fn provider_models_endpoint_uses_protocol_for_custom_openai_provider() {
    assert_eq!(
        helm_lib::providers::provider_models_endpoint_for_protocol(
            &Protocol::OpenAiResponses,
            "https://api.example.com/v1"
        ),
        "https://api.example.com/v1/models"
    );
    assert_eq!(
        helm_lib::providers::provider_models_endpoint_for_protocol(
            &Protocol::OpenAiResponses,
            "https://api.example.com"
        ),
        "https://api.example.com/v1/models"
    );
    assert_eq!(
        helm_lib::providers::provider_models_endpoint_for_protocol(
            &Protocol::OpenAiChat,
            "https://api.example.com/"
        ),
        "https://api.example.com/v1/models"
    );
    assert_eq!(
        helm_lib::providers::provider_models_endpoint_for_protocol(
            &Protocol::Anthropic,
            "https://api.example.com"
        ),
        "https://api.example.com/v1/models"
    );
}

#[test]
fn parse_synced_models_preserves_existing_model_metadata() {
    let existing = vec![ModelConfig {
        id: "gpt-5-codex".to_string(),
        provider_id: "openai".to_string(),
        display_name: "GPT-5 Codex".to_string(),
        input_price_per_mtok: 1.25,
        output_price_per_mtok: 10.0,
        price_source: Some(PriceSource::Manual),
        enabled: false,
    }];

    let models = helm_lib::providers::models_from_provider_response(
        &Protocol::OpenAiResponses,
        "openai",
        r#"{"data":[{"id":"gpt-5-codex"},{"id":"gpt-5-mini"}]}"#,
        &existing,
    )
    .unwrap();

    assert_eq!(models.len(), 2);
    assert_eq!(models[0].id, "gpt-5-codex");
    assert_eq!(models[0].display_name, "GPT-5 Codex");
    assert_eq!(models[0].input_price_per_mtok, 1.25);
    assert_eq!(models[0].price_source, Some(PriceSource::Manual));
    assert!(!models[0].enabled);
    assert_eq!(models[1].id, "gpt-5-mini");
    assert_eq!(models[1].price_source, Some(PriceSource::Builtin));
    assert!(models[1].enabled);
}

#[test]
fn parse_synced_models_applies_builtin_pricing_for_known_models() {
    let models = helm_lib::providers::models_from_provider_response(
        &Protocol::OpenAiResponses,
        "openai",
        r#"{"data":[{"id":"gpt-5-codex"},{"id":"unknown-gateway-model"}]}"#,
        &[],
    )
    .unwrap();

    assert_eq!(models[0].id, "gpt-5-codex");
    assert_eq!(models[0].input_price_per_mtok, 1.25);
    assert_eq!(models[0].output_price_per_mtok, 10.0);
    assert_eq!(models[0].price_source, Some(PriceSource::Builtin));
    assert_eq!(models[1].id, "unknown-gateway-model");
    assert_eq!(models[1].input_price_per_mtok, 0.0);
    assert_eq!(models[1].output_price_per_mtok, 0.0);
    assert_eq!(models[1].price_source, Some(PriceSource::Unknown));
}

#[test]
fn load_backfills_builtin_pricing_for_existing_zero_price_models() {
    let path = temp_config_path("builtin-price-backfill");
    fs::write(
        &path,
        r#"{
  "providers": [
    {
      "id": "openai",
      "name": "OpenAI",
      "baseUrl": "https://api.openai.com/v1",
      "keyRef": null,
      "ready": false,
      "lastTest": null,
      "protocol": "openai-responses",
      "authMethod": "apikey"
    }
  ],
  "models": [
    {
      "id": "gpt-5.5",
      "providerId": "openai",
      "displayName": "gpt-5.5",
      "inputPricePerMtok": 0.0,
      "outputPricePerMtok": 0.0,
      "enabled": true
    },
    {
      "id": "gateway-only-model",
      "providerId": "openai",
      "displayName": "gateway-only-model",
      "inputPricePerMtok": 0.0,
      "outputPricePerMtok": 0.0,
      "enabled": true
    }
  ],
  "engines": [],
  "bindings": [],
  "defaultEngine": "codex",
  "defaultModel": "gpt-5.5"
}"#,
    )
    .unwrap();
    let store = ProviderStore::new(path, MemorySecretStore::default());

    let loaded = store.load().unwrap();

    let known = loaded
        .models
        .iter()
        .find(|model| model.id == "gpt-5.5")
        .unwrap();
    assert_eq!(known.input_price_per_mtok, 1.25);
    assert_eq!(known.output_price_per_mtok, 10.0);
    assert_eq!(known.price_source, Some(PriceSource::Builtin));
    let unknown = loaded
        .models
        .iter()
        .find(|model| model.id == "gateway-only-model")
        .unwrap();
    assert_eq!(unknown.input_price_per_mtok, 0.0);
    assert_eq!(unknown.output_price_per_mtok, 0.0);
    assert_eq!(unknown.price_source, Some(PriceSource::Unknown));
}

#[test]
fn load_does_not_overwrite_manual_or_provider_pricing() {
    let path = temp_config_path("preserve-explicit-price-source");
    fs::write(
        &path,
        r#"{
  "providers": [
    {
      "id": "openai",
      "name": "OpenAI",
      "baseUrl": "https://api.openai.com/v1",
      "keyRef": null,
      "ready": false,
      "lastTest": null,
      "protocol": "openai-responses",
      "authMethod": "apikey"
    }
  ],
  "models": [
    {
      "id": "gpt-5.5",
      "providerId": "openai",
      "displayName": "gpt-5.5",
      "inputPricePerMtok": 9.0,
      "outputPricePerMtok": 99.0,
      "priceSource": "manual",
      "enabled": true
    },
    {
      "id": "gpt-5-codex",
      "providerId": "openai",
      "displayName": "gpt-5-codex",
      "inputPricePerMtok": 8.0,
      "outputPricePerMtok": 88.0,
      "priceSource": "provider",
      "enabled": true
    }
  ],
  "engines": [],
  "bindings": [],
  "defaultEngine": "codex",
  "defaultModel": "gpt-5.5"
}"#,
    )
    .unwrap();
    let store = ProviderStore::new(path, MemorySecretStore::default());

    let loaded = store.load().unwrap();

    let manual = loaded
        .models
        .iter()
        .find(|model| model.id == "gpt-5.5")
        .unwrap();
    assert_eq!(manual.input_price_per_mtok, 9.0);
    assert_eq!(manual.output_price_per_mtok, 99.0);
    assert_eq!(manual.price_source, Some(PriceSource::Manual));
    let provider = loaded
        .models
        .iter()
        .find(|model| model.id == "gpt-5-codex")
        .unwrap();
    assert_eq!(provider.input_price_per_mtok, 8.0);
    assert_eq!(provider.output_price_per_mtok, 88.0);
    assert_eq!(provider.price_source, Some(PriceSource::Provider));
}

#[test]
fn parse_synced_models_deduplicates_model_ids_per_provider() {
    let models = helm_lib::providers::models_from_provider_response(
        &Protocol::OpenAiResponses,
        "openai",
        r#"{"data":[{"id":"gpt-5.4-mini"},{"id":"gpt-5.4-mini"},{"id":"gpt-5.5"}]}"#,
        &[],
    )
    .unwrap();

    assert_eq!(
        models
            .iter()
            .map(|model| model.id.as_str())
            .collect::<Vec<_>>(),
        vec!["gpt-5.4-mini", "gpt-5.5"]
    );
}

#[test]
fn provider_last_test_can_be_recorded_after_reachability_test() {
    let path = temp_config_path("provider-last-test");
    let store = ProviderStore::new(path, MemorySecretStore::default());

    store
        .save_provider(
            ProviderConfig {
                id: "custom".to_string(),
                name: "Custom".to_string(),
                base_url: "https://api.example.com/v1".to_string(),
                key_ref: None,
                ready: false,
                last_test: None,
                protocol: Protocol::OpenAiResponses,
                auth_method: AuthMethod::ApiKey,
            },
            None,
        )
        .unwrap();

    let loaded = store
        .record_test_result(
            "custom",
            ProviderTest {
                result: TestOutcome::Ok,
                latency_ms: Some(123),
                at: 1_717_171_717,
            },
        )
        .unwrap();

    let last_test = loaded.providers[0].last_test.as_ref().unwrap();
    assert_eq!(last_test.result, TestOutcome::Ok);
    assert_eq!(last_test.latency_ms, Some(123));
    assert_eq!(last_test.at, 1_717_171_717);
}

#[test]
fn default_engine_and_model_round_trip_through_config() {
    let path = temp_config_path("defaults");
    let store = ProviderStore::new(path, MemorySecretStore::default());

    store.save_provider(anthropic_provider(), None).unwrap();
    store.save_model(claude_model()).unwrap();
    store
        .save_engine(EngineConfig {
            id: "claude-code".to_string(),
            name: "Claude Code".to_string(),
            bin: "D:\\Tools\\claude.cmd".to_string(),
            default_model: "claude-sonnet-4.6".to_string(),
            status: EngineStatus::Ready,
            version: Some("1.0.0".to_string()),
        })
        .unwrap();
    store
        .set_defaults("claude-code", "claude-sonnet-4.6")
        .unwrap();

    let loaded = store.load().unwrap();
    assert_eq!(loaded.default_engine, "claude-code");
    assert_eq!(loaded.default_model, "claude-sonnet-4.6");
    assert_eq!(
        loaded.engine_bin("claude-code").unwrap(),
        "D:\\Tools\\claude.cmd"
    );
}

#[test]
fn missing_config_starts_without_seeded_providers_or_models() {
    let path = temp_config_path("seeded");
    let store = ProviderStore::new(path, MemorySecretStore::default());

    let loaded: AppConfig = store.load().unwrap();

    assert_eq!(loaded.default_engine, "claude-code");
    assert!(loaded.providers.is_empty());
    assert!(loaded.models.is_empty());
    assert!(loaded.bindings.is_empty());
    assert!(loaded.engines.iter().any(|e| e.id == "claude-code"));
    assert!(loaded.engines.iter().any(|e| e.id == "codex"));
}

#[test]
fn defaults_must_reference_existing_engine_and_model() {
    let path = temp_config_path("validated-defaults");
    let store = ProviderStore::new(path, MemorySecretStore::default());

    let missing_engine = store.set_defaults("missing-engine", "claude-sonnet-4.6");
    assert!(missing_engine.is_err());

    let missing_model = store.set_defaults("claude-code", "missing-model");
    assert!(missing_model.is_err());
}

#[test]
fn model_enablement_round_trips_through_config() {
    let path = temp_config_path("model-enable");
    let store = ProviderStore::new(path, MemorySecretStore::default());

    store.save_provider(anthropic_provider(), None).unwrap();
    store
        .save_model(ModelConfig {
            id: "claude-sonnet-4.6".to_string(),
            provider_id: "anthropic".to_string(),
            display_name: "claude-sonnet-4.6".to_string(),
            input_price_per_mtok: 3.0,
            output_price_per_mtok: 15.0,
            price_source: Some(PriceSource::Manual),
            enabled: false,
        })
        .unwrap();

    let loaded = store.load().unwrap();
    let model = loaded
        .models
        .iter()
        .find(|model| model.id == "claude-sonnet-4.6")
        .unwrap();
    assert!(!model.enabled);
}

#[test]
fn deleting_provider_removes_related_models_and_keeps_valid_defaults() {
    let path = temp_config_path("delete-provider");
    let store = ProviderStore::new(path, MemorySecretStore::default());

    store
        .save_provider(
            ProviderConfig {
                id: "custom".to_string(),
                name: "Custom".to_string(),
                base_url: "https://api.example.com".to_string(),
                key_ref: None,
                ready: false,
                last_test: None,
                protocol: Protocol::Anthropic,
                auth_method: AuthMethod::ApiKey,
            },
            None,
        )
        .unwrap();
    store
        .save_model(ModelConfig {
            id: "custom-model".to_string(),
            provider_id: "custom".to_string(),
            display_name: "custom-model".to_string(),
            input_price_per_mtok: 1.0,
            output_price_per_mtok: 2.0,
            price_source: Some(PriceSource::Manual),
            enabled: true,
        })
        .unwrap();
    store.set_defaults("claude-code", "custom-model").unwrap();

    let loaded = store.delete_provider("custom").unwrap();

    assert!(!loaded
        .providers
        .iter()
        .any(|provider| provider.id == "custom"));
    assert!(!loaded
        .models
        .iter()
        .any(|model| model.provider_id == "custom"));
    assert_eq!(loaded.default_engine, "claude-code");
    assert_ne!(loaded.default_model, "custom-model");
}

#[test]
fn old_config_without_protocol_auth_or_bindings_loads_with_migrated_binding() {
    let path = temp_config_path("old-config-upgrade");
    fs::write(
        &path,
        r#"{
  "providers": [
    {
      "id": "anthropic",
      "name": "Anthropic",
      "baseUrl": "https://api.anthropic.com",
      "keyRef": null,
      "status": "connected"
    }
  ],
  "models": [
    {
      "id": "claude-sonnet-4.6",
      "providerId": "anthropic",
      "displayName": "claude-sonnet-4.6",
      "inputPricePerMtok": 3.0,
      "outputPricePerMtok": 15.0,
      "enabled": true
    }
  ],
  "engines": [
    {
      "id": "claude-code",
      "name": "Claude Code",
      "bin": "claude",
      "defaultModel": "claude-sonnet-4.6",
      "status": "ready",
      "version": null
    }
  ],
  "defaultEngine": "claude-code",
  "defaultModel": "claude-sonnet-4.6"
}"#,
    )
    .unwrap();
    let store = ProviderStore::new(path, MemorySecretStore::default());

    let loaded = store.load().unwrap();

    assert_eq!(loaded.providers[0].protocol, Protocol::Anthropic);
    assert_eq!(loaded.providers[0].auth_method, AuthMethod::ApiKey);
    assert!(!loaded.providers[0].ready);
    assert!(loaded.providers[0].last_test.is_none());
    assert_eq!(loaded.bindings.len(), 1);
    assert_eq!(loaded.bindings[0].engine_id, "claude-code");
    assert_eq!(loaded.bindings[0].provider_id, "anthropic");
    assert_eq!(loaded.bindings[0].primary_model, "claude-sonnet-4.6");
}

#[test]
fn save_binding_rejects_provider_protocol_that_engine_does_not_accept() {
    let path = temp_config_path("binding-protocol");
    let store = ProviderStore::new(path, MemorySecretStore::default());

    store.save_provider(openai_provider(), None).unwrap();
    store.save_model(codex_model()).unwrap();
    let result = store.save_binding(BindingConfig {
        engine_id: "claude-code".to_string(),
        provider_id: "openai".to_string(),
        primary_model: "gpt-5-codex".to_string(),
        fast_model: None,
    });

    assert!(result.is_err());
    assert!(result.unwrap_err().contains("协议不兼容"));
}

#[test]
fn save_binding_requires_model_to_belong_to_provider_and_be_enabled() {
    let path = temp_config_path("binding-model-provider");
    let store = ProviderStore::new(path, MemorySecretStore::default());

    store.save_provider(anthropic_provider(), None).unwrap();
    store.save_provider(openai_provider(), None).unwrap();
    store.save_model(claude_model()).unwrap();
    store.save_model(codex_model()).unwrap();
    let wrong_provider = store.save_binding(BindingConfig {
        engine_id: "codex".to_string(),
        provider_id: "openai".to_string(),
        primary_model: "claude-sonnet-4.6".to_string(),
        fast_model: None,
    });
    assert!(wrong_provider.is_err());

    store
        .save_model(ModelConfig {
            id: "gpt-disabled".to_string(),
            provider_id: "openai".to_string(),
            display_name: "gpt-disabled".to_string(),
            input_price_per_mtok: 1.0,
            output_price_per_mtok: 2.0,
            price_source: Some(PriceSource::Manual),
            enabled: false,
        })
        .unwrap();
    let disabled = store.save_binding(BindingConfig {
        engine_id: "codex".to_string(),
        provider_id: "openai".to_string(),
        primary_model: "gpt-disabled".to_string(),
        fast_model: None,
    });
    assert!(disabled.is_err());
}

#[test]
fn save_binding_persists_valid_binding() {
    let path = temp_config_path("binding-save");
    let store = ProviderStore::new(path, MemorySecretStore::default());

    store.save_provider(openai_provider(), None).unwrap();
    store.save_model(codex_model()).unwrap();
    let loaded = store
        .save_binding(BindingConfig {
            engine_id: "codex".to_string(),
            provider_id: "openai".to_string(),
            primary_model: "gpt-5-codex".to_string(),
            fast_model: Some("gpt-5-codex".to_string()),
        })
        .unwrap();

    let binding = loaded
        .bindings
        .iter()
        .find(|binding| binding.engine_id == "codex")
        .unwrap();
    assert_eq!(binding.provider_id, "openai");
    assert_eq!(binding.primary_model, "gpt-5-codex");
    assert_eq!(binding.fast_model.as_deref(), Some("gpt-5-codex"));
}

#[test]
fn save_binding_accepts_same_model_id_from_selected_provider() {
    let path = temp_config_path("binding-duplicate-model-id");
    let store = ProviderStore::new(path, MemorySecretStore::default());

    let mut gateway_a = openai_provider();
    gateway_a.id = "gateway-a".to_string();
    let mut gateway_b = openai_provider();
    gateway_b.id = "gateway-b".to_string();
    store.save_provider(gateway_a, None).unwrap();
    store.save_provider(gateway_b, None).unwrap();
    store
        .save_models_for_provider(
            "gateway-a",
            vec![ModelConfig {
                id: "gpt-5.5".to_string(),
                provider_id: "gateway-a".to_string(),
                display_name: "gpt-5.5".to_string(),
                input_price_per_mtok: 0.0,
                output_price_per_mtok: 0.0,
                price_source: None,
                enabled: true,
            }],
        )
        .unwrap();
    store
        .save_models_for_provider(
            "gateway-b",
            vec![ModelConfig {
                id: "gpt-5.5".to_string(),
                provider_id: "gateway-b".to_string(),
                display_name: "gpt-5.5".to_string(),
                input_price_per_mtok: 0.0,
                output_price_per_mtok: 0.0,
                price_source: None,
                enabled: true,
            }],
        )
        .unwrap();

    let loaded = store
        .save_binding(BindingConfig {
            engine_id: "codex".to_string(),
            provider_id: "gateway-b".to_string(),
            primary_model: "gpt-5.5".to_string(),
            fast_model: Some("gpt-5.5".to_string()),
        })
        .unwrap();

    let binding = loaded
        .bindings
        .iter()
        .find(|binding| binding.engine_id == "codex")
        .unwrap();
    assert_eq!(binding.provider_id, "gateway-b");
    assert_eq!(binding.primary_model, "gpt-5.5");
}

#[test]
fn equivalent_env_matches_provider_protocol_and_binding_models() {
    let path = temp_config_path("equivalent-env");
    let store = ProviderStore::new(path, MemorySecretStore::default());

    store
        .save_provider(anthropic_provider(), Some("sk-ant-secret"))
        .unwrap();
    store.save_model(claude_model()).unwrap();
    store.save_model(claude_fast_model()).unwrap();
    let env = store
        .equivalent_env(&BindingConfig {
            engine_id: "claude-code".to_string(),
            provider_id: "anthropic".to_string(),
            primary_model: "claude-sonnet-4.6".to_string(),
            fast_model: Some("claude-haiku-4.6".to_string()),
        })
        .unwrap();

    assert_eq!(
        env,
        vec![
            (
                "ANTHROPIC_BASE_URL".to_string(),
                "https://api.anthropic.com".to_string()
            ),
            (
                "ANTHROPIC_MODEL".to_string(),
                "claude-sonnet-4.6".to_string()
            ),
            (
                "ANTHROPIC_DEFAULT_HAIKU_MODEL".to_string(),
                "claude-haiku-4.6".to_string()
            ),
            (
                "ANTHROPIC_AUTH_TOKEN".to_string(),
                "••••（系统钥匙串）".to_string()
            )
        ]
    );
}

#[test]
fn launch_env_resolves_secret_value_without_persisting_plaintext() {
    let path = temp_config_path("launch-env");
    let secrets = MemorySecretStore::default();
    let store = ProviderStore::new(path.clone(), secrets);

    store
        .save_provider(anthropic_provider(), Some("sk-ant-runtime-secret"))
        .unwrap();
    store.save_model(claude_model()).unwrap();
    store.save_model(claude_fast_model()).unwrap();

    let env = store
        .launch_env(&BindingConfig {
            engine_id: "claude-code".to_string(),
            provider_id: "anthropic".to_string(),
            primary_model: "claude-sonnet-4.6".to_string(),
            fast_model: Some("claude-haiku-4.6".to_string()),
        })
        .unwrap();

    assert_eq!(
        env,
        vec![
            (
                "ANTHROPIC_BASE_URL".to_string(),
                "https://api.anthropic.com".to_string()
            ),
            (
                "ANTHROPIC_MODEL".to_string(),
                "claude-sonnet-4.6".to_string()
            ),
            (
                "ANTHROPIC_DEFAULT_HAIKU_MODEL".to_string(),
                "claude-haiku-4.6".to_string()
            ),
            (
                "ANTHROPIC_AUTH_TOKEN".to_string(),
                "sk-ant-runtime-secret".to_string()
            )
        ]
    );
    assert!(!fs::read_to_string(path)
        .unwrap()
        .contains("sk-ant-runtime-secret"));
}

#[test]
fn codex_launch_env_includes_wire_api_hint_but_equivalent_env_does_not() {
    let path = temp_config_path("codex-wire-api");
    let store = ProviderStore::new(path, MemorySecretStore::default());

    store
        .save_provider(
            ProviderConfig {
                protocol: Protocol::OpenAiChat,
                ..openai_provider()
            },
            Some("sk-openai-runtime-secret"),
        )
        .unwrap();
    store.save_model(codex_model()).unwrap();
    let binding = BindingConfig {
        engine_id: "codex".to_string(),
        provider_id: "openai".to_string(),
        primary_model: "gpt-5-codex".to_string(),
        fast_model: None,
    };

    let equivalent = store.equivalent_env(&binding).unwrap();
    let launch = store.launch_env(&binding).unwrap();

    assert!(!equivalent
        .iter()
        .any(|(key, _)| key == "HELM_CODEX_WIRE_API"));
    assert!(launch
        .iter()
        .any(|(key, value)| key == "HELM_CODEX_WIRE_API" && value == "chat"));
}

#[test]
fn launch_env_normalizes_custom_openai_base_url_to_v1() {
    let path = temp_config_path("openai-base-url-normalized");
    let store = ProviderStore::new(path, MemorySecretStore::default());

    store
        .save_provider(
            ProviderConfig {
                id: "custom".to_string(),
                name: "Custom".to_string(),
                base_url: "https://api.example.com".to_string(),
                key_ref: None,
                ready: true,
                last_test: None,
                protocol: Protocol::OpenAiResponses,
                auth_method: AuthMethod::ApiKey,
            },
            Some("sk-openai-runtime-secret"),
        )
        .unwrap();
    store
        .save_model(ModelConfig {
            id: "gpt-5-codex".to_string(),
            provider_id: "custom".to_string(),
            display_name: "GPT-5 Codex".to_string(),
            input_price_per_mtok: 1.25,
            output_price_per_mtok: 10.0,
            price_source: Some(PriceSource::Builtin),
            enabled: true,
        })
        .unwrap();

    let env = store
        .launch_env(&BindingConfig {
            engine_id: "codex".to_string(),
            provider_id: "custom".to_string(),
            primary_model: "gpt-5-codex".to_string(),
            fast_model: None,
        })
        .unwrap();

    assert!(env.contains(&(
        "OPENAI_BASE_URL".to_string(),
        "https://api.example.com/v1".to_string()
    )));
}

#[test]
fn engine_config_file_read_and_write_use_real_files_with_validation() {
    let mut claude_path = std::env::temp_dir();
    claude_path.push(format!("helm-claude-settings-{}.json", std::process::id()));
    let mut codex_path = std::env::temp_dir();
    codex_path.push(format!("helm-codex-config-{}.toml", std::process::id()));
    let _ = fs::remove_file(&claude_path);
    let _ = fs::remove_file(&codex_path);

    helm_lib::providers::write_engine_config_file_at(
        "claude-code",
        &claude_path,
        r#"{"env":{"ANTHROPIC_MODEL":"claude-sonnet-4.6"}}"#,
    )
    .unwrap();
    let claude_file =
        helm_lib::providers::read_engine_config_file_at("claude-code", &claude_path).unwrap();
    assert_eq!(claude_file.path, claude_path);
    assert!(claude_file.content.contains("ANTHROPIC_MODEL"));

    let invalid_json = helm_lib::providers::write_engine_config_file_at(
        "claude-code",
        &claude_path,
        r#"{"env":"missing-close""#,
    );
    assert!(invalid_json.is_err());
    assert!(fs::read_to_string(&claude_path)
        .unwrap()
        .contains("ANTHROPIC_MODEL"));

    helm_lib::providers::write_engine_config_file_at(
        "codex",
        &codex_path,
        "model = \"gpt-5-codex\"\n",
    )
    .unwrap();
    let codex_file = helm_lib::providers::read_engine_config_file_at("codex", &codex_path).unwrap();
    assert_eq!(codex_file.path, codex_path);
    assert!(codex_file.content.contains("gpt-5-codex"));

    let invalid_toml =
        helm_lib::providers::write_engine_config_file_at("codex", &codex_path, "model = ");
    assert!(invalid_toml.is_err());
    assert!(fs::read_to_string(&codex_path)
        .unwrap()
        .contains("gpt-5-codex"));
}

#[test]
fn subscription_provider_is_ready_without_key_and_uses_cli_login() {
    // P3-1 订阅登录一等公民：OAuth 服务商无 Key 即就绪；
    // launch_env 不注入令牌、不注入 BASE_URL（订阅走 CLI 自己的官方线路与登录态）。
    let path = temp_config_path("subscription-ready");
    let store = ProviderStore::new(path, MemorySecretStore::default());

    let mut provider = anthropic_provider();
    provider.auth_method = AuthMethod::OAuth;
    provider.key_ref = None;
    store.save_provider(provider, None).unwrap();
    store.save_model(claude_model()).unwrap();

    let config = store.load().unwrap();
    let saved = config
        .providers
        .iter()
        .find(|provider| provider.id == "anthropic")
        .unwrap();
    assert!(
        saved.ready,
        "订阅登录服务商无 Key 也必须就绪，才能进入绑定列表"
    );

    let env = store
        .launch_env(&BindingConfig {
            engine_id: "claude-code".to_string(),
            provider_id: "anthropic".to_string(),
            primary_model: "claude-sonnet-4.6".to_string(),
            fast_model: None,
        })
        .unwrap();
    assert!(
        !env.iter().any(|(key, _)| key == "ANTHROPIC_AUTH_TOKEN"),
        "订阅模式不注入令牌，凭证由 CLI 登录态提供"
    );
    assert!(
        !env.iter().any(|(key, _)| key == "ANTHROPIC_BASE_URL"),
        "订阅模式不注入 BASE_URL，避免把订阅流量指到中转地址"
    );
    assert!(
        env.contains(&(
            "ANTHROPIC_MODEL".to_string(),
            "claude-sonnet-4.6".to_string()
        )),
        "模型选择仍然生效"
    );
}

#[test]
fn subscription_codex_provider_keeps_real_codex_home() {
    // P3-1：OAuth Codex 服务商不注入 OPENAI_API_KEY / OPENAI_BASE_URL，
    // 因此 adapter 不会创建临时 CODEX_HOME，codex 使用本机 ~/.codex 登录态。
    let path = temp_config_path("subscription-codex");
    let store = ProviderStore::new(path, MemorySecretStore::default());

    let mut provider = openai_provider();
    provider.auth_method = AuthMethod::OAuth;
    provider.key_ref = None;
    store.save_provider(provider, None).unwrap();
    store.save_model(codex_model()).unwrap();

    let env = store
        .launch_env(&BindingConfig {
            engine_id: "codex".to_string(),
            provider_id: "openai".to_string(),
            primary_model: "gpt-5-codex".to_string(),
            fast_model: None,
        })
        .unwrap();
    assert!(!env.iter().any(|(key, _)| key == "OPENAI_API_KEY"));
    assert!(!env.iter().any(|(key, _)| key == "OPENAI_BASE_URL"));
}

#[tokio::test]
async fn subscription_provider_without_token_is_unverified_not_successful() {
    let path = temp_config_path("subscription-unverified");
    let store = ProviderStore::new(path, MemorySecretStore::default());

    let mut provider = anthropic_provider();
    provider.auth_method = AuthMethod::OAuth;
    provider.key_ref = None;
    store.save_provider(provider, None).unwrap();

    let result = test_provider_connection(&store, "anthropic").await.unwrap();

    assert!(!result.ok, "未实际探活不能报告成功");
    assert!(!result.verified, "未读取 CLI 登录态时必须标记为未验证");
    assert!(result.message.contains("未验证"));
}

#[test]
fn api_key_provider_without_saved_key_fails_launch_env_loudly() {
    // 可靠性检查 S6：ApiKey 服务商缺密钥必须硬错误，不再静默跳过注入
    let path = temp_config_path("apikey-missing-loud");
    let store = ProviderStore::new(path, MemorySecretStore::default());

    // 不传 key：key_ref 保持 None
    store.save_provider(anthropic_provider(), None).unwrap();
    store.save_model(claude_model()).unwrap();

    let err = store
        .launch_env(&BindingConfig {
            engine_id: "claude-code".to_string(),
            provider_id: "anthropic".to_string(),
            primary_model: "claude-sonnet-4.6".to_string(),
            fast_model: None,
        })
        .unwrap_err();
    assert!(
        err.contains("还没有保存 API 密钥"),
        "错误必须指明缺密钥：{err}"
    );
}

#[test]
fn deleting_provider_removes_its_secret_from_secret_store() {
    let path = temp_config_path("delete-provider-secret");
    let store = ProviderStore::new(path, MemorySecretStore::default());

    store
        .save_provider(anthropic_provider(), Some("sk-delete-me"))
        .unwrap();
    let key_ref = store.load().unwrap().providers[0]
        .key_ref
        .clone()
        .expect("保存 API Key 后应有 key_ref");
    assert_eq!(
        store.secret(&key_ref).unwrap().as_deref(),
        Some("sk-delete-me")
    );

    store.delete_provider("anthropic").unwrap();

    assert_eq!(
        store.secret(&key_ref).unwrap(),
        None,
        "删除服务商必须清理密钥"
    );
}
