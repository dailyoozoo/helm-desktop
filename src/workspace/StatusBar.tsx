import type { TurnActivity } from '../engine/useSession';
import type { ThreadItem } from '../engine/useSession';

/**
 * 变更-34/35 · B5：执行状态指标派生（原常驻执行状态条 StatusBar 已按原型移除，
 * 运行态只体现在 workstrip 与任务列表；statusBarLabel 仍供 workstrip 显示当前动作）。
 * 红线：无百分比、无进度条、无伪造定时器语义。
 */

export interface StatusBarData {
  working: boolean;
  activity: TurnActivity | null;
  turnStartedAt: number | null;
  items: ThreadItem[];
  /** 本轮真实累计成本（由 useSession 按 activeTurnId 过滤 token_usage 累加）。
   * 缺省/0 且无本轮 Usage 事实时不显示成本，不用会话累计值代替。 */
  turnCostUsd?: number;
  /** 当前运行中 Turn 的稳定身份；用于只投影本轮工具/diff，不混入上一轮。 */
  activeTurnId?: string | null;
}

export interface StatusBarModel {
  tools: number;
  files: number;
  additions: number;
  deletions: number;
}

/** 从本轮真实 item 派生状态条指标：工具调用数、改动文件数与 ±行数（无 diff 记为 0）。
 * 传入 activeTurnId 时只统计该 Turn 的工具，不混入历史轮次。 */
export function statusBarModel(items: ThreadItem[], activeTurnId?: string | null): StatusBarModel {
  let tools = 0;
  const files = new Set<string>();
  let additions = 0;
  let deletions = 0;
  const filterByTurn = activeTurnId != null;
  for (const item of items) {
    if (item.kind !== 'tool') continue;
    if (filterByTurn && item.turnId !== activeTurnId) continue;
    tools += 1;
    if (item.diff && item.diff.hunks.length > 0) {
      files.add(item.diff.path);
      for (const hunk of item.diff.hunks) {
        for (const line of hunk.lines) {
          if (line.kind === 'add') additions += 1;
          else if (line.kind === 'del') deletions += 1;
        }
      }
    }
  }
  return { tools, files: files.size, additions, deletions };
}

/** 当前动作的中文文案：以真实 TurnStage 为准，工具名可省、不得伪造。 */
export function statusBarLabel(activity: TurnActivity | null): string {
  if (!activity) return '正在准备…';
  const stage = activity.stage;
  const tool = activity.toolName || undefined;
  const target = activity.target || undefined;
  if (stage === 'reasoning') return '正在思考…';
  if (stage === 'using_tool' && tool) {
    return target ? `${tool} · ${target}` : `正在执行 ${tool}`;
  }
  if (stage === 'using_tool') return '正在执行工具…';
  if (stage === 'waiting_approval') return '等待审批…';
  if (stage === 'responding') return '正在回复…';
  if (stage === 'finalizing') return '正在收尾…';
  if (stage === 'stalled') return '执行受阻，等待处理…';
  if (stage === 'retrying') return '正在重试…';
  return '正在执行…';
}
