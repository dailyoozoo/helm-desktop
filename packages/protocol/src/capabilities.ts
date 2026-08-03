import type { ReasoningEffort } from './reasoning';

export type CapabilitySupport = 'supported' | 'degraded' | 'unsupported' | 'unknown';

export interface CapabilityEvidence {
  support: CapabilitySupport;
  source: string;
  diagnostic: string;
}

export interface CapabilitySet {
  modelOverride: CapabilityEvidence;
  reasoningEffort: CapabilityEvidence;
  nativeResume: CapabilityEvidence;
  approval: CapabilityEvidence;
  search: CapabilityEvidence;
  fetch: CapabilityEvidence;
  usage: CapabilityEvidence;
  interrupt: CapabilityEvidence;
  modelOnlyOperation: CapabilityEvidence;
  /** Claude Auto 的身份级运行时证据；旧快照缺省为 unknown。 */
  autoApproval?: CapabilityEvidence;
  reasoningEfforts?: ReasoningEffort[];
  defaultReasoningEffort?: ReasoningEffort;
  contextWindow?: number;
}

export interface CapabilityIdentity {
  engineId: string;
  adapterVersion: string;
  binaryIdentity: string;
  engineProfileDigest: string;
  providerLaunchProfileRef: string;
  providerLaunchProfileDigest: string;
  launchProfileIdentity: string;
  modelCapabilityKey: string;
}

export interface EngineCapabilitySnapshot {
  id: string;
  identity: CapabilityIdentity;
  capabilities: CapabilitySet;
  probeKind: string;
  probedAt: number;
}
