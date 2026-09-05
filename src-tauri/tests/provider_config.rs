use helm_lib::providers::{
    sync_provider_models, test_provider_connection, AppConfig, AuthMethod, BindingConfig,
    EngineConfig, EngineStatus, MemorySecretStore, ModelConfig, PriceSource, Protocol,
    ProviderConfig, ProviderKind, ProviderStore, ProviderTest, SecretStore, TestOutcome,
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
        kind: ProviderKind::Api,
        base_url: "https://api.anthropic.com".to_string(),
        key_ref: None,
        ready: true,
        last_test: None,
        protocol: Protocol::Anthropic,
        auth_method: AuthMethod::ApiKey,
        access_type: None,
        role_models: None,
        last_sync_at: None,
    }
}

fn openai_provider() -> ProviderConfig {
    ProviderConfig {
        id: "openai".to_string(),
        name: "OpenAI".to_string(),
        kind: ProviderKind::Api,
        base_url: "https://api.openai.com/v1".to_string(),
        key_ref: None,
        ready: true,
        last_test: None,
        protocol: Protocol::OpenAiResponses,
        auth_method: AuthMethod::ApiKey,
        access_type: None,
        role_models: None,
        last_sync_at: None,
    }
}

fn change_27c_binding() -> BindingConfig {
    BindingConfig {
        engine_id: "claude-code".into(),
        provider_id: "anthropic".into(),
        primary_model: "claude-sonnet-4.6".into(),
        fast_model: None,
        assistant_model_id: None,
        thinking_enabled: None,
        context_1m: None,
        reasoning_effort: None,
        revision: 0,
    }
}

fn change_27c_provider_store(name: &str) -> (std::path::PathBuf, ProviderStore<MemorySecretStore>) {
    let path = temp_config_path(name);
    let store = ProviderStore::new(path.clone(), MemorySecretStore::default());
    store.save_provider(anthropic_provider(), None).unwrap();
    store.save_model(claude_model()).unwrap();
    (path, store)
}

#[test]
fn change_27c_binding_revision_is_monotonic_persisted_and_invalidates_candidates() {
    let (path, store) = change_27c_provider_store("change-27c-binding-revision");
    let first = store.save_binding(change_27c_binding()).unwrap();
    assert_eq!(first.bindings[0].revision, 1);
    let stale = store.route_candidate().unwrap();
    let second = store.save_binding(change_27c_binding()).unwrap();
    assert_eq!(second.bindings[0].revision, 2);
    assert!(store
        .commit_route_if_unchanged(&stale.config_digest, |_| Ok(()))
        .unwrap()
        .is_none());

    let current = store.route_candidate().unwrap();
    assert!(store
        .commit_route_if_unchanged(&current.config_digest, |_| Ok("committed"))
        .unwrap()
        .is_some());
    drop(second);
    let reopened = ProviderStore::new(path, MemorySecretStore::default());
    assert_eq!(reopened.load().unwrap().bindings[0].revision, 2);
}

#[test]
fn change_27c_concurrent_binding_saves_have_one_total_order() {
    let (_, store) = change_27c_provider_store("change-27c-binding-concurrency");
    let barrier = Arc::new(std::sync::Barrier::new(9));
    let mut workers = Vec::new();
    for _ in 0..8 {
        let store = store.clone();
        let barrier = barrier.clone();
        workers.push(std::thread::spawn(move || {
            barrier.wait();
            store.save_binding(change_27c_binding()).unwrap().bindings[0].revision
        }));
    }
    barrier.wait();
    let mut revisions = workers
        .into_iter()
        .map(|worker| worker.join().unwrap())
        .collect::<Vec<_>>();
    revisions.sort_unstable();
    assert_eq!(revisions, (1..=8).collect::<Vec<_>>());
    assert_eq!(store.load().unwrap().bindings[0].revision, 8);
}

#[test]
fn change_27c_route_commit_and_binding_save_share_one_gate_order() {
    let (_, store) = change_27c_provider_store("change-27c-route-gate");
    let candidate = store.route_candidate().unwrap();
    let commit_store = store.clone();
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let commit = std::thread::spawn(move || {
        commit_store
            .commit_route_if_unchanged(&candidate.config_digest, |_| {
                entered_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                Ok("turn-committed")
            })
            .unwrap()
    });
    entered_rx.recv().unwrap();

    let save_store = store.clone();
    let (saved_tx, saved_rx) = std::sync::mpsc::channel();
    let save = std::thread::spawn(move || {
        let revision = save_store
            .save_binding(change_27c_binding())
            .unwrap()
            .bindings[0]
            .revision;
        saved_tx.send(revision).unwrap();
    });
    assert!(saved_rx
        .recv_timeout(std::time::Duration::from_millis(50))
        .is_err());
    release_tx.send(()).unwrap();
    assert_eq!(commit.join().unwrap(), Some("turn-committed"));
    assert_eq!(saved_rx.recv().unwrap(), 1);
    save.join().unwrap();
}

#[test]
fn change_27i_operation_commit_and_binding_save_freeze_the_ordered_revision() {
    let (_, store) = change_27c_provider_store("change-27i-operation-route-gate");
    let first = store.save_binding(change_27c_binding()).unwrap();
    assert_eq!(first.bindings[0].revision, 1);
    let candidate = store.route_candidate().unwrap();
    let frozen_revision = candidate.config.bindings[0].revision;

    let commit_store = store.clone();
    let candidate_digest = candidate.config_digest.clone();
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let operation_commit = std::thread::spawn(move || {
        commit_store
            .commit_route_if_unchanged(&candidate_digest, |_| {
                entered_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                Ok(frozen_revision)
            })
            .unwrap()
    });
    entered_rx.recv().unwrap();

    let save_store = store.clone();
    let (saved_tx, saved_rx) = std::sync::mpsc::channel();
    let save = std::thread::spawn(move || {
        let revision = save_store
            .save_binding(change_27c_binding())
            .unwrap()
            .bindings[0]
            .revision;
        saved_tx.send(revision).unwrap();
    });
    assert!(saved_rx
        .recv_timeout(std::time::Duration::from_millis(50))
        .is_err());
    release_tx.send(()).unwrap();
    assert_eq!(operation_commit.join().unwrap(), Some(1));
    assert_eq!(saved_rx.recv().unwrap(), 2);
    save.join().unwrap();

    assert!(store
        .commit_route_if_unchanged(&candidate.config_digest, |_| Ok(()))
        .unwrap()
        .is_none());
    let current = store.route_candidate().unwrap();
    let current_revision = current.config.bindings[0].revision;
    assert_eq!(current_revision, 2);
    assert_eq!(
        store
            .commit_route_if_unchanged(&current.config_digest, |_| Ok(current_revision))
            .unwrap(),
        Some(2)
    );
}

#[test]
fn change_27c_binding_revision_overflow_fails_without_overwriting_json() {
    let (path, store) = change_27c_provider_store("change-27c-binding-overflow");
    let mut config = store.load().unwrap();
    let mut binding = change_27c_binding();
    binding.revision = u64::MAX;
    config.bindings = vec![binding];
    store.save(&config).unwrap();
    let before = fs::read(&path).unwrap();

    let error = store.save_binding(change_27c_binding()).unwrap_err();
    assert!(error.contains("溢出"));
    assert_eq!(fs::read(path).unwrap(), before);
}

fn claude_model() -> ModelConfig {
    ModelConfig {
        id: "claude-sonnet-4.6".to_string(),
        provider_id: "anthropic".to_string(),
        display_name: "claude-sonnet-4.6".to_string(),
        input_price_per_mtok: 3.0,
        output_price_per_mtok: 15.0,
        cached_input_price_per_mtok: None,
        price_source: Some(PriceSource::Manual),
        enabled: true,
        context_window: None,
        capabilities: None,
    }
}

fn claude_fast_model() -> ModelConfig {
    ModelConfig {
        id: "claude-haiku-4.6".to_string(),
        provider_id: "anthropic".to_string(),
        display_name: "claude-haiku-4.6".to_string(),
        input_price_per_mtok: 1.0,
        output_price_per_mtok: 5.0,
        cached_input_price_per_mtok: None,
        price_source: Some(PriceSource::Manual),
        enabled: true,
        context_window: None,
        capabilities: None,
    }
}

fn codex_model() -> ModelConfig {
    ModelConfig {
        id: "gpt-5-codex".to_string(),
        provider_id: "openai".to_string(),
        display_name: "gpt-5-codex".to_string(),
        input_price_per_mtok: 1.25,
        output_price_per_mtok: 10.0,
        cached_input_price_per_mtok: None,
        price_source: Some(PriceSource::Builtin),
        enabled: true,
        context_window: None,
        capabilities: None,
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
        kind: ProviderKind::Api,
        base_url: "https://api.anthropic.com".to_string(),
        key_ref: None,
        ready: false,
        last_test: None,
        protocol: Protocol::Anthropic,
        auth_method: AuthMethod::ApiKey,
        access_type: None,
        role_models: None,
        last_sync_at: None,
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
                kind: ProviderKind::Api,
                base_url: "https://api.anthropic.com".to_string(),
                key_ref: None,
                ready: false,
                last_test: None,
                protocol: Protocol::Anthropic,
                auth_method: AuthMethod::ApiKey,
                access_type: None,
                role_models: None,
                last_sync_at: None,
            },
            Some("sk-ant-secret-value"),
        )
        .unwrap();

    store
        .save_provider(
            ProviderConfig {
                id: "anthropic".to_string(),
                name: "Anthropic".to_string(),
                kind: ProviderKind::Api,
                base_url: "https://api.anthropic.com".to_string(),
                key_ref: None,
                ready: false,
                last_test: None,
                protocol: Protocol::Anthropic,
                auth_method: AuthMethod::ApiKey,
                access_type: None,
                role_models: None,
                last_sync_at: None,
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
                kind: ProviderKind::Api,
                base_url: "https://api.anthropic.com".to_string(),
                key_ref: None,
                ready: false,
                last_test: None,
                protocol: Protocol::Anthropic,
                auth_method: AuthMethod::ApiKey,
                access_type: None,
                role_models: None,
                last_sync_at: None,
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
            kind: ProviderKind::Api,
            base_url: "https://api.anthropic.com".to_string(),
            key_ref: None,
            ready: false,
            last_test: None,
            protocol: Protocol::Anthropic,
            auth_method: AuthMethod::ApiKey,
            access_type: None,
            role_models: None,
            last_sync_at: None,
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
        cached_input_price_per_mtok: None,
        price_source: Some(PriceSource::Manual),
        enabled: false,
        context_window: None,
        capabilities: None,
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
fn parse_synced_models_prices_all_gpt_56_models_from_offline_catalog() {
    let models = helm_lib::providers::models_from_provider_response(
        &Protocol::OpenAiResponses,
        "gateway",
        r#"{"data":[{"id":"gpt-5.6-sol"},{"id":"gpt-5.6-terra"},{"id":"gpt-5.6-luna"}]}"#,
        &[],
    )
    .unwrap();

    let prices = models
        .iter()
        .map(|model| {
            (
                model.id.as_str(),
                model.input_price_per_mtok,
                model.output_price_per_mtok,
                model.price_source.clone(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        prices,
        vec![
            ("gpt-5.6-sol", 5.0, 30.0, Some(PriceSource::Builtin)),
            ("gpt-5.6-terra", 2.5, 15.0, Some(PriceSource::Builtin)),
            ("gpt-5.6-luna", 1.0, 6.0, Some(PriceSource::Builtin)),
        ]
    );
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
    assert_eq!(known.input_price_per_mtok, 5.0);
    assert_eq!(known.output_price_per_mtok, 30.0);
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
fn load_deduplicates_legacy_models_and_preserves_enabled_entry() {
    let path = temp_config_path("dedupe-legacy-models");
    fs::write(
        &path,
        r#"{
  "providers": [{
    "id": "openai",
    "name": "OpenAI",
    "baseUrl": "https://api.openai.com/v1",
    "keyRef": null,
    "ready": false,
    "lastTest": null,
    "protocol": "openai-responses",
    "authMethod": "apikey"
  }],
  "models": [
    {
      "id": "gpt-5.4-mini",
      "providerId": "openai",
      "displayName": "disabled duplicate",
      "inputPricePerMtok": 0.0,
      "outputPricePerMtok": 0.0,
      "enabled": false
    },
    {
      "id": "gpt-5.4-mini",
      "providerId": "openai",
      "displayName": "enabled duplicate",
      "inputPricePerMtok": 0.0,
      "outputPricePerMtok": 0.0,
      "enabled": true
    }
  ],
  "engines": [],
  "bindings": [],
  "defaultEngine": "codex",
  "defaultModel": "gpt-5.4-mini"
}"#,
    )
    .unwrap();
    let store = ProviderStore::new(path, MemorySecretStore::default());

    let loaded = store.load().unwrap();
    let models = loaded
        .models
        .iter()
        .filter(|model| model.provider_id == "openai" && model.id == "gpt-5.4-mini")
        .collect::<Vec<_>>();

    assert_eq!(models.len(), 1);
    assert!(models[0].enabled);
    assert_eq!(models[0].display_name, "enabled duplicate");
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
                kind: ProviderKind::Api,
                base_url: "https://api.example.com/v1".to_string(),
                key_ref: None,
                ready: false,
                last_test: None,
                protocol: Protocol::OpenAiResponses,
                auth_method: AuthMethod::ApiKey,
                access_type: None,
                role_models: None,
                last_sync_at: None,
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
                failure_category: None,
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
            env_vars: None,
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
            cached_input_price_per_mtok: None,
            price_source: Some(PriceSource::Manual),
            enabled: false,
            context_window: None,
            capabilities: None,
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
                kind: ProviderKind::Api,
                base_url: "https://api.example.com".to_string(),
                key_ref: None,
                ready: false,
                last_test: None,
                protocol: Protocol::Anthropic,
                auth_method: AuthMethod::ApiKey,
                access_type: None,
                role_models: None,
                last_sync_at: None,
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
            cached_input_price_per_mtok: None,
            price_source: Some(PriceSource::Manual),
            enabled: true,
            context_window: None,
            capabilities: None,
        })
        .unwrap();
    store
        .save_provider(
            ProviderConfig {
                id: "fallback".to_string(),
                name: "Fallback".to_string(),
                kind: ProviderKind::Local,
                base_url: "http://localhost:11434/v1".to_string(),
                key_ref: None,
                ready: false,
                last_test: None,
                protocol: Protocol::OpenAiChat,
                auth_method: AuthMethod::Local,
                access_type: None,
                role_models: None,
                last_sync_at: None,
            },
            None,
        )
        .unwrap();
    store
        .save_model(ModelConfig {
            id: "fallback-model".to_string(),
            provider_id: "fallback".to_string(),
            display_name: "fallback-model".to_string(),
            input_price_per_mtok: 0.0,
            output_price_per_mtok: 0.0,
            cached_input_price_per_mtok: None,
            price_source: Some(PriceSource::Manual),
            enabled: true,
            context_window: None,
            capabilities: None,
        })
        .unwrap();
    store
        .save_binding(BindingConfig {
            engine_id: "codex".to_string(),
            provider_id: "fallback".to_string(),
            primary_model: "fallback-model".to_string(),
            fast_model: None,
            assistant_model_id: None,
            thinking_enabled: None,
            context_1m: None,
            reasoning_effort: None,
            revision: 0,
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
        assistant_model_id: None,
        thinking_enabled: None,
        context_1m: None,
        reasoning_effort: None,
        revision: 0,
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
        assistant_model_id: None,
        thinking_enabled: None,
        context_1m: None,
        reasoning_effort: None,
        revision: 0,
    });
    assert!(wrong_provider.is_err());

    store
        .save_model(ModelConfig {
            id: "gpt-disabled".to_string(),
            provider_id: "openai".to_string(),
            display_name: "gpt-disabled".to_string(),
            input_price_per_mtok: 1.0,
            output_price_per_mtok: 2.0,
            cached_input_price_per_mtok: None,
            price_source: Some(PriceSource::Manual),
            enabled: false,
            context_window: None,
            capabilities: None,
        })
        .unwrap();
    let disabled = store.save_binding(BindingConfig {
        engine_id: "codex".to_string(),
        provider_id: "openai".to_string(),
        primary_model: "gpt-disabled".to_string(),
        fast_model: None,
        assistant_model_id: None,
        thinking_enabled: None,
        context_1m: None,
        reasoning_effort: None,
        revision: 0,
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
            assistant_model_id: None,
            thinking_enabled: None,
            context_1m: None,
            reasoning_effort: None,
            revision: 0,
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
                cached_input_price_per_mtok: None,
                price_source: None,
                enabled: true,
                context_window: None,
                capabilities: None,
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
                cached_input_price_per_mtok: None,
                price_source: None,
                enabled: true,
                context_window: None,
                capabilities: None,
            }],
        )
        .unwrap();

    let loaded = store
        .save_binding(BindingConfig {
            engine_id: "codex".to_string(),
            provider_id: "gateway-b".to_string(),
            primary_model: "gpt-5.5".to_string(),
            fast_model: Some("gpt-5.5".to_string()),
            assistant_model_id: None,
            thinking_enabled: None,
            context_1m: None,
            reasoning_effort: None,
            revision: 0,
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
            assistant_model_id: None,
            thinking_enabled: None,
            context_1m: None,
            reasoning_effort: None,
            revision: 0,
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
            assistant_model_id: None,
            thinking_enabled: None,
            context_1m: None,
            reasoning_effort: None,
            revision: 0,
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
                role_models: None,
                last_sync_at: None,
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
        assistant_model_id: None,
        thinking_enabled: None,
        context_1m: None,
        reasoning_effort: None,
        revision: 0,
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
                kind: ProviderKind::Api,
                base_url: "https://api.example.com".to_string(),
                key_ref: None,
                ready: true,
                last_test: None,
                protocol: Protocol::OpenAiResponses,
                auth_method: AuthMethod::ApiKey,
                access_type: None,
                role_models: None,
                last_sync_at: None,
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
            cached_input_price_per_mtok: None,
            price_source: Some(PriceSource::Builtin),
            enabled: true,
            context_window: None,
            capabilities: None,
        })
        .unwrap();

    let env = store
        .launch_env(&BindingConfig {
            engine_id: "codex".to_string(),
            provider_id: "custom".to_string(),
            primary_model: "gpt-5-codex".to_string(),
            fast_model: None,
            assistant_model_id: None,
            thinking_enabled: None,
            context_1m: None,
            reasoning_effort: None,
            revision: 0,
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
            assistant_model_id: None,
            thinking_enabled: None,
            context_1m: None,
            reasoning_effort: None,
            revision: 0,
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
fn subscription_codex_provider_leaves_profile_selection_to_the_runtime() {
    // Provider launch env 只负责官方订阅路由，不注入 API 认证；运行时再把
    // Helm-owned 持久 CODEX_HOME 加入进程环境。
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
            assistant_model_id: None,
            thinking_enabled: None,
            context_1m: None,
            reasoning_effort: None,
            revision: 0,
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

#[tokio::test]
async fn claude_subscription_model_sync_restores_official_aliases_without_provider_credentials() {
    let path = temp_config_path("subscription-model-sync");
    let store = ProviderStore::new(path, MemorySecretStore::default());

    let mut provider = anthropic_provider();
    provider.kind = ProviderKind::Subscription;
    provider.auth_method = AuthMethod::OAuth;
    provider.key_ref = None;
    store.save_provider(provider, None).unwrap();

    let loaded = sync_provider_models(&store, "anthropic").await.unwrap();
    let models: Vec<_> = loaded
        .models
        .iter()
        .filter(|model| model.provider_id == "anthropic")
        .collect();
    assert_eq!(models.len(), 5);
    assert_eq!(models[0].id, "default");
    assert_eq!(models[1].id, "best");
    assert!(models
        .iter()
        .all(|model| model.price_source == Some(PriceSource::Subscription)));
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
            assistant_model_id: None,
            thinking_enabled: None,
            context_1m: None,
            reasoning_effort: None,
            revision: 0,
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

#[test]
fn deleting_bound_provider_is_rejected_without_mutating_config_or_secret() {
    let path = temp_config_path("delete-bound-provider");
    let store = ProviderStore::new(path, MemorySecretStore::default());
    let config = store
        .save_provider(anthropic_provider(), Some("sk-bound-provider"))
        .unwrap();
    let key_ref = config.providers[0].key_ref.clone().unwrap();
    store.save_model(claude_model()).unwrap();
    store
        .save_binding(BindingConfig {
            engine_id: "claude-code".to_string(),
            provider_id: "anthropic".to_string(),
            primary_model: "claude-sonnet-4.6".to_string(),
            fast_model: None,
            assistant_model_id: None,
            thinking_enabled: None,
            context_1m: None,
            reasoning_effort: None,
            revision: 0,
        })
        .unwrap();

    let error = store.delete_provider("anthropic").unwrap_err();

    assert!(error.contains("Claude Code"));
    let config = store.load().unwrap();
    assert!(config
        .providers
        .iter()
        .any(|provider| provider.id == "anthropic"));
    assert!(config
        .bindings
        .iter()
        .any(|binding| binding.provider_id == "anthropic"));
    assert_eq!(
        store.secret(&key_ref).unwrap().as_deref(),
        Some("sk-bound-provider")
    );
}

#[test]
fn old_provider_config_derives_provider_kind() {
    let path = temp_config_path("provider-kind-migration");
    fs::write(
        &path,
        r#"{
          "defaultEngine": "claude-code",
          "defaultModel": "",
          "providers": [
            {"id":"subscription","name":"Claude 订阅","baseUrl":"https://api.anthropic.com","keyRef":null,"ready":true,"lastTest":null,"protocol":"anthropic","authMethod":"oauth"},
            {"id":"api","name":"Anthropic API","baseUrl":"https://api.anthropic.com","keyRef":"helm:provider:api:api-key","ready":true,"lastTest":null,"protocol":"anthropic","authMethod":"apikey"},
            {"id":"local","name":"Ollama","baseUrl":"http://localhost:11434/v1","keyRef":null,"ready":true,"lastTest":null,"protocol":"openai-chat","authMethod":"local"}
          ],
          "models": [],
          "engines": [],
          "bindings": []
        }"#,
    )
    .unwrap();
    let store = ProviderStore::new(path, MemorySecretStore::default());

    let config = store.load().unwrap();

    assert_eq!(config.providers[0].kind, ProviderKind::Subscription);
    assert_eq!(config.providers[1].kind, ProviderKind::Api);
    assert_eq!(config.providers[2].kind, ProviderKind::Local);
}

#[test]
fn subscription_price_source_round_trips() {
    let value = serde_json::to_value(PriceSource::Subscription).unwrap();
    assert_eq!(value, serde_json::json!("subscription"));
    assert_eq!(
        serde_json::from_value::<PriceSource>(value).unwrap(),
        PriceSource::Subscription
    );
}

#[test]
fn saving_claude_subscription_seeds_engine_models() {
    let path = temp_config_path("claude-subscription-models");
    let store = ProviderStore::new(path, MemorySecretStore::default());
    let mut provider = anthropic_provider();
    provider.kind = ProviderKind::Subscription;
    provider.auth_method = AuthMethod::OAuth;

    let config = store.save_provider(provider, None).unwrap();

    let models: Vec<_> = config
        .models
        .iter()
        .filter(|model| model.provider_id == "anthropic")
        .collect();
    assert_eq!(
        models
            .iter()
            .map(|model| model.id.as_str())
            .collect::<Vec<_>>(),
        vec!["default", "best", "sonnet", "opus", "haiku"]
    );
    assert!(models.iter().all(|model| {
        model.enabled
            && model.input_price_per_mtok == 0.0
            && model.output_price_per_mtok == 0.0
            && model.price_source == Some(PriceSource::Subscription)
    }));
}

#[test]
fn saving_codex_subscription_waits_for_account_model_discovery() {
    let path = temp_config_path("codex-subscription-models");
    let store = ProviderStore::new(path, MemorySecretStore::default());
    let mut provider = openai_provider();
    provider.kind = ProviderKind::Subscription;
    provider.auth_method = AuthMethod::OAuth;

    let config = store.save_provider(provider, None).unwrap();

    let ids: Vec<_> = config
        .models
        .iter()
        .filter(|model| model.provider_id == "openai")
        .map(|model| model.id.as_str())
        .collect();
    assert!(
        ids.is_empty(),
        "ChatGPT 订阅模型必须来自当前账号 model/list，保存 Provider 时不能伪造固定目录"
    );
}

#[test]
fn saving_codex_subscription_preserves_discovered_models_and_enablement() {
    let path = temp_config_path("codex-subscription-preserves-models");
    let store = ProviderStore::new(path, MemorySecretStore::default());
    let mut provider = openai_provider();
    provider.kind = ProviderKind::Subscription;
    provider.auth_method = AuthMethod::OAuth;
    store.save_provider(provider.clone(), None).unwrap();

    let mut terra = codex_model();
    terra.id = "gpt-5.6-terra".to_string();
    terra.display_name = "GPT-5.6-Terra".to_string();
    terra.price_source = Some(PriceSource::Subscription);
    let mut luna = terra.clone();
    luna.id = "gpt-5.6-luna".to_string();
    luna.display_name = "GPT-5.6-Luna".to_string();
    store
        .save_models_for_provider("openai", vec![terra.clone(), luna.clone()])
        .unwrap();
    store
        .save_provider_model_selection("openai", &[terra.id.clone()])
        .unwrap();

    let saved = store.save_provider(provider, None).unwrap();
    let models = saved
        .models
        .iter()
        .filter(|model| model.provider_id == "openai")
        .collect::<Vec<_>>();
    assert_eq!(models.len(), 2);
    assert!(
        models
            .iter()
            .find(|model| model.id == terra.id)
            .unwrap()
            .enabled
    );
    assert!(
        !models
            .iter()
            .find(|model| model.id == luna.id)
            .unwrap()
            .enabled
    );

    let refreshed = store
        .save_models_for_provider("openai", vec![terra, luna])
        .unwrap();
    assert!(
        !refreshed
            .models
            .iter()
            .find(|model| model.id == "gpt-5.6-luna")
            .unwrap()
            .enabled
    );
}

#[test]
fn model_selection_cannot_disable_models_used_by_binding() {
    let path = temp_config_path("model-selection-binding-protection");
    let store = ProviderStore::new(path, MemorySecretStore::default());
    store.save_provider(openai_provider(), None).unwrap();
    let primary = codex_model();
    let mut fast = primary.clone();
    fast.id = "gpt-5-mini".to_string();
    fast.display_name = "gpt-5-mini".to_string();
    store.save_model(primary.clone()).unwrap();
    store.save_model(fast.clone()).unwrap();
    store
        .save_binding(BindingConfig {
            engine_id: "codex".to_string(),
            provider_id: "openai".to_string(),
            primary_model: primary.id.clone(),
            fast_model: Some(fast.id.clone()),
            assistant_model_id: None,
            thinking_enabled: None,
            context_1m: None,
            reasoning_effort: None,
            revision: 0,
        })
        .unwrap();

    // 2026-09-03：在用模型允许改/关，绑定自动改到仍启用的那条，不再报「请先更改引擎绑定」。
    let retarget_primary = store
        .save_provider_model_selection("openai", &[fast.id.clone()])
        .unwrap();
    let binding = retarget_primary
        .bindings
        .iter()
        .find(|binding| binding.engine_id == "codex")
        .unwrap();
    assert_eq!(binding.primary_model, fast.id);
    assert_eq!(binding.fast_model.as_deref(), Some(fast.id.as_str()));

    store
        .save_binding(BindingConfig {
            engine_id: "codex".to_string(),
            provider_id: "openai".to_string(),
            primary_model: primary.id.clone(),
            fast_model: Some(fast.id.clone()),
            assistant_model_id: None,
            thinking_enabled: None,
            context_1m: None,
            reasoning_effort: None,
            revision: 0,
        })
        .unwrap();
    let retarget_fast = store
        .save_provider_model_selection("openai", &[primary.id.clone()])
        .unwrap();
    let binding = retarget_fast
        .bindings
        .iter()
        .find(|binding| binding.engine_id == "codex")
        .unwrap();
    assert_eq!(binding.primary_model, primary.id);
    assert_eq!(binding.fast_model.as_deref(), Some(primary.id.as_str()));

    let saved = store
        .save_provider_model_selection("openai", &[primary.id, fast.id])
        .unwrap();
    assert!(saved
        .models
        .iter()
        .filter(|model| model.provider_id == "openai")
        .all(|model| model.enabled));
}

#[test]
fn api_provider_does_not_receive_subscription_models() {
    let path = temp_config_path("api-no-subscription-models");
    let store = ProviderStore::new(path, MemorySecretStore::default());

    let config = store
        .save_provider(anthropic_provider(), Some("sk-api-only"))
        .unwrap();

    assert!(config.models.is_empty());
}

#[test]
fn subscription_save_clears_stale_key_ref_and_secret() {
    let path = temp_config_path("subscription-clears-key");
    let store = ProviderStore::new(path, MemorySecretStore::default());
    let config = store
        .save_provider(anthropic_provider(), Some("sk-stale-api-key"))
        .unwrap();
    let key_ref = config.providers[0].key_ref.clone().unwrap();

    let mut provider = config.providers[0].clone();
    provider.kind = ProviderKind::Subscription;
    provider.auth_method = AuthMethod::OAuth;
    let config = store.save_provider(provider, None).unwrap();

    assert_eq!(config.providers[0].key_ref, None);
    assert_eq!(store.secret(&key_ref).unwrap(), None);
}

#[test]
fn subscription_save_clears_legacy_base_url_and_rejects_same_protocol_duplicate() {
    let path = temp_config_path("subscription-normalization");
    let store = ProviderStore::new(path, MemorySecretStore::default());
    let mut provider = anthropic_provider();
    provider.kind = ProviderKind::Subscription;
    provider.auth_method = AuthMethod::OAuth;
    provider.base_url = "https://legacy.example.com".to_string();

    let config = store.save_provider(provider, None).unwrap();
    assert_eq!(config.providers[0].base_url, "");
    assert!(config.providers[0].ready);

    let mut duplicate = anthropic_provider();
    duplicate.id = "claude-subscription-2".to_string();
    duplicate.name = "另一个 Claude 订阅".to_string();
    duplicate.kind = ProviderKind::Subscription;
    duplicate.auth_method = AuthMethod::OAuth;
    let error = store.save_provider(duplicate, None).unwrap_err();
    assert!(error.contains("已存在，请直接复用"));
}

#[test]
fn subscription_equivalent_and_launch_env_do_not_claim_or_inject_secret() {
    let path = temp_config_path("subscription-env-boundary");
    let store = ProviderStore::new(path, MemorySecretStore::default());
    let mut provider = anthropic_provider();
    provider.kind = ProviderKind::Subscription;
    provider.auth_method = AuthMethod::OAuth;
    store.save_provider(provider, None).unwrap();
    let binding = BindingConfig {
        engine_id: "claude-code".to_string(),
        provider_id: "anthropic".to_string(),
        primary_model: "sonnet".to_string(),
        fast_model: Some("haiku".to_string()),
        assistant_model_id: None,
        thinking_enabled: None,
        context_1m: None,
        reasoning_effort: None,
        revision: 0,
    };

    let equivalent = store.equivalent_env(&binding).unwrap();
    let launch = store.launch_env(&binding).unwrap();

    for pairs in [&equivalent, &launch] {
        assert!(!pairs.iter().any(|(key, _)| key == "ANTHROPIC_BASE_URL"));
        assert!(!pairs.iter().any(|(key, _)| key == "ANTHROPIC_AUTH_TOKEN"));
    }
    assert!(equivalent
        .iter()
        .any(|(key, value)| { key == "# 凭证" && value.contains("Helm 独立订阅 Profile") }));
}
