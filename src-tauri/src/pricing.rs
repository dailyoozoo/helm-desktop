use crate::providers::Protocol;
use crate::sessions::SessionHistoryStore;
use crate::settings::{load_app_settings_from_store, AppSettings};
use crate::util::now_seconds;
use base64::Engine as _;
use minisign_verify::{PublicKey, Signature};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, State};

pub const PRICING_PUBLIC_KEY_BASE64: &str =
    "RWRHXYdrQnYvw41LlO/UMQ0ef2yjkPQPAqOonnnfXb5gEqSvQMbhT3dP";
pub const BUILTIN_CATALOG_JSON: &str = include_str!("../assets/pricing-catalog.json");
const MAX_CATALOG_BYTES: usize = 2 * 1024 * 1024;
const MAX_CATALOG_MODELS: usize = 10_000;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PricingCatalog {
    pub schema_version: u32,
    pub catalog_version: String,
    pub sequence: u64,
    pub published_at: String,
    pub models: Vec<ModelRateCard>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ModelRateCard {
    pub vendor: String,
    pub model_id: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    pub currency: String,
    pub unit: String,
    pub source_url: String,
    pub observed_at: String,
    pub tiers: HashMap<ServiceTier, PricingTier>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum ServiceTier {
    Standard,
    Batch,
    Flex,
    Priority,
}

impl Default for ServiceTier {
    fn default() -> Self {
        Self::Standard
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PricingTier {
    pub bands: Vec<PricingBand>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PricingBand {
    #[serde(default)]
    pub min_input_tokens: Option<u64>,
    #[serde(default)]
    pub max_input_tokens: Option<u64>,
    pub input: f64,
    #[serde(default)]
    pub cached_input: Option<f64>,
    #[serde(default)]
    pub cache_write: Option<f64>,
    pub output: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedRate {
    pub catalog_version: String,
    pub vendor: String,
    pub canonical_model_id: String,
    pub currency: String,
    pub unit: String,
    pub source_url: String,
    pub observed_at: String,
    pub service_tier: ServiceTier,
    pub band: PricingBand,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedPricingProfile {
    pub catalog_version: String,
    pub source: String,
    pub currency: String,
    pub source_url: String,
    pub observed_at: String,
    pub tiers: HashMap<ServiceTier, PricingTier>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PricingCatalogStatus {
    pub source: String,
    pub catalog_version: String,
    pub sequence: u64,
    pub published_at: String,
    pub last_checked_at: Option<i64>,
    pub last_error: Option<String>,
    pub stale: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ModelPriceOverride {
    pub provider_id: String,
    pub model_id: String,
    pub currency: String,
    pub tiers: HashMap<ServiceTier, PricingTier>,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProviderPricingPreference {
    pub provider_id: String,
    pub mode: ProviderPricingMode,
    pub multiplier_basis_points: u32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ProviderPricingMode {
    Auto,
    Provider,
    OfficialReference,
    Manual,
    Disabled,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct PricingOverrides {
    #[serde(default)]
    models: Vec<ModelPriceOverride>,
    #[serde(default)]
    providers: Vec<ProviderPricingPreference>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PricingCacheEnvelope {
    catalog_json: String,
    signature: String,
}

#[derive(Clone)]
pub struct PricingCatalogStore {
    directory: PathBuf,
    public_key_base64: String,
}

impl PricingCatalog {
    pub fn builtin() -> Result<Self, String> {
        parse_and_validate_catalog(BUILTIN_CATALOG_JSON.as_bytes())
    }

    pub fn resolve(
        &self,
        protocol: &Protocol,
        model_id: &str,
        service_tier: ServiceTier,
        input_tokens: u64,
    ) -> Option<ResolvedRate> {
        let vendor = vendor_for_protocol(protocol);
        let normalized = normalize_model_id(model_id);
        let card = self.models.iter().find(|card| {
            vendor_matches(&card.vendor, vendor, &normalized)
                && (normalize_model_id(&card.model_id) == normalized
                    || card
                        .aliases
                        .iter()
                        .any(|alias| normalize_model_id(alias) == normalized))
        })?;
        let (resolved_tier, tier) = card
            .tiers
            .get(&service_tier)
            .map(|tier| (service_tier, tier))
            .or_else(|| {
                card.tiers
                    .get(&ServiceTier::Standard)
                    .map(|tier| (ServiceTier::Standard, tier))
            })?;
        let band = tier
            .bands
            .iter()
            .find(|band| band_matches(band, input_tokens))
            .or_else(|| tier.bands.first())?
            .clone();
        Some(ResolvedRate {
            catalog_version: self.catalog_version.clone(),
            vendor: card.vendor.clone(),
            canonical_model_id: card.model_id.clone(),
            currency: card.currency.clone(),
            unit: card.unit.clone(),
            source_url: card.source_url.clone(),
            observed_at: card.observed_at.clone(),
            service_tier: resolved_tier,
            band,
        })
    }
}

impl PricingCatalogStore {
    pub fn new(directory: PathBuf) -> Self {
        Self {
            directory,
            public_key_base64: PRICING_PUBLIC_KEY_BASE64.to_string(),
        }
    }

    #[cfg(test)]
    pub fn with_public_key(directory: PathBuf, public_key_base64: &str) -> Self {
        Self {
            directory,
            public_key_base64: public_key_base64.to_string(),
        }
    }

    pub fn catalog_path(&self) -> PathBuf {
        self.directory.join("pricing-catalog.json")
    }

    pub fn signature_path(&self) -> PathBuf {
        self.directory.join("pricing-catalog.json.sig")
    }

    pub fn state_path(&self) -> PathBuf {
        self.directory.join("pricing-state.json")
    }

    pub fn cache_bundle_path(&self) -> PathBuf {
        self.directory.join("pricing-cache.json")
    }

    pub fn overrides_path(&self) -> PathBuf {
        self.directory.join("pricing-overrides.json")
    }

    pub fn active_catalog(&self) -> Result<(PricingCatalog, PricingCatalogStatus), String> {
        let builtin = PricingCatalog::builtin()?;
        let cached = self.read_verified_cache();
        let (catalog, source, error) = match cached {
            Ok(catalog) if catalog.sequence >= builtin.sequence => (catalog, "cache", None),
            Ok(_) => (
                builtin,
                "builtin",
                Some("缓存价格目录版本低于内置目录，已拒绝降级".to_string()),
            ),
            Err(error)
                if self.cache_bundle_path().exists()
                    || self.catalog_path().exists()
                    || self.signature_path().exists() =>
            {
                (builtin, "builtin", Some(error))
            }
            Err(_) => (builtin, "builtin", None),
        };
        let state = self.read_state().ok();
        Ok((
            catalog.clone(),
            PricingCatalogStatus {
                source: source.to_string(),
                catalog_version: catalog.catalog_version.clone(),
                sequence: catalog.sequence,
                published_at: catalog.published_at.clone(),
                last_checked_at: state.as_ref().and_then(|item| item.last_checked_at),
                last_error: error.or_else(|| state.and_then(|item| item.last_error)),
                stale: catalog_is_stale(&catalog.published_at, 30),
            },
        ))
    }

    pub fn resolve_for_provider(
        &self,
        catalog: &PricingCatalog,
        protocol: &Protocol,
        provider_id: &str,
        model_id: &str,
        service_tier: ServiceTier,
        input_tokens: u64,
    ) -> Result<Option<ResolvedRate>, String> {
        let preference = self.provider_preference(provider_id)?;
        if matches!(preference.mode, ProviderPricingMode::Disabled) {
            return Ok(None);
        }
        if let Some(price_override) = self.model_override(provider_id, model_id)? {
            let (tier_name, tier) = price_override
                .tiers
                .get(&service_tier)
                .map(|tier| (service_tier, tier))
                .or_else(|| {
                    price_override
                        .tiers
                        .get(&ServiceTier::Standard)
                        .map(|tier| (ServiceTier::Standard, tier))
                })
                .ok_or_else(|| "手动价格覆盖缺少可用服务层级".to_string())?;
            let band = tier
                .bands
                .iter()
                .find(|band| band_matches(band, input_tokens))
                .or_else(|| tier.bands.first())
                .cloned()
                .ok_or_else(|| "手动价格覆盖缺少价格区间".to_string())?;
            return Ok(Some(ResolvedRate {
                catalog_version: format!("manual:{}", price_override.updated_at),
                vendor: "manual".to_string(),
                canonical_model_id: model_id.to_string(),
                currency: price_override.currency,
                unit: "per-million-tokens".to_string(),
                source_url: String::new(),
                observed_at: price_override.updated_at.to_string(),
                service_tier: tier_name,
                band,
            }));
        }
        if matches!(
            preference.mode,
            ProviderPricingMode::Manual | ProviderPricingMode::Provider
        ) {
            return Ok(None);
        }
        let mut resolved = catalog.resolve(protocol, model_id, service_tier, input_tokens);
        if let Some(rate) = resolved.as_mut() {
            let multiplier = preference.multiplier_basis_points as f64 / 10_000.0;
            rate.band.input *= multiplier;
            rate.band.output *= multiplier;
            rate.band.cached_input = rate.band.cached_input.map(|value| value * multiplier);
            rate.band.cache_write = rate.band.cache_write.map(|value| value * multiplier);
        }
        Ok(resolved)
    }

    pub fn profile_for_provider(
        &self,
        catalog: &PricingCatalog,
        protocol: &Protocol,
        provider_id: &str,
        model_id: &str,
    ) -> Result<Option<ResolvedPricingProfile>, String> {
        let preference = self.provider_preference(provider_id)?;
        if matches!(preference.mode, ProviderPricingMode::Disabled) {
            return Ok(None);
        }
        if let Some(price_override) = self.model_override(provider_id, model_id)? {
            return Ok(Some(ResolvedPricingProfile {
                catalog_version: format!("manual:{}", price_override.updated_at),
                source: "manual".to_string(),
                currency: price_override.currency,
                source_url: String::new(),
                observed_at: price_override.updated_at.to_string(),
                tiers: price_override.tiers,
            }));
        }
        if matches!(
            preference.mode,
            ProviderPricingMode::Manual | ProviderPricingMode::Provider
        ) {
            return Ok(None);
        }
        let normalized = normalize_model_id(model_id);
        let vendor = vendor_for_protocol(protocol);
        let card = catalog.models.iter().find(|card| {
            vendor_matches(&card.vendor, vendor, &normalized)
                && (normalize_model_id(&card.model_id) == normalized
                    || card
                        .aliases
                        .iter()
                        .any(|alias| normalize_model_id(alias) == normalized))
        });
        let Some(card) = card else {
            return Ok(None);
        };
        let multiplier = preference.multiplier_basis_points as f64 / 10_000.0;
        let mut tiers = card.tiers.clone();
        for tier in tiers.values_mut() {
            for band in &mut tier.bands {
                band.input *= multiplier;
                band.output *= multiplier;
                band.cached_input = band.cached_input.map(|value| value * multiplier);
                band.cache_write = band.cache_write.map(|value| value * multiplier);
            }
        }
        Ok(Some(ResolvedPricingProfile {
            catalog_version: catalog.catalog_version.clone(),
            source: "official-reference".to_string(),
            currency: card.currency.clone(),
            source_url: card.source_url.clone(),
            observed_at: card.observed_at.clone(),
            tiers,
        }))
    }

    pub fn install_verified(
        &self,
        catalog_bytes: &[u8],
        signature_text: &str,
    ) -> Result<PricingCatalogStatus, String> {
        verify_minisign(catalog_bytes, signature_text, &self.public_key_base64)?;
        let candidate = parse_and_validate_catalog(catalog_bytes)?;
        let (current, _) = self.active_catalog()?;
        if candidate.sequence < current.sequence {
            return Err(format!(
                "价格目录拒绝降级：候选序号 {} 小于当前序号 {}",
                candidate.sequence, current.sequence
            ));
        }
        fs::create_dir_all(&self.directory).map_err(|e| format!("创建价格目录失败：{e}"))?;
        let envelope = PricingCacheEnvelope {
            catalog_json: String::from_utf8(catalog_bytes.to_vec())
                .map_err(|_| "价格目录必须是 UTF-8 JSON".to_string())?,
            signature: signature_text.to_string(),
        };
        let envelope_bytes =
            serde_json::to_vec(&envelope).map_err(|e| format!("序列化价格缓存失败：{e}"))?;
        write_atomically_bytes(&self.cache_bundle_path(), &envelope_bytes)?;
        self.write_state(&PricingState {
            last_checked_at: Some(now_seconds()),
            last_error: None,
        })?;
        Ok(PricingCatalogStatus {
            source: "cache".to_string(),
            catalog_version: candidate.catalog_version,
            sequence: candidate.sequence,
            published_at: candidate.published_at,
            last_checked_at: Some(now_seconds()),
            last_error: None,
            stale: false,
        })
    }

    pub fn record_error(&self, message: &str) {
        let _ = self.write_state(&PricingState {
            last_checked_at: Some(now_seconds()),
            last_error: Some(message.to_string()),
        });
    }

    pub fn list_overrides(&self) -> Result<Vec<ModelPriceOverride>, String> {
        Ok(self.read_overrides()?.models)
    }

    pub fn model_override(
        &self,
        provider_id: &str,
        model_id: &str,
    ) -> Result<Option<ModelPriceOverride>, String> {
        Ok(self
            .read_overrides()?
            .models
            .into_iter()
            .find(|item| item.provider_id == provider_id && item.model_id == model_id))
    }

    pub fn save_model_override(
        &self,
        mut price_override: ModelPriceOverride,
    ) -> Result<ModelPriceOverride, String> {
        validate_override(&price_override)?;
        price_override.updated_at = now_seconds();
        let mut overrides = self.read_overrides()?;
        overrides.models.retain(|item| {
            item.provider_id != price_override.provider_id
                || item.model_id != price_override.model_id
        });
        overrides.models.push(price_override.clone());
        self.write_overrides(&overrides)?;
        Ok(price_override)
    }

    pub fn delete_model_override(&self, provider_id: &str, model_id: &str) -> Result<bool, String> {
        let mut overrides = self.read_overrides()?;
        let before = overrides.models.len();
        overrides
            .models
            .retain(|item| item.provider_id != provider_id || item.model_id != model_id);
        if overrides.models.len() == before {
            return Ok(false);
        }
        self.write_overrides(&overrides)?;
        Ok(true)
    }

    pub fn provider_preference(
        &self,
        provider_id: &str,
    ) -> Result<ProviderPricingPreference, String> {
        Ok(self
            .read_overrides()?
            .providers
            .into_iter()
            .find(|item| item.provider_id == provider_id)
            .unwrap_or_else(|| ProviderPricingPreference {
                provider_id: provider_id.to_string(),
                mode: ProviderPricingMode::Auto,
                multiplier_basis_points: 10_000,
            }))
    }

    pub fn save_provider_preference(
        &self,
        preference: ProviderPricingPreference,
    ) -> Result<ProviderPricingPreference, String> {
        if preference.provider_id.trim().is_empty()
            || preference.multiplier_basis_points == 0
            || preference.multiplier_basis_points > 1_000_000
        {
            return Err("服务商价格策略或倍率无效".to_string());
        }
        let mut overrides = self.read_overrides()?;
        overrides
            .providers
            .retain(|item| item.provider_id != preference.provider_id);
        overrides.providers.push(preference.clone());
        self.write_overrides(&overrides)?;
        Ok(preference)
    }

    pub async fn refresh_from_urls(&self, urls: &[String]) -> Result<PricingCatalogStatus, String> {
        if urls.is_empty() {
            return Err("尚未配置价格目录镜像；可复用更新发布源或在设置中添加镜像".to_string());
        }
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(3))
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|e| format!("创建价格目录客户端失败：{e}"))?;
        let mut failures = Vec::new();
        for (index, raw_url) in urls.iter().enumerate() {
            let url = raw_url.trim();
            if url.is_empty() {
                continue;
            }
            let signature_url = format!("{url}.sig");
            let result = async {
                let catalog_response = client
                    .get(url)
                    .send()
                    .await
                    .map_err(|_| "目录请求失败（地址已脱敏）".to_string())?;
                if !catalog_response.status().is_success() {
                    return Err(format!(
                        "目录返回 HTTP {}",
                        catalog_response.status().as_u16()
                    ));
                }
                if catalog_response.content_length().unwrap_or_default() > MAX_CATALOG_BYTES as u64
                {
                    return Err("目录超过 2 MiB 上限".to_string());
                }
                let catalog =
                    read_response_limited(catalog_response, MAX_CATALOG_BYTES, "价格目录").await?;
                let signature_response = client
                    .get(&signature_url)
                    .send()
                    .await
                    .map_err(|_| "签名请求失败（地址已脱敏）".to_string())?;
                if !signature_response.status().is_success() {
                    return Err(format!(
                        "签名返回 HTTP {}",
                        signature_response.status().as_u16()
                    ));
                }
                if signature_response.content_length().unwrap_or_default() > 64 * 1024 {
                    return Err("签名文件超过 64 KiB 上限".to_string());
                }
                let signature =
                    read_response_limited(signature_response, 64 * 1024, "签名").await?;
                let signature =
                    String::from_utf8(signature).map_err(|_| "签名文件不是 UTF-8".to_string())?;
                self.install_verified(&catalog, &signature)
            }
            .await;
            match result {
                Ok(status) => return Ok(status),
                Err(error) => failures.push(format!("镜像 {}：{error}", index + 1)),
            }
        }
        let message = if failures.is_empty() {
            "没有可用的价格目录镜像".to_string()
        } else {
            failures.join("；")
        };
        self.record_error(&message);
        Err(message)
    }

    pub fn import_from_files(
        &self,
        catalog_path: &Path,
        signature_path: &Path,
    ) -> Result<PricingCatalogStatus, String> {
        let catalog = read_file_limited(catalog_path, MAX_CATALOG_BYTES, "价格目录")?;
        let signature = read_file_limited(signature_path, 64 * 1024, "价格目录签名")?;
        let signature =
            String::from_utf8(signature).map_err(|_| "价格目录签名文件不是 UTF-8".to_string())?;
        self.install_verified(&catalog, &signature)
    }

    fn read_verified_pair(
        &self,
        catalog_path: &Path,
        signature_path: &Path,
    ) -> Result<PricingCatalog, String> {
        let catalog = read_file_limited(catalog_path, MAX_CATALOG_BYTES, "缓存价格目录")?;
        let signature = read_file_limited(signature_path, 64 * 1024, "缓存价格目录签名")?;
        let signature =
            String::from_utf8(signature).map_err(|_| "缓存价格目录签名不是 UTF-8".to_string())?;
        verify_minisign(&catalog, &signature, &self.public_key_base64)?;
        parse_and_validate_catalog(&catalog)
    }

    fn read_verified_cache(&self) -> Result<PricingCatalog, String> {
        if self.cache_bundle_path().exists() {
            let raw = read_file_limited(
                &self.cache_bundle_path(),
                MAX_CATALOG_BYTES + 128 * 1024,
                "价格缓存",
            )?;
            let envelope: PricingCacheEnvelope =
                serde_json::from_slice(&raw).map_err(|e| format!("解析价格缓存失败：{e}"))?;
            verify_minisign(
                envelope.catalog_json.as_bytes(),
                &envelope.signature,
                &self.public_key_base64,
            )?;
            return parse_and_validate_catalog(envelope.catalog_json.as_bytes());
        }
        self.read_verified_pair(&self.catalog_path(), &self.signature_path())
    }

    fn read_state(&self) -> Result<PricingState, String> {
        let raw = fs::read_to_string(self.state_path()).map_err(|e| e.to_string())?;
        serde_json::from_str(&raw).map_err(|e| e.to_string())
    }

    fn write_state(&self, state: &PricingState) -> Result<(), String> {
        fs::create_dir_all(&self.directory).map_err(|e| format!("创建价格目录失败：{e}"))?;
        let raw =
            serde_json::to_vec_pretty(state).map_err(|e| format!("序列化价格状态失败：{e}"))?;
        write_atomically_bytes(&self.state_path(), &raw)
    }

    fn read_overrides(&self) -> Result<PricingOverrides, String> {
        if !self.overrides_path().exists() {
            return Ok(PricingOverrides::default());
        }
        let raw = fs::read_to_string(self.overrides_path())
            .map_err(|e| format!("读取价格覆盖失败：{e}"))?;
        serde_json::from_str(&raw).map_err(|e| format!("解析价格覆盖失败：{e}"))
    }

    fn write_overrides(&self, overrides: &PricingOverrides) -> Result<(), String> {
        fs::create_dir_all(&self.directory).map_err(|e| format!("创建价格目录失败：{e}"))?;
        let raw =
            serde_json::to_vec_pretty(overrides).map_err(|e| format!("序列化价格覆盖失败：{e}"))?;
        write_atomically_bytes(&self.overrides_path(), &raw)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PricingState {
    last_checked_at: Option<i64>,
    last_error: Option<String>,
}

pub fn verify_minisign(
    content: &[u8],
    signature_text: &str,
    public_key_base64: &str,
) -> Result<(), String> {
    let key = PublicKey::from_base64(public_key_base64)
        .map_err(|e| format!("解析价格目录公钥失败：{e}"))?;
    let signature_text = normalize_signature_text(signature_text)?;
    let signature =
        Signature::decode(&signature_text).map_err(|e| format!("解析价格目录签名失败：{e}"))?;
    key.verify(content, &signature, false)
        .map_err(|e| format!("价格目录签名验证失败：{e}"))
}

/// Tauri signer 的 `.sig` 是 Base64 包裹的 minisign 文本；离线运维也可能直接
/// 提供裸 minisign 文件。两种格式都接受，但解码后必须仍是标准 minisign 文本。
fn normalize_signature_text(signature_text: &str) -> Result<String, String> {
    let trimmed = signature_text.trim();
    if trimmed.starts_with("untrusted comment:") {
        return Ok(trimmed.to_string());
    }
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(trimmed.as_bytes())
        .map_err(|_| "价格目录签名既不是 minisign 文本，也不是 Tauri Base64 签名".to_string())?;
    let decoded =
        String::from_utf8(decoded).map_err(|_| "Tauri Base64 签名解码后不是 UTF-8".to_string())?;
    if !decoded.trim_start().starts_with("untrusted comment:") {
        return Err("Tauri Base64 签名解码后不是 minisign 文本".to_string());
    }
    Ok(decoded.trim().to_string())
}

pub fn parse_and_validate_catalog(bytes: &[u8]) -> Result<PricingCatalog, String> {
    if bytes.is_empty() || bytes.len() > MAX_CATALOG_BYTES {
        return Err("价格目录为空或超过 2 MiB 上限".to_string());
    }
    let catalog: PricingCatalog =
        serde_json::from_slice(bytes).map_err(|e| format!("解析价格目录失败：{e}"))?;
    if catalog.schema_version != 1 {
        return Err(format!(
            "不支持的价格目录 schemaVersion：{}",
            catalog.schema_version
        ));
    }
    if catalog.catalog_version.trim().is_empty() || catalog.published_at.trim().is_empty() {
        return Err("价格目录缺少版本或发布时间".to_string());
    }
    if catalog.models.is_empty() || catalog.models.len() > MAX_CATALOG_MODELS {
        return Err("价格目录模型数量无效".to_string());
    }
    let mut identities = std::collections::HashSet::new();
    for card in &catalog.models {
        for model_id in std::iter::once(&card.model_id).chain(card.aliases.iter()) {
            let normalized = normalize_model_id(model_id);
            if normalized.is_empty() {
                return Err(format!("模型 {} 包含空模型 ID 或别名", card.model_id));
            }
            let identity = format!("{}:{normalized}", card.vendor.to_ascii_lowercase());
            if !identities.insert(identity.clone()) {
                return Err(format!("价格目录包含重复模型或别名：{identity}"));
            }
        }
        if card.currency != "USD" || card.unit != "per-million-tokens" || card.tiers.is_empty() {
            return Err(format!("模型 {} 的币种、单位或服务层级无效", card.model_id));
        }
        for tier in card.tiers.values() {
            if tier.bands.is_empty() {
                return Err(format!("模型 {} 存在空价格区间", card.model_id));
            }
            validate_band_ranges(&tier.bands, &card.model_id)?;
            for band in &tier.bands {
                let values = [
                    Some(band.input),
                    band.cached_input,
                    band.cache_write,
                    Some(band.output),
                ];
                if values
                    .into_iter()
                    .flatten()
                    .any(|value| !value.is_finite() || value < 0.0)
                {
                    return Err(format!("模型 {} 包含非法价格", card.model_id));
                }
                if let (Some(min), Some(max)) = (band.min_input_tokens, band.max_input_tokens) {
                    if min > max {
                        return Err(format!("模型 {} 的价格区间上下界颠倒", card.model_id));
                    }
                }
            }
        }
    }
    Ok(catalog)
}

fn validate_override(price_override: &ModelPriceOverride) -> Result<(), String> {
    if price_override.provider_id.trim().is_empty()
        || price_override.model_id.trim().is_empty()
        || price_override.currency != "USD"
        || price_override.tiers.is_empty()
    {
        return Err("模型价格覆盖缺少服务商、模型、USD 币种或服务层级".to_string());
    }
    for tier in price_override.tiers.values() {
        if tier.bands.is_empty() {
            return Err("模型价格覆盖包含空价格区间".to_string());
        }
        validate_band_ranges(&tier.bands, &price_override.model_id)?;
        for band in &tier.bands {
            if [
                Some(band.input),
                band.cached_input,
                band.cache_write,
                Some(band.output),
            ]
            .into_iter()
            .flatten()
            .any(|value| !value.is_finite() || value < 0.0)
            {
                return Err("模型价格覆盖包含非法价格".to_string());
            }
        }
    }
    Ok(())
}

fn validate_band_ranges(bands: &[PricingBand], model_id: &str) -> Result<(), String> {
    let mut previous_max = None;
    for (index, band) in bands.iter().enumerate() {
        if index == 0 {
            if band.min_input_tokens.unwrap_or(0) != 0 {
                return Err(format!("模型 {model_id} 的首个价格区间没有从 0 开始"));
            }
        } else {
            let expected_min = previous_max
                .and_then(|value: u64| value.checked_add(1))
                .ok_or_else(|| format!("模型 {model_id} 在无上限区间之后仍有价格区间"))?;
            if band.min_input_tokens != Some(expected_min) {
                return Err(format!("模型 {model_id} 的价格区间存在空洞或重叠"));
            }
        }
        if index + 1 < bands.len() && band.max_input_tokens.is_none() {
            return Err(format!("模型 {model_id} 的非末尾价格区间缺少上限"));
        }
        previous_max = band.max_input_tokens;
    }
    Ok(())
}

pub fn vendor_for_protocol(protocol: &Protocol) -> &'static str {
    match protocol {
        Protocol::Anthropic => "anthropic",
        Protocol::OpenAiResponses | Protocol::OpenAiChat => "openai",
        Protocol::Bedrock | Protocol::Vertex => "",
    }
}

fn vendor_matches(card_vendor: &str, protocol_vendor: &str, normalized_model_id: &str) -> bool {
    card_vendor.eq_ignore_ascii_case(protocol_vendor)
        || (normalized_model_id.starts_with("minimax-")
            && card_vendor.eq_ignore_ascii_case("minimax"))
}

pub fn normalize_model_id(model_id: &str) -> String {
    model_id
        .trim()
        .to_ascii_lowercase()
        .replace('@', "-")
        .trim_start_matches("models/")
        .trim_start_matches("anthropic/")
        .trim_start_matches("openai/")
        .to_string()
}

fn band_matches(band: &PricingBand, input_tokens: u64) -> bool {
    band.min_input_tokens
        .map(|min| input_tokens >= min)
        .unwrap_or(true)
        && band
            .max_input_tokens
            .map(|max| input_tokens <= max)
            .unwrap_or(true)
}

fn write_atomically_bytes(path: &Path, contents: &[u8]) -> Result<(), String> {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let tmp = path.with_extension(format!("tmp-{}-{nonce}", std::process::id()));
    fs::write(&tmp, contents).map_err(|e| format!("写入临时文件失败：{e}"))?;
    let result = replace_file_atomically(&tmp, path);
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

fn read_file_limited(path: &Path, limit: usize, label: &str) -> Result<Vec<u8>, String> {
    let metadata = fs::metadata(path).map_err(|e| format!("读取{label}失败：{e}"))?;
    if metadata.len() > limit as u64 {
        return Err(format!("{label}超过大小上限"));
    }
    let contents = fs::read(path).map_err(|e| format!("读取{label}失败：{e}"))?;
    if contents.len() > limit {
        return Err(format!("{label}超过大小上限"));
    }
    Ok(contents)
}

#[cfg(windows)]
fn replace_file_atomically(source: &Path, destination: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };

    let source = source
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let destination = destination
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let moved = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if moved == 0 {
        return Err(format!("替换文件失败：{}", std::io::Error::last_os_error()));
    }
    Ok(())
}

#[cfg(not(windows))]
fn replace_file_atomically(source: &Path, destination: &Path) -> Result<(), String> {
    fs::rename(source, destination).map_err(|e| format!("替换文件失败：{e}"))
}

async fn read_response_limited(
    mut response: reqwest::Response,
    limit: usize,
    label: &str,
) -> Result<Vec<u8>, String> {
    let mut output = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| format!("读取{label}失败"))?
    {
        if output.len().saturating_add(chunk.len()) > limit {
            return Err(format!("{label}超过大小上限"));
        }
        output.extend_from_slice(&chunk);
    }
    Ok(output)
}

fn catalog_is_stale(published_at: &str, max_age_days: u32) -> bool {
    let Some(published) = utc_date_seconds(published_at) else {
        return true;
    };
    now_seconds().saturating_sub(published) > i64::from(max_age_days) * 86_400
}

fn utc_date_seconds(value: &str) -> Option<i64> {
    let date = value.get(0..10)?;
    let mut parts = date.split('-');
    let year = parts.next()?.parse::<i64>().ok()?;
    let month = parts.next()?.parse::<i64>().ok()?;
    let day = parts.next()?.parse::<i64>().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let adjusted_year = year - i64::from(month <= 2);
    let era = if adjusted_year >= 0 {
        adjusted_year
    } else {
        adjusted_year - 399
    } / 400;
    let year_of_era = adjusted_year - era * 400;
    let adjusted_month = month + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * adjusted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    Some((era * 146_097 + day_of_era - 719_468) * 86_400)
}

pub fn catalog_urls_from_settings(settings: &AppSettings) -> Vec<String> {
    let mut urls = settings
        .general
        .pricing_feed_urls
        .iter()
        .map(|url| url.trim().to_string())
        .filter(|url| !url.is_empty())
        .collect::<Vec<_>>();
    if urls.is_empty() {
        let update_feed = settings.general.update_feed_url.trim();
        if let Ok(mut url) = reqwest::Url::parse(update_feed) {
            if let Ok(mut segments) = url.path_segments_mut() {
                segments.pop_if_empty();
                segments.pop();
                segments.push("pricing-catalog.json");
            }
            urls.push(url.to_string());
        }
    }
    urls.dedup();
    urls
}

#[tauri::command]
pub fn get_pricing_catalog_status(
    store: State<'_, PricingCatalogStore>,
    history_store: State<'_, SessionHistoryStore>,
) -> Result<PricingCatalogStatus, String> {
    let settings = load_app_settings_from_store(&history_store)?;
    store.active_catalog().map(|(catalog, mut status)| {
        status.stale =
            catalog_is_stale(&catalog.published_at, settings.general.pricing_max_age_days);
        status
    })
}

#[tauri::command]
pub async fn refresh_pricing_catalog(
    store: State<'_, PricingCatalogStore>,
    history_store: State<'_, SessionHistoryStore>,
) -> Result<PricingCatalogStatus, String> {
    let settings = load_app_settings_from_store(&history_store)?;
    let urls = catalog_urls_from_settings(&settings);
    store.refresh_from_urls(&urls).await
}

#[tauri::command]
pub fn import_pricing_catalog(
    store: State<'_, PricingCatalogStore>,
    catalog_path: String,
    signature_path: Option<String>,
) -> Result<PricingCatalogStatus, String> {
    let catalog = PathBuf::from(catalog_path);
    let signature = signature_path
        .filter(|path| !path.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(format!("{}.sig", catalog.display())));
    store.import_from_files(&catalog, &signature)
}

#[tauri::command]
pub fn list_model_price_overrides(
    store: State<'_, PricingCatalogStore>,
) -> Result<Vec<ModelPriceOverride>, String> {
    store.list_overrides()
}

#[tauri::command]
pub fn save_model_price_override(
    store: State<'_, PricingCatalogStore>,
    price_override: ModelPriceOverride,
) -> Result<ModelPriceOverride, String> {
    store.save_model_override(price_override)
}

#[tauri::command]
pub fn delete_model_price_override(
    store: State<'_, PricingCatalogStore>,
    provider_id: String,
    model_id: String,
) -> Result<bool, String> {
    store.delete_model_override(&provider_id, &model_id)
}

#[tauri::command]
pub fn get_provider_pricing_preference(
    store: State<'_, PricingCatalogStore>,
    provider_id: String,
) -> Result<ProviderPricingPreference, String> {
    store.provider_preference(&provider_id)
}

#[tauri::command]
pub fn save_provider_pricing_preference(
    store: State<'_, PricingCatalogStore>,
    preference: ProviderPricingPreference,
) -> Result<ProviderPricingPreference, String> {
    store.save_provider_preference(preference)
}

pub fn spawn_background_refresh(
    _app: AppHandle,
    store: PricingCatalogStore,
    history_store: SessionHistoryStore,
) {
    tauri::async_runtime::spawn(async move {
        let Ok(settings) = load_app_settings_from_store(&history_store) else {
            return;
        };
        if !settings.general.pricing_auto_update {
            return;
        }
        let status = store.active_catalog().ok().map(|(_, status)| status);
        let checked_recently = status
            .and_then(|status| status.last_checked_at)
            .map(|checked| now_seconds().saturating_sub(checked) < 24 * 60 * 60)
            .unwrap_or(false);
        if checked_recently {
            return;
        }
        let urls = catalog_urls_from_settings(&settings);
        if urls.is_empty() {
            return;
        }
        let _ = store.refresh_from_urls(&urls).await;
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    const TEST_PUBLIC_KEY: &str = "RWTTiV0bzI4j8gp43kVpne5Wp2GxEu0SLAMz0K1EFCT8R+46Q0hxktxA";
    const TEST_CATALOG: &[u8] = include_bytes!("../tests/fixtures/pricing-catalog.json");
    const TEST_SIGNATURE: &str = include_str!("../tests/fixtures/pricing-catalog.json.sig");

    fn test_directory(name: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "helm-pricing-{name}-{}-{nonce}",
            std::process::id()
        ))
    }

    async fn spawn_catalog_server(
        catalog: Vec<u8>,
        signature: Vec<u8>,
        catalog_length_override: Option<usize>,
        request_count: usize,
    ) -> (String, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            for _ in 0..request_count {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = vec![0_u8; 4096];
                let read = socket.read(&mut request).await.unwrap();
                let request = String::from_utf8_lossy(&request[..read]);
                let path = request.split_whitespace().nth(1).unwrap_or("/");
                let is_signature = path.ends_with(".sig");
                let body = if is_signature { &signature } else { &catalog };
                let content_length = if is_signature {
                    body.len()
                } else {
                    catalog_length_override.unwrap_or(body.len())
                };
                let header = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {content_length}\r\nConnection: close\r\n\r\n"
                );
                socket.write_all(header.as_bytes()).await.unwrap();
                if catalog_length_override.is_none() || is_signature {
                    socket.write_all(body).await.unwrap();
                }
                socket.shutdown().await.unwrap();
            }
        });
        (format!("http://{address}/pricing-catalog.json"), task)
    }

    #[test]
    fn builtin_catalog_resolves_gpt_56_alias_and_long_context_band() {
        let catalog = PricingCatalog::builtin().unwrap();
        let base = catalog
            .resolve(
                &Protocol::OpenAiResponses,
                "gpt-5.6",
                ServiceTier::Standard,
                10,
            )
            .unwrap();
        assert_eq!(base.canonical_model_id, "gpt-5.6-sol");
        assert_eq!(base.band.input, 5.0);
        assert_eq!(base.band.cache_write, Some(6.25));

        let long = catalog
            .resolve(
                &Protocol::OpenAiResponses,
                "gpt-5.6-sol",
                ServiceTier::Standard,
                272_001,
            )
            .unwrap();
        assert_eq!(long.band.input, 10.0);
        assert_eq!(long.band.output, 45.0);

        let terra_batch_long = catalog
            .resolve(
                &Protocol::OpenAiResponses,
                "gpt-5.6-terra",
                ServiceTier::Batch,
                272_001,
            )
            .unwrap();
        assert_eq!(terra_batch_long.band.input, 2.5);
        assert_eq!(terra_batch_long.band.cache_write, Some(3.125));
        assert_eq!(terra_batch_long.band.output, 11.25);
    }

    #[test]
    fn aliases_are_explicit_not_fuzzy() {
        let catalog = PricingCatalog::builtin().unwrap();
        assert!(catalog
            .resolve(
                &Protocol::OpenAiResponses,
                "gpt-5.6-sol-extra",
                ServiceTier::Standard,
                0,
            )
            .is_none());
        assert_eq!(
            catalog
                .resolve(
                    &Protocol::OpenAiResponses,
                    "gpt-5.6-terra",
                    ServiceTier::Standard,
                    0,
                )
                .unwrap()
                .band
                .output,
            15.0
        );
    }

    #[test]
    fn rejects_duplicate_or_negative_catalog_entries() {
        let duplicate = br#"{
          "schemaVersion":1,"catalogVersion":"x","sequence":1,"publishedAt":"x",
          "models":[
            {"vendor":"openai","modelId":"m","currency":"USD","unit":"per-million-tokens","sourceUrl":"x","observedAt":"x","tiers":{"standard":{"bands":[{"input":1,"output":1}]}}},
            {"vendor":"openai","modelId":"m","currency":"USD","unit":"per-million-tokens","sourceUrl":"x","observedAt":"x","tiers":{"standard":{"bands":[{"input":-1,"output":1}]}}}
          ]}"#;
        assert!(parse_and_validate_catalog(duplicate)
            .unwrap_err()
            .contains("重复模型"));

        let overlapping = br#"{
          "schemaVersion":1,"catalogVersion":"x","sequence":1,"publishedAt":"2026-07-17",
          "models":[
            {"vendor":"openai","modelId":"m","currency":"USD","unit":"per-million-tokens","sourceUrl":"x","observedAt":"x","tiers":{"standard":{"bands":[
              {"maxInputTokens":100,"input":1,"output":1},
              {"minInputTokens":100,"input":2,"output":2}
            ]}}}
          ]}"#;
        assert!(parse_and_validate_catalog(overlapping)
            .unwrap_err()
            .contains("空洞或重叠"));
    }

    #[test]
    fn verifies_prehashed_minisign_and_rejects_tampering() {
        let key = "RWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3";
        let signature = "untrusted comment: signature from minisign secret key
RUQf6LRCGA9i559r3g7V1qNyJDApGip8MfqcadIgT9CuhV3EMhHoN1mGTkUidF/z7SrlQgXdy8ofjb7bNJJylDOocrCo8KLzZwo=
trusted comment: timestamp:1556193335\tfile:test
y/rUw2y8/hOUYjZU71eHp/Wo1KZ40fGy2VJEDl34XMJM+TX48Ss/17u3IvIfbVR1FkZZSNCisQbuQY+bHwhEBg==";
        verify_minisign(b"test", signature, key).unwrap();
        assert!(verify_minisign(b"tampered", signature, key).is_err());
        verify_minisign(TEST_CATALOG, TEST_SIGNATURE, TEST_PUBLIC_KEY).unwrap();
    }

    #[test]
    fn offline_store_falls_back_to_builtin_and_manual_override_wins() {
        let directory = std::env::temp_dir().join(format!(
            "helm-pricing-store-{}-{}",
            std::process::id(),
            now_seconds()
        ));
        let _ = fs::remove_dir_all(&directory);
        let store = PricingCatalogStore::new(directory.clone());
        let (catalog, status) = store.active_catalog().unwrap();
        assert_eq!(status.source, "builtin");
        let mut tiers = HashMap::new();
        tiers.insert(
            ServiceTier::Standard,
            PricingTier {
                bands: vec![PricingBand {
                    min_input_tokens: None,
                    max_input_tokens: None,
                    input: 7.0,
                    cached_input: Some(0.7),
                    cache_write: Some(8.75),
                    output: 42.0,
                }],
            },
        );
        store
            .save_model_override(ModelPriceOverride {
                provider_id: "gateway".to_string(),
                model_id: "gpt-5.6-sol".to_string(),
                currency: "USD".to_string(),
                tiers,
                updated_at: 0,
            })
            .unwrap();
        let resolved = store
            .resolve_for_provider(
                &catalog,
                &Protocol::OpenAiResponses,
                "gateway",
                "gpt-5.6-sol",
                ServiceTier::Standard,
                1,
            )
            .unwrap()
            .unwrap();
        assert_eq!(resolved.vendor, "manual");
        assert_eq!(resolved.band.input, 7.0);
        assert_eq!(resolved.band.output, 42.0);
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn derives_pricing_feed_from_update_feed_without_openai_dependency() {
        let mut settings = AppSettings::default();
        settings.general.update_feed_url = "https://cn.example.com/helm/latest.json".to_string();
        assert_eq!(
            catalog_urls_from_settings(&settings),
            vec!["https://cn.example.com/helm/pricing-catalog.json"]
        );
        settings.general.pricing_feed_urls = vec![
            "https://oss.example.cn/pricing-catalog.json".to_string(),
            "https://cos.example.cn/pricing-catalog.json".to_string(),
        ];
        assert_eq!(catalog_urls_from_settings(&settings).len(), 2);
    }

    #[tokio::test]
    async fn remote_refresh_falls_back_to_second_mirror_and_survives_restart() {
        let directory = test_directory("mirror-fallback");
        let _ = fs::remove_dir_all(&directory);
        let store = PricingCatalogStore::with_public_key(directory.clone(), TEST_PUBLIC_KEY);
        let (fallback_url, server) = spawn_catalog_server(
            TEST_CATALOG.to_vec(),
            TEST_SIGNATURE.as_bytes().to_vec(),
            None,
            2,
        )
        .await;

        let status = store
            .refresh_from_urls(&["not-a-valid-url".to_string(), fallback_url])
            .await
            .unwrap();
        assert_eq!(status.source, "cache");
        assert_eq!(status.catalog_version, "test.2026.07.17.99");
        assert!(store.cache_bundle_path().exists());
        server.await.unwrap();

        store
            .install_verified(TEST_CATALOG, TEST_SIGNATURE)
            .expect("Windows 上重复刷新必须原子替换现有缓存和状态文件");

        let restarted = PricingCatalogStore::with_public_key(directory.clone(), TEST_PUBLIC_KEY);
        let (catalog, status) = restarted.active_catalog().unwrap();
        assert_eq!(status.source, "cache");
        assert_eq!(catalog.catalog_version, "test.2026.07.17.99");
        let _ = fs::remove_dir_all(directory);
    }

    #[tokio::test]
    async fn invalid_remote_signature_keeps_last_verified_cache() {
        let directory = test_directory("bad-signature");
        let _ = fs::remove_dir_all(&directory);
        let store = PricingCatalogStore::with_public_key(directory.clone(), TEST_PUBLIC_KEY);
        store
            .install_verified(TEST_CATALOG, TEST_SIGNATURE)
            .unwrap();
        let before = fs::read(store.cache_bundle_path()).unwrap();
        let (url, server) =
            spawn_catalog_server(TEST_CATALOG.to_vec(), b"not-a-signature".to_vec(), None, 2).await;

        let error = store.refresh_from_urls(&[url]).await.unwrap_err();
        assert!(error.contains("镜像 1"));
        assert_eq!(fs::read(store.cache_bundle_path()).unwrap(), before);
        assert_eq!(store.active_catalog().unwrap().1.source, "cache");
        server.await.unwrap();
        let _ = fs::remove_dir_all(directory);
    }

    #[tokio::test]
    async fn oversized_remote_catalog_is_rejected_before_reading_body() {
        let directory = test_directory("oversized");
        let _ = fs::remove_dir_all(&directory);
        let store = PricingCatalogStore::with_public_key(directory.clone(), TEST_PUBLIC_KEY);
        let (url, server) =
            spawn_catalog_server(Vec::new(), Vec::new(), Some(MAX_CATALOG_BYTES + 1), 1).await;

        let error = store.refresh_from_urls(&[url.clone()]).await.unwrap_err();
        assert!(error.contains("超过 2 MiB 上限"));
        assert!(!error.contains(&url), "错误信息不得回显镜像 URL");
        server.await.unwrap();
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn oversized_offline_import_is_rejected_before_loading_file() {
        let directory = test_directory("oversized-import");
        fs::create_dir_all(&directory).unwrap();
        let catalog_path = directory.join("oversized.json");
        let signature_path = directory.join("oversized.json.sig");
        let file = fs::File::create(&catalog_path).unwrap();
        file.set_len((MAX_CATALOG_BYTES + 1) as u64).unwrap();
        fs::write(&signature_path, TEST_SIGNATURE).unwrap();
        let store = PricingCatalogStore::with_public_key(directory.clone(), TEST_PUBLIC_KEY);

        let error = store
            .import_from_files(&catalog_path, &signature_path)
            .unwrap_err();
        assert!(error.contains("超过大小上限"));
        assert!(!store.cache_bundle_path().exists());
        let _ = fs::remove_dir_all(directory);
    }
}
