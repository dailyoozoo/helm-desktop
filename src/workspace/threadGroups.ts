import type { ThreadItem } from '../engine/useSession';

export type ThreadRenderEntry =
  | { kind: 'item'; item: ThreadItem }
  | { kind: 'tool-group'; id: string; items: Extract<ThreadItem, { kind: 'tool' }>[] };

export type TurnProcessEntry = {
  kind: 'turn-process';
  id: string;
  turnId: string;
  entries: ThreadRenderEntry[];
  completed: boolean;
  terminalStatus?: 'succeeded' | 'failed' | 'interrupted';
};

export type ThreadLayoutEntry = ThreadRenderEntry | TurnProcessEntry;

const TERMINAL_TOOL_NAMES = new Set([
  'bash',
  'shell',
  'terminal',
  'command',
  'executecommand',
  'runcommand',
  'run_terminal_cmd',
]);

const DELIVERY_TOOL_NAMES = new Set(['write', 'edit', 'multiedit', 'notebookedit']);

export function isTerminalToolName(name: string): boolean {
  return TERMINAL_TOOL_NAMES.has(name.toLowerCase());
}

/** Diff 和终端是需要始终直接可见的交付物，不能被工具组默认折叠。 */
export function isStandaloneTool(item: Extract<ThreadItem, { kind: 'tool' }>): boolean {
  const name = item.name.toLowerCase();
  return Boolean(item.diff) || isTerminalToolName(name) || DELIVERY_TOOL_NAMES.has(name);
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
    if (isStandaloneTool(item)) {
      entries.push({ kind: 'item', item });
      index += 1;
      continue;
    }
    const group = [item];
    let next = index + 1;
    while (next < items.length) {
      const candidate = items[next];
      if (candidate.kind !== 'tool') break;
      if (isStandaloneTool(candidate)) break;
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
 * 在 ToolGroup 之上派生纯视图层 Turn 过程容器。最终答复和所有交付物仍留在容器外，
 * 容器只收纳同一稳定 Turn 中连续的 thinking、过程正文、普通单工具和 ToolGroup。
 */
export function layoutThreadItems(items: ThreadItem[]): ThreadLayoutEntry[] {
  const grouped = groupThreadItems(items);
  const finalAssistants = new Map<string, Extract<ThreadItem, { kind: 'assistant' }>>();

  for (const item of items) {
    if (item.kind !== 'assistant' || !item.turnId) continue;
    finalAssistants.set(item.turnId, item);
  }

  const isProcessEntry = (entry: ThreadRenderEntry): string | null => {
    if (entry.kind === 'tool-group') return entry.items[0]?.turnId ?? null;
    const item = entry.item;
    if (!item.turnId) return null;
    if (item.kind === 'thinking') return item.turnId;
    if (item.kind === 'tool' && !isStandaloneTool(item)) return item.turnId;
    if (item.kind === 'assistant' && finalAssistants.get(item.turnId)?.id !== item.id) {
      return item.turnId;
    }
    return null;
  };

  const processEntries = new Map<string, ThreadRenderEntry[]>();
  for (const entry of grouped) {
    const turnId = isProcessEntry(entry);
    if (!turnId) continue;
    const entries = processEntries.get(turnId) ?? [];
    entries.push(entry);
    processEntries.set(turnId, entries);
  }

  const result: ThreadLayoutEntry[] = [];
  const emitted = new Set<string>();
  for (const entry of grouped) {
    const turnId = isProcessEntry(entry);
    if (!turnId) {
      result.push(entry);
      continue;
    }
    if (emitted.has(turnId)) continue;
    emitted.add(turnId);
    const entries = processEntries.get(turnId) ?? [entry];
    const firstId = entry.kind === 'tool-group' ? entry.id : entry.item.id;
    const finalAssistant = finalAssistants.get(turnId);
    const terminalStatus =
      entries
        .flatMap((entry) => (entry.kind === 'tool-group' ? entry.items : [entry.item]))
        .find((item) => item.turnStatus)?.turnStatus ?? finalAssistant?.turnStatus;
    result.push({
      kind: 'turn-process',
      id: `turn-process-${firstId}`,
      turnId,
      entries,
      completed:
        terminalStatus === 'succeeded' ||
        (terminalStatus == null && Boolean(finalAssistant && !finalAssistant.interrupted)),
      ...(terminalStatus ? { terminalStatus } : {}),
    });
  }
  return result;
}
