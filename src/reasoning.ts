import type { EngineId, ReasoningEffort, ReasoningEffortCapability } from '@helm/protocol';

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

/**
 * 推理强度档位表（第四轮用户决议，偏离变更-17 逐模型探测口径）：
 * 强度跟随 Agent（引擎）而非模型——各引擎固定展示其 CLI 声明过的档位集：
 * Claude Code `--effort` = low/medium/high/xhigh/max（docs/变更-17 §事实），
 * Codex = minimal/low/medium/high/xhigh；`auto` 恒为首项（模型默认）。
 * 所选档位仍随首条消息冻结进 TurnExecutionSpec。
 * 2026-08-27 用户裁决：新任务页与工作区 Composer 共用本表——真实探测明确支持时
 * 以探测的逐模型集合为准；探测不可用（unknown）时回落本表；显式 unsupported 仅自动。
 */
const ENGINE_EFFORT_TIERS: Record<EngineId, ReasoningEffort[]> = {
  'claude-code': ['auto', 'low', 'medium', 'high', 'xhigh', 'max'],
  codex: ['auto', 'minimal', 'low', 'medium', 'high', 'xhigh'],
};

export function engineEffortTiers(engine: EngineId): ReasoningEffort[] {
  return ENGINE_EFFORT_TIERS[engine] ?? ['auto'];
}

/** Composer 菜单档位集：探测明确支持 → 逐模型真实集合；显式不支持 → 仅自动；其余回落引擎档位表。 */
export function effortOptionsFor(
  capability: ReasoningEffortCapability | null | undefined,
  engine?: EngineId | string,
): ReasoningEffort[] {
  if (capability?.support === 'supported' && capability.options.length > 0) {
    return capability.options;
  }
  if (capability?.support === 'unsupported') return ['auto'];
  return ENGINE_EFFORT_TIERS[engine as EngineId] ?? ['auto'];
}

export function normalizeReasoningEffort(
  capability: ReasoningEffortCapability | null | undefined,
  effort: ReasoningEffort | null | undefined,
  engine?: EngineId,
): ReasoningEffort {
  const desired = effort ?? 'auto';
  const options = engine ? effortOptionsFor(capability, engine) : (capability?.options ?? ['auto']);
  return options.includes(desired) ? desired : 'auto';
}
