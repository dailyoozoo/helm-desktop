import { describe, expect, it } from 'vitest';
import { normalizeReasoningEffort, reasoningEffortLabel } from './reasoning';

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
});
