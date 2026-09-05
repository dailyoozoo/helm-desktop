import type { ThreadItem } from '../engine/useSession';
import { isSubagentToolName } from './items/taskViewModel';

export type ThreadRenderEntry =
  | { kind: 'item'; item: ThreadItem }
  | { kind: 'tool-group'; id: string; items: Extract<ThreadItem, { kind: 'tool' }>[] }
  | { kind: 'subagent'; id: string; items: Extract<ThreadItem, { kind: 'tool' }>[] };

export type ThreadLayoutEntry = ThreadRenderEntry;

const TERMINAL_TOOL_NAMES = new Set([
  'bash',
  'shell',
  'terminal',
  'command',
  'executecommand',
  'runcommand',
  'run_terminal_cmd',
]);

export function isTerminalToolName(name: string): boolean {
  return TERMINAL_TOOL_NAMES.has(name.toLowerCase());
}

/**
 * 失败工具例外（TurnProcess 渲染契约「失败卡等 children 常驻可见、不依赖整轮折叠」）：
 * 独立失败工具由 ToolBlock 提成 [data-kind="fail"] > .failc 就地展开卡，作为 children
 * 渲染在可折叠过程体之外——收起的轮次也保持失败可见（2026-09-04 视觉矩阵断言锚定）。
 * 口径与 TurnProcess 的 isActualFailure / ToolGroup 的 isDenied 一致：
 * status=error 且 outcome 非拒绝、非 auto_review 复核态（这些保持过程区内联呈现）。
 */
export function isLiftedFailureEntry(entry: ThreadRenderEntry): boolean {
  if (entry.kind !== 'item' || entry.item.kind !== 'tool') return false;
  const item = entry.item;
  return (
    item.status === 'error' &&
    item.outcome !== 'auto_review_unavailable' &&
    item.outcome !== 'auto_review_parse_error' &&
    item.outcome !== 'auto_review_blocked' &&
    item.outcome !== 'runtime_denied'
  );
}

/** 只聚合同一 Turn 中相邻的工具调用，交付物和正文会明确切断分组。 */
export function groupThreadItems(items: ThreadItem[]): ThreadRenderEntry[] {
  const entries: ThreadRenderEntry[] = [];
  let index = 0;
  while (index < items.length) {
    const item = items[index];
    if (item.kind !== 'tool') {
      entries.push({ kind: 'item', item });
      index += 1;
      continue;
    }
    // 子代理工具（C1）：连续的同 Turn 子代理合入一张并行子代理卡
    if (isSubagentToolName(item.name)) {
      const group = [item];
      let next = index + 1;
      while (next < items.length) {
        const candidate = items[next];
        if (candidate.kind !== 'tool') break;
        if (!isSubagentToolName(candidate.name)) break;
        if (candidate.turnId !== item.turnId) break;
        if (Boolean(item.reverted) !== Boolean(candidate.reverted)) break;
        group.push(candidate);
        next += 1;
      }
      entries.push({ kind: 'subagent', id: group[0].id, items: group });
      index = next;
      continue;
    }
    const group = [item];
    let next = index + 1;
    while (next < items.length) {
      const candidate = items[next];
      if (candidate.kind !== 'tool') break;
      if (candidate.turnId !== item.turnId) break;
      if (Boolean(item.reverted) !== Boolean(candidate.reverted)) break;
      group.push(candidate);
      next += 1;
    }
    if (group.length >= 2) {
      entries.push({ kind: 'tool-group', id: group[0].id, items: group });
    } else {
      entries.push({ kind: 'item', item });
    }
    index = next;
  }
  return entries;
}

/**
 * 渲染形态 B（WorkBuddy 模式，ADR 0019）：工具/思考/diff 按真实时序就地交错，
 * 不再抽进过程胶囊重排。直接返回 groupThreadItems 结果（thinking/tool/tool-group/
 * subagent/assistant 均为就地条目）；轮次分组交由 Thread.groupIntoTurnBlocks 按
 * user/turnId 完成。
 */
export function layoutThreadItems(items: ThreadItem[]): ThreadLayoutEntry[] {
  return groupThreadItems(items);
}
