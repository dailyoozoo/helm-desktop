import { describe, expect, it } from 'vitest';
import type { AppConfig, ModelConfig, PricingCatalogEntry } from './api';
import {
  applicableEngineLabels,
  canBindProvider,
  compatibleProvidersForEngine,
  buildManualModel,
  catalogPriceMatch,
  createProviderDraft,
  modelAccessGroups,
  modelGroupPriceSummary,
  multiplierInputToBasisPoints,
  normalizeModelGroupId,
  pathBillingLabel,
  providerAccessGroup,
  providerCardStatus,
  templatesForAccessGroup,
  PROVIDER_ACCESS_GROUPS,
  providerDeleteConfirmation,
  providerDeleteBlockedReason,
  providerModelEmptyState,
  envPairsToText,
  failureCategoryLabel,
  lastTestText,
  lastTestTimeText,
  loginStateLabel,
  matchingSubscriptionProvider,
  modelCatalog,
  modelCatalogForProvider,
  mergeSyncedModelSelection,
  modelsForProvider,
  normalizeBindingDraft,
  priceSourceText,
  providerCapabilities,
  providerCanDelete,
  providerModelMode,
  providerFailureCategory,
  providerSetupCopy,
  providerRuntimeReady,
  protocolLabel,
  commitProviderModelRow,
  reachabilityStatus,
  readinessText,
  subscriptionLoginWarning,
  sessionModelOptions,
} from './providerViewModel';

const config: AppConfig = {
  defaultEngine: 'claude-code',
  defaultModel: 'claude-sonnet-4.6',
  providers: [
    {
      id: 'anthropic',
      name: 'Anthropic',
      kind: 'api',
      baseUrl: 'https://api.anthropic.com',
      keyRef: null,
      ready: true,
      lastTest: { result: 'ok', latencyMs: 120, at: 1_717_171_717 },
      protocol: 'anthropic',
      authMethod: 'apikey',
    },
    {
      id: 'openai',
      name: 'OpenAI',
      kind: 'api',
      baseUrl: 'https://api.openai.com/v1',
      keyRef: null,
      ready: true,
      lastTest: null,
      protocol: 'openai-responses',
      authMethod: 'apikey',
    },
    {
      id: 'local',
      name: 'Local',
      kind: 'local',
      baseUrl: 'http://localhost:11434/v1',
      keyRef: null,
      ready: false,
      lastTest: { result: 'fail', latencyMs: 9, at: 1_717_171_718 },
      protocol: 'openai-chat',
      authMethod: 'local',
    },
  ],
  models: [
    {
      id: 'claude-sonnet-4.6',
      providerId: 'anthropic',
      displayName: 'claude-sonnet-4.6',
      inputPricePerMtok: 3,
      outputPricePerMtok: 15,
      enabled: true,
    },
    {
      id: 'gpt-5-codex',
      providerId: 'openai',
      displayName: 'gpt-5-codex',
      inputPricePerMtok: 1.25,
      outputPricePerMtok: 10,
      priceSource: 'builtin',
      enabled: true,
    },
    {
      id: 'gpt-5-mini',
      providerId: 'openai',
      displayName: 'gpt-5-mini',
      inputPricePerMtok: 0.25,
      outputPricePerMtok: 2,
      priceSource: 'builtin',
      enabled: true,
    },
    {
      id: 'disabled-openai-model',
      providerId: 'openai',
      displayName: 'disabled-openai-model',
      inputPricePerMtok: 0.25,
      outputPricePerMtok: 2,
      priceSource: 'manual',
      enabled: false,
    },
  ],
  engines: [
    {
      id: 'claude-code',
      name: 'Claude Code',
      bin: 'claude',
      defaultModel: 'claude-sonnet-4.6',
      status: 'ready',
      version: null,
    },
    {
      id: 'codex',
      name: 'Codex',
      bin: 'codex',
      defaultModel: 'gpt-5-codex',
      status: 'ready',
      version: null,
    },
  ],
  bindings: [
    {
      engineId: 'claude-code',
      providerId: 'anthropic',
      primaryModel: 'claude-sonnet-4.6',
      fastModel: null,
    },
  ],
};

describe('provider view model', () => {
  it('only returns ready providers whose protocol is compatible with the engine', () => {
    expect(
      compatibleProvidersForEngine(config, 'claude-code').map((provider) => provider.id),
    ).toEqual(['anthropic']);
    expect(compatibleProvidersForEngine(config, 'codex').map((provider) => provider.id)).toEqual([
      'openai',
    ]);
  });

  it('filters model choices by provider ownership and enablement', () => {
    expect(modelsForProvider(config, 'openai').map((model) => model.id)).toEqual([
      'gpt-5-codex',
      'gpt-5-mini',
    ]);
  });

  it('normalizes binding models to the selected provider when switching providers', () => {
    expect(
      normalizeBindingDraft(config, {
        engineId: 'codex',
        providerId: 'openai',
        primaryModel: 'claude-sonnet-4.6',
        fastModel: 'disabled-openai-model',
      }),
    ).toEqual({
      engineId: 'codex',
      providerId: 'openai',
      primaryModel: 'gpt-5-codex',
      fastModel: 'gpt-5-mini',
    });
  });

  it('describes which engines can use a provider protocol', () => {
    expect(applicableEngineLabels('anthropic')).toEqual(['Claude Code']);
    expect(applicableEngineLabels('openai-responses')).toEqual(['Codex']);
    expect(applicableEngineLabels('bedrock')).toEqual([]);
    expect(applicableEngineLabels('vertex')).toEqual([]);
  });

  it('returns a provider model catalog independent from engine bindings', () => {
    expect(modelCatalogForProvider(config, 'anthropic').map((model) => model.id)).toEqual([
      'claude-sonnet-4.6',
    ]);
  });

  it('deduplicates repeated model entries per provider but keeps same id across providers', () => {
    const duplicated: AppConfig = {
      ...config,
      models: [
        ...config.models,
        { ...config.models[1], displayName: 'duplicate gpt-5-codex' },
        {
          ...config.models[1],
          providerId: 'local',
          displayName: 'local gpt-5-codex',
        },
      ],
    };

    expect(modelCatalogForProvider(duplicated, 'openai').map((model) => model.displayName)).toEqual(
      ['gpt-5-codex', 'gpt-5-mini', 'disabled-openai-model'],
    );
    expect(modelCatalog(duplicated).filter((model) => model.id === 'gpt-5-codex')).toHaveLength(2);
  });

  it('formats protocol labels and equivalent env pairs for display', () => {
    expect(protocolLabel('openai-responses')).toBe('OpenAI 兼容 · Responses');
    expect(
      envPairsToText([
        ['ANTHROPIC_BASE_URL', 'https://api.anthropic.com'],
        ['ANTHROPIC_MODEL', 'claude-sonnet-4.6'],
      ]),
    ).toBe('ANTHROPIC_BASE_URL=https://api.anthropic.com\nANTHROPIC_MODEL=claude-sonnet-4.6');
  });

  it('formats provider readiness and reachability separately', () => {
    expect(readinessText(config.providers[0])).toBe('配置就绪');
    expect(lastTestText(config.providers[0])).toBe('可用 · 120ms');
    expect(
      lastTestText({
        lastTest: { result: 'unverified', latencyMs: 0, at: 1_717_171_719 },
      }),
    ).toBe('未验证');
    expect(lastTestText(config.providers[1])).toBe('尚未测试');
    expect(lastTestTimeText(config.providers[1])).toBe('尚未测试');
  });

  it('formats model price sources for display', () => {
    expect(priceSourceText({ priceSource: 'builtin' })).toBe('官方参考');
    expect(priceSourceText({ priceSource: 'manual' })).toBe('手动');
    expect(priceSourceText({ priceSource: 'provider' })).toBe('服务商');
    expect(priceSourceText({ priceSource: 'subscription' })).toBe('订阅内');
    expect(priceSourceText({ priceSource: 'unknown' })).toBe('待配置');
    expect(priceSourceText({})).toBe('待配置');
  });

  it('creates provider drafts from user intent instead of a generic example record', () => {
    expect(createProviderDraft('official-openai', 1783300000000)).toMatchObject({
      id: 'custom-1783300000000',
      name: 'OpenAI API',
      baseUrl: 'https://api.openai.com/v1',
      protocol: 'openai-responses',
      authMethod: 'apikey',
      kind: 'api',
      accessType: 'official',
      ready: false,
      lastTest: null,
    });
    expect(createProviderDraft('claude-subscription', 1783300000002)).toMatchObject({
      name: 'Claude 订阅',
      baseUrl: '',
      authMethod: 'oauth',
      kind: 'subscription',
    });
    expect(createProviderDraft('local-openai', 1783300000001)).toMatchObject({
      name: '本地 OpenAI 兼容服务',
      baseUrl: 'http://localhost:11434/v1',
      protocol: 'openai-chat',
      authMethod: 'local',
      kind: 'local',
    });
  });

  it('finds an existing subscription with the same protocol for reuse', () => {
    const subscription = createProviderDraft('claude-subscription', 4);
    expect(matchingSubscriptionProvider([subscription], 'anthropic')?.id).toBe(subscription.id);
    expect(matchingSubscriptionProvider([subscription], 'openai-responses')).toBeUndefined();
  });

  it('turns an empty provider model catalog into the next best action', () => {
    expect(providerModelEmptyState({ ...config.providers[1], ready: false })).toEqual({
      title: '先完成服务商配置',
      body: '保存名称、接口规范、认证方式和基础 URL 后，再同步这个服务商提供的模型。',
      action: '保存更改',
    });
    expect(providerModelEmptyState(config.providers[1])).toEqual({
      title: '建议先测试可达性',
      body: '测试通过后再同步模型列表，可以避免把密钥或基础 URL 问题误判成没有模型。',
      action: '测试可达性',
    });
    expect(providerModelEmptyState(config.providers[0])).toEqual({
      title: '可以同步模型目录',
      body: 'Helm 会从这个服务商的真实接口拉取模型；如果接口不支持列表能力，会保留当前目录并提示原因。',
      action: '同步模型列表',
    });
  });

  it('branches setup fields and next steps by provider access type', () => {
    const subscription = createProviderDraft('claude-subscription', 1);
    expect(providerSetupCopy(subscription).nextStep).toContain('Helm 独立订阅登录');
    expect(providerCapabilities(subscription)).toEqual({
      showBaseUrl: false,
      showApiKey: false,
      canTestHttp: false,
      canSyncModels: true,
    });
    expect(providerCapabilities(config.providers[0])).toEqual({
      showBaseUrl: true,
      showApiKey: true,
      canTestHttp: true,
      canSyncModels: true,
    });
  });

  it('does not send an empty subscription catalog into the HTTP reachability loop', () => {
    const subscription = {
      ...createProviderDraft('codex-subscription', 2),
      ready: true,
      lastTest: { result: 'unverified' as const, latencyMs: 0, at: 1 },
    };
    expect(providerModelEmptyState(subscription).action).not.toBe('测试可达性');
    expect(providerModelEmptyState(subscription)).toEqual({
      title: '读取账号可用模型',
      body: '完成 Helm 独立订阅登录后，将通过本机 CLI 读取当前账号可用的模型。',
      action: '同步模型列表',
    });
  });

  it('chooses the account default as primary and a lightweight model for background work', () => {
    const subscription = createProviderDraft('codex-subscription', 6);
    const dynamicConfig: AppConfig = {
      ...config,
      providers: [...config.providers, { ...subscription, ready: true }],
      models: [
        ...config.models,
        {
          id: 'gpt-5.6-terra',
          providerId: subscription.id,
          displayName: 'GPT-5.6-Terra（账号默认）',
          inputPricePerMtok: 0,
          outputPricePerMtok: 0,
          enabled: true,
        },
        {
          id: 'gpt-5.4-mini',
          providerId: subscription.id,
          displayName: 'GPT-5.4-Mini',
          inputPricePerMtok: 0,
          outputPricePerMtok: 0,
          enabled: true,
        },
      ],
    };

    expect(
      normalizeBindingDraft(dynamicConfig, {
        engineId: 'codex',
        providerId: subscription.id,
        primaryModel: 'gpt-5.4',
        fastModel: 'gpt-5.3-codex',
      }),
    ).toMatchObject({
      primaryModel: 'gpt-5.6-terra',
      fastModel: 'gpt-5.4-mini',
    });
  });

  it('only allows a subscription binding after authoritative login and model availability', () => {
    const subscription = {
      ...createProviderDraft('claude-subscription', 3),
      ready: true,
    };
    expect(canBindProvider(subscription, { state: 'missing' }, 3)).toBe(false);
    expect(canBindProvider(subscription, { state: 'ok' }, 0)).toBe(false);
    expect(canBindProvider(subscription, { state: 'ok', authMethod: 'apikey' }, 3)).toBe(false);
    expect(canBindProvider(subscription, { state: 'ok', authMethod: 'subscription' }, 3)).toBe(
      true,
    );
    expect(canBindProvider(config.providers[0], null, 1)).toBe(true);
  });

  it('derives runtime readiness by provider kind', () => {
    const subscription = { ...createProviderDraft('claude-subscription', 5), ready: true };
    expect(providerRuntimeReady(subscription, { state: 'ok', authMethod: 'subscription' })).toBe(
      true,
    );
    expect(providerRuntimeReady(subscription, { state: 'ok', authMethod: 'apikey' })).toBe(false);
    expect(providerRuntimeReady(config.providers[0])).toBe(true);
    expect(providerRuntimeReady(config.providers[1])).toBe(false);
  });

  it('labels authoritative CLI login states without treating unknown as logged in', () => {
    expect(loginStateLabel({ state: 'ok' })).toBe('已登录');
    expect(loginStateLabel({ state: 'ok', authMethod: 'apikey' })).toBe('API Key 模式');
    expect(loginStateLabel({ state: 'missing' })).toBe('未登录');
    expect(loginStateLabel({ state: 'expired' })).toBe('登录失效');
    expect(loginStateLabel({ state: 'unknown' })).toBe('无法判断');
    expect(loginStateLabel(null)).toBe('检测中…');
  });

  it('explains why an API-key CLI login cannot activate a subscription provider', () => {
    expect(subscriptionLoginWarning({ state: 'ok', authMethod: 'apikey' })).toContain(
      '当前 CLI 使用 API Key',
    );
    expect(subscriptionLoginWarning({ state: 'missing' })).toContain('尚未验证');
    expect(subscriptionLoginWarning({ state: 'ok', authMethod: 'subscription' })).toBeNull();
  });

  it('describes destructive provider deletion before asking for confirmation', () => {
    expect(providerDeleteConfirmation(config.providers[0], 3, 1)).toEqual({
      title: '移除 Anthropic？',
      body: '将删除这个服务商、3 个模型目录项，并让 1 条引擎绑定失效。API 密钥引用也会从 Helm 配置中移除。',
      confirmLabel: '移除服务商',
    });
  });

  it('allows deleting the last unbound provider and explains bound-provider blocking', () => {
    expect(providerCanDelete(0)).toBe(true);
    expect(providerCanDelete(1)).toBe(false);
    expect(providerDeleteBlockedReason(0)).toBeNull();
    expect(providerDeleteBlockedReason(1)).toContain('解绑后可删除');
  });

  it('reports reachability status from lastTest result', () => {
    expect(reachabilityStatus(config.providers[0])).toBe('reachable');
    expect(reachabilityStatus({ lastTest: { result: 'fail', at: 1 } })).toBe('unreachable');
    expect(reachabilityStatus({ lastTest: null })).toBe('unknown');
    expect(reachabilityStatus({ lastTest: { result: 'unverified', at: 1 } })).toBe('unknown');
  });

  it('returns failure category from persisted lastTest', () => {
    expect(providerFailureCategory(config.providers[0])).toBeNull();
    expect(
      providerFailureCategory({ lastTest: { result: 'fail', at: 1, failureCategory: 'auth' } }),
    ).toBe('auth');
    expect(
      providerFailureCategory({ lastTest: { result: 'fail', at: 1, failureCategory: 'network' } }),
    ).toBe('network');
    expect(
      providerFailureCategory({ lastTest: { result: 'fail', at: 1, failureCategory: 'timeout' } }),
    ).toBe('timeout');
    expect(providerFailureCategory({ lastTest: { result: 'fail', at: 1 } })).toBeNull();
  });

  it('maps failure categories to Chinese labels', () => {
    expect(failureCategoryLabel('network')).toBe('网络');
    expect(failureCategoryLabel('auth')).toBe('认证');
    expect(failureCategoryLabel('timeout')).toBe('超时');
    expect(failureCategoryLabel('unknown')).toBe('未知');
  });

  // ===== S6：接入类型分组 / 状态视觉 / 模型接入路径 =====

  it('classifies access groups into the four user-decided buckets (legacy falls back to relay)', () => {
    expect(providerAccessGroup({ kind: 'subscription' })).toBe('subscription');
    // 本地服务并入第三方中转展示（用户四分类裁决）
    expect(providerAccessGroup({ kind: 'local' })).toBe('relay');
    expect(providerAccessGroup({ kind: 'api', accessType: 'official' })).toBe('official');
    expect(providerAccessGroup({ kind: 'api', accessType: 'plan' })).toBe('plan');
    expect(providerAccessGroup({ kind: 'api', accessType: 'relay' })).toBe('relay');
    // 历史数据：官方域名推断为直连，其余一律归入第三方中转
    expect(
      providerAccessGroup({ kind: 'api', accessType: null, baseUrl: 'https://api.anthropic.com' }),
    ).toBe('official');
    expect(
      providerAccessGroup({
        kind: 'api',
        accessType: null,
        baseUrl: 'https://gw.internal.example.com',
      }),
    ).toBe('relay');
    expect(providerAccessGroup({ kind: 'api', accessType: null })).toBe('relay');
  });

  it('derives card status pills only from real readiness/login/model data', () => {
    const base = {
      kind: 'api' as const,
      protocol: 'openai-responses' as const,
      ready: true,
      lastTest: null,
    };
    expect(providerCardStatus(base, null, 0)).toEqual({ label: '待选模型', tone: 'warn' });
    expect(providerCardStatus({ ...base, lastTest: { result: 'fail', at: 1 } }, null, 2)).toEqual({
      label: '探活失败',
      tone: 'warn',
    });
    expect(providerCardStatus({ ...base, lastTest: { result: 'ok', at: 1 } }, null, 2)).toEqual({
      label: '配置就绪',
      tone: 'ready',
    });
    expect(providerCardStatus({ ...base, ready: false }, null, 2)).toEqual({
      label: '待配置',
      tone: 'muted',
    });
    // 订阅：登录成功 → 已登录/待选模型；未登录 → 登录态文案
    const sub = {
      kind: 'subscription' as const,
      protocol: 'openai-responses' as const,
      ready: true,
      lastTest: null,
    };
    expect(providerCardStatus(sub, { state: 'ok', authMethod: 'subscription' }, 1)).toEqual({
      label: '已登录',
      tone: 'ready',
    });
    expect(providerCardStatus(sub, { state: 'ok', authMethod: 'subscription' }, 0)).toEqual({
      label: '待选模型',
      tone: 'warn',
    });
    expect(providerCardStatus(sub, { state: 'missing' }, 1).tone).toBe('warn');
    // 角色模式（Anthropic 兼容中转）：模型在 roleModels，不在模型目录，按角色配置判定
    const relay = {
      kind: 'api' as const,
      protocol: 'anthropic' as const,
      ready: true,
      lastTest: null,
    };
    expect(providerCardStatus({ ...relay, roleModels: {} }, null, 0)).toEqual({
      label: '待选模型',
      tone: 'warn',
    });
    expect(
      providerCardStatus({ ...relay, roleModels: { default: '  ', opus: '' } }, null, 0),
    ).toEqual({ label: '待选模型', tone: 'warn' });
    expect(
      providerCardStatus({ ...relay, roleModels: { opus: 'claude-sonnet-5' } }, null, 0),
    ).toEqual({ label: '配置就绪', tone: 'ready' });
    expect(
      providerCardStatus(
        { ...relay, roleModels: { opus: 'claude-sonnet-5' }, lastTest: { result: 'fail', at: 1 } },
        null,
        0,
      ),
    ).toEqual({ label: '探活失败', tone: 'warn' });
  });

  it('normalizes model ids with the same rules as Rust pricing::normalize_model_id', () => {
    expect(normalizeModelGroupId('Claude-Sonnet-4')).toBe('claude-sonnet-4');
    expect(normalizeModelGroupId('models/gpt-5')).toBe('gpt-5');
    expect(normalizeModelGroupId('anthropic/claude-3')).toBe('claude-3');
    expect(normalizeModelGroupId('openai/gpt-5')).toBe('gpt-5');
    expect(normalizeModelGroupId('qwen@2025')).toBe('qwen-2025');
    expect(normalizeModelGroupId('a@b@c')).toBe('a-b-c');
    expect(normalizeModelGroupId('models/openai/gpt-x')).toBe('gpt-x');
    expect(normalizeModelGroupId('  GPT-5 ')).toBe('gpt-5');
  });

  it('labels path billing from the real access group', () => {
    expect(pathBillingLabel({ kind: 'subscription' })).toBe('订阅折算');
    expect(pathBillingLabel({ kind: 'api', accessType: 'plan' })).toBe('套餐等效');
    expect(pathBillingLabel({ kind: 'api', accessType: 'official' })).toBe('官方费率');
    expect(pathBillingLabel({ kind: 'api', accessType: 'relay' })).toBe('中转报价');
    expect(pathBillingLabel({ kind: 'local' })).toBe('中转报价');
    expect(pathBillingLabel({ kind: 'api' })).toBe('中转报价');
  });

  it('aggregates cross-provider access paths per normalized model id with bindings', () => {
    const duplicated: AppConfig = {
      ...config,
      providers: [
        ...config.providers,
        {
          id: 'kimi-plan',
          name: 'Kimi 套餐',
          kind: 'api',
          baseUrl: 'https://plan.example.com',
          keyRef: null,
          ready: true,
          lastTest: null,
          protocol: 'openai-responses',
          authMethod: 'apikey',
          accessType: 'plan',
        },
      ],
      models: [
        ...config.models,
        {
          id: 'gpt-5-codex',
          providerId: 'kimi-plan',
          displayName: 'GPT-5-Codex',
          inputPricePerMtok: 0,
          outputPricePerMtok: 0,
          enabled: false,
        },
      ],
      bindings: [
        ...config.bindings,
        { engineId: 'codex', providerId: 'openai', primaryModel: 'gpt-5-codex', fastModel: null },
      ],
    };
    const groups = modelAccessGroups(duplicated);
    const gpt = groups.find((group) => group.key === 'gpt-5-codex');
    expect(gpt).toBeDefined();
    expect(gpt?.paths.map((path) => path.provider.id)).toEqual(['openai', 'kimi-plan']);
    expect(gpt?.paths[0].boundEngines).toContain('Codex');
    expect(gpt?.paths[1].billing).toBe('套餐等效');
  });

  it('shows role-mode paths from roleModels and hides legacy catalog leftovers', () => {
    const relayProvider = {
      id: 'relay-anthropic',
      name: 'sui-xiang',
      kind: 'api' as const,
      baseUrl: 'https://sui-xiang.com',
      keyRef: null,
      ready: true,
      lastTest: null,
      protocol: 'anthropic' as const,
      authMethod: 'apikey' as const,
      // 四个角色共用 claude-sonnet-5 → 合并为一条路径并标注角色；haiku 独立模型 → 单独一条
      roleModels: {
        default: 'claude-sonnet-5',
        opus: 'claude-sonnet-5',
        sonnet: 'claude-sonnet-5',
        haiku: 'claude-haiku-4',
      },
    };
    const roleMode: AppConfig = {
      ...config,
      providers: [...config.providers, relayProvider],
      // 目录里的 claude-sonnet-5 / claude-haiku-4 是历史同步残留，角色模式下不应展示
      models: [
        ...config.models,
        {
          id: 'claude-sonnet-5',
          providerId: 'relay-anthropic',
          displayName: 'claude-sonnet-5',
          inputPricePerMtok: 0,
          outputPricePerMtok: 0,
          enabled: true,
        },
      ],
      bindings: [
        ...config.bindings,
        {
          engineId: 'claude-code',
          providerId: 'relay-anthropic',
          primaryModel: 'role:opus',
          fastModel: 'role:opus',
        },
      ],
    };
    const groups = modelAccessGroups(roleMode);
    const sonnet = groups.find((group) => group.key === 'claude-sonnet-5');
    expect(sonnet).toBeDefined();
    expect(sonnet?.paths).toHaveLength(1);
    expect(sonnet?.paths[0].provider.id).toBe('relay-anthropic');
    expect(sonnet?.paths[0].roles).toEqual(['默认模型', 'Sonnet', 'Opus']);
    expect(sonnet?.paths[0].boundEngines).toContain('Claude Code');
    const haiku = groups.find((group) => group.key === 'claude-haiku-4');
    expect(haiku?.paths[0].roles).toEqual(['Haiku']);
    expect(haiku?.paths[0].boundEngines).toEqual([]);
    // 非订阅 anthropic 服务商的目录条目一律不进模型列表
    const anthropicCatalog = groups.find((group) => group.key === 'claude-sonnet-4.6');
    expect(anthropicCatalog).toBeUndefined();
  });

  it('summarizes group price preferring token-priced paths over plan fallbacks', () => {
    const makeModel = (inputPricePerMtok: number, outputPricePerMtok: number): ModelConfig => ({
      id: 'm1',
      providerId: 'p1',
      displayName: 'm1',
      inputPricePerMtok,
      outputPricePerMtok,
      enabled: true,
    });
    const summary = modelGroupPriceSummary([
      {
        model: makeModel(3, 15),
        provider: { ...config.providers[0] },
        billing: '',
        boundEngines: [],
      },
    ]);
    expect(summary.plan).toBe(false);
    expect(summary.text).toContain('$3.00');
    expect(summary.text).toContain('$15.00');
    expect(summary.segments.map((segment) => segment.label)).toEqual(['输入', '输出']);
    const withCache = modelGroupPriceSummary([
      {
        model: { ...makeModel(3, 15), cachedInputPricePerMtok: 0.3 },
        provider: { ...config.providers[0] },
        billing: '',
        boundEngines: [],
      },
    ]);
    expect(withCache.segments.map((segment) => `${segment.label} ${segment.value}`)).toEqual([
      '输入 $3.00',
      '缓存 $0.30',
      '输出 $15.00',
    ]);
    const subscriptionOnly = modelGroupPriceSummary([
      {
        model: makeModel(0, 0),
        provider: {
          id: 'sub',
          name: 'Claude 订阅',
          kind: 'subscription',
          baseUrl: '',
          keyRef: null,
          ready: true,
          lastTest: null,
          protocol: 'anthropic',
          authMethod: 'oauth',
          accessType: null,
        },
        billing: '',
        boundEngines: [],
      },
    ]);
    expect(subscriptionOnly).toEqual({ segments: [], text: '订阅内', plan: true });
    const unpriced = modelGroupPriceSummary([
      {
        model: makeModel(0, 0),
        provider: config.providers[2],
        billing: '',
        boundEngines: [],
      },
    ]);
    expect(unpriced).toEqual({ segments: [], text: '待配置', plan: true });
  });

  it('keeps add-flow templates grouped and stamps api drafts with an explicit access type', () => {
    for (const group of ['subscription', 'official', 'plan', 'relay'] as const) {
      expect(templatesForAccessGroup(group).length).toBeGreaterThan(0);
    }
    expect(PROVIDER_ACCESS_GROUPS.map((item) => item.id).sort()).toEqual([
      'official',
      'plan',
      'relay',
      'subscription',
    ]);
    const relayDraft = createProviderDraft('relay-anthropic', 42);
    expect(relayDraft.accessType).toBe('relay');
    expect(relayDraft.kind).toBe('api');
    const planDraft = createProviderDraft('plan-kimi', 43);
    expect(planDraft.accessType).toBe('plan');
    const subDraft = createProviderDraft('claude-subscription', 44);
    expect(subDraft.accessType).toBeNull();
  });
});

describe('commitProviderModelRow（空行保留 / 重复不删行）', () => {
  it('keeps an empty row instead of deleting it', () => {
    expect(commitProviderModelRow(['gpt-5', ''], 1, '   ')).toEqual({ action: 'keep-empty' });
  });

  it('rejects a duplicate id without asking the caller to drop the row', () => {
    expect(commitProviderModelRow(['gpt-5', ''], 1, 'gpt-5')).toEqual({ action: 'duplicate' });
  });

  it('applies a new trimmed id', () => {
    expect(commitProviderModelRow(['gpt-5', ''], 1, '  gpt-5-mini  ')).toEqual({
      action: 'apply',
      id: 'gpt-5-mini',
    });
  });
});

describe('multiplierInputToBasisPoints（矩阵 H-4 倍率钳制）', () => {
  it('空值按 1 倍，负数与零钳到 1bp 下限，小数按基点取整', () => {
    expect(multiplierInputToBasisPoints('')).toBe(10000);
    expect(multiplierInputToBasisPoints('0')).toBe(1);
    expect(multiplierInputToBasisPoints('-5')).toBe(1);
    expect(multiplierInputToBasisPoints('0.01')).toBe(100);
    expect(multiplierInputToBasisPoints('0.5')).toBe(5000);
    expect(multiplierInputToBasisPoints('1.5')).toBe(15000);
    expect(multiplierInputToBasisPoints('100')).toBe(1000000);
  });
});

describe('目录定价只接受规范 ID、显式别名与适用协议', () => {
  const catalog: PricingCatalogEntry[] = [
    {
      vendor: 'openai',
      modelId: 'gpt-example-2026-01-01',
      aliases: ['gpt-example'],
      protocols: ['openai-responses', 'openai-chat'],
      currency: 'USD',
      input: 2,
      cachedInput: 0.2,
      output: 8,
      observedAt: '2026-01-01',
    },
    {
      vendor: 'anthropic',
      modelId: 'claude-example',
      aliases: ['shared-alias'],
      protocols: ['anthropic', 'bedrock', 'vertex'],
      currency: 'USD',
      input: 3,
      cachedInput: 0.3,
      output: 15,
      observedAt: '2026-01-01',
    },
  ];

  it('精确 ID 与显式 alias 使用同一后端规范化口径', () => {
    expect(catalogPriceMatch(catalog, 'gpt-example-2026-01-01', 'openai-responses')).toBe(
      catalog[0],
    );
    expect(catalogPriceMatch(catalog, ' MODELS/OPENAI/GPT-EXAMPLE ', 'openai-chat')).toBe(
      catalog[0],
    );
    expect(catalogPriceMatch(catalog, 'gateway:shared-alias', 'anthropic')).toBe(catalog[1]);
    expect(catalogPriceMatch(catalog, 'models/anthropic/claude@example', 'vertex')).toBe(
      catalog[1],
    );
  });

  it.each([
    'gpt-example-latest',
    'gpt-example-thinking',
    'gpt-example-responses',
    'gpt-example-2026-02-01',
    'example',
    '',
  ])('不对 %s 做相似名称、协议后缀或日期猜价', (modelId) => {
    expect(catalogPriceMatch(catalog, modelId, 'openai-responses')).toBeNull();
  });

  it('同名与别名都不能跨协议套用价格；缺少协议证据按未知处理', () => {
    expect(catalogPriceMatch(catalog, 'gpt-example', 'anthropic')).toBeNull();
    expect(catalogPriceMatch(catalog, 'shared-alias', 'openai-chat')).toBeNull();
    expect(
      catalogPriceMatch([{ ...catalog[0], protocols: undefined }], 'gpt-example', 'openai-chat'),
    ).toBeNull();
    expect(
      catalogPriceMatch([{ ...catalog[0], protocols: [] }], 'gpt-example', 'openai-chat'),
    ).toBeNull();
  });

  it('名称相同时按适用协议选择，不把目录中的第一个厂商当权威', () => {
    const duplicate = { ...catalog[1], modelId: catalog[0].modelId };
    expect(catalogPriceMatch([duplicate, ...catalog], catalog[0].modelId, 'openai-responses')).toBe(
      catalog[0],
    );
  });

  it('未命中是 unknown，不把默认零价冒充手动或目录价', () => {
    const model = buildManualModel('relay', 'custom-model', null);
    expect(model).toMatchObject({
      id: 'custom-model',
      providerId: 'relay',
      priceSource: 'unknown',
      inputPricePerMtok: 0,
      outputPricePerMtok: 0,
      enabled: true,
    });
  });

  it('命中仅复制权威价，不改写实际模型 ID', () => {
    const modelId = 'gpt-example';
    const match = catalogPriceMatch(catalog, modelId, 'openai-responses');
    expect(buildManualModel('api', modelId, match)).toMatchObject({
      id: modelId,
      displayName: modelId,
      inputPricePerMtok: 2,
      cachedInputPricePerMtok: 0.2,
      outputPricePerMtok: 8,
      priceSource: 'builtin',
    });
    expect(buildManualModel('api', modelId, { ...catalog[0], input: 0, output: 0 })).toMatchObject({
      priceSource: 'builtin',
      inputPricePerMtok: 0,
      outputPricePerMtok: 0,
    });
  });
});

describe('订阅目录与本地勾选', () => {
  it.each(['claude-subscription', 'codex-subscription'] as const)(
    '%s 只展示官方目录，不开放手写角色',
    (template) => {
      const provider = createProviderDraft(template, 1);
      const model = { ...config.models[0], providerId: provider.id };
      const next = { ...config, providers: [provider], models: [model] };
      expect(providerModelMode(provider)).toBe('list-openai');
      expect(sessionModelOptions(next, provider.id)).toEqual([model]);
    },
  );

  it('API Anthropic 保持角色配置，API OpenAI 保持可编辑列表', () => {
    expect(providerModelMode(config.providers[0])).toBe('roles-anthropic');
    expect(providerModelMode(config.providers[1])).toBe('list-openai');
  });

  it('刷新只保留同服务商同模型的勾选，其余字段和增删以最新官方目录为准', () => {
    const current = { ...config.models[0], enabled: true, inputPricePerMtok: 9 };
    const added = { ...current, id: 'new-official-model', enabled: true };
    const drafts = [
      { ...current, enabled: false, inputPricePerMtok: 999, displayName: 'edited' },
      { ...current, id: 'removed-model', enabled: true },
      { ...added, providerId: 'other-provider', enabled: false },
    ];
    const merged = mergeSyncedModelSelection([current, added], drafts);
    expect(merged).toEqual([{ ...current, enabled: false }, added]);
    expect(current.enabled).toBe(true);
    expect(drafts[0].inputPricePerMtok).toBe(999);
  });
});
