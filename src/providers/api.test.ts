import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { invoke } from '@tauri-apps/api/core';
import * as api from './api';
import { buildManualModel, createProviderDraft } from './providerViewModel';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

const provider = createProviderDraft('codex-subscription', 1);
const model = buildManualModel(provider.id, 'official-model', null);
const engine: api.EngineConfig = {
  id: 'codex',
  name: 'Codex',
  bin: 'codex',
  defaultModel: model.id,
  status: 'ready',
  version: null,
};
const binding: api.BindingConfig = {
  engineId: engine.id,
  providerId: provider.id,
  primaryModel: model.id,
  fastModel: null,
};
const config: api.AppConfig = {
  defaultEngine: engine.id,
  defaultModel: model.id,
  providers: [provider],
  models: [model],
  engines: [engine],
  bindings: [binding],
};
const preference: api.ProviderPricingPreference = {
  providerId: provider.id,
  mode: 'auto',
  multiplierBasisPoints: 10000,
};

const configOperations: [string, () => Promise<unknown>][] = [
  ['detect_cli_login', () => api.detectCliLogin('codex')],
  ['login_cli_account', () => api.loginCliAccount('codex')],
  ['logout_cli_account', () => api.logoutCliAccount('codex')],
  ['save_provider_config', () => api.saveProviderConfig(provider)],
  ['delete_provider_config', () => api.deleteProviderConfig(provider.id)],
  ['save_engine_config', () => api.saveEngineConfig(engine)],
  ['save_model_config', () => api.saveModelConfig(model)],
  ['save_provider_model_selection', () => api.saveProviderModelSelection(provider.id, [model.id])],
  ['rename_provider_model', () => api.renameProviderModel(provider.id, 'old', 'new')],
  ['delete_provider_model', () => api.deleteProviderModel(provider.id, model.id)],
  ['save_provider_models_config', () => api.saveProviderModelsConfig(provider.id, [model])],
  ['sync_provider_models_config', () => api.syncProviderModels(provider.id)],
  ['save_binding_config', () => api.saveBindingConfig(binding)],
  ['write_engine_config_file', () => api.writeEngineConfigFile('codex', '{}')],
  ['test_provider_config', () => api.testProviderConfig(provider.id)],
  [
    'test_provider_draft_config',
    () => api.testProviderDraft('http://localhost:1234', '', 'openai-chat'),
  ],
  ['list_provider_models_config', () => api.listProviderModels(provider.id)],
  ['test_engine_config', () => api.testEngineConfig('custom-codex')],
  ['refresh_pricing_catalog', () => api.refreshPricingCatalog()],
  ['import_pricing_catalog', () => api.importPricingCatalog('catalog.json', 'catalog.sig')],
  [
    'save_model_price_override',
    () =>
      api.saveModelPriceOverride({
        providerId: provider.id,
        modelId: model.id,
        currency: 'USD',
        tiers: { standard: { bands: [{ input: 2, output: 8 }] } },
        updatedAt: 1,
      }),
  ],
  ['delete_model_price_override', () => api.deleteModelPriceOverride(provider.id, model.id)],
  ['save_provider_pricing_preference', () => api.saveProviderPricingPreference(preference)],
];

const readOperations: [string, () => Promise<unknown>][] = [
  ['get_provider_config', () => api.getProviderConfig()],
  ['reveal_provider_secret', () => api.revealProviderSecret(provider.id)],
  ['get_equivalent_env', () => api.getEquivalentEnv(binding)],
  ['read_engine_config_file', () => api.readEngineConfigFile('codex')],
  ['get_pricing_catalog_status', () => api.getPricingCatalogStatus()],
  ['get_pricing_catalog_entries', () => api.getPricingCatalogEntries()],
  ['list_model_price_overrides', () => api.listModelPriceOverrides()],
  ['get_provider_pricing_preference', () => api.getProviderPricingPreference(provider.id)],
  ['open_external_url', () => api.openExternalUrl('https://example.com')],
];

beforeEach(() => {
  vi.stubGlobal('window', new EventTarget());
  vi.mocked(invoke).mockReset();
});

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe('配置 API 集中发送失效事件', () => {
  it.each(configOperations)('%s 完成后通知一次，保留权威返回结果', async (command, run) => {
    const changed = vi.fn();
    window.addEventListener(api.PROVIDER_CONFIG_CHANGED_EVENT, changed);
    vi.mocked(invoke).mockResolvedValue(config);
    const result = await run();
    expect(invoke).toHaveBeenCalledTimes(1);
    expect(vi.mocked(invoke).mock.calls[0][0]).toBe(command);
    expect(result).toBe(config);
    expect(changed).toHaveBeenCalledTimes(1);
    expect(changed.mock.calls[0][0].detail).toBeUndefined();
  });

  it.each(configOperations)('%s 拒绝时不伪报配置变化', async (_command, run) => {
    const changed = vi.fn();
    window.addEventListener(api.PROVIDER_CONFIG_CHANGED_EVENT, changed);
    const failure = new Error('command failed before commit');
    vi.mocked(invoke).mockRejectedValue(failure);
    await expect(run()).rejects.toBe(failure);
    expect(changed).not.toHaveBeenCalled();
  });

  it.each(readOperations)('%s 读取不触发失效，避免刷新循环', async (command, run) => {
    const changed = vi.fn();
    window.addEventListener(api.PROVIDER_CONFIG_CHANGED_EVENT, changed);
    vi.mocked(invoke).mockResolvedValue(config);
    expect(await run()).toBe(config);
    expect(vi.mocked(invoke).mock.calls[0][0]).toBe(command);
    expect(changed).not.toHaveBeenCalled();
  });

  it('等待后台完成后才刷新；检测明确不可用时仍刷新旧能力', async () => {
    let complete!: (result: api.CliLoginState) => void;
    vi.mocked(invoke).mockReturnValue(new Promise((resolve) => (complete = resolve)));
    const changed = vi.fn();
    window.addEventListener(api.PROVIDER_CONFIG_CHANGED_EVENT, changed);
    const pending = api.detectCliLogin('codex');
    expect(changed).not.toHaveBeenCalled();
    const state: api.CliLoginState = { state: 'missing', detail: 'not signed in' };
    complete(state);
    expect(await pending).toBe(state);
    expect(changed).toHaveBeenCalledTimes(1);
  });

  it('没有 window 时保持命令语义，不因通知机制失败', async () => {
    vi.stubGlobal('window', undefined);
    vi.mocked(invoke).mockResolvedValue(config);
    await expect(api.saveEngineConfig(engine)).resolves.toBe(config);
  });
});

describe('Provider 原子保存参数', () => {
  it('订阅在一次调用中保留官方 models 的全部字段和最后勾选，不新增协议参数', async () => {
    const rows = [
      { ...model, enabled: false },
      { ...model, id: 'official-second', displayName: 'Official Second', enabled: true },
    ];
    const saved = { ...config, models: rows };
    vi.mocked(invoke).mockResolvedValue(saved);
    const result = await api.saveProviderConfig(provider, undefined, rows);
    expect(invoke).toHaveBeenCalledTimes(1);
    expect(invoke).toHaveBeenCalledWith('save_provider_config', {
      provider,
      apiKey: null,
      models: rows,
      modelRenames: [],
    });
    expect(result).toBe(saved);
    const args = vi.mocked(invoke).mock.calls[0][1] as { models: api.ModelConfig[] };
    expect(args.models).toBe(rows);
  });

  it('省略目录不同于提交空目录；非订阅改名随同一 bundle 提交', async () => {
    vi.mocked(invoke).mockResolvedValue(config);
    await api.saveProviderConfig(provider);
    expect(invoke).toHaveBeenLastCalledWith('save_provider_config', {
      provider,
      apiKey: null,
      models: null,
      modelRenames: [],
    });
    const apiProvider = { ...provider, kind: 'api', authMethod: 'apikey' } as const;
    await api.saveProviderConfig(apiProvider, '  test-key  ', [], [['old', 'new']]);
    expect(invoke).toHaveBeenLastCalledWith('save_provider_config', {
      provider: apiProvider,
      apiKey: 'test-key',
      models: [],
      modelRenames: [['old', 'new']],
    });
  });
});
