import { invoke } from '@tauri-apps/api/core';
import type {
  AppConfig,
  AuthMethod,
  Binding,
  Engine,
  FailureCategory,
  Model,
  Protocol,
  Provider,
  ProviderKind,
  ProviderTest,
  ModelPriceOverride,
  PricingCatalogStatus,
  ProviderPricingPreference,
} from '@helm/protocol';

export type EngineStatus = 'ready' | 'missing' | 'error';
export type ProviderConfig = Provider;
export type { ProviderKind };
export type ProviderTestConfig = ProviderTest;
export type ModelConfig = Model;
export type EngineConfig = Engine;
export type BindingConfig = Binding;
export type ProviderProtocol = Protocol;
export type ProviderAuthMethod = AuthMethod;
export type { AppConfig, FailureCategory, ProviderTest };
export type { ModelPriceOverride, PricingCatalogStatus, ProviderPricingPreference };

export interface ConnectionResult {
  ok: boolean;
  verified: boolean;
  message: string;
  latencyMs: number;
}

/** CLI 登录态（订阅登录一等公民，P3-1） */
export interface CliLoginState {
  state: 'ok' | 'missing' | 'expired' | 'unknown';
  authMethod?: 'subscription' | 'apikey' | 'unknown' | null;
  accountLabel?: string | null;
  plan?: string | null;
  detail: string;
}

export function detectCliLogin(engine: 'claude-code' | 'codex'): Promise<CliLoginState> {
  return invoke<CliLoginState>('detect_cli_login', { engine });
}

export function loginCliAccount(engine: 'claude-code' | 'codex'): Promise<CliLoginState> {
  return invoke<CliLoginState>('login_cli_account', { engine });
}

export function logoutCliAccount(engine: 'claude-code' | 'codex'): Promise<CliLoginState> {
  return invoke<CliLoginState>('logout_cli_account', { engine });
}

export interface EngineConfigFile {
  path: string;
  content: string;
}

export function getProviderConfig(): Promise<AppConfig> {
  return invoke<AppConfig>('get_provider_config');
}

export function revealProviderSecret(providerId: string): Promise<string> {
  return invoke<string>('reveal_provider_secret', { providerId });
}

export function saveProviderConfig(provider: ProviderConfig, apiKey?: string): Promise<AppConfig> {
  return invoke<AppConfig>('save_provider_config', {
    provider,
    apiKey: apiKey && apiKey.trim() ? apiKey.trim() : null,
  });
}

export function deleteProviderConfig(providerId: string): Promise<AppConfig> {
  return invoke<AppConfig>('delete_provider_config', { providerId });
}

export function saveEngineConfig(engine: EngineConfig): Promise<AppConfig> {
  return invoke<AppConfig>('save_engine_config', { engine });
}

export function saveModelConfig(model: ModelConfig): Promise<AppConfig> {
  return invoke<AppConfig>('save_model_config', { model });
}

export function saveProviderModelSelection(
  providerId: string,
  enabledModelIds: string[],
): Promise<AppConfig> {
  return invoke<AppConfig>('save_provider_model_selection', { providerId, enabledModelIds });
}

export function syncProviderModels(providerId: string): Promise<AppConfig> {
  return invoke<AppConfig>('sync_provider_models_config', { providerId });
}

export function saveBindingConfig(binding: BindingConfig): Promise<AppConfig> {
  return invoke<AppConfig>('save_binding_config', { binding });
}

export function getEquivalentEnv(binding: BindingConfig): Promise<[string, string][]> {
  return invoke<[string, string][]>('get_equivalent_env', { binding });
}

export function readEngineConfigFile(engineId: string): Promise<EngineConfigFile> {
  return invoke<EngineConfigFile>('read_engine_config_file', { engineId });
}

export function writeEngineConfigFile(
  engineId: string,
  content: string,
): Promise<EngineConfigFile> {
  return invoke<EngineConfigFile>('write_engine_config_file', { engineId, content });
}

export function testProviderConfig(providerId: string): Promise<ConnectionResult> {
  return invoke<ConnectionResult>('test_provider_config', { providerId });
}

export function testEngineConfig(bin: string): Promise<ConnectionResult> {
  return invoke<ConnectionResult>('test_engine_config', { bin });
}

export function getPricingCatalogStatus(): Promise<PricingCatalogStatus> {
  return invoke<PricingCatalogStatus>('get_pricing_catalog_status');
}

export function refreshPricingCatalog(): Promise<PricingCatalogStatus> {
  return invoke<PricingCatalogStatus>('refresh_pricing_catalog');
}

export function importPricingCatalog(
  catalogPath: string,
  signaturePath?: string,
): Promise<PricingCatalogStatus> {
  return invoke<PricingCatalogStatus>('import_pricing_catalog', {
    catalogPath,
    signaturePath: signaturePath ?? null,
  });
}

export function listModelPriceOverrides(): Promise<ModelPriceOverride[]> {
  return invoke<ModelPriceOverride[]>('list_model_price_overrides');
}

export function saveModelPriceOverride(
  priceOverride: ModelPriceOverride,
): Promise<ModelPriceOverride> {
  return invoke<ModelPriceOverride>('save_model_price_override', { priceOverride });
}

export function deleteModelPriceOverride(providerId: string, modelId: string): Promise<boolean> {
  return invoke<boolean>('delete_model_price_override', { providerId, modelId });
}

export function getProviderPricingPreference(
  providerId: string,
): Promise<ProviderPricingPreference> {
  return invoke<ProviderPricingPreference>('get_provider_pricing_preference', { providerId });
}

export function saveProviderPricingPreference(
  preference: ProviderPricingPreference,
): Promise<ProviderPricingPreference> {
  return invoke<ProviderPricingPreference>('save_provider_pricing_preference', { preference });
}
