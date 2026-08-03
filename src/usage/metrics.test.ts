import { describe, expect, it } from 'vitest';
import type { ProviderConfig } from '../providers/api';
import {
  comparisonText,
  createRequestGate,
  mergeProviderUsage,
  percentageChange,
  projectedMonthEndCost,
} from './metrics';

const providers: ProviderConfig[] = [
  {
    id: 'api',
    name: 'API Provider',
    kind: 'api',
    baseUrl: 'https://example.com',
    keyRef: 'key',
    ready: true,
    lastTest: { result: 'ok', at: 1 },
    protocol: 'anthropic',
    authMethod: 'apikey',
  },
  {
    id: 'subscription',
    name: 'Subscription Provider',
    kind: 'subscription',
    baseUrl: '',
    keyRef: null,
    ready: true,
    lastTest: null,
    protocol: 'openai-responses',
    authMethod: 'oauth',
  },
];

describe('usage metrics', () => {
  it('calculates comparisons without inventing a percentage from a zero baseline', () => {
    expect(percentageChange(12, 10)).toBe(20);
    expect(percentageChange(0, 0)).toBe(0);
    expect(percentageChange(5, 0)).toBeNull();
    expect(comparisonText(5, 0)).toBe('较前一期无基数');
  });

  it('uses one calendar-aware month-end projection algorithm', () => {
    expect(projectedMonthEndCost(140, new Date(2026, 1, 14))).toBe(280);
    expect(projectedMonthEndCost(31, new Date(2026, 0, 1))).toBe(961);
  });

  it('keeps zero-cost runtime-ready providers in the provider rows', () => {
    const rows = mergeProviderUsage(providers, [{ provider: 'api', cost_usd: 4, share: 1 }], {
      subscription: { state: 'ok', authMethod: 'subscription', detail: 'ok' },
    });

    expect(rows).toEqual([
      expect.objectContaining({ provider: 'api', cost_usd: 4, ready: true }),
      expect.objectContaining({ provider: 'subscription', cost_usd: 0, ready: true }),
    ]);
  });

  it('does not call an API-key CLI login a ready subscription', () => {
    const rows = mergeProviderUsage(providers, [], {
      subscription: { state: 'ok', authMethod: 'apikey', detail: 'API key' },
    });
    expect(rows.map((row) => row.provider)).toEqual(['api']);
  });

  it('accepts only the newest result during concurrent refreshes', () => {
    const gate = createRequestGate();
    const first = gate.begin();
    const second = gate.begin();
    expect(gate.isCurrent(first)).toBe(false);
    expect(gate.isCurrent(second)).toBe(true);
  });

  it('returns an empty provider list when config and usage are both empty', () => {
    expect(mergeProviderUsage([], [])).toEqual([]);
  });
});
