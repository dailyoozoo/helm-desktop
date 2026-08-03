import { useEffect, useState } from 'react';
import type { EngineId, ReasoningEffortCapability } from '@helm/protocol';
import { getReasoningEffortCapability } from './transport';

const capabilityCache = new Map<string, Promise<ReasoningEffortCapability>>();

function loadCapability(
  engine: EngineId,
  model: string,
  providerId: string,
): Promise<ReasoningEffortCapability> {
  const key = `${engine}:${providerId}:${model}`;
  const existing = capabilityCache.get(key);
  if (existing) return existing;
  const request = getReasoningEffortCapability(engine, model, providerId || undefined).catch(
    (error) => {
      capabilityCache.delete(key);
      throw error;
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
    if (!model.trim()) {
      setCapability(null);
      setLoading(false);
      setError(null);
      return;
    }
    setCapability(null);
    setLoading(true);
    setError(null);
    loadCapability(engine, model, providerId)
      .then((next) => {
        if (active) setCapability(next);
      })
      .catch((reason: unknown) => {
        if (active) setError(reason instanceof Error ? reason.message : String(reason));
      })
      .finally(() => {
        if (active) setLoading(false);
      });
    return () => {
      active = false;
    };
  }, [engine, model, providerId]);

  return { capability, loading, error };
}

export function resetReasoningCapabilityCacheForTests(): void {
  capabilityCache.clear();
}
