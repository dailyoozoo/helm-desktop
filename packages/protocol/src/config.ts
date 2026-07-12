import type { EngineId } from './events';

export type Protocol = 'anthropic' | 'openai-responses' | 'openai-chat' | 'bedrock' | 'vertex';

export type AuthMethod = 'apikey' | 'oauth' | 'cloud' | 'local';

export interface ProviderTest {
  result: 'ok' | 'fail' | 'unverified';
  latencyMs?: number;
  at: number;
}

export interface Provider {
  id: string;
  name: string;
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
  priceSource?: 'provider' | 'builtin' | 'manual' | 'unknown';
  enabled: boolean;
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
  engineId: EngineId;
  providerId: string;
  primaryModel: string;
  fastModel?: string | null;
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
