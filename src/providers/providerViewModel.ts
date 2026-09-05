import type {
  AppConfig,
  BindingConfig,
  CliLoginState,
  EngineConfig,
  FailureCategory,
  ModelConfig,
  ProviderAccessType,
  ProviderAuthMethod,
  ProviderConfig,
  ProviderKind,
  ProviderProtocol,
  ProviderTest,
  ProviderRoleKey,
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
  | 'official-anthropic'
  | 'official-openai'
  | 'official-deepseek'
  | 'plan-glm-cn'
  | 'plan-glm-intl'
  | 'plan-kimi'
  | 'plan-minimax-cn'
  | 'plan-minimax-intl'
  | 'plan-mimo-usage'
  | 'plan-mimo-token'
  | 'plan-volc'
  | 'plan-bailian'
  | 'plan-qwen-personal'
  | 'plan-qwen-team'
  | 'relay-anthropic'
  | 'relay-openai'
  | 'local-openai'
  | 'cloud-bedrock';

/** 服务商 Tab / 添加流程的接入类型分组（S6，对齐原型 providers 分组） */
export type ProviderAccessGroup =
  | 'subscription'
  | 'official'
  | 'plan'
  | 'relay'
  | 'local'
  | 'uncategorized';

export const PROVIDER_ACCESS_GROUPS_DISPLAY: {
  id: ProviderAccessGroup;
  label: string;
  hint: string;
}[] = [
  { id: 'subscription', label: '授权登录', hint: '订阅账号通过隔离登录接入，按订阅折算计费。' },
  {
    id: 'official',
    label: '官方 API 直连',
    hint: '填入密钥后同步模型：Anthropic 兼容按角色选择，OpenAI 格式逐个启用。',
  },
  {
    id: 'plan',
    label: '兼容套餐',
    hint: '国内 Coding 套餐预设（Anthropic 兼容），Base URL 固定只读；自定义地址走第三方中转。',
  },
  {
    id: 'relay',
    label: '第三方中转',
    hint: '按端点兼容格式选择入口，填入中转端点与密钥后同步模型。',
  },
  { id: 'local', label: '本地服务', hint: 'Ollama、LM Studio 等本机推理服务。' },
  { id: 'uncategorized', label: '待分类', hint: '旧数据未记录接入类型，打开详情补选后保存即可。' },
];

export function providerAccessGroupLabel(group: ProviderAccessGroup): string {
  return PROVIDER_ACCESS_GROUPS.find((item) => item.id === group)?.label ?? group;
}

/** 真实字段 → 展示分组：subscription 由 kind 决定；api 优先 accessType，
 * 历史数据（accessType 缺失）按官方域名推断，其余一律归入第三方中转（用户裁决四分类）。 */
const OFFICIAL_HOSTS = ['api.anthropic.com', 'api.openai.com', 'api.deepseek.com'];
export function providerAccessGroup(
  provider: Pick<ProviderConfig, 'kind'> & Partial<Pick<ProviderConfig, 'accessType' | 'baseUrl'>>,
): ProviderAccessGroup {
  if (provider.kind === 'subscription') return 'subscription';
  switch (provider.accessType) {
    case 'official':
      return 'official';
    case 'plan':
      return 'plan';
    case 'relay':
      return 'relay';
    default:
      break;
  }
  const host = safeHost(provider.baseUrl);
  if (host && OFFICIAL_HOSTS.includes(host)) return 'official';
  return 'relay';
}

function safeHost(baseUrl: string | undefined): string | null {
  if (!baseUrl) return null;
  try {
    return new URL(baseUrl).hostname.toLowerCase();
  } catch {
    return null;
  }
}

function accessTypeForGroup(group: ProviderTemplate['accessGroup']): ProviderAccessType | null {
  if (group === 'official' || group === 'plan' || group === 'relay') return group;
  return null;
}

export interface ProviderTemplate {
  id: ProviderTemplateId;
  /** 添加流程分段：模板归属的接入类型卡（S6） */
  accessGroup: Exclude<ProviderAccessGroup, 'uncategorized'>;
  title: string;
  description: string;
  name: string;
  baseUrl: string;
  protocol: ProviderProtocol;
  authMethod: ProviderAuthMethod;
  kind: ProviderKind;
  /** 品牌图标：官方/套餐用品牌 SVG，中转用线框图标（原型 §7.5.1 卡片语言） */
  brand?:
    | 'anthropic'
    | 'openai'
    | 'deepseek'
    | 'zhipu'
    | 'moonshot'
    | 'minimax'
    | 'mimo'
    | 'volc'
    | 'bytedance'
    | 'xiaomi'
    | 'alibabacloud'
    | 'bailian'
    | 'qwen';
  icon?: 'gitbranch';
  /** 中转默认显示名（名称可编辑） */
  defaultName?: string;
}

export const PROVIDER_TEMPLATES: ProviderTemplate[] = [
  {
    id: 'claude-subscription',
    accessGroup: 'subscription',
    title: 'Claude 订阅',
    description: 'Claude Max / Pro 订阅，隔离登录接入 Claude Code。',
    name: 'Claude 订阅',
    baseUrl: '',
    protocol: 'anthropic',
    authMethod: 'oauth',
    kind: 'subscription',
    brand: 'anthropic',
  },
  {
    id: 'codex-subscription',
    accessGroup: 'subscription',
    title: 'ChatGPT 订阅',
    description: 'ChatGPT Plus / Pro 订阅，隔离登录接入 Codex。',
    name: 'ChatGPT 订阅',
    baseUrl: '',
    protocol: 'openai-responses',
    authMethod: 'oauth',
    kind: 'subscription',
    brand: 'openai',
  },
  {
    id: 'official-anthropic',
    accessGroup: 'official',
    title: 'Anthropic API',
    description: 'Claude 官方端点，Anthropic 兼容格式。',
    name: 'Anthropic API',
    baseUrl: 'https://api.anthropic.com',
    protocol: 'anthropic',
    authMethod: 'apikey',
    kind: 'api',
    brand: 'anthropic',
  },
  {
    id: 'official-openai',
    accessGroup: 'official',
    title: 'OpenAI API',
    description: 'GPT 官方端点，OpenAI Responses 格式。',
    name: 'OpenAI API',
    baseUrl: 'https://api.openai.com/v1',
    protocol: 'openai-responses',
    authMethod: 'apikey',
    kind: 'api',
    brand: 'openai',
  },
  {
    id: 'official-deepseek',
    accessGroup: 'official',
    title: 'DeepSeek API',
    description: 'DeepSeek 官方端点，Anthropic 兼容格式。',
    name: 'DeepSeek API',
    baseUrl: 'https://api.deepseek.com',
    protocol: 'anthropic',
    authMethod: 'apikey',
    kind: 'api',
    brand: 'deepseek',
  },
  {
    id: 'plan-glm-cn',
    accessGroup: 'plan',
    title: 'GLM 中国区',
    description: '智谱开放平台中国区 Coding 套餐。',
    name: 'GLM 中国区',
    baseUrl: 'https://open.bigmodel.cn/api/anthropic',
    protocol: 'anthropic',
    authMethod: 'apikey',
    kind: 'api',
    brand: 'zhipu',
  },
  {
    id: 'plan-glm-intl',
    accessGroup: 'plan',
    title: 'GLM 国际区',
    description: 'Z.ai 国际区 Coding 套餐。',
    name: 'GLM 国际区',
    baseUrl: 'https://api.z.ai/api/anthropic',
    protocol: 'anthropic',
    authMethod: 'apikey',
    kind: 'api',
    brand: 'zhipu',
  },
  {
    id: 'plan-kimi',
    accessGroup: 'plan',
    title: 'Kimi Coding Plan',
    description: '月之暗面 Kimi Coding 套餐，Anthropic 兼容接入。',
    name: 'Kimi Coding Plan',
    baseUrl: 'https://api.moonshot.cn/v1',
    protocol: 'anthropic',
    authMethod: 'apikey',
    kind: 'api',
    brand: 'moonshot',
  },
  {
    id: 'plan-minimax-cn',
    accessGroup: 'plan',
    title: 'MiniMax 中国区',
    description: 'MiniMax 中国区 Coding 套餐。',
    name: 'MiniMax 中国区',
    baseUrl: 'https://api.minimaxi.chat/anthropic',
    protocol: 'anthropic',
    authMethod: 'apikey',
    kind: 'api',
    brand: 'minimax',
  },
  {
    id: 'plan-minimax-intl',
    accessGroup: 'plan',
    title: 'MiniMax 国际区',
    description: 'MiniMax 国际区 Coding 套餐。',
    name: 'MiniMax 国际区',
    baseUrl: 'https://api.minimax.io/anthropic',
    protocol: 'anthropic',
    authMethod: 'apikey',
    kind: 'api',
    brand: 'minimax',
  },
  {
    id: 'plan-mimo-usage',
    accessGroup: 'plan',
    title: 'MiMo 按量 API',
    description: '小米 MiMo 按量计费 API。',
    name: 'MiMo 按量 API',
    baseUrl: 'https://api-open.xiaomi.com/v1',
    protocol: 'anthropic',
    authMethod: 'apikey',
    kind: 'api',
    brand: 'mimo',
  },
  {
    id: 'plan-mimo-token',
    accessGroup: 'plan',
    title: 'MiMo Token Plan',
    description: '小米 MiMo Token 套餐。',
    name: 'MiMo Token Plan',
    baseUrl: 'https://api-open.xiaomi.com/v1',
    protocol: 'anthropic',
    authMethod: 'apikey',
    kind: 'api',
    brand: 'mimo',
  },
  {
    id: 'plan-volc',
    accessGroup: 'plan',
    title: '火山方舟 Coding Plan',
    description: '字节跳动火山方舟 Coding 套餐。',
    name: '火山方舟 Coding Plan',
    baseUrl: 'https://ark.cn-beijing.volces.com/api/v3',
    protocol: 'anthropic',
    authMethod: 'apikey',
    kind: 'api',
    brand: 'volc',
  },
  {
    id: 'plan-bailian',
    accessGroup: 'plan',
    title: '阿里云百炼 Coding Plan',
    description: '阿里云百炼 Coding 套餐。',
    name: '阿里云百炼 Coding Plan',
    baseUrl: 'https://dashscope.aliyuncs.com/compatible-mode/v1',
    protocol: 'anthropic',
    authMethod: 'apikey',
    kind: 'api',
    brand: 'bailian',
  },
  {
    id: 'plan-qwen-personal',
    accessGroup: 'plan',
    title: '千问 Token Plan 个人版',
    description: '个人版 Token 套餐，覆盖 Qwen Coder 系列。',
    name: '千问 Token Plan 个人版',
    baseUrl: 'https://dashscope.aliyuncs.com/compatible-mode/v1',
    protocol: 'anthropic',
    authMethod: 'apikey',
    kind: 'api',
    brand: 'qwen',
  },
  {
    id: 'plan-qwen-team',
    accessGroup: 'plan',
    title: '千问 Token Plan 团队版',
    description: '团队版 Token 套餐，覆盖 Qwen Coder 系列。',
    name: '千问 Token Plan 团队版',
    baseUrl: 'https://dashscope.aliyuncs.com/compatible-mode/v1',
    protocol: 'anthropic',
    authMethod: 'apikey',
    kind: 'api',
    brand: 'qwen',
  },
  {
    id: 'relay-anthropic',
    accessGroup: 'relay',
    title: 'Anthropic 兼容中转',
    description: '自定义中转端点，同步后按四个角色行选择模型。',
    name: '我的 Anthropic 中转',
    defaultName: '我的 Anthropic 中转',
    baseUrl: '',
    protocol: 'anthropic',
    authMethod: 'apikey',
    kind: 'api',
    brand: 'anthropic',
  },
  {
    id: 'relay-openai',
    accessGroup: 'relay',
    title: 'OpenAI Responses 兼容中转',
    description: '自定义中转端点，同步后逐个启用模型。',
    name: '我的 OpenAI 中转',
    defaultName: '我的 OpenAI 中转',
    baseUrl: '',
    protocol: 'openai-responses',
    authMethod: 'apikey',
    kind: 'api',
    brand: 'openai',
  },
  // 历史数据渲染保留；按用户裁决不再出现在添加流程（ADD_FLOW_GROUPS 不含 local）。
  {
    id: 'local-openai',
    accessGroup: 'local',
    title: '本地 OpenAI 兼容服务',
    description: '适合 Ollama、LM Studio 或本机兼容网关，通常无需 API 密钥。',
    name: '本地 OpenAI 兼容服务',
    baseUrl: 'http://localhost:11434/v1',
    protocol: 'openai-chat',
    authMethod: 'local',
    kind: 'local',
  },
];

export function templatesForAccessGroup(
  group: ProviderTemplate['accessGroup'],
): ProviderTemplate[] {
  return PROVIDER_TEMPLATES.filter((template) => template.accessGroup === group);
}

export function accessGroupHint(group: ProviderTemplate['accessGroup']): string {
  return PROVIDER_ACCESS_GROUPS.find((item) => item.id === group)?.hint ?? '';
}

/** 订阅匹配按协议家族（anthropic / openai*）而非精确值，避免历史数据协议漂移漏判 */
export function subscriptionProtocolFamily(protocol: ProviderProtocol): 'anthropic' | 'openai' {
  return protocol === 'anthropic' ? 'anthropic' : 'openai';
}

export function matchingSubscriptionProvider(
  providers: readonly Pick<ProviderConfig, 'id' | 'name' | 'kind' | 'protocol'>[],
  protocol: ProviderProtocol,
) {
  const family = subscriptionProtocolFamily(protocol);
  return providers.find(
    (provider) =>
      provider.kind === 'subscription' && subscriptionProtocolFamily(provider.protocol) === family,
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
    accessType: accessTypeForGroup(template.accessGroup),
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

/** 价格策略倍率输入 → 基点；空值按 1 倍，负数/零钳到 1bp 下限（矩阵 H-4）。 */
export function multiplierInputToBasisPoints(raw: string): number {
  return Math.max(1, Math.round(Number(raw || '1') * 10000));
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
  // 原型 providers.html providerDeleteCluster：阻断态提示「已绑定 N · 解绑后可删除」
  return `已绑定 ${bindingCount} · 解绑后可删除`;
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
  // at<=0 视为无效（旧数据/fixture 占位），不渲染 1970 噪声
  if (!provider.lastTest || !provider.lastTest.at || provider.lastTest.at <= 0) return '尚未测试';
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

/** 绑定/引擎偏好可用的模型选项（方案 a）：
 *  roles-anthropic 服务商且已配置角色 → 选择范围限定为各角色对应的模型；
 *  其余（订阅目录 / list-openai）→ 启用模型目录。 */
export function bindingModelOptions(config: AppConfig, providerId: string): ModelConfig[] {
  const provider = config.providers.find((item) => item.id === providerId);
  if (!provider) return [];
  if (providerModelMode(provider) === 'roles-anthropic') {
    const roleValues = [...new Set(Object.values(provider.roleModels ?? {}).filter(Boolean))];
    if (roleValues.length > 0) {
      const catalog = new Map(
        modelCatalogForProvider(config, providerId).map((model) => [model.id, model]),
      );
      return roleValues.map(
        (id) =>
          catalog.get(id) ?? {
            id,
            providerId,
            displayName: id,
            inputPricePerMtok: 0,
            outputPricePerMtok: 0,
            priceSource: 'manual' as const,
            enabled: true,
          },
      );
    }
  }
  return modelsForProvider(config, providerId);
}

export function modelCatalogForProvider(config: AppConfig, providerId: string): ModelConfig[] {
  return uniqueModels(config.models.filter((model) => model.providerId === providerId));
}

export function modelCatalog(config: AppConfig): ModelConfig[] {
  return uniqueModels(config.models);
}

export function normalizeBindingDraft(config: AppConfig, draft: BindingConfig): BindingConfig {
  const models = bindingModelOptions(config, draft.providerId);
  const modelIds = new Set(models.map((model) => model.id));
  // 方案 b：role: 前缀表示绑定到角色，启动时解析，不参与目录校验
  if (draft.primaryModel.startsWith('role:')) {
    return { ...draft, primaryModel: draft.primaryModel };
  }
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

// ===== S6：接入类型状态视觉与「模型」接入路径聚合 =====

export interface ProviderCardStatus {
  label: string;
  tone: 'ready' | 'warn' | 'muted';
}

/** 服务商卡片状态 pill：只消费真实 ready/lastTest/登录态/模型数，不估算 */
export function providerCardStatus(
  provider: Pick<ProviderConfig, 'kind' | 'ready' | 'lastTest'>,
  login: Pick<CliLoginState, 'state' | 'authMethod'> | null,
  modelCount: number,
): ProviderCardStatus {
  if (!provider.ready) return { label: '待配置', tone: 'muted' };
  if (isSubscriptionProvider(provider)) {
    if (login?.state === 'ok' && login.authMethod === 'subscription') {
      return modelCount > 0
        ? { label: '已登录', tone: 'ready' }
        : { label: '待选模型', tone: 'warn' };
    }
    return { label: loginStateLabel(login), tone: 'warn' };
  }
  if (modelCount === 0) return { label: '待选模型', tone: 'warn' };
  if (provider.lastTest?.result === 'fail') return { label: '探活失败', tone: 'warn' };
  return { label: '配置就绪', tone: 'ready' };
}

/** 与 Rust pricing::normalize_model_id 同口径：模型 Tab 按规范 ID 聚合接入路径。
 * 镜像实现（pricing.rs:891）：trim → ASCII 小写 → '@' 全替换为 '-' → models//anthropic//openai/ 三前缀各自循环剥除。 */
export function normalizeModelGroupId(modelId: string): string {
  let s = modelId.trim().replace(/[A-Z]/g, (c) => c.toLowerCase());
  // 供应商标记前缀（如 aion-labs:aion-2.0 / openrouter:deepseek/chat）：取最后一段
  if (s.includes(':')) s = s.slice(s.lastIndexOf(':') + 1);
  s = s.replace(/@/g, '-');
  for (const prefix of ['models/', 'anthropic/', 'openai/']) {
    while (s.startsWith(prefix)) s = s.slice(prefix.length);
  }
  return s;
}

/** 服务商卡片的模型数＝勾选启用的模型数（用户口径），不是同步全集 */
export function enabledModelCount(config: AppConfig, providerId: string): number {
  return config.models.filter((m) => m.providerId === providerId && m.enabled).length;
}

/** 角色模型行定义（原型 §7.5.4）：Anthropic 兼容四角色；ChatGPT 订阅 主力/快速 */
export const PROVIDER_ROLE_ROWS: Record<
  'anthropic' | 'chatgptSub',
  { key: ProviderRoleKey; label: string }[]
> = {
  anthropic: [
    { key: 'default', label: '默认模型' },
    { key: 'sonnet', label: 'Sonnet' },
    { key: 'opus', label: 'Opus' },
    { key: 'haiku', label: 'Haiku' },
  ],
  chatgptSub: [
    { key: 'main', label: '主力模型' },
    { key: 'fast', label: '快速模型' },
  ],
};

/** 模型配置展示模式（用户裁决）：Anthropic 兼容=按角色；其余（含 OpenAI 系订阅）=同步全量可勾选列表 */
export type ProviderModelMode = 'roles-anthropic' | 'list-openai';

export function providerModelMode(
  provider: Pick<ProviderConfig, 'kind' | 'protocol'>,
): ProviderModelMode {
  return provider.protocol === 'anthropic' ? 'roles-anthropic' : 'list-openai';
}

/** 列表卡/网格品牌键：套餐按预设品牌，其余按协议家族 */
export function providerBrandKey(
  provider: Pick<ProviderConfig, 'kind' | 'protocol' | 'baseUrl'> & { id?: string },
): string {
  if (providerAccessGroup(provider) === 'plan') {
    const host = safeHost(provider.baseUrl);
    const tpl = PROVIDER_TEMPLATES.find(
      (item) => item.accessGroup === 'plan' && safeHost(item.baseUrl) === host,
    );
    if (tpl?.brand) return tpl.brand;
  }
  return subscriptionProtocolFamily(provider.protocol) === 'anthropic' ? 'anthropic' : 'openai';
}

/** 中转服务商字标（原型 pv-card__brand cm-brand--word：名称前两个字符） */
export function relayWordMark(name: string): string {
  return name.trim().slice(0, 2).toUpperCase();
}

/** 会话侧（新任务/工作台）模型选项：roles-anthropic 服务商返回角色条目（方案 b，大写显示）。 */
export function sessionModelOptions(config: AppConfig, providerId: string): ModelConfig[] {
  const provider = config.providers.find((item) => item.id === providerId);
  if (!provider) return [];
  if (providerModelMode(provider) === 'roles-anthropic') {
    return PROVIDER_ROLE_ROWS.anthropic.map((role) => ({
      id: `role:${role.key}`,
      providerId,
      displayName: role.label.toUpperCase(),
      inputPricePerMtok: 0,
      outputPricePerMtok: 0,
      priceSource: 'manual' as const,
      enabled: true,
    }));
  }
  return modelsForProvider(config, providerId);
}

/** 绑定模型值的人类可读标签：role:键 显示为角色名，其余原样返回。 */
export function bindingModelLabel(value: string): string {
  if (!value.startsWith('role:')) return value;
  const role = PROVIDER_ROLE_ROWS.anthropic.find((item) => item.key === value.slice(5));
  return role ? `${role.label}（角色）` : value;
}

/** 模型配置卡计费口径小字（原型 calLabel：套餐等效 / 中转报价 / 自动定价） */
export function modelCalibrationLabel(
  provider: Pick<ProviderConfig, 'kind'> & Partial<Pick<ProviderConfig, 'accessType'>>,
): string {
  switch (providerAccessGroup(provider)) {
    case 'plan':
      return '套餐等效';
    case 'relay':
      return '中转报价';
    default:
      return '自动定价';
  }
}

/** 最近同步时间文案（原型第三元信息：同步于 今天 HH:mm / M月D日 / 尚未同步） */
export function lastSyncTimeText(
  provider: Pick<ProviderConfig, 'lastSyncAt'>,
  hasModels = false,
): string {
  if (!provider.lastSyncAt || provider.lastSyncAt <= 0) return hasModels ? '已同步' : '尚未同步';
  const d = new Date(provider.lastSyncAt * 1000);
  if (Number.isNaN(d.getTime())) return '尚未同步';
  const hm = d.toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit', hour12: false });
  if (d.toDateString() === new Date().toDateString()) return `同步于 今天 ${hm}`;
  return `同步于 ${d.getMonth() + 1}月${d.getDate()}日`;
}

/** 紧凑价签（原型 priceChip：$输入/$缓存读取/$输出；全零=未计价） */
export function priceChipFor(
  model: Pick<ModelConfig, 'inputPricePerMtok' | 'outputPricePerMtok' | 'cachedInputPricePerMtok'>,
): { priced: boolean; text: string } {
  const hasAny =
    model.inputPricePerMtok > 0 ||
    model.outputPricePerMtok > 0 ||
    (model.cachedInputPricePerMtok ?? 0) > 0;
  if (!hasAny) return { priced: false, text: '未计价' };
  const fmt = (v?: number) => `$${Number(v ?? 0)}`;
  return {
    priced: true,
    text: `${fmt(model.inputPricePerMtok)}/${fmt(model.cachedInputPricePerMtok)}/${fmt(model.outputPricePerMtok)}`,
  };
}

/** 中转行才出现「改价」入口（原型仅 relay 行提供手动覆盖） */
export function isRelayProvider(
  provider: Pick<ProviderConfig, 'kind'> & Partial<Pick<ProviderConfig, 'accessType'>>,
): boolean {
  return providerAccessGroup(provider) === 'relay';
}

export function providerRoleModelId(provider: ProviderConfig, key: ProviderRoleKey): string {
  return provider.roleModels?.[key] ?? '';
}

export function withRoleModel(
  provider: ProviderConfig,
  key: ProviderRoleKey,
  modelId: string,
): ProviderConfig {
  return { ...provider, roleModels: { ...(provider.roleModels ?? {}), [key]: modelId } };
}

/** 模型行确认：空值保留该行（同步后仍可从下拉选）；重复 ID 不删行。 */
export type ModelRowCommit =
  | { action: 'keep-empty' }
  | { action: 'duplicate' }
  | { action: 'apply'; id: string };

export function commitProviderModelRow(
  currentIds: readonly string[],
  index: number,
  nextId: string,
): ModelRowCommit {
  const trimmed = nextId.trim();
  if (!trimmed) return { action: 'keep-empty' };
  if (currentIds.some((id, at) => at !== index && id === trimmed)) return { action: 'duplicate' };
  return { action: 'apply', id: trimmed };
}
/** 接入路径计费口径标签（来自真实接入类型，不做价格推算） */
export function pathBillingLabel(
  provider: Pick<ProviderConfig, 'kind'> & Partial<Pick<ProviderConfig, 'accessType'>>,
): string {
  switch (providerAccessGroup(provider)) {
    case 'subscription':
      return '订阅折算';
    case 'plan':
      return '套餐等效';
    case 'official':
      return '官方费率';
    case 'relay':
      return '中转报价';
    case 'local':
      return '本地服务';
    default:
      return '按 Token 计费';
  }
}

export interface ModelAccessPath {
  model: ModelConfig;
  provider: ProviderConfig;
  /** 订阅折算 / 套餐等效 / 官方费率 / 中转费率 / 本地服务 / 按 Token 计费 */
  billing: string;
  /** 该路径当前被哪些引擎绑定（主或快速模型命中即算，取引擎显示名） */
  boundEngines: string[];
}

export interface ModelAccessGroup {
  /** 规范化模型 ID（与定价目录同口径） */
  key: string;
  displayName: string;
  paths: ModelAccessPath[];
}

export function modelBoundEngineNames(
  config: AppConfig,
  providerId: string,
  modelId: string,
): string[] {
  return config.bindings
    .filter(
      (binding) =>
        binding.providerId === providerId &&
        (binding.primaryModel === modelId || binding.fastModel === modelId),
    )
    .map(
      (binding) =>
        config.engines.find((engine) => engine.id === binding.engineId)?.name ?? binding.engineId,
    );
}

/** 跨服务商把同一模型的接入路径聚合到一起；空目录返回空数组 */
export function modelAccessGroups(config: AppConfig): ModelAccessGroup[] {
  const groups = new Map<string, ModelAccessGroup>();
  for (const model of uniqueModels(config.models)) {
    const provider = config.providers.find((item) => item.id === model.providerId);
    if (!provider) continue;
    const key = normalizeModelGroupId(model.displayName || model.id);
    let group = groups.get(key);
    if (!group) {
      group = { key, displayName: key, paths: [] };
      groups.set(key, group);
    }

    group.paths.push({
      model,
      provider,
      billing: pathBillingLabel(provider),
      boundEngines: modelBoundEngineNames(config, model.providerId, model.id),
    });
  }
  const list = [...groups.values()];
  for (const group of list) {
    // 已被引擎绑定的路径排在前（真实生效路由先露出），同组内按服务商名称稳定排序
    group.paths.sort(
      (a, b) =>
        b.boundEngines.length - a.boundEngines.length ||
        a.provider.name.localeCompare(b.provider.name, 'zh-Hans-CN'),
    );
    // 组头展示名取排序后首路径的可读名（避免字典序挑出怪别名）
    group.displayName = group.paths[0]?.model.displayName || group.key;
  }
  list.sort((a, b) => a.displayName.localeCompare(b.displayName, 'zh-Hans-CN'));
  return list;
}

export interface ModelGroupPriceSummary {
  /** 真实 Token 价的三段（缓存段目录缺省时不出现） */
  segments: { label: string; value: string }[];
  /** 兼容文案：三段拼接；无价时为「订阅内 / 待配置」 */
  text: string;
  /** true 表示非 Token 计价（订阅内 / 待配置），UI 用弱化色 */
  plan: boolean;
}

/** 组头价格摘要：优先真实 Token 价路径；全订阅 → 订阅内；无价 → 待配置（决策 B-2b 三段价） */
export function modelGroupPriceSummary(paths: ModelAccessPath[]): ModelGroupPriceSummary {
  const priced = paths.find(
    (path) =>
      path.provider.kind !== 'subscription' &&
      (path.model.inputPricePerMtok > 0 || path.model.outputPricePerMtok > 0),
  );
  if (priced) {
    const segments = [
      { label: '输入', value: `$${priced.model.inputPricePerMtok.toFixed(2)}` },
      ...(priced.model.cachedInputPricePerMtok && priced.model.cachedInputPricePerMtok > 0
        ? [{ label: '缓存', value: `$${priced.model.cachedInputPricePerMtok.toFixed(2)}` }]
        : []),
      { label: '输出', value: `$${priced.model.outputPricePerMtok.toFixed(2)}` },
    ];
    return {
      segments,
      text: `${segments.map((segment) => `${segment.label} ${segment.value}`).join(' · ')} / M`,
      plan: false,
    };
  }
  if (paths.some((path) => path.provider.kind === 'subscription')) {
    return { segments: [], text: '订阅内', plan: true };
  }
  return { segments: [], text: '待配置', plan: true };
}

/** 服务商页四个展示分组（用户裁决：授权登录 / 官方 API 直连 / 兼容套餐 / 第三方中转） */
export const PROVIDER_ACCESS_GROUPS = PROVIDER_ACCESS_GROUPS_DISPLAY.filter(
  (item) =>
    item.id === 'subscription' ||
    item.id === 'official' ||
    item.id === 'plan' ||
    item.id === 'relay',
) as typeof PROVIDER_ACCESS_GROUPS_DISPLAY;
