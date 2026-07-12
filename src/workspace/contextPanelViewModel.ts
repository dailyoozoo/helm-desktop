import type { ThreadItem } from '../engine/useSession';

export interface ChangedFileSummary {
  path: string;
  added: number;
  removed: number;
  /** 对同一文件的编辑次数：行数是多次编辑的累计值，UI 需标注「累计」（变更-11） */
  edits: number;
}

export interface TerminalOutputSummary {
  id: string;
  command: string;
  status: 'pending' | 'success' | 'error';
  output?: string;
}

export interface ToolSummary {
  id: string;
  name: string;
  status: 'pending' | 'success' | 'error';
}

export interface ContextWindowSummary {
  usedTokens: number;
  maxTokens?: number;
  usedRatio: number;
  mountedPathCount: number;
  fileTokenDetailAvailable: boolean;
}

export interface ContextPanelCost {
  inputTokens: number;
  outputTokens: number;
  contextWindow?: number;
}

export interface ContextPanelData {
  changedFiles: ChangedFileSummary[];
  mountedPaths: string[];
  contextWindow: ContextWindowSummary;
  terminalOutputs: TerminalOutputSummary[];
  tools: ToolSummary[];
}

function commandFromInput(input: unknown): string | null {
  if (!input || typeof input !== 'object' || !('command' in input)) return null;
  const command = (input as { command?: unknown }).command;
  return typeof command === 'string' && command.trim() ? command : null;
}

export function contextPanelData(items: ThreadItem[], cost?: ContextPanelCost): ContextPanelData {
  const changedFiles = new Map<string, ChangedFileSummary>();
  const mountedPaths = new Set<string>();
  const terminalOutputs: TerminalOutputSummary[] = [];
  const tools: ToolSummary[] = [];

  for (const item of items) {
    // 回溯过滤（变更-11）：被回滚的轮次不再计入右栏（文件已还原、上下文已截断），
    // 与线程的 reverted 淡化语义一致
    if ('reverted' in item && item.reverted) continue;

    if (item.kind === 'user') {
      for (const path of item.attachments ?? []) {
        const trimmed = path.trim();
        if (trimmed) mountedPaths.add(trimmed);
      }
      continue;
    }

    if (item.kind !== 'tool') continue;

    tools.push({ id: item.id, name: item.name, status: item.status });

    if (item.diff) {
      const current = changedFiles.get(item.diff.path) ?? {
        path: item.diff.path,
        added: 0,
        removed: 0,
        edits: 0,
      };
      current.edits += 1;
      for (const hunk of item.diff.hunks) {
        for (const line of hunk.lines) {
          if (line.kind === 'add') current.added += 1;
          if (line.kind === 'del') current.removed += 1;
        }
      }
      changedFiles.set(item.diff.path, current);
    }

    if (item.name === 'Bash') {
      const command = commandFromInput(item.input);
      if (command) {
        terminalOutputs.push({
          id: item.id,
          command,
          status: item.status,
          output: item.output,
        });
      }
    }
  }

  const usedTokens = Math.max(0, (cost?.inputTokens ?? 0) + (cost?.outputTokens ?? 0));
  const maxTokens =
    typeof cost?.contextWindow === 'number' && cost.contextWindow > 0
      ? cost.contextWindow
      : undefined;
  const usedRatio = maxTokens ? Math.min(1, usedTokens / maxTokens) : 0;

  return {
    changedFiles: Array.from(changedFiles.values()),
    mountedPaths: Array.from(mountedPaths.values()),
    contextWindow: {
      usedTokens,
      ...(maxTokens ? { maxTokens } : {}),
      usedRatio,
      mountedPathCount: mountedPaths.size,
      fileTokenDetailAvailable: false,
    },
    terminalOutputs,
    tools,
  };
}
