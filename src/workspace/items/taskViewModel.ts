import type { ThreadItem } from '../../engine/useSession';
import { isTerminalToolName } from '../threadGroups';

// 变更-34 · C1/C2/E1：并行子代理与后台命令的可视化数据模型。
// 线程是线性的，Agent 的工作是并行的 —— 这两类工具是"并行工作"的唯一真实来源。

export type AgentStatus = 'pending' | 'success' | 'error';
export type AgentState = 'run' | 'ok' | 'err';

export interface SubagentEntry {
  id: string;
  name: string;
  task: string;
  dur: string;
  state: AgentState;
  status: AgentStatus;
  output?: string;
  turnId?: string;
}

export interface BackgroundEntry {
  id: string;
  command: string;
  dur: string;
  state: AgentState;
  status: AgentStatus;
  turnId?: string;
}

/** Claude Code 的 Task、Codex 的 Agent/subagent 都视为子代理工具。 */
const SUBAGENT_NAME_RE = /^(task|agent|subagent)([- _/]|$)/i;

export function isSubagentToolName(name: string): boolean {
  return SUBAGENT_NAME_RE.test(name);
}

export function isSubagentTool(item: ThreadItem): item is Extract<ThreadItem, { kind: 'tool' }> {
  return item.kind === 'tool' && isSubagentToolName(item.name);
}

/** 从工具输入提取子代理任务描述（不同 CLI 字段名不同，按权威顺序取）。 */
export function subagentTaskLabel(input: unknown): string {
  const raw = (input ?? {}) as Record<string, unknown>;
  for (const key of ['description', 'prompt', 'instructions', 'command']) {
    const value = raw[key];
    if (typeof value === 'string' && value.trim()) return value.trim();
  }
  return '';
}

/** 子代理显示名：优先 CLI 提供的 name，否则退回工具名。 */
export function subagentDisplayName(input: unknown, fallback: string): string {
  const raw = (input ?? {}) as Record<string, unknown>;
  for (const key of ['name', 'tool_name', 'id']) {
    const value = raw[key];
    if (typeof value === 'string' && value.trim()) return value.trim();
  }
  return fallback;
}

export function agentState(status: AgentStatus): AgentState {
  if (status === 'pending') return 'run';
  if (status === 'error') return 'err';
  return 'ok';
}

export function agentStateLabel(state: AgentState): string {
  if (state === 'run') return '运行中';
  if (state === 'err') return '失败';
  return '完成';
}

/** 耗时文案：与原型一致（1.1s / 42s / 2m 10s）。 */
export function formatDuration(ms: number): string {
  if (ms >= 3_600_000) return `${Math.max(0, Math.round(ms / 360_000) / 10)}h`;
  if (ms >= 10_000) {
    const m = Math.floor(ms / 60_000);
    const s = Math.round((ms % 60_000) / 1000);
    return m ? (s ? `${m}m ${s}s` : `${m}m`) : `${s}s`;
  }
  return `${Math.max(0, Math.round(ms / 100) / 10)}s`;
}

export function durationLabel(
  startedAt: number | undefined,
  endedAt: number | undefined,
  now: number,
): string {
  if (!startedAt) return '';
  return formatDuration(Math.max(0, (endedAt ?? now) - startedAt));
}

export function collectSubagents(items: ThreadItem[], now = Date.now()): SubagentEntry[] {
  return items.filter(isSubagentTool).map((item) => ({
    id: item.id,
    name: subagentDisplayName(item.input, item.name),
    task: subagentTaskLabel(item.input),
    dur: durationLabel(item.startedAt, item.endedAt, now),
    state: agentState(item.status),
    status: item.status,
    ...(item.output ? { output: item.output } : {}),
    ...(item.turnId ? { turnId: item.turnId } : {}),
  }));
}

/**
 * 后台命令判定：终端工具且满足真实后台信号（is_background）或长时间运行（timeout ≥ 10 分钟）。
 * 只能基于 CLI 如实上报的字段，不能靠猜。
 */
export function isBackgroundCommand(item: Extract<ThreadItem, { kind: 'tool' }>): boolean {
  if (!isTerminalToolName(item.name)) return false;
  const input = (item.input ?? {}) as Record<string, unknown>;
  if (input.is_background === true || input.background === true) return true;
  return typeof input.timeout === 'number' && input.timeout >= 600_000;
}

export function collectBackgroundCommands(
  items: ThreadItem[],
  now = Date.now(),
): BackgroundEntry[] {
  return items
    .filter((item): item is Extract<ThreadItem, { kind: 'tool' }> => {
      if (item.kind !== 'tool') return false;
      if (item.status === 'error' && !item.started) return false;
      return isBackgroundCommand(item);
    })
    .map((item) => {
      const input = (item.input ?? {}) as Record<string, unknown>;
      const command =
        typeof input.command === 'string'
          ? input.command
          : typeof input.script === 'string'
            ? input.script
            : JSON.stringify(item.input ?? {});
      return {
        id: item.id,
        command,
        dur: durationLabel(item.startedAt, item.endedAt, now),
        state: agentState(item.status),
        status: item.status,
        ...(item.turnId ? { turnId: item.turnId } : {}),
      };
    });
}
