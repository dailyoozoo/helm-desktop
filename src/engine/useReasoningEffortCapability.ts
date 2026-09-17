import { useEffect, useState } from 'react';
import type { EngineId, ReasoningEffortCapability } from '@helm/protocol';
import { PROVIDER_CONFIG_CHANGED_EVENT } from '../providers/api';
import { getReasoningEffortCapability } from './transport';

const capabilityCache = new Map<string, Promise<ReasoningEffortCapability>>();
const refreshListeners = new Set<() => void>();

function invalidateCapabilities(): void {
  capabilityCache.clear();
  for (const refresh of [...refreshListeners]) refresh();
}

function subscribeToCapabilityChanges(refresh: () => void): () => void {
  if (refreshListeners.size === 0 && typeof window !== 'undefined') {
    window.addEventListener(PROVIDER_CONFIG_CHANGED_EVENT, invalidateCapabilities);
    window.addEventListener('focus', invalidateCapabilities);
  }
  refreshListeners.add(refresh);
  return () => {
    refreshListeners.delete(refresh);
    if (refreshListeners.size === 0) {
      capabilityCache.clear();
      if (typeof window !== 'undefined') {
        window.removeEventListener(PROVIDER_CONFIG_CHANGED_EVENT, invalidateCapabilities);
        window.removeEventListener('focus', invalidateCapabilities);
      }
    }
  };
}

function loadCapability(
  engine: EngineId,
  model: string,
  providerId: string,
): Promise<ReasoningEffortCapability> {
  const key = JSON.stringify([engine, providerId, model]);
  const existing = capabilityCache.get(key);
  if (existing) return existing;
  const request = getReasoningEffortCapability(engine, model, providerId || undefined).finally(
    () => {
      if (capabilityCache.get(key) === request) capabilityCache.delete(key);
    },
  );
  capabilityCache.set(key, request);
  return request;
}

export function useReasoningEffortCapability(
  engine: EngineId,
  model: string,
  providerId = '',
): {
  capability: ReasoningEffortCapability | null;
  loading: boolean;
  error: string | null;
} {
  const [capability, setCapability] = useState<ReasoningEffortCapability | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let active = true;
    let requestVersion = 0;
    if (!model.trim()) {
      setCapability(null);
      setLoading(false);
      setError(null);
      return;
    }
    const refresh = () => {
      const version = ++requestVersion;
      setCapability(null);
      setLoading(true);
      setError(null);
      loadCapability(engine, model, providerId)
        .then((next) => {
          if (active && version === requestVersion) setCapability(next);
        })
        .catch((reason: unknown) => {
          if (active && version === requestVersion) {
            setError(reason instanceof Error ? reason.message : String(reason));
          }
        })
        .finally(() => {
          if (active && version === requestVersion) setLoading(false);
        });
    };
    const unsubscribe = subscribeToCapabilityChanges(refresh);
    refresh();
    return () => {
      active = false;
      unsubscribe();
    };
  }, [engine, model, providerId]);

  return { capability, loading, error };
}

export function resetReasoningCapabilityCacheForTests(): void {
  capabilityCache.clear();
}
