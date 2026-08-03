use crate::pricing::{
    PricingBand, PricingCatalog, PricingCatalogStore, PricingTier, ProviderPricingMode,
    ResolvedPricingProfile, ServiceTier,
};
use crate::reasoning::ReasoningEffort;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio::process::Command;

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const KEYRING_SERVICE: &str = "Helm";

/// temp+rename 原子写：写一半崩溃或磁盘满都不会留下损坏的配置文件。
/// （Windows 上 `fs::rename` 走 MOVEFILE_REPLACE_EXISTING，可覆盖已存在的目标。）
pub(crate) fn write_atomically(path: &Path, contents: &str) -> Result<(), String> {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, contents).map_err(|e| format!("写入配置失败：{e}"))?;
    fs::rename(&tmp, path).map_err(|e| format!("写入配置失败（替换文件）：{e}"))
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum EngineStatus {
    Ready,
    Missing,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum Protocol {
    #[serde(rename = "anthropic")]
    Anthropic,
    #[serde(rename = "openai-responses")]
    OpenAiResponses,
    #[serde(rename = "openai-chat")]
    OpenAiChat,
    #[serde(rename = "bedrock")]
    Bedrock,
    #[serde(rename = "vertex")]
    Vertex,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum AuthMethod {
    #[serde(rename = "apikey")]
    ApiKey,
    #[serde(rename = "oauth")]
    OAuth,
    #[serde(rename = "cloud")]
    Cloud,
    #[serde(rename = "local")]
    Local,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ProviderKind {
    Subscription,
    Api,
    Local,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TestOutcome {
    Ok,
    Fail,
    Unverified,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum FailureCategory {
    Network,
    Auth,
    Timeout,
    Unknown,
}

/// 从错误消息文本推断失败分类。
pub fn classify_failure(message: &str, ok: bool, verified: bool) -> Option<FailureCategory> {
    if ok || !verified {
        return None;
    }
    let lower = message.to_lowercase();
    if lower.contains("timed out") || lower.contains("timeout") || lower.contains("超时") {
        Some(FailureCategory::Timeout)
    } else if lower.contains("401")
        || lower.contains("403")
        || lower.contains("密钥")
        || lower.contains("unauthorized")
        || lower.contains("forbidden")
        || lower.contains("api key")
    {
        Some(FailureCategory::Auth)
    } else if lower.contains("connection")
        || lower.contains("connect")
        || lower.contains("dns")
        || lower.contains("network")
        || lower.contains("http")
        || lower.contains("网络")
    {
        Some(FailureCategory::Network)
    } else {
        Some(FailureCategory::Unknown)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProviderTest {
    pub result: TestOutcome,
    pub latency_ms: Option<u128>,
    pub at: i64,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub failure_category: Option<FailureCategory>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderConfig {
    pub id: String,
    pub name: String,
    #[serde(default = "default_provider_kind")]
    pub kind: ProviderKind,
    pub base_url: String,
    pub key_ref: Option<String>,
    #[serde(default)]
    pub ready: bool,
    #[serde(default)]
    pub last_test: Option<ProviderTest>,
    #[serde(default = "default_provider_protocol")]
    pub protocol: Protocol,
    #[serde(default = "default_auth_method")]
    pub auth_method: AuthMethod,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelConfig {
    pub id: String,
    pub provider_id: String,
    pub display_name: String,
    pub input_price_per_mtok: f64,
    pub output_price_per_mtok: f64,
    #[serde(default)]
    pub price_source: Option<PriceSource>,
    pub enabled: bool,
    /// 模型上下文窗口大小（token 数），上游无数据时为 None
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
    /// 模型能力标签列表，上游无数据时为 None
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PriceSource {
    Provider,
    Builtin,
    Manual,
    Subscription,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineConfig {
    pub id: String,
    pub name: String,
    pub bin: String,
    pub default_model: String,
    pub status: EngineStatus,
    pub version: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BindingConfig {
    pub engine_id: String,
    pub provider_id: String,
    pub primary_model: String,
    pub fast_model: Option<String>,
    /// 旧配置迁移输入；读取后迁入 fast_model，保存时清理。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assistant_model_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<ReasoningEffort>,
    #[serde(default)]
    pub revision: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppConfig {
    pub providers: Vec<ProviderConfig>,
    pub models: Vec<ModelConfig>,
    pub engines: Vec<EngineConfig>,
    #[serde(default)]
    pub bindings: Vec<BindingConfig>,
    pub default_engine: String,
    pub default_model: String,
}

impl AppConfig {
    pub fn engine_bin(&self, engine_id: &str) -> Option<&str> {
        self.engines
            .iter()
            .find(|engine| engine.id == engine_id)
            .map(|engine| engine.bin.as_str())
    }
}

fn default_provider_protocol() -> Protocol {
    Protocol::Anthropic
}

fn default_auth_method() -> AuthMethod {
    AuthMethod::ApiKey
}

fn default_provider_kind() -> ProviderKind {
    ProviderKind::Api
}

fn refresh_provider_readiness(config: &mut AppConfig) {
    for provider in &mut config.providers {
        provider.ready = provider_is_ready(provider);
    }
}

fn provider_is_ready(provider: &ProviderConfig) -> bool {
    if provider.id.trim().is_empty() || provider.name.trim().is_empty() {
        return false;
    }
    if !matches!(provider.kind, ProviderKind::Subscription) && provider.base_url.trim().is_empty() {
        return false;
    }
    match provider.auth_method {
        AuthMethod::ApiKey => provider
            .key_ref
            .as_deref()
            .is_some_and(|key_ref| !key_ref.trim().is_empty()),
        // 订阅登录一等公民（P3-1）：凭证在 CLI 自己的登录态里（claude login / codex login），
        // 不需要在 Helm 里存 Key，配置完整即视为就绪；登录态检测在就绪度报告里单独呈现。
        AuthMethod::OAuth | AuthMethod::Cloud | AuthMethod::Local => true,
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionResult {
    pub ok: bool,
    pub verified: bool,
    pub message: String,
    pub latency_ms: u128,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EngineConfigFile {
    pub path: PathBuf,
    pub content: String,
}

pub trait SecretStore: Clone + Send + Sync + 'static {
    fn set(&self, key_ref: &str, secret: &str) -> Result<(), String>;
    fn get(&self, key_ref: &str) -> Result<Option<String>, String>;
    fn delete(&self, key_ref: &str) -> Result<(), String>;
}

#[derive(Clone, Default)]
pub struct MemorySecretStore {
    values: Arc<Mutex<HashMap<String, String>>>,
}

impl SecretStore for MemorySecretStore {
    fn set(&self, key_ref: &str, secret: &str) -> Result<(), String> {
        self.values
            .lock()
            .map_err(|_| "测试密钥存储锁中毒".to_string())?
            .insert(key_ref.to_string(), secret.to_string());
        Ok(())
    }

    fn get(&self, key_ref: &str) -> Result<Option<String>, String> {
        Ok(self
            .values
            .lock()
            .map_err(|_| "测试密钥存储锁中毒".to_string())?
            .get(key_ref)
            .cloned())
    }

    fn delete(&self, key_ref: &str) -> Result<(), String> {
        self.values
            .lock()
            .map_err(|_| "测试密钥存储锁中毒".to_string())?
            .remove(key_ref);
        Ok(())
    }
}

#[derive(Clone, Default)]
pub struct KeyringSecretStore;

impl SecretStore for KeyringSecretStore {
    fn set(&self, key_ref: &str, secret: &str) -> Result<(), String> {
        let entry = keyring::Entry::new(KEYRING_SERVICE, key_ref)
            .map_err(|e| format!("打开钥匙串失败：{e}"))?;
        entry
            .set_password(secret)
            .map_err(|e| format!("写入钥匙串失败：{e}"))
    }

    fn get(&self, key_ref: &str) -> Result<Option<String>, String> {
        let entry = keyring::Entry::new(KEYRING_SERVICE, key_ref)
            .map_err(|e| format!("打开钥匙串失败：{e}"))?;
        match entry.get_password() {
            Ok(secret) => Ok(Some(secret)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(format!("读取钥匙串失败：{e}")),
        }
    }

    fn delete(&self, key_ref: &str) -> Result<(), String> {
        let entry = keyring::Entry::new(KEYRING_SERVICE, key_ref)
            .map_err(|e| format!("打开钥匙串失败：{e}"))?;
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(format!("删除钥匙串密钥失败：{e}")),
        }
    }
}

#[derive(Clone)]
pub struct ProviderStore<S: SecretStore> {
    path: PathBuf,
    secrets: S,
    gate: Arc<Mutex<ProviderGateState>>,
}

#[derive(Default)]
struct ProviderGateState {
    published: Option<AppConfig>,
}

#[derive(Debug, Clone)]
pub struct RouteCandidate {
    pub config: AppConfig,
    pub config_digest: String,
}

impl<S: SecretStore> ProviderStore<S> {
    pub fn new(path: PathBuf, secrets: S) -> Self {
        Self {
            path,
            secrets,
            gate: Arc::new(Mutex::new(ProviderGateState::default())),
        }
    }

    pub fn load(&self) -> Result<AppConfig, String> {
        let mut gate = self
            .gate
            .lock()
            .map_err(|_| "ProviderStore 闸门锁中毒".to_string())?;
        self.load_locked(&mut gate)
    }

    fn load_locked(&self, gate: &mut ProviderGateState) -> Result<AppConfig, String> {
        if let Some(config) = &gate.published {
            return Ok(config.clone());
        }
        let config = self.load_from_disk()?;
        gate.published = Some(config.clone());
        Ok(config)
    }

    fn load_from_disk(&self) -> Result<AppConfig, String> {
        if !self.path.exists() {
            return Ok(seed_config());
        }
        let raw = fs::read_to_string(&self.path).map_err(|e| format!("读取服务商配置失败：{e}"))?;
        let mut value: serde_json::Value = match serde_json::from_str(&raw) {
            Ok(value) => value,
            Err(e) => {
                // 配置文件损坏：备份后重建默认配置，而不是让整个应用瘫在这里。
                let backup = self.path.with_extension("json.corrupt.bak");
                let _ = fs::copy(&self.path, &backup);
                return Err(format!(
                    "服务商配置文件损坏（{e}）。已备份到 {}，删除 {} 后重启可重建默认配置。",
                    backup.display(),
                    self.path.display()
                ));
            }
        };
        apply_config_defaults(&mut value);
        let mut config: AppConfig =
            serde_json::from_value(value).map_err(|e| format!("解析服务商配置失败：{e}"))?;
        refresh_provider_readiness(&mut config);
        let pricing_store = PricingCatalogStore::new(
            self.path
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .to_path_buf(),
        );
        let catalog = pricing_store
            .active_catalog()
            .map(|(catalog, _)| catalog)
            .or_else(|_| PricingCatalog::builtin())?;
        apply_catalog_model_pricing(&mut config, &catalog, Some(&pricing_store));
        ensure_subscription_model_catalogs(&mut config);
        deduplicate_models(&mut config.models);
        migrate_bindings(&mut config);
        Ok(config)
    }

    pub fn save(&self, config: &AppConfig) -> Result<(), String> {
        let mut gate = self
            .gate
            .lock()
            .map_err(|_| "ProviderStore 闸门锁中毒".to_string())?;
        self.save_locked(&mut gate, config)
    }

    fn save_locked(&self, gate: &mut ProviderGateState, config: &AppConfig) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("创建配置目录失败：{e}"))?;
        }
        let raw = serde_json::to_string_pretty(config)
            .map_err(|e| format!("序列化服务商配置失败：{e}"))?;
        write_atomically(&self.path, &raw)?;
        gate.published = Some(config.clone());
        Ok(())
    }

    pub fn route_candidate(&self) -> Result<RouteCandidate, String> {
        let config = self.load()?;
        let config_digest = crate::turn_start::digest_json(&config)?;
        Ok(RouteCandidate {
            config,
            config_digest,
        })
    }

    pub fn commit_route_if_unchanged<T>(
        &self,
        expected_config_digest: &str,
        commit: impl FnOnce(&AppConfig) -> Result<T, String>,
    ) -> Result<Option<T>, String> {
        let mut gate = self
            .gate
            .lock()
            .map_err(|_| "ProviderStore 闸门锁中毒".to_string())?;
        let config = self.load_locked(&mut gate)?;
        if crate::turn_start::digest_json(&config)? != expected_config_digest {
            return Ok(None);
        }
        commit(&config).map(Some)
    }

    pub fn model_pricing_profile(
        &self,
        config: &AppConfig,
        model: &ModelConfig,
    ) -> Result<Option<ResolvedPricingProfile>, String> {
        let provider = config
            .providers
            .iter()
            .find(|provider| provider.id == model.provider_id)
            .ok_or_else(|| format!("找不到模型所属服务商：{}", model.provider_id))?;
        if matches!(provider.kind, ProviderKind::Subscription)
            || matches!(model.price_source, Some(PriceSource::Subscription))
        {
            return Ok(Some(simple_pricing_profile(
                "subscription",
                "subscription",
                0.0,
                0.0,
            )));
        }
        let pricing_store = PricingCatalogStore::new(
            self.path
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .to_path_buf(),
        );
        let (catalog, _) = pricing_store.active_catalog()?;
        if let Some(profile) = pricing_store.profile_for_provider(
            &catalog,
            &provider.protocol,
            &provider.id,
            &model.id,
        )? {
            return Ok(Some(profile));
        }
        Ok(match model.price_source {
            Some(PriceSource::Manual) => Some(simple_pricing_profile(
                "manual:legacy",
                "manual",
                model.input_price_per_mtok,
                model.output_price_per_mtok,
            )),
            Some(PriceSource::Provider) => Some(simple_pricing_profile(
                "provider",
                "provider",
                model.input_price_per_mtok,
                model.output_price_per_mtok,
            )),
            _ => None,
        })
    }

    pub fn save_provider(
        &self,
        mut provider: ProviderConfig,
        api_key: Option<&str>,
    ) -> Result<AppConfig, String> {
        let mut gate = self
            .gate
            .lock()
            .map_err(|_| "ProviderStore 闸门锁中毒".to_string())?;
        let mut config = self.load_locked(&mut gate)?;
        let previous_key_ref = config
            .providers
            .iter()
            .find(|existing| existing.id == provider.id)
            .and_then(|existing| existing.key_ref.clone());
        if matches!(provider.auth_method, AuthMethod::OAuth) {
            provider.kind = ProviderKind::Subscription;
        } else if matches!(provider.auth_method, AuthMethod::Local) {
            provider.kind = ProviderKind::Local;
        }
        if matches!(provider.kind, ProviderKind::Subscription) {
            if let Some(existing) = config.providers.iter().find(|existing| {
                existing.id != provider.id
                    && matches!(existing.kind, ProviderKind::Subscription)
                    && existing.protocol == provider.protocol
            }) {
                return Err(format!("同协议订阅「{}」已存在，请直接复用", existing.name));
            }
            provider.auth_method = AuthMethod::OAuth;
            provider.base_url.clear();
            provider.key_ref = None;
            provider.last_test = None;
            let previous_secret = previous_key_ref
                .as_deref()
                .map(|key_ref| self.secret(key_ref))
                .transpose()?
                .flatten();
            if let Some(key_ref) = previous_key_ref.as_deref() {
                self.secrets.delete(key_ref)?;
            }
            provider.ready = provider_is_ready(&provider);
            upsert_by_id(&mut config.providers, provider.clone(), |item| &item.id);
            let seeded_models = subscription_models_for_provider(&provider);
            if !seeded_models.is_empty() {
                let enabled_by_id = config
                    .models
                    .iter()
                    .filter(|model| model.provider_id == provider.id)
                    .map(|model| (model.id.clone(), model.enabled))
                    .collect::<HashMap<_, _>>();
                config
                    .models
                    .retain(|model| model.provider_id != provider.id);
                config
                    .models
                    .extend(seeded_models.into_iter().map(|mut model| {
                        if let Some(enabled) = enabled_by_id.get(&model.id) {
                            model.enabled = *enabled;
                        }
                        model
                    }));
            }
            if let Err(save_error) = self.save_locked(&mut gate, &config) {
                if let (Some(key_ref), Some(secret)) =
                    (previous_key_ref.as_deref(), previous_secret.as_deref())
                {
                    let _ = self.secrets.set(key_ref, secret);
                }
                return Err(save_error);
            }
            return Ok(config);
        }
        if let Some(secret) = api_key.filter(|secret| !secret.is_empty()) {
            let key_ref = key_ref_for_provider(&provider.id);
            self.secrets.set(&key_ref, secret)?;
            if self.secrets.get(&key_ref)?.is_none() {
                return Err("API 密钥写入后无法从钥匙串读回，请重新保存".to_string());
            }
            provider.key_ref = Some(key_ref);
        } else if provider.key_ref.is_none() {
            provider.key_ref = config
                .providers
                .iter()
                .find(|existing| existing.id == provider.id)
                .and_then(|existing| existing.key_ref.clone());
        }
        provider.ready = provider_is_ready(&provider);
        upsert_by_id(&mut config.providers, provider, |item| &item.id);
        self.save_locked(&mut gate, &config)?;
        Ok(config)
    }

    pub fn save_engine(&self, engine: EngineConfig) -> Result<AppConfig, String> {
        let mut gate = self
            .gate
            .lock()
            .map_err(|_| "ProviderStore 闸门锁中毒".to_string())?;
        let mut config = self.load_locked(&mut gate)?;
        upsert_by_id(&mut config.engines, engine, |item| &item.id);
        self.save_locked(&mut gate, &config)?;
        Ok(config)
    }

    pub fn save_model(&self, model: ModelConfig) -> Result<AppConfig, String> {
        let mut gate = self
            .gate
            .lock()
            .map_err(|_| "ProviderStore 闸门锁中毒".to_string())?;
        let mut config = self.load_locked(&mut gate)?;
        let model = normalize_saved_model(model);
        if !config
            .providers
            .iter()
            .any(|provider| provider.id == model.provider_id)
        {
            return Err(format!("找不到模型所属服务商：{}", model.provider_id));
        }
        upsert_model(&mut config.models, model);
        self.save_locked(&mut gate, &config)?;
        Ok(config)
    }

    pub fn save_models_for_provider(
        &self,
        provider_id: &str,
        models: Vec<ModelConfig>,
    ) -> Result<AppConfig, String> {
        let mut gate = self
            .gate
            .lock()
            .map_err(|_| "ProviderStore 闸门锁中毒".to_string())?;
        let mut config = self.load_locked(&mut gate)?;
        if !config
            .providers
            .iter()
            .any(|provider| provider.id == provider_id)
        {
            return Err(format!("找不到模型所属服务商：{provider_id}"));
        }
        let enabled_by_id = config
            .models
            .iter()
            .filter(|model| model.provider_id == provider_id)
            .map(|model| (model.id.clone(), model.enabled))
            .collect::<HashMap<_, _>>();
        config
            .models
            .retain(|model| model.provider_id != provider_id);
        config.models.extend(models.into_iter().map(|model| {
            let mut model = normalize_saved_model(model);
            if let Some(enabled) = enabled_by_id.get(&model.id) {
                model.enabled = *enabled;
            }
            model
        }));
        self.save_locked(&mut gate, &config)?;
        Ok(config)
    }

    pub fn save_provider_model_selection(
        &self,
        provider_id: &str,
        enabled_model_ids: &[String],
    ) -> Result<AppConfig, String> {
        let mut gate = self
            .gate
            .lock()
            .map_err(|_| "ProviderStore 闸门锁中毒".to_string())?;
        let mut config = self.load_locked(&mut gate)?;
        if !config
            .providers
            .iter()
            .any(|provider| provider.id == provider_id)
        {
            return Err(format!("找不到服务商：{provider_id}"));
        }
        let enabled = enabled_model_ids.iter().cloned().collect::<HashSet<_>>();
        let known = config
            .models
            .iter()
            .filter(|model| model.provider_id == provider_id)
            .map(|model| model.id.clone())
            .collect::<HashSet<_>>();
        if let Some(unknown) = enabled.iter().find(|model_id| !known.contains(*model_id)) {
            return Err(format!("当前服务商没有模型目录项：{unknown}"));
        }
        for binding in config
            .bindings
            .iter()
            .filter(|binding| binding.provider_id == provider_id)
        {
            if !enabled.contains(&binding.primary_model) {
                return Err(format!(
                    "模型 {} 正在被 {} 用作主模型，请先更改引擎绑定",
                    binding.primary_model, binding.engine_id
                ));
            }
            if let Some(fast_model) = binding
                .fast_model
                .as_deref()
                .filter(|model| !model.trim().is_empty())
            {
                if !enabled.contains(fast_model) {
                    return Err(format!(
                        "模型 {fast_model} 正在被 {} 用作快速模型，请先更改引擎绑定",
                        binding.engine_id
                    ));
                }
            }
        }
        for model in config
            .models
            .iter_mut()
            .filter(|model| model.provider_id == provider_id)
        {
            model.enabled = enabled.contains(&model.id);
        }
        self.save_locked(&mut gate, &config)?;
        Ok(config)
    }

    pub fn record_test_result(
        &self,
        provider_id: &str,
        test: ProviderTest,
    ) -> Result<AppConfig, String> {
        let mut gate = self
            .gate
            .lock()
            .map_err(|_| "ProviderStore 闸门锁中毒".to_string())?;
        let mut config = self.load_locked(&mut gate)?;
        let provider = config
            .providers
            .iter_mut()
            .find(|provider| provider.id == provider_id)
            .ok_or_else(|| format!("找不到服务商：{provider_id}"))?;
        provider.last_test = Some(test);
        self.save_locked(&mut gate, &config)?;
        Ok(config)
    }

    pub fn save_binding(&self, mut binding: BindingConfig) -> Result<AppConfig, String> {
        let mut gate = self
            .gate
            .lock()
            .map_err(|_| "ProviderStore 闸门锁中毒".to_string())?;
        let mut config = self.load_locked(&mut gate)?;
        if binding.fast_model.as_deref().is_none_or(str::is_empty) {
            if let Some(legacy) = binding
                .assistant_model_id
                .as_deref()
                .filter(|model| !model.trim().is_empty())
            {
                let valid = config.models.iter().any(|model| {
                    model.enabled && model.provider_id == binding.provider_id && model.id == legacy
                });
                if valid {
                    binding.fast_model = Some(legacy.to_string());
                }
            }
        }
        binding.assistant_model_id = None;
        validate_binding(&config, &binding)?;
        let previous_revision = config
            .bindings
            .iter()
            .find(|item| item.engine_id == binding.engine_id)
            .map(|item| item.revision)
            .unwrap_or(0);
        binding.revision = previous_revision
            .checked_add(1)
            .ok_or_else(|| format!("Binding revision 已溢出：{}", binding.engine_id))?;
        upsert_by_id(&mut config.bindings, binding, |item| &item.engine_id);
        self.save_locked(&mut gate, &config)?;
        Ok(config)
    }

    pub fn migrate_legacy_assistant_model(
        &self,
        legacy_general_model: Option<&str>,
    ) -> Result<AppConfig, String> {
        let mut gate = self
            .gate
            .lock()
            .map_err(|_| "ProviderStore 闸门锁中毒".to_string())?;
        let mut config = self.load_locked(&mut gate)?;
        let enabled_models = config
            .models
            .iter()
            .filter(|model| model.enabled)
            .map(|model| (model.id.clone(), model.provider_id.clone()))
            .collect::<std::collections::HashMap<_, _>>();
        let mut changed = false;
        for binding in &mut config.bindings {
            let legacy = binding
                .assistant_model_id
                .as_deref()
                .filter(|value| !value.trim().is_empty())
                .or_else(|| legacy_general_model.filter(|value| !value.trim().is_empty()));
            if binding.fast_model.as_deref().is_none_or(str::is_empty) {
                if let Some(legacy) = legacy.filter(|legacy| {
                    enabled_models
                        .get(*legacy)
                        .is_some_and(|provider_id| provider_id == &binding.provider_id)
                }) {
                    binding.fast_model = Some(legacy.to_string());
                    binding.revision = binding
                        .revision
                        .checked_add(1)
                        .ok_or_else(|| format!("Binding revision 已溢出：{}", binding.engine_id))?;
                    changed = true;
                }
            }
            if binding.assistant_model_id.take().is_some() {
                changed = true;
            }
        }
        if changed {
            self.save_locked(&mut gate, &config)?;
        }
        Ok(config)
    }

    pub fn equivalent_env(&self, binding: &BindingConfig) -> Result<Vec<(String, String)>, String> {
        let config = self.load()?;
        env_for_config(&config, binding, SecretValueMode::Masked)
    }

    pub fn launch_env(&self, binding: &BindingConfig) -> Result<Vec<(String, String)>, String> {
        let config = self.load()?;
        self.launch_env_for_config(&config, binding)
    }

    pub fn launch_env_for_config(
        &self,
        config: &AppConfig,
        binding: &BindingConfig,
    ) -> Result<Vec<(String, String)>, String> {
        let mut env = env_for_config(config, binding, SecretValueMode::Omit)?;
        let provider = config
            .providers
            .iter()
            .find(|provider| provider.id == binding.provider_id)
            .ok_or_else(|| format!("找不到服务商：{}", binding.provider_id))?;
        if let Some(secret_name) = auth_env_name(provider) {
            match provider.auth_method {
                AuthMethod::ApiKey => {
                    // API Key 模式缺密钥是硬错误（可靠性检查 S6）：不再静默跳过注入
                    let key_ref = provider
                        .key_ref
                        .as_deref()
                        .filter(|key_ref| !key_ref.trim().is_empty())
                        .ok_or_else(|| {
                            format!(
                                "服务商「{}」还没有保存 API 密钥，请先在服务商页填写",
                                provider.name
                            )
                        })?;
                    let secret = self
                        .secret(key_ref)?
                        .ok_or_else(|| "钥匙串中没有找到 API 密钥".to_string())?;
                    env.push((secret_name.to_string(), secret));
                }
                AuthMethod::OAuth => {
                    // 订阅凭证只由官方 CLI 管理，Helm 永不读取或注入。
                }
                AuthMethod::Cloud | AuthMethod::Local => {}
            }
        }
        Ok(env)
    }

    pub fn delete_provider(&self, provider_id: &str) -> Result<AppConfig, String> {
        let mut gate = self
            .gate
            .lock()
            .map_err(|_| "ProviderStore 闸门锁中毒".to_string())?;
        let original = self.load_locked(&mut gate)?;
        let bound_engines: Vec<String> = original
            .bindings
            .iter()
            .filter(|binding| binding.provider_id == provider_id)
            .map(|binding| {
                original
                    .engines
                    .iter()
                    .find(|engine| engine.id == binding.engine_id)
                    .map(|engine| engine.name.clone())
                    .unwrap_or_else(|| binding.engine_id.clone())
            })
            .collect();
        if !bound_engines.is_empty() {
            return Err(format!(
                "该服务商正在被 {} 使用，请先更改或解除引擎绑定",
                bound_engines.join("、")
            ));
        }
        let removed_key_ref = original
            .providers
            .iter()
            .find(|provider| provider.id == provider_id)
            .ok_or_else(|| format!("找不到服务商：{provider_id}"))?
            .key_ref
            .clone();
        let mut config = original.clone();
        let before = config.providers.len();
        config
            .providers
            .retain(|provider| provider.id != provider_id);
        if config.providers.len() == before {
            return Err(format!("找不到服务商：{provider_id}"));
        }
        config
            .models
            .retain(|model| model.provider_id != provider_id);
        config
            .bindings
            .retain(|binding| binding.provider_id != provider_id);
        if !config
            .models
            .iter()
            .any(|model| model.id == config.default_model)
        {
            config.default_model = config
                .models
                .iter()
                .find(|model| model.enabled)
                .map(|model| model.id.clone())
                .unwrap_or_default();
        }
        self.save_locked(&mut gate, &config)?;
        if let Some(key_ref) = removed_key_ref.filter(|key_ref| {
            !config
                .providers
                .iter()
                .any(|provider| provider.key_ref.as_deref() == Some(key_ref.as_str()))
        }) {
            if let Err(secret_error) = self.secrets.delete(&key_ref) {
                return match self.save_locked(&mut gate, &original) {
                    Ok(()) => Err(format!("{secret_error}；服务商配置已恢复，请重试删除")),
                    Err(restore_error) => Err(format!(
                        "{secret_error}；同时恢复服务商配置失败：{restore_error}"
                    )),
                };
            }
        }
        Ok(config)
    }

    pub fn set_defaults(&self, engine_id: &str, model_id: &str) -> Result<AppConfig, String> {
        let mut gate = self
            .gate
            .lock()
            .map_err(|_| "ProviderStore 闸门锁中毒".to_string())?;
        let mut config = self.load_locked(&mut gate)?;
        if !config.engines.iter().any(|engine| engine.id == engine_id) {
            return Err(format!("找不到引擎：{engine_id}"));
        }
        if !model_id.is_empty() && !config.models.iter().any(|model| model.id == model_id) {
            return Err(format!("找不到模型：{model_id}"));
        }
        config.default_engine = engine_id.to_string();
        config.default_model = model_id.to_string();
        if let Some(engine) = config
            .engines
            .iter_mut()
            .find(|engine| engine.id == engine_id)
        {
            engine.default_model = model_id.to_string();
        }
        self.save_locked(&mut gate, &config)?;
        Ok(config)
    }

    pub fn secret(&self, key_ref: &str) -> Result<Option<String>, String> {
        self.secrets.get(key_ref)
    }

    pub fn provider_secret(&self, provider_id: &str) -> Result<String, String> {
        let config = self.load()?;
        let provider = config
            .providers
            .iter()
            .find(|provider| provider.id == provider_id)
            .ok_or_else(|| format!("找不到服务商：{provider_id}"))?;
        let key_ref = provider
            .key_ref
            .as_deref()
            .ok_or_else(|| "该服务商还没有保存 API 密钥".to_string())?;
        self.secret(key_ref)?
            .ok_or_else(|| "钥匙串中没有找到 API 密钥".to_string())
    }
}

fn apply_config_defaults(value: &mut serde_json::Value) {
    if let Some(providers) = value
        .get_mut("providers")
        .and_then(serde_json::Value::as_array_mut)
    {
        for provider in providers {
            let id = provider
                .get("id")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string();
            if provider.get("protocol").is_none() {
                provider["protocol"] = serde_json::Value::String(default_protocol_for_id(&id));
            }
            if provider.get("authMethod").is_none() {
                provider["authMethod"] = serde_json::Value::String("apikey".to_string());
            }
            if provider.get("kind").is_none() {
                let kind = match provider
                    .get("authMethod")
                    .and_then(serde_json::Value::as_str)
                {
                    Some("oauth") => "subscription",
                    Some("local") => "local",
                    _ => "api",
                };
                provider["kind"] = serde_json::Value::String(kind.to_string());
            }
        }
    }
    if value.get("bindings").is_none() {
        value["bindings"] = serde_json::Value::Array(Vec::new());
    }
}

fn default_protocol_for_id(provider_id: &str) -> String {
    match provider_id {
        "openai" => "openai-responses",
        _ => "anthropic",
    }
    .to_string()
}

fn migrate_bindings(config: &mut AppConfig) {
    if !config.bindings.is_empty() {
        return;
    }
    let mut bindings = Vec::new();
    for engine in &config.engines {
        let model_id = if !engine.default_model.is_empty() {
            engine.default_model.as_str()
        } else if engine.id == config.default_engine && !config.default_model.is_empty() {
            config.default_model.as_str()
        } else {
            ""
        };
        if model_id.is_empty() {
            continue;
        }
        let Some(model) = config
            .models
            .iter()
            .find(|model| model.id == model_id && model.enabled)
        else {
            continue;
        };
        let Some(provider) = config
            .providers
            .iter()
            .find(|provider| provider.id == model.provider_id)
        else {
            continue;
        };
        if engine_accepts(&engine.id, &provider.protocol) {
            bindings.push(BindingConfig {
                engine_id: engine.id.clone(),
                provider_id: provider.id.clone(),
                primary_model: model.id.clone(),
                fast_model: None,
                assistant_model_id: None,
                reasoning_effort: None,
                revision: 0,
            });
        }
    }
    config.bindings = bindings;
}

pub fn engine_accepts(engine_id: &str, protocol: &Protocol) -> bool {
    match engine_id {
        "claude-code" => matches!(protocol, Protocol::Anthropic),
        "codex" => matches!(protocol, Protocol::OpenAiResponses | Protocol::OpenAiChat),
        _ => false,
    }
}

fn validate_binding(config: &AppConfig, binding: &BindingConfig) -> Result<(), String> {
    if !config
        .engines
        .iter()
        .any(|engine| engine.id == binding.engine_id)
    {
        return Err(format!("找不到引擎：{}", binding.engine_id));
    }
    let provider = config
        .providers
        .iter()
        .find(|provider| provider.id == binding.provider_id)
        .ok_or_else(|| format!("找不到服务商：{}", binding.provider_id))?;
    if !engine_accepts(&binding.engine_id, &provider.protocol) {
        return Err(format!(
            "协议不兼容：{} 不能绑定服务商 {}",
            binding.engine_id, binding.provider_id
        ));
    }
    validate_binding_model(
        config,
        &binding.provider_id,
        &binding.primary_model,
        "主模型",
    )?;
    if let Some(fast_model) = binding
        .fast_model
        .as_deref()
        .filter(|model| !model.is_empty())
    {
        validate_binding_model(config, &binding.provider_id, fast_model, "快速模型")?;
    }
    if binding.engine_id == "claude-code"
        && binding
            .reasoning_effort
            .is_some_and(|effort| !effort.is_claude_level())
    {
        return Err("Claude Code 不支持该推理强度".to_string());
    }
    Ok(())
}

fn validate_binding_model(
    config: &AppConfig,
    provider_id: &str,
    model_id: &str,
    label: &str,
) -> Result<(), String> {
    let model = config
        .models
        .iter()
        .find(|model| model.provider_id == provider_id && model.id == model_id)
        .ok_or_else(|| format!("找不到{label}：{model_id}"))?;
    if !model.enabled {
        return Err(format!("{label}未启用：{model_id}"));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy)]
enum SecretValueMode {
    Masked,
    Omit,
}

fn env_for_config(
    config: &AppConfig,
    binding: &BindingConfig,
    secret_mode: SecretValueMode,
) -> Result<Vec<(String, String)>, String> {
    validate_binding(config, binding)?;
    let provider = config
        .providers
        .iter()
        .find(|provider| provider.id == binding.provider_id)
        .ok_or_else(|| format!("找不到服务商：{}", binding.provider_id))?;
    // 订阅登录（P3-1）：走 CLI 自己的官方线路与登录态，不注入自定义 BASE_URL，
    // 否则会把订阅流量指到中转地址（订阅与中转互斥；中转请用 API Key 认证）。
    let inject_base_url = !matches!(provider.auth_method, AuthMethod::OAuth);
    let mut env = match provider.protocol {
        Protocol::Anthropic => {
            let mut env = Vec::new();
            if inject_base_url {
                env.push(("ANTHROPIC_BASE_URL".to_string(), provider.base_url.clone()));
            }
            env.push(("ANTHROPIC_MODEL".to_string(), binding.primary_model.clone()));
            env
        }
        Protocol::OpenAiResponses | Protocol::OpenAiChat => {
            let mut env = Vec::new();
            if inject_base_url {
                env.push((
                    "OPENAI_BASE_URL".to_string(),
                    normalize_openai_api_base_url(&provider.base_url),
                ));
            }
            env.push(("OPENAI_MODEL".to_string(), binding.primary_model.clone()));
            env
        }
        Protocol::Bedrock => vec![
            (
                "AWS_BEDROCK_BASE_URL".to_string(),
                provider.base_url.clone(),
            ),
            ("BEDROCK_MODEL".to_string(), binding.primary_model.clone()),
        ],
        Protocol::Vertex => vec![
            ("VERTEX_BASE_URL".to_string(), provider.base_url.clone()),
            ("VERTEX_MODEL".to_string(), binding.primary_model.clone()),
        ],
    };
    if let Some(fast_model) = binding
        .fast_model
        .as_deref()
        .filter(|model| !model.is_empty())
    {
        match provider.protocol {
            Protocol::Anthropic => {
                env.push((
                    "ANTHROPIC_DEFAULT_HAIKU_MODEL".to_string(),
                    fast_model.to_string(),
                ));
            }
            Protocol::OpenAiResponses | Protocol::OpenAiChat => {
                env.push(("OPENAI_FAST_MODEL".to_string(), fast_model.to_string()));
            }
            Protocol::Bedrock => {
                env.push(("BEDROCK_FAST_MODEL".to_string(), fast_model.to_string()));
            }
            Protocol::Vertex => {
                env.push(("VERTEX_FAST_MODEL".to_string(), fast_model.to_string()));
            }
        }
    }
    if matches!(secret_mode, SecretValueMode::Omit) {
        match provider.protocol {
            Protocol::OpenAiChat => {
                env.push(("HELM_CODEX_WIRE_API".to_string(), "chat".to_string()));
            }
            Protocol::OpenAiResponses => {
                env.push(("HELM_CODEX_WIRE_API".to_string(), "responses".to_string()));
            }
            _ => {}
        }
    }
    if matches!(secret_mode, SecretValueMode::Masked) {
        if matches!(provider.kind, ProviderKind::Subscription) {
            env.push((
                "# 凭证".to_string(),
                "由 Helm 独立订阅 Profile 管理，Helm 不注入".to_string(),
            ));
        } else if let Some(secret_name) = auth_env_name(provider) {
            match provider.auth_method {
                AuthMethod::ApiKey => {
                    env.push((secret_name.to_string(), "••••（系统钥匙串）".to_string()));
                }
                AuthMethod::OAuth => {}
                AuthMethod::Cloud => env.push((secret_name.to_string(), "云凭证链".to_string())),
                AuthMethod::Local => {}
            }
        }
    }
    Ok(env)
}

fn auth_env_name(provider: &ProviderConfig) -> Option<&'static str> {
    match provider.protocol {
        Protocol::Anthropic => Some("ANTHROPIC_AUTH_TOKEN"),
        Protocol::OpenAiResponses | Protocol::OpenAiChat => Some("OPENAI_API_KEY"),
        Protocol::Bedrock => Some("AWS_CREDENTIALS"),
        Protocol::Vertex => Some("GOOGLE_APPLICATION_CREDENTIALS"),
    }
}

fn upsert_by_id<T, F>(items: &mut Vec<T>, item: T, id: F)
where
    F: Fn(&T) -> &str,
{
    if let Some(existing) = items.iter_mut().find(|existing| id(existing) == id(&item)) {
        *existing = item;
    } else {
        items.push(item);
    }
}

fn upsert_model(models: &mut Vec<ModelConfig>, model: ModelConfig) {
    if let Some(existing) = models
        .iter_mut()
        .find(|existing| existing.provider_id == model.provider_id && existing.id == model.id)
    {
        *existing = model;
    } else {
        models.push(model);
    }
}

fn deduplicate_models(models: &mut Vec<ModelConfig>) {
    let mut deduplicated: Vec<ModelConfig> = Vec::with_capacity(models.len());
    for model in models.drain(..) {
        if let Some(existing) = deduplicated
            .iter_mut()
            .find(|existing| existing.provider_id == model.provider_id && existing.id == model.id)
        {
            let enabled = existing.enabled || model.enabled;
            if model.enabled || !existing.enabled {
                *existing = model;
            }
            existing.enabled = enabled;
        } else {
            deduplicated.push(model);
        }
    }
    *models = deduplicated;
}

pub fn key_ref_for_provider(provider_id: &str) -> String {
    format!("helm:provider:{provider_id}:api-key")
}

pub fn engine_config_file_path(engine_id: &str) -> Result<PathBuf, String> {
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .ok_or_else(|| "无法定位用户主目录".to_string())?;
    match engine_id {
        "claude-code" => Ok(home.join(".claude").join("settings.json")),
        "codex" => Ok(home.join(".codex").join("config.toml")),
        _ => Err(format!("未知引擎：{engine_id}")),
    }
}

pub fn read_engine_config_file(engine_id: &str) -> Result<EngineConfigFile, String> {
    let path = engine_config_file_path(engine_id)?;
    read_engine_config_file_at(engine_id, &path)
}

pub fn read_engine_config_file_at(
    engine_id: &str,
    path: &Path,
) -> Result<EngineConfigFile, String> {
    validate_engine_id_for_config_file(engine_id)?;
    let content = if path.exists() {
        fs::read_to_string(path).map_err(|e| format!("读取引擎配置文件失败：{e}"))?
    } else {
        String::new()
    };
    Ok(EngineConfigFile {
        path: path.to_path_buf(),
        content,
    })
}

pub fn write_engine_config_file(
    engine_id: &str,
    content: &str,
) -> Result<EngineConfigFile, String> {
    let path = engine_config_file_path(engine_id)?;
    write_engine_config_file_at(engine_id, &path, content)
}

pub fn write_engine_config_file_at(
    engine_id: &str,
    path: &Path,
    content: &str,
) -> Result<EngineConfigFile, String> {
    validate_engine_config_content(engine_id, content)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("创建引擎配置目录失败：{e}"))?;
    }
    fs::write(path, content).map_err(|e| format!("写入引擎配置文件失败：{e}"))?;
    Ok(EngineConfigFile {
        path: path.to_path_buf(),
        content: content.to_string(),
    })
}

fn validate_engine_id_for_config_file(engine_id: &str) -> Result<(), String> {
    match engine_id {
        "claude-code" | "codex" => Ok(()),
        _ => Err(format!("未知引擎：{engine_id}")),
    }
}

fn validate_engine_config_content(engine_id: &str, content: &str) -> Result<(), String> {
    match engine_id {
        "claude-code" => serde_json::from_str::<serde_json::Value>(content)
            .map(|_| ())
            .map_err(|e| format!("Claude Code 配置不是合法 JSON：{e}")),
        "codex" => toml::from_str::<toml::Value>(content)
            .map(|_| ())
            .map_err(|e| format!("Codex 配置不是合法 TOML：{e}")),
        _ => Err(format!("未知引擎：{engine_id}")),
    }
}

pub fn provider_models_endpoint(provider_id: &str, base_url: &str) -> String {
    let base = base_url.trim().trim_end_matches('/');
    if base.ends_with("/models") {
        return base.to_string();
    }
    match provider_id {
        "anthropic" => {
            if base.ends_with("/v1") {
                format!("{base}/models")
            } else {
                format!("{base}/v1/models")
            }
        }
        "openai" => format!("{base}/models"),
        _ => base.to_string(),
    }
}

pub fn provider_models_endpoint_for_protocol(protocol: &Protocol, base_url: &str) -> String {
    let base = base_url.trim().trim_end_matches('/');
    if base.ends_with("/models") {
        return base.to_string();
    }
    match protocol {
        Protocol::Anthropic => {
            if base.ends_with("/v1") {
                format!("{base}/models")
            } else {
                format!("{base}/v1/models")
            }
        }
        Protocol::OpenAiResponses | Protocol::OpenAiChat => {
            format!("{}/models", normalize_openai_api_base_url(base))
        }
        Protocol::Bedrock | Protocol::Vertex => base.to_string(),
    }
}

/// 会话标题生成（P3-5）用的补全端点：Anthropic → /v1/messages，OpenAI 系 → /chat/completions
pub fn provider_completion_endpoint(protocol: &Protocol, base_url: &str) -> String {
    let base = base_url.trim().trim_end_matches('/');
    match protocol {
        Protocol::Anthropic => {
            if base.ends_with("/v1") {
                format!("{base}/messages")
            } else {
                format!("{base}/v1/messages")
            }
        }
        Protocol::OpenAiResponses | Protocol::OpenAiChat => {
            format!("{}/chat/completions", normalize_openai_api_base_url(base))
        }
        Protocol::Bedrock | Protocol::Vertex => base.to_string(),
    }
}

fn normalize_openai_api_base_url(base_url: &str) -> String {
    let mut base = base_url.trim().trim_end_matches('/').to_string();
    for suffix in ["/models", "/responses", "/chat/completions"] {
        if base.ends_with(suffix) {
            base.truncate(base.len() - suffix.len());
            break;
        }
    }
    if base.ends_with("/v1") {
        base
    } else {
        format!("{base}/v1")
    }
}

#[derive(Debug, Deserialize)]
struct ModelsEnvelope {
    data: Vec<ModelEnvelopeItem>,
}

#[derive(Debug, Deserialize)]
struct ModelEnvelopeItem {
    id: String,
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default, rename = "displayName")]
    display_name_camel: Option<String>,
    #[serde(default)]
    name: Option<String>,
    /// Anthropic: context_window; OpenAI: context_length
    #[serde(default, alias = "context_length")]
    context_window: Option<u64>,
    /// 能力标签数组
    #[serde(default)]
    capabilities: Option<Vec<String>>,
}

pub fn models_from_provider_response(
    protocol: &Protocol,
    provider_id: &str,
    body: &str,
    existing: &[ModelConfig],
) -> Result<Vec<ModelConfig>, String> {
    let parsed: ModelsEnvelope =
        serde_json::from_str(body).map_err(|e| format!("解析模型列表失败：{e}"))?;
    let mut models = Vec::new();
    let mut seen_ids = HashSet::new();
    for item in parsed.data {
        if item.id.trim().is_empty() {
            continue;
        }
        if !seen_ids.insert(item.id.clone()) {
            continue;
        }
        if let Some(existing_model) = existing
            .iter()
            .find(|model| model.provider_id == provider_id && model.id == item.id)
        {
            let mut model = existing_model.clone();
            apply_builtin_model_pricing_to_model(protocol, &mut model);
            models.push(model);
            continue;
        }
        let display_name = item
            .display_name
            .or(item.display_name_camel)
            .or(item.name)
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| item.id.clone());
        let builtin = builtin_model_pricing(protocol, &item.id);
        let (input_price_per_mtok, output_price_per_mtok) = builtin.unwrap_or((0.0, 0.0));
        models.push(ModelConfig {
            id: item.id,
            provider_id: provider_id.to_string(),
            display_name,
            input_price_per_mtok,
            output_price_per_mtok,
            price_source: Some(if builtin.is_some() {
                PriceSource::Builtin
            } else {
                PriceSource::Unknown
            }),
            enabled: true,
            context_window: item.context_window,
            capabilities: item.capabilities,
        });
    }
    Ok(models)
}

fn builtin_model_pricing(protocol: &Protocol, model_id: &str) -> Option<(f64, f64)> {
    PricingCatalog::builtin()
        .ok()?
        .resolve(protocol, model_id, ServiceTier::Standard, 0)
        .map(|rate| (rate.band.input, rate.band.output))
}

fn apply_catalog_model_pricing(
    config: &mut AppConfig,
    catalog: &PricingCatalog,
    pricing_store: Option<&PricingCatalogStore>,
) {
    for idx in 0..config.models.len() {
        let provider = config
            .providers
            .iter()
            .find(|provider| provider.id == config.models[idx].provider_id);
        let protocol = provider
            .map(|provider| provider.protocol.clone())
            .unwrap_or(Protocol::OpenAiResponses);
        if let (Some(store), Some(provider)) = (pricing_store, provider) {
            if let Ok(Some(rate)) = store.resolve_for_provider(
                catalog,
                &protocol,
                &provider.id,
                &config.models[idx].id,
                ServiceTier::Standard,
                0,
            ) {
                if rate.vendor == "manual" {
                    config.models[idx].input_price_per_mtok = rate.band.input;
                    config.models[idx].output_price_per_mtok = rate.band.output;
                    config.models[idx].price_source = Some(PriceSource::Manual);
                    continue;
                }
            }
            if let Ok(preference) = store.provider_preference(&provider.id) {
                if matches!(
                    preference.mode,
                    ProviderPricingMode::Disabled
                        | ProviderPricingMode::Manual
                        | ProviderPricingMode::Provider
                ) && matches!(
                    config.models[idx].price_source,
                    None | Some(PriceSource::Builtin | PriceSource::Unknown)
                ) {
                    config.models[idx].input_price_per_mtok = 0.0;
                    config.models[idx].output_price_per_mtok = 0.0;
                    config.models[idx].price_source = Some(PriceSource::Unknown);
                    continue;
                }
            }
        }
        apply_catalog_model_pricing_to_model(catalog, &protocol, &mut config.models[idx]);
    }
}

fn apply_builtin_model_pricing_to_model(protocol: &Protocol, model: &mut ModelConfig) {
    let Ok(catalog) = PricingCatalog::builtin() else {
        return;
    };
    apply_catalog_model_pricing_to_model(&catalog, protocol, model);
}

fn apply_catalog_model_pricing_to_model(
    catalog: &PricingCatalog,
    protocol: &Protocol,
    model: &mut ModelConfig,
) {
    if matches!(
        model.price_source,
        Some(PriceSource::Manual | PriceSource::Provider | PriceSource::Subscription)
    ) {
        return;
    }
    if let Some(rate) = catalog.resolve(protocol, &model.id, ServiceTier::Standard, 0) {
        let input = rate.band.input;
        let output = rate.band.output;
        if model.input_price_per_mtok == 0.0 || model.output_price_per_mtok == 0.0 {
            model.input_price_per_mtok = input;
            model.output_price_per_mtok = output;
            model.price_source = Some(PriceSource::Builtin);
        } else if model.price_source.is_none() {
            model.price_source = Some(PriceSource::Builtin);
        }
    } else if model.price_source.is_none() {
        model.price_source = Some(PriceSource::Unknown);
    }
}

fn normalize_saved_model(mut model: ModelConfig) -> ModelConfig {
    if model.price_source.is_none() {
        model.price_source = Some(
            if model.input_price_per_mtok > 0.0 || model.output_price_per_mtok > 0.0 {
                PriceSource::Manual
            } else {
                PriceSource::Unknown
            },
        );
    }
    model
}

fn simple_pricing_profile(
    catalog_version: &str,
    source: &str,
    input: f64,
    output: f64,
) -> ResolvedPricingProfile {
    ResolvedPricingProfile {
        catalog_version: catalog_version.to_string(),
        source: source.to_string(),
        currency: "USD".to_string(),
        source_url: String::new(),
        observed_at: String::new(),
        tiers: HashMap::from([(
            ServiceTier::Standard,
            PricingTier {
                bands: vec![PricingBand {
                    min_input_tokens: None,
                    max_input_tokens: None,
                    input,
                    cached_input: None,
                    cache_write: None,
                    output,
                }],
            },
        )]),
    }
}

pub(crate) fn subscription_models_for_provider(provider: &ProviderConfig) -> Vec<ModelConfig> {
    let catalog: &[(&str, &str)] = match provider.protocol {
        Protocol::Anthropic => &[
            ("default", "Claude 默认（当前账号推荐）"),
            ("best", "Claude Best（当前账号可用的最强模型）"),
            ("sonnet", "Claude Sonnet（订阅）"),
            ("opus", "Claude Opus（订阅）"),
            ("haiku", "Claude Haiku（订阅）"),
        ],
        // ChatGPT 订阅的可用模型由当前登录账号的 Codex app-server
        // `model/list` 决定，不能用发布时固定目录代替账号权限。
        Protocol::OpenAiResponses => &[],
        Protocol::OpenAiChat | Protocol::Bedrock | Protocol::Vertex => &[],
    };
    catalog
        .iter()
        .map(|(id, display_name)| ModelConfig {
            id: (*id).to_string(),
            provider_id: provider.id.clone(),
            display_name: (*display_name).to_string(),
            input_price_per_mtok: 0.0,
            output_price_per_mtok: 0.0,
            price_source: Some(PriceSource::Subscription),
            enabled: true,
            context_window: None,
            capabilities: None,
        })
        .collect()
}

fn ensure_subscription_model_catalogs(config: &mut AppConfig) {
    let providers: Vec<_> = config
        .providers
        .iter()
        .filter(|provider| matches!(provider.kind, ProviderKind::Subscription))
        .cloned()
        .collect();
    for provider in providers {
        if config
            .models
            .iter()
            .any(|model| model.provider_id == provider.id)
        {
            continue;
        }
        config
            .models
            .extend(subscription_models_for_provider(&provider));
    }
}

pub fn seed_config() -> AppConfig {
    AppConfig {
        default_engine: "claude-code".to_string(),
        default_model: String::new(),
        providers: Vec::new(),
        models: Vec::new(),
        engines: vec![
            EngineConfig {
                id: "claude-code".to_string(),
                name: "Claude Code".to_string(),
                bin: "claude".to_string(),
                default_model: String::new(),
                status: EngineStatus::Missing,
                version: None,
            },
            EngineConfig {
                id: "codex".to_string(),
                name: "Codex".to_string(),
                bin: "codex".to_string(),
                default_model: String::new(),
                status: EngineStatus::Missing,
                version: None,
            },
        ],
        bindings: Vec::new(),
    }
}

pub async fn test_provider_connection<S: SecretStore>(
    store: &ProviderStore<S>,
    provider_id: &str,
) -> Result<ConnectionResult, String> {
    let started = Instant::now();
    let config = store.load()?;
    let provider = config
        .providers
        .iter()
        .find(|provider| provider.id == provider_id)
        .ok_or_else(|| format!("找不到服务商：{provider_id}"))?;
    // 订阅登录且未存令牌（P3-1）：凭证在 CLI 登录态里，Helm 拿不到，
    // 无法代表用户做 HTTP 探活——如实说明而不是拿空 Key 去撞 401。
    if matches!(provider.auth_method, AuthMethod::OAuth)
        && provider
            .key_ref
            .as_deref()
            .is_none_or(|key_ref| key_ref.trim().is_empty())
    {
        return Ok(ConnectionResult {
            ok: false,
            verified: false,
            message: "未验证：订阅登录使用 Helm 独立 CLI Profile；请用「检测登录态」确认该 Profile 已登录。"
                .to_string(),
            latency_ms: started.elapsed().as_millis(),
        });
    }
    let key_ref = provider
        .key_ref
        .as_deref()
        .ok_or_else(|| "请先保存 API 密钥".to_string())?;
    let api_key = store
        .secret(key_ref)?
        .ok_or_else(|| "钥匙串中没有找到 API 密钥".to_string())?;

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .connect_timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| format!("创建 HTTP 客户端失败：{e}"))?;
    let response = match provider.protocol {
        Protocol::Anthropic => {
            client
                .get(provider_models_endpoint_for_protocol(
                    &provider.protocol,
                    &provider.base_url,
                ))
                .header("x-api-key", api_key)
                .header("anthropic-version", "2023-06-01")
                .send()
                .await
        }
        Protocol::OpenAiResponses | Protocol::OpenAiChat => {
            client
                .get(provider_models_endpoint_for_protocol(
                    &provider.protocol,
                    &provider.base_url,
                ))
                .bearer_auth(api_key)
                .send()
                .await
        }
        Protocol::Bedrock | Protocol::Vertex => {
            client
                .get(provider.base_url.as_str())
                .bearer_auth(api_key)
                .send()
                .await
        }
    }
    .map_err(|e| format!("测试可达性失败：{e}"))?;

    let status = response.status();
    Ok(ConnectionResult {
        ok: status.is_success(),
        verified: true,
        message: if status.is_success() {
            "探活成功".to_string()
        } else {
            format!("探活失败：HTTP {}", status.as_u16())
        },
        latency_ms: started.elapsed().as_millis(),
    })
}

pub async fn sync_provider_models<S: SecretStore>(
    store: &ProviderStore<S>,
    provider_id: &str,
) -> Result<AppConfig, String> {
    let config = store.load()?;
    let provider = config
        .providers
        .iter()
        .find(|provider| provider.id == provider_id)
        .ok_or_else(|| format!("找不到服务商：{provider_id}"))?;
    if matches!(provider.kind, ProviderKind::Subscription) {
        return store
            .save_models_for_provider(provider_id, subscription_models_for_provider(provider));
    }
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .connect_timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| format!("创建 HTTP 客户端失败：{e}"))?;
    let mut request = client.get(provider_models_endpoint_for_protocol(
        &provider.protocol,
        &provider.base_url,
    ));
    match provider.auth_method {
        AuthMethod::ApiKey => {
            let key_ref = provider
                .key_ref
                .as_deref()
                .ok_or_else(|| "请先保存 API 密钥".to_string())?;
            let api_key = store
                .secret(key_ref)?
                .ok_or_else(|| "钥匙串中没有找到 API 密钥".to_string())?;
            request = match provider.protocol {
                Protocol::Anthropic => request
                    .header("x-api-key", api_key)
                    .header("anthropic-version", "2023-06-01"),
                _ => request.bearer_auth(api_key),
            };
        }
        AuthMethod::OAuth => {
            return Err("订阅接入不调用服务商模型接口，请重新保存服务商以恢复内置目录".to_string());
        }
        AuthMethod::Cloud | AuthMethod::Local => {}
    }
    let response = request
        .send()
        .await
        .map_err(|e| format!("同步模型失败：{e}"))?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("同步模型失败：HTTP {}", status.as_u16()));
    }
    let body = response
        .text()
        .await
        .map_err(|e| format!("读取模型列表失败：{e}"))?;
    let models =
        models_from_provider_response(&provider.protocol, provider_id, &body, &config.models)?;
    store.save_models_for_provider(provider_id, models)
}

pub async fn test_engine_connection(bin: &str) -> ConnectionResult {
    let started = Instant::now();
    let mut cmd = build_version_command(bin);
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    match cmd.output().await {
        Ok(output) if output.status.success() => {
            let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
            ConnectionResult {
                ok: true,
                verified: true,
                message: if text.is_empty() {
                    "引擎可执行文件可用".to_string()
                } else {
                    text
                },
                latency_ms: started.elapsed().as_millis(),
            }
        }
        Ok(output) => ConnectionResult {
            ok: false,
            verified: true,
            message: String::from_utf8_lossy(&output.stderr).trim().to_string(),
            latency_ms: started.elapsed().as_millis(),
        },
        Err(e) => ConnectionResult {
            ok: false,
            verified: true,
            message: format!("无法执行引擎：{e}"),
            latency_ms: started.elapsed().as_millis(),
        },
    }
}

fn build_version_command(bin: &str) -> Command {
    #[cfg(target_os = "windows")]
    {
        let mut cmd = Command::new("cmd");
        cmd.arg("/C")
            .arg(bin)
            .arg("--version")
            .creation_flags(CREATE_NO_WINDOW);
        cmd
    }
    #[cfg(not(target_os = "windows"))]
    {
        let mut cmd = Command::new(bin);
        cmd.arg("--version");
        cmd
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_failure_returns_none_when_ok() {
        assert_eq!(classify_failure("anything", true, true), None);
    }

    #[test]
    fn classify_failure_returns_none_when_unverified() {
        assert_eq!(classify_failure("anything", false, false), None);
    }

    #[test]
    fn classify_failure_timeout_from_timed_out() {
        assert_eq!(
            classify_failure("connection timed out", false, true),
            Some(FailureCategory::Timeout)
        );
    }

    #[test]
    fn classify_failure_auth_from_401() {
        assert_eq!(
            classify_failure("探活失败：HTTP 401", false, true),
            Some(FailureCategory::Auth)
        );
    }

    #[test]
    fn classify_failure_auth_from_api_key() {
        assert_eq!(
            classify_failure("请先保存 API 密钥", false, true),
            Some(FailureCategory::Auth)
        );
    }

    #[test]
    fn classify_failure_network_from_http_status() {
        assert_eq!(
            classify_failure("探活失败：HTTP 502", false, true),
            Some(FailureCategory::Network)
        );
    }

    #[test]
    fn classify_failure_unknown_for_generic_error() {
        assert_eq!(
            classify_failure("something unexpected happened", false, true),
            Some(FailureCategory::Unknown)
        );
    }
}
