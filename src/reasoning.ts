import type { ReasoningEffort, ReasoningEffortCapability } from '@helm/protocol';

export const AUTO_REASONING_CAPABILITY: ReasoningEffortCapability = {
  support: 'unknown',
  options: ['auto'],
  source: 'builtin-catalog',
};

const LABELS: Record<ReasoningEffort, string> = {
  auto: '自动',
  none: '关闭',
  minimal: '最少',
  low: '低',
  medium: '中',
  high: '高',
  xhigh: '超高',
  max: '最大',
};

export function reasoningEffortLabel(effort: ReasoningEffort): string {
  return LABELS[effort];
}

export function normalizeReasoningEffort(
  capability: ReasoningEffortCapability | null,
  effort: ReasoningEffort | null | undefined,
): ReasoningEffort {
  const desired = effort ?? 'auto';
  return capability?.options.includes(desired) ? desired : 'auto';
}
