import type { EngineId, RuntimeCapabilitySnapshot } from './events';
import type { ReasoningEffort } from './reasoning';
import type { EngineCapabilitySnapshot } from './capabilities';

/** Runtime 的稳定业务所有者。Operation 分支由 27I 的后台任务复用。 */
export type RuntimeOwnerRef = { kind: 'session'; id: string } | { kind: 'operation'; id: string };

export interface RuntimeGeneration {
  id: string;
  owner: RuntimeOwnerRef;
  engineId: EngineId;
  compatibilityKey: string;
  engineProfileDigest: string;
  providerLaunchProfileRef: string;
  providerLaunchProfileDigest: string;
  capabilitySnapshotId: string;
  canonicalCwd: string;
  createdAt: number;
}

export interface NativeSessionRef {
  id: string;
  generationId: string;
  owner: RuntimeOwnerRef;
  engineId: EngineId;
  nativeKind: 'claude_session_id' | 'codex_thread_id';
  nativeId: string;
  launchProfileIdentity: string;
  createdAt: number;
}

export type TurnAttemptDeliveryState =
  | 'prepared'
  | 'accepted'
  | 'rejected'
  | 'completed'
  | 'interrupted'
  | 'error'
  | 'delivery_unknown';

export interface TurnAttempt {
  turnId: string;
  attemptNo: number;
  owner: Extract<RuntimeOwnerRef, { kind: 'session' }>;
  generationId: string;
  runtimeCompatibilityKey: string;
  inputNativeRefId?: string;
  outputNativeRefId?: string;
  observedModelId?: string;
  observedReasoningEffort?: ReasoningEffort;
  actualCapabilitySnapshot?: EngineCapabilitySnapshot | RuntimeCapabilitySnapshot;
  deliveryState: TurnAttemptDeliveryState;
  terminalReceipt?: string;
  createdAt: number;
  acceptedAt?: number;
  endedAt?: number;
}

export interface TurnRecoveryInput {
  turnId: string;
  attemptNo: number;
  owner: Extract<RuntimeOwnerRef, { kind: 'session' }>;
  generationId: string;
  deliveryState: Extract<TurnAttemptDeliveryState, 'prepared' | 'accepted' | 'delivery_unknown'>;
  inputNativeRefId?: string;
  outputNativeRefId?: string;
}
