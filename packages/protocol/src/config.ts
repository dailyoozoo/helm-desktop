import type { EngineId } from './events';
import type { ReasoningEffort } from './reasoning';

export type Protocol = 'anthropic' | 'openai-responses' | 'openai-chat' | 'bedrock' | 'vertex';

export type AuthMethod = 'apikey' | 'oauth' | 'cloud' | 'local';

export type ProviderKind = 'subscription' | 'api' | 'local';
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

export type FailureCategory = 'network' | 'auth' | 'timeout' | 'unknown';

export interface ProviderTest {
  result: 'ok' | 'fail' | 'unverified';
  latencyMs?: number;
  at: number;
  failureCategory?: FailureCategory;
}

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
}

export interface Model {
  id: string;
  providerId: string;
  displayName: string;
  inputPricePerMtok: number;
  outputPricePerMtok: number;
  priceSource?: 'provider' | 'builtin' | 'manual' | 'subscription' | 'unknown';
  enabled: boolean;
  /** 模型上下文窗口大小（token 数），上游无数据时为 undefined */
  contextWindow?: number;
  /** 模型能力标签列表（如 'vision', 'tool_use' 等），上游无数据时为 undefined */
  capabilities?: string[];
}

export interface Engine {
  id: EngineId;
  name: string;
  bin: string;
  defaultModel: string;
  status: 'ready' | 'missing' | 'error';
  version?: string | null;
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
