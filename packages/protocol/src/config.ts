import type { EngineId } from './events';
import type { ReasoningEffort } from './reasoning';

export type Protocol = 'anthropic' | 'openai-responses' | 'openai-chat' | 'bedrock' | 'vertex';

export type AuthMethod = 'apikey' | 'oauth' | 'cloud' | 'local';

export type ProviderKind = 'subscription' | 'api' | 'local';
/** 服务商接入类型（S6）：api 类服务商在添加/详情时声明的分组意图；缺省由 UI 归「待分类」 */
export type ProviderAccessType = 'official' | 'plan' | 'relay';
export type ServiceTier = 'standard' | 'batch' | 'flex' | 'priority';
export type ProviderPricingMode =
  | 'auto'
  | 'provider'
  | 'official-reference'
  | 'manual'
  | 'disabled';

export interface PricingBand {
  minInputTokens?: number | null;
  maxInputTokens?: number | null;
  input: number;
  cachedInput?: number | null;
  cacheWrite?: number | null;
  output: number;
}

export interface PricingTier {
  bands: PricingBand[];
}

export interface ModelPriceOverride {
  providerId: string;
  modelId: string;
  currency: 'USD';
  tiers: Partial<Record<ServiceTier, PricingTier>>;
  updatedAt: number;
}

export interface ProviderPricingPreference {
  providerId: string;
  mode: ProviderPricingMode;
  multiplierBasisPoints: number;
}

export interface PricingCatalogStatus {
  source: 'builtin' | 'cache' | string;
  catalogVersion: string;
  sequence: number;
  publishedAt: string;
  lastCheckedAt?: number | null;
  lastError?: string | null;
  stale: boolean;
}

/** 定价目录标准价格条目（S6「标准价格表」只读参考费率，来自已验签缓存或内置目录） */
export interface PricingCatalogEntry {
  vendor: string;
  modelId: string;
  currency: string;
  input: number;
  cachedInput?: number | null;
  cacheWrite?: number | null;
  output: number;
  observedAt: string;
}

export type FailureCategory = 'network' | 'auth' | 'timeout' | 'unknown';

export interface ProviderTest {
  result: 'ok' | 'fail' | 'unverified';
  latencyMs?: number;
  at: number;
  failureCategory?: FailureCategory;
}

/** 角色模型键：Anthropic 兼容格式四角色 + ChatGPT 订阅主力/快速（原型 §7.5.4） */
export type ProviderRoleKey = 'default' | 'sonnet' | 'opus' | 'haiku' | 'main' | 'fast';

export interface Provider {
  id: string;
  name: string;
  kind: ProviderKind;
  baseUrl: string;
  keyRef?: string | null;
  ready: boolean;
  lastTest?: ProviderTest | null;
  protocol: Protocol;
  authMethod: AuthMethod;
  /** 接入类型：仅 api 类需要；subscription/local 由 kind 决定分组 */
  accessType?: ProviderAccessType | null;
  /** 角色模型选择（Anthropic 兼容四角色 / ChatGPT 订阅主力+快速）；启动注入待接（已知限制） */
  roleModels?: Partial<Record<ProviderRoleKey, string>> | null;
  /** 最近一次成功同步模型目录时间（秒级 epoch，与 lastTest.at 同口径）；缺省=尚未同步 */
  lastSyncAt?: number | null;
}

export interface Model {
  id: string;
  providerId: string;
  displayName: string;
  inputPricePerMtok: number;
  outputPricePerMtok: number;
  /** 缓存读取价（USD / M tokens）；目录无该档或上游未提供时为 undefined（决策 B-2b） */
  cachedInputPricePerMtok?: number;
  priceSource?: 'provider' | 'builtin' | 'manual' | 'subscription' | 'unknown';
  enabled: boolean;
  /** 模型上下文窗口大小（token 数），上游无数据时为 undefined */
  contextWindow?: number;
  /** 模型能力标签列表（如 'vision', 'tool_use' 等），上游无数据时为 undefined */
  capabilities?: string[];
}

export interface EngineEnvVar {
  name: string;
  /** 秘密值不落盘：secret=true 时 value 恒为空，仅本次会话生效 */
  value?: string;
  secret?: boolean;
}

export interface Engine {
  id: EngineId;
  name: string;
  bin: string;
  defaultModel: string;
  status: 'ready' | 'missing' | 'error';
  version?: string | null;
  /** 引擎级环境变量覆盖（对 Helm 与原生 Agent 生效；启动注入待接 Runtime spec） */
  envVars?: EngineEnvVar[] | null;
}

export interface Binding {
  /** Persistent monotonic revision; legacy config defaults to 0. */
  revision?: number;
  engineId: EngineId;
  providerId: string;
  primaryModel: string;
  fastModel?: string | null;
  /** 辅助模型，用于起标题等轻量任务；无则 fallback 到 fast_model / 主模型。 */
  assistantModelId?: string | null;
  /** 新建 Session 的默认推理强度；旧配置缺省等价于 auto。 */
  reasoningEffort?: ReasoningEffort;
  /** Claude Code 引擎偏好：默认开启思考（原生能力，独立于推理强度）。 */
  thinkingEnabled?: boolean;
  /** Claude Code 引擎偏好：1M 上下文（开启时先校验服务商与模型能力）。 */
  context1m?: boolean;
}

export interface AppConfig {
  providers: Provider[];
  models: Model[];
  engines: Engine[];
  bindings: Binding[];
  defaultEngine: EngineId;
  defaultModel: string;
}

export interface EquivalentEnvVar {
  name: string;
  value: string;
}
