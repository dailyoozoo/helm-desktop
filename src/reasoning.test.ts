import { describe, expect, it } from 'vitest';
import {
  effortOptionsFor,
  engineEffortTiers,
  normalizeReasoningEffort,
  reasoningEffortLabel,
} from './reasoning';
import type { ReasoningEffortCapability } from '@helm/protocol';

const capabilityOf = (
  support: ReasoningEffortCapability['support'],
  options: ReasoningEffortCapability['options'],
): ReasoningEffortCapability => ({
  support,
  options,
  source: 'engine-probe',
});

describe('reasoning effort view model', () => {
  it('falls back to auto when a model does not advertise the selected level', () => {
    expect(
      normalizeReasoningEffort(
        {
          support: 'supported',
          options: ['auto', 'low', 'high'],
          defaultEffort: 'low',
          source: 'engine-probe',
        },
        'xhigh',
      ),
    ).toBe('auto');
  });

  it('uses concise Chinese labels', () => {
    expect(reasoningEffortLabel('auto')).toBe('自动');
    expect(reasoningEffortLabel('xhigh')).toBe('超高');
  });

  // 2026-08-27 用户裁决：档位跟随 Agent（引擎）——探测明确支持时以探测为准，
  // unknown 回落引擎档位表（Claude 有 max，Codex 有 minimal），unsupported 仅自动。
  it('engine tiers follow the agent when the probe is unavailable', () => {
    expect(engineEffortTiers('claude-code')).toEqual([
      'auto',
      'low',
      'medium',
      'high',
      'xhigh',
      'max',
    ]);
    expect(engineEffortTiers('codex')).toEqual([
      'auto',
      'minimal',
      'low',
      'medium',
      'high',
      'xhigh',
    ]);
    expect(effortOptionsFor(null, 'claude-code')).toEqual(engineEffortTiers('claude-code'));
    expect(effortOptionsFor(capabilityOf('unknown', ['auto']), 'codex')).toEqual(
      engineEffortTiers('codex'),
    );
  });

  it('a supported probe wins over the engine tiers; unsupported collapses to auto', () => {
    expect(effortOptionsFor(capabilityOf('supported', ['auto', 'low']), 'codex')).toEqual([
      'auto',
      'low',
    ]);
    expect(effortOptionsFor(capabilityOf('unsupported', ['auto']), 'claude-code')).toEqual([
      'auto',
    ]);
  });

  it('normalize keeps a valid engine-tier selection when the probe is unknown', () => {
    expect(normalizeReasoningEffort(capabilityOf('unknown', ['auto']), 'max', 'claude-code')).toBe(
      'max',
    );
    expect(normalizeReasoningEffort(capabilityOf('unknown', ['auto']), 'max', 'codex')).toBe(
      'auto',
    );
    expect(
      normalizeReasoningEffort(capabilityOf('unsupported', ['auto']), 'high', 'claude-code'),
    ).toBe('auto');
  });
});
