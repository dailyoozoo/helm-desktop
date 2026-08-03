import { describe, expect, it } from 'vitest';
import type {
  NativeSessionRef,
  RuntimeGeneration,
  RuntimeOwnerRef,
  TurnAttempt,
  TurnRecoveryInput,
} from '@helm/protocol';

describe('runtime ownership contract', () => {
  it('keeps Session and Operation owners structurally distinct', () => {
    const sessionOwner = { kind: 'session', id: 'session-1' } satisfies RuntimeOwnerRef;
    const operationOwner = { kind: 'operation', id: 'operation-1' } satisfies RuntimeOwnerRef;
    expect(sessionOwner.kind).not.toBe(operationOwner.kind);
  });

  it('models generation, native ref, attempt and recovery without route-spec fields', () => {
    const owner = { kind: 'session', id: 'session-1' } as const;
    const generation = {
      id: 'runtime-1',
      owner,
      engineId: 'codex',
      compatibilityKey: 'sha256:compat',
      engineProfileDigest: 'sha256:engine',
      providerLaunchProfileRef: 'provider:openai:api',
      providerLaunchProfileDigest: 'sha256:provider-launch',
      capabilitySnapshotId: 'capability-1',
      canonicalCwd: 'c:\\repo',
      createdAt: 1,
    } satisfies RuntimeGeneration;
    const nativeRef = {
      id: 'native-ref-1',
      generationId: generation.id,
      owner,
      engineId: 'codex',
      nativeKind: 'codex_thread_id',
      nativeId: 'thread-1',
      launchProfileIdentity: generation.providerLaunchProfileRef,
      createdAt: 2,
    } satisfies NativeSessionRef;
    const attempt = {
      turnId: 'turn-1',
      attemptNo: 1,
      owner,
      generationId: generation.id,
      runtimeCompatibilityKey: generation.compatibilityKey,
      outputNativeRefId: nativeRef.id,
      deliveryState: 'accepted',
      createdAt: 2,
      acceptedAt: 3,
    } satisfies TurnAttempt;
    const recovery = {
      turnId: attempt.turnId,
      attemptNo: attempt.attemptNo,
      owner,
      generationId: generation.id,
      deliveryState: 'accepted',
      outputNativeRefId: nativeRef.id,
    } satisfies TurnRecoveryInput;
    expect(recovery).not.toHaveProperty('providerId');
    expect(recovery.outputNativeRefId).toBe(nativeRef.id);
  });
});
