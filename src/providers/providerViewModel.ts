import type {
  AppConfig,
  BindingConfig,
  CliLoginState,
  EngineConfig,
  FailureCategory,
  ModelConfig,
  ProviderAuthMethod,
  ProviderConfig,
  ProviderKind,
  ProviderProtocol,
  ProviderTest,
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
  kind: ProviderKind;
}

export const PROVIDER_TEMPLATES: ProviderTemplate[] = [
  {
    id: 'claude-subscription',
    title: 'Claude 订阅（Pro / Max）',
    description: '复用本机 claude 登录态，无需 API Key；适合已订阅 Claude 的用户。',
    name: 'Claude 订阅',
    baseUrl: '',
    protocol: 'anthropic',
    authMethod: 'oauth',
    kind: 'subscription',
  },
  {
    id: 'codex-subscription',
    title: 'ChatGPT 订阅（Codex）',
    description: '复用本机 codex login 登录态，无需 API Key；适合 ChatGPT 订阅用户。',
    name: 'ChatGPT 订阅',
    baseUrl: '',
    protocol: 'openai-responses',
    authMethod: 'oauth',
    kind: 'subscription',
  },
  {
    id: 'codex-openai',
    title: 'Codex / OpenAI 兼容',
    description: '适合 OpenAI 官方、转发网关，以及支持 Responses API 的服务商。',
    name: 'OpenAI 兼容服务商',
    baseUrl: '',
    protocol: 'openai-responses',
    authMethod: 'apikey',
    kind: 'api',
  },
  {
    id: 'claude-anthropic',
    title: 'Claude Code / Anthropic 兼容',
    description: '适合 Anthropic 官方、Claude Code 兼容网关，以及 Anthropic 协议服务。',
    name: 'Anthropic 兼容服务商',
    baseUrl: '',
    protocol: 'anthropic',
    authMethod: 'apikey',
    kind: 'api',
  },
  {
    id: 'local-openai',
    title: '本地 OpenAI 兼容服务',
    description: '适合 Ollama、LM Studio 或本机兼容网关，通常无需 API 密钥。',
    name: '本地 OpenAI 兼容服务',
    baseUrl: 'http://localhost:11434/v1',
    protocol: 'openai-chat',
    authMethod: 'local',
    kind: 'local',
  },
  // cloud-bedrock 模板暂时下架：Bedrock/Vertex 尚无真实 CLI 链路（可靠性检查 G2），
  // 恢复前需实现 CLAUDE_CODE_USE_BEDROCK 等真实环境变量与云凭证校验。
];

export function matchingSubscriptionProvider(
  providers: readonly Pick<ProviderConfig, 'id' | 'name' | 'kind' | 'protocol'>[],
  protocol: ProviderProtocol,
) {
  return providers.find(
    (provider) => provider.kind === 'subscription' && provider.protocol === protocol,
  );
}

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
    kind: template.kind,
    baseUrl: template.baseUrl,
    keyRef: null,
    ready: false,
    lastTest: null,
    protocol: template.protocol,
    authMethod: template.authMethod,
  };
}

export function isSubscriptionProvider(provider: Pick<ProviderConfig, 'kind'>): boolean {
  return provider.kind === 'subscription';
}

export function providerSetupCopy(provider: Pick<ProviderConfig, 'kind'>): {
  nextStep: string;
} {
  if (isSubscriptionProvider(provider)) {
    return {
      nextStep: '检测 Helm 独立订阅登录状态；未登录时完成官方账号登录，然后选择模型并绑定。',
    };
  }
  if (provider.kind === 'local') {
    return { nextStep: '确认基础 URL 后测试可达性，再同步模型目录。' };
  }
  return { nextStep: '填写基础 URL 和 API 密钥，保存后测试可达性，再同步模型目录。' };
}

export function providerCapabilities(provider: Pick<ProviderConfig, 'kind'>): {
  showBaseUrl: boolean;
  showApiKey: boolean;
  canTestHttp: boolean;
  canSyncModels: boolean;
} {
  if (isSubscriptionProvider(provider)) {
    return {
      showBaseUrl: false,
      showApiKey: false,
      canTestHttp: false,
      canSyncModels: true,
    };
  }
  return {
    showBaseUrl: true,
    showApiKey: provider.kind === 'api',
    canTestHttp: true,
    canSyncModels: true,
  };
}

export function canBindProvider(
  provider: Pick<ProviderConfig, 'kind' | 'ready'>,
  login: Pick<CliLoginState, 'state' | 'authMethod'> | null,
  modelCount: number,
): boolean {
  if (!provider.ready || modelCount === 0) return false;
  return (
    !isSubscriptionProvider(provider) ||
    (login?.state === 'ok' && login.authMethod === 'subscription')
  );
}

export function providerRuntimeReady(
  provider: Pick<ProviderConfig, 'kind' | 'ready' | 'lastTest'>,
  login: Pick<CliLoginState, 'state' | 'authMethod'> | null = null,
): boolean {
  if (!provider.ready) return false;
  if (isSubscriptionProvider(provider)) {
    return login?.state === 'ok' && login.authMethod === 'subscription';
  }
  return provider.lastTest?.result === 'ok';
}

export function loginStateLabel(login: Pick<CliLoginState, 'state' | 'authMethod'> | null): string {
  if (!login) return '检测中…';
  if (login.state === 'ok' && login.authMethod === 'apikey') return 'API Key 模式';
  if (login.state === 'ok') return '已登录';
  if (login.state === 'missing') return '未登录';
  if (login.state === 'expired') return '登录失效';
  return '无法判断';
}

export function subscriptionLoginWarning(
  login: Pick<CliLoginState, 'state' | 'authMethod'> | null,
): string | null {
  if (!login) return null;
  if (login.state === 'ok' && login.authMethod === 'subscription') return null;
  if (login.state === 'ok' && login.authMethod === 'apikey') {
    return '当前 CLI 使用 API Key，不属于订阅登录。请先在服务商详情切换为官方账号订阅登录。';
  }
  return '订阅账号尚未验证，请先在服务商详情完成登录。';
}

export function providerModelEmptyState(
  provider: Pick<ProviderConfig, 'kind' | 'ready' | 'lastTest'>,
): {
  title: string;
  body: string;
  action: '保存更改' | '测试可达性' | '同步模型列表';
} {
  if (isSubscriptionProvider(provider)) {
    return {
      title: provider.ready ? '读取账号可用模型' : '先保存订阅接入',
      body: provider.ready
        ? '完成 Helm 独立订阅登录后，将通过本机 CLI 读取当前账号可用的模型。'
        : '保存后先完成 Helm 独立订阅登录，再读取当前账号可用模型。',
      action: provider.ready ? '同步模型列表' : '保存更改',
    };
  }
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

export function providerCanDelete(bindingCount: number): boolean {
  return bindingCount === 0;
}

export function providerDeleteBlockedReason(bindingCount: number): string | null {
  if (bindingCount === 0) return null;
  return `该服务商仍被 ${bindingCount} 条引擎绑定使用，请先在「引擎与绑定」中更改绑定。`;
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

/** 可达性状态：绿=可达，红=不可达，灰=未测试 */
export type ReachabilityStatus = 'reachable' | 'unreachable' | 'unknown';

export function reachabilityStatus(provider: Pick<ProviderConfig, 'lastTest'>): ReachabilityStatus {
  const lastTest = provider.lastTest;
  if (!lastTest || lastTest.result === 'unverified') return 'unknown';
  return lastTest.result === 'ok' ? 'reachable' : 'unreachable';
}

/** 失败分类中文标签 */
const FAILURE_CATEGORY_LABELS: Record<FailureCategory, string> = {
  network: '网络',
  auth: '认证',
  timeout: '超时',
  unknown: '未知',
};

export function failureCategoryLabel(category: FailureCategory): string {
  return FAILURE_CATEGORY_LABELS[category] ?? '未知';
}

/** 从 lastTest 获取失败分类（已持久化到 ProviderTest.failureCategory） */
export function providerFailureCategory(
  provider: Pick<ProviderConfig, 'lastTest'>,
): FailureCategory | null {
  const lastTest = provider.lastTest as ProviderTest | null | undefined;
  if (!lastTest || lastTest.result !== 'fail') return null;
  return lastTest.failureCategory ?? null;
}

export function priceSourceText(model: Pick<ModelConfig, 'priceSource'>): string {
  if (model.priceSource === 'builtin') return '官方参考';
  if (model.priceSource === 'manual') return '手动';
  if (model.priceSource === 'provider') return '服务商';
  if (model.priceSource === 'subscription') return '订阅内';
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

export function normalizeBindingDraft(config: AppConfig, draft: BindingConfig): BindingConfig {
  const models = modelsForProvider(config, draft.providerId);
  const modelIds = new Set(models.map((model) => model.id));
  const primaryModel = modelIds.has(draft.primaryModel)
    ? draft.primaryModel
    : (models[0]?.id ?? '');
  const fastModel =
    draft.fastModel && modelIds.has(draft.fastModel)
      ? draft.fastModel
      : recommendedFastModel(models, primaryModel);

  return {
    ...draft,
    primaryModel,
    fastModel,
  };
}

function recommendedFastModel(models: ModelConfig[], primaryModel: string): string | null {
  const preferred = ['haiku', 'mini', 'spark', 'terra'];
  for (const marker of preferred) {
    const match = models.find((model) => model.id.toLowerCase().includes(marker));
    if (match) return match.id;
  }
  return models.find((model) => model.id !== primaryModel)?.id ?? models[0]?.id ?? null;
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
