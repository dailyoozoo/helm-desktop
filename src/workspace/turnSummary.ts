import type { ThreadRenderEntry } from './threadGroups';
import type { SessionTurn } from '../sessions/api';

export interface TurnSummaryMeta {
  /** 第几轮（用户消息序号） */
  turnNumber: number;
  /** 轮次路由模型，来自 TurnLedger（SessionTurn.routedModelId）；缺项不显示。 */
  model?: string;
  /** 已完成的耗时（秒），进行中不显示。 */
  durationSec?: number;
  /** 思考条目累计时长（秒，真实 startedAt/endedAt 求和）；原型「思考了 N 秒」。 */
  thinkingSec?: number;
  toolCount?: number;
  added?: number;
  removed?: number;
}

/** 统计一个 Turn 内所有交付物与工具产生的 add/del 行数（含工具组内条目）。 */
export function turnDiffStats(entries: ThreadRenderEntry[]): {
  added: number;
  removed: number;
  toolCount: number;
} {
  let added = 0;
  let removed = 0;
  let toolCount = 0;
  for (const entry of entries) {
    const items =
      entry.kind === 'tool-group' || entry.kind === 'subagent'
        ? entry.items
        : entry.item.kind === 'tool'
          ? [entry.item]
          : [];
    for (const item of items) {
      toolCount += 1;
      if (!item.diff) continue;
      for (const hunk of item.diff.hunks) {
        for (const line of hunk.lines) {
          if (line.kind === 'add') added += 1;
          else if (line.kind === 'del') removed += 1;
        }
      }
    }
  }
  return { added, removed, toolCount };
}

/**
 * 由真实条目汇总轮次摘要事实（变更-34/35 · B2）。
 * - 模型优先取 TurnLedger 的 routedModelId；缺项不显示。
 * - 耗时仅在该轮所有事件都已结束（Turn.endedAt 或条目的 endedAt 齐全）时给出，
 *   进行中的轮次不估算、不显示，避免伪进度。
 */
export function summarizeTurn(
  entries: ThreadRenderEntry[],
  turnNumber: number,
  turn?: SessionTurn | null,
): TurnSummaryMeta {
  const { added, removed, toolCount } = turnDiffStats(entries);

  let durationSec: number | undefined;
  if (turn?.startedAt != null && turn.endedAt != null) {
    durationSec = Math.max(0, (turn.endedAt - turn.startedAt) / 1000);
  } else {
    const started = entries
      .flatMap((entry) =>
        entry.kind === 'tool-group' || entry.kind === 'subagent'
          ? entry.items.map((item) => item.startedAt)
          : [entry.item.startedAt],
      )
      .filter((value): value is number => typeof value === 'number');
    const ended = entries
      .flatMap((entry) =>
        entry.kind === 'tool-group' || entry.kind === 'subagent'
          ? entry.items.map((item) => item.endedAt)
          : [entry.item.endedAt],
      )
      .filter((value): value is number => typeof value === 'number');
    if (started.length > 0 && ended.length === started.length) {
      durationSec = Math.max(0, (Math.max(...ended) - Math.min(...started)) / 1000);
    }
  }

  // 思考时长：真实 thinking 条目 startedAt/endedAt 求和（缺项不计，不估算）。
  const thinkingSec = entries
    .filter((entry) => entry.kind === 'item' && entry.item.kind === 'thinking')
    .map((entry) => {
      const item = entry.kind === 'item' ? entry.item : null;
      return item && item.kind === 'thinking' && item.startedAt != null && item.endedAt != null
        ? Math.max(0, (item.endedAt - item.startedAt) / 1000)
        : 0;
    })
    .reduce((sum, value) => sum + value, 0);

  const summary: TurnSummaryMeta = { turnNumber, toolCount };
  if (turn?.routedModelId) summary.model = turn.routedModelId;
  if (toolCount > 0) summary.toolCount = toolCount;
  if (durationSec != null) summary.durationSec = durationSec;
  if (thinkingSec > 0) summary.thinkingSec = thinkingSec;
  if (added > 0) summary.added = added;
  if (removed > 0) summary.removed = removed;
  return summary;
}

export function formatTurnDuration(sec: number): string {
  const s = Math.round(sec);
  if (s < 60) return `${s}秒`;
  const minutes = Math.floor(s / 60);
  const seconds = s % 60;
  return `${minutes}分${seconds}秒`;
}
