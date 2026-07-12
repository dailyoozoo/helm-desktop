import type {
  AppConfig,
  BindingConfig,
  EngineConfig,
  ModelConfig,
  ProviderAuthMethod,
  ProviderConfig,
  ProviderProtocol,
} from './api';

export const PROTOCOL_LABELS: Record<ProviderProtocol, string> = {
  anthropic: 'Anthropic 兼容',
  'openai-responses': 'OpenAI 兼容 · Responses',
  'openai-chat': 'OpenAI 兼容 · Chat',
  bedrock: 'Amazon Bedrock',
  vertex: 'Google Vertex AI',
};

export const AUTH_METHOD_LABELS = {
  apikey: '密钥',
  oauth: '订阅登录 OAuth',
  cloud: '云凭证',
  local: '本地',
} as const;

export type ProviderTemplateId =
  | 'claude-subscription'
  | 'codex-subscription'
  | 'claude-anthropic'
  | 'codex-openai'
  | 'local-openai'
  | 'cloud-bedrock';

export interface ProviderTemplate {
  id: ProviderTemplateId;
  title: string;
  description: string;
  name: string;
  baseUrl: string;
  protocol: ProviderProtocol;
  authMethod: ProviderAuthMethod;
}

export const PROVIDER_TEMPLATES: ProviderTemplate[] = [
  {
    id: 'claude-subscription',
    title: 'Claude 订阅（Pro / Max）',
    description: '复用本机 claude 登录态，无需 API Key；适合已订阅 Claude 的用户。',
    name: 'Claude 订阅',
    baseUrl: 'https://api.anthropic.com',
    protocol: 'anthropic',
    authMethod: 'oauth',
  },
  {
    id: 'codex-subscription',
    title: 'ChatGPT 订阅（Codex）',
    description: '复用本机 codex login 登录态，无需 API Key；适合 ChatGPT 订阅用户。',
    name: 'ChatGPT 订阅',
    baseUrl: 'https://api.openai.com/v1',
    protocol: 'openai-responses',
    authMethod: 'oauth',
  },
  {
    id: 'codex-openai',
    title: 'Codex / OpenAI 兼容',
    description: '适合 OpenAI 官方、转发网关，以及支持 Responses API 的服务商。',
    name: 'OpenAI 兼容服务商',
    baseUrl: '',
    protocol: 'openai-responses',
    authMethod: 'apikey',
  },
  {
    id: 'claude-anthropic',
    title: 'Claude Code / Anthropic 兼容',
    description: '适合 Anthropic 官方、Claude Code 兼容网关，以及 Anthropic 协议服务。',
    name: 'Anthropic 兼容服务商',
    baseUrl: '',
    protocol: 'anthropic',
    authMethod: 'apikey',
  },
  {
    id: 'local-openai',
    title: '本地 OpenAI 兼容服务',
    description: '适合 Ollama、LM Studio 或本机兼容网关，通常无需 API 密钥。',
    name: '本地 OpenAI 兼容服务',
    baseUrl: 'http://localhost:11434/v1',
    protocol: 'openai-chat',
    authMethod: 'local',
  },
  // cloud-bedrock 模板暂时下架：Bedrock/Vertex 尚无真实 CLI 链路（可靠性检查 G2），
  // 恢复前需实现 CLAUDE_CODE_USE_BEDROCK 等真实环境变量与云凭证校验。
];

export function protocolLabel(protocol: ProviderProtocol): string {
  return PROTOCOL_LABELS[protocol];
}

export function createProviderDraft(
  templateId: ProviderTemplateId,
  timestamp = Date.now(),
): ProviderConfig {
  const template =
    PROVIDER_TEMPLATES.find((item) => item.id === templateId) ?? PROVIDER_TEMPLATES[0];
  return {
    id: `custom-${timestamp}`,
    name: template.name,
    baseUrl: template.baseUrl,
    keyRef: null,
    ready: false,
    lastTest: null,
    protocol: template.protocol,
    authMethod: template.authMethod,
  };
}

export function providerModelEmptyState(provider: Pick<ProviderConfig, 'ready' | 'lastTest'>): {
  title: string;
  body: string;
  action: '保存更改' | '测试可达性' | '同步模型列表';
} {
  if (!provider.ready) {
    return {
      title: '先完成服务商配置',
      body: '保存名称、接口规范、认证方式和基础 URL 后，再同步这个服务商提供的模型。',
      action: '保存更改',
    };
  }
  if (provider.lastTest?.result !== 'ok') {
    return {
      title: '建议先测试可达性',
      body: '测试通过后再同步模型列表，可以避免把密钥或基础 URL 问题误判成没有模型。',
      action: '测试可达性',
    };
  }
  return {
    title: '可以同步模型目录',
    body: 'Helm 会从这个服务商的真实接口拉取模型；如果接口不支持列表能力，会保留当前目录并提示原因。',
    action: '同步模型列表',
  };
}

export function providerDeleteConfirmation(
  provider: Pick<ProviderConfig, 'name'>,
  modelCount: number,
  bindingCount: number,
): { title: string; body: string; confirmLabel: string } {
  return {
    title: `移除 ${provider.name}？`,
    body: `将删除这个服务商、${modelCount} 个模型目录项，并让 ${bindingCount} 条引擎绑定失效。API 密钥引用也会从 Helm 配置中移除。`,
    confirmLabel: '移除服务商',
  };
}

export function applicableEngineLabels(protocol: ProviderProtocol): string[] {
  if (protocol === 'anthropic') return ['Claude Code'];
  if (protocol === 'openai-responses' || protocol === 'openai-chat') return ['Codex'];
  return [];
}

export function engineAccepts(engineId: string, protocol: ProviderProtocol): boolean {
  if (engineId === 'claude-code') return protocol === 'anthropic';
  if (engineId === 'codex') return protocol === 'openai-responses' || protocol === 'openai-chat';
  return false;
}

export function compatibleProvidersForEngine(
  config: AppConfig,
  engineId: string,
): ProviderConfig[] {
  return config.providers.filter(
    (provider) => provider.ready && engineAccepts(engineId, provider.protocol),
  );
}

export function readinessText(provider: Pick<ProviderConfig, 'ready'>): string {
  return provider.ready ? '配置就绪' : '待配置';
}

export function lastTestText(provider: Pick<ProviderConfig, 'lastTest'>): string {
  const lastTest = provider.lastTest;
  if (!lastTest) return '尚未测试';
  if (lastTest.result === 'unverified') return '未验证';
  const latency = lastTest.latencyMs ? ` · ${lastTest.latencyMs}ms` : '';
  return `${lastTest.result === 'ok' ? '可用' : '失败'}${latency}`;
}

export function lastTestTimeText(provider: Pick<ProviderConfig, 'lastTest'>): string {
  if (!provider.lastTest) return '尚未测试';
  return new Date(provider.lastTest.at * 1000).toLocaleString('zh-CN');
}

export function priceSourceText(model: Pick<ModelConfig, 'priceSource'>): string {
  if (model.priceSource === 'builtin') return '官方参考';
  if (model.priceSource === 'manual') return '手动';
  if (model.priceSource === 'provider') return '服务商';
  return '待配置';
}

export function modelsForProvider(config: AppConfig, providerId: string): ModelConfig[] {
  return uniqueModels(
    config.models.filter((model) => model.providerId === providerId && model.enabled),
  );
}

export function modelCatalogForProvider(config: AppConfig, providerId: string): ModelConfig[] {
  return uniqueModels(config.models.filter((model) => model.providerId === providerId));
}

export function modelCatalog(config: AppConfig): ModelConfig[] {
  return uniqueModels(config.models);
}

export function modelsForEngine(config: AppConfig, engineId: string): ModelConfig[] {
  const providerIds = new Set(
    compatibleProvidersForEngine(config, engineId).map((provider) => provider.id),
  );
  return uniqueModels(
    config.models.filter((model) => providerIds.has(model.providerId) && model.enabled),
  );
}

export function normalizeBindingDraft(config: AppConfig, draft: BindingConfig): BindingConfig {
  const models = modelsForProvider(config, draft.providerId);
  const modelIds = new Set(models.map((model) => model.id));
  const primaryModel = modelIds.has(draft.primaryModel)
    ? draft.primaryModel
    : (models[0]?.id ?? '');
  const fastModel =
    draft.fastModel && modelIds.has(draft.fastModel)
      ? draft.fastModel
      : (models[1]?.id ?? models[0]?.id ?? null);

  return {
    ...draft,
    primaryModel,
    fastModel,
  };
}

export function bindingForEngine(
  config: AppConfig,
  engine: EngineConfig,
): BindingConfig | undefined {
  return config.bindings.find((binding) => binding.engineId === engine.id);
}

export function envPairsToText(pairs: [string, string][]): string {
  return pairs.map(([key, value]) => `${key}=${value}`).join('\n');
}

function uniqueModels(models: ModelConfig[]): ModelConfig[] {
  const seen = new Set<string>();
  return models.filter((model) => {
    const key = `${model.providerId}:${model.id}`;
    if (seen.has(key)) return false;
    seen.add(key);
    return true;
  });
}
