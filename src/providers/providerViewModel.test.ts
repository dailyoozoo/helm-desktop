import { describe, expect, it } from 'vitest';
import type { AppConfig } from './api';
import {
  applicableEngineLabels,
  compatibleProvidersForEngine,
  createProviderDraft,
  providerDeleteConfirmation,
  providerModelEmptyState,
  envPairsToText,
  lastTestText,
  lastTestTimeText,
  modelCatalog,
  modelCatalogForProvider,
  modelsForProvider,
  normalizeBindingDraft,
  priceSourceText,
  protocolLabel,
  readinessText,
} from './providerViewModel';

const config: AppConfig = {
  defaultEngine: 'claude-code',
  defaultModel: 'claude-sonnet-4.6',
  providers: [
    {
      id: 'anthropic',
      name: 'Anthropic',
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
    expect(priceSourceText({ priceSource: 'unknown' })).toBe('待配置');
    expect(priceSourceText({})).toBe('待配置');
  });

  it('creates provider drafts from user intent instead of a generic example record', () => {
    expect(createProviderDraft('codex-openai', 1783300000000)).toMatchObject({
      id: 'custom-1783300000000',
      name: 'OpenAI 兼容服务商',
      baseUrl: '',
      protocol: 'openai-responses',
      authMethod: 'apikey',
      ready: false,
      lastTest: null,
    });
    expect(createProviderDraft('local-openai', 1783300000001)).toMatchObject({
      name: '本地 OpenAI 兼容服务',
      baseUrl: 'http://localhost:11434/v1',
      protocol: 'openai-chat',
      authMethod: 'local',
    });
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

  it('describes destructive provider deletion before asking for confirmation', () => {
    expect(providerDeleteConfirmation(config.providers[0], 3, 1)).toEqual({
      title: '移除 Anthropic？',
      body: '将删除这个服务商、3 个模型目录项，并让 1 条引擎绑定失效。API 密钥引用也会从 Helm 配置中移除。',
      confirmLabel: '移除服务商',
    });
  });
});
