import type { ThreadItem } from '../engine/useSession';
import type { GitStatus, StagedFile } from '../engine/transport';

export interface ChangedFileSummary {
  path: string;
  added: number;
  removed: number;
  /** 对同一文件的编辑次数：行数是多次编辑的累计值，UI 需标注「累计」（变更-11） */
  edits: number;
}

/** 变更行条目（批次 I：变更导航器） */
export interface ChangeLineEntry {
  /** 文件路径 */
  path: string;
  /** 行号（新文件行号） */
  lineNumber: number;
  /** 变更类型：add=新增，del=删除，ctx=上下文 */
  kind: 'add' | 'del' | 'ctx';
  /** 行内容（截断显示） */
  text: string;
  /** 关联的工具 ID（用于跳转） */
  toolId: string;
}

/** 变更文件条目（批次 I：变更导航器） */
export interface ChangeFileEntry {
  /** 文件路径 */
  path: string;
  /** 变更行列表 */
  lines: ChangeLineEntry[];
  /** 新增行数 */
  added: number;
  /** 删除行数 */
  removed: number;
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

export interface ContextUsageSummary {
  tokens?: number;
  maxTokens?: number;
  ratio?: number;
  level: 'none' | 'normal' | 'warning' | 'danger';
}

export interface BillingTokenSummary {
  freshInput: number;
  cacheWrite: number;
  cacheRead: number;
  output: number;
  total: number;
  cacheReadShare?: number;
}

export interface ContextPanelCost {
  inputTokens: number;
  cachedInputTokens?: number;
  cacheWriteInputTokens?: number;
  outputTokens: number;
  contextTokens?: number;
  contextWindow?: number;
}

export interface ContextPanelData {
  changedFiles: ChangedFileSummary[];
  messageCount: number;
  historicalAttachments: string[];
  /** @deprecated 历史兼容字段；等同 historicalAttachments。 */
  mountedPaths: string[];
  contextWindow: ContextWindowSummary;
  tools: ToolSummary[];
  contextUsage: ContextUsageSummary;
  billing: BillingTokenSummary;
  /** Git 工作区状态（批次 E） */
  gitStatus?: GitStatus;
  /** 暂存区文件列表（批次 E） */
  stagedFiles?: StagedFile[];
  /** 变更文件条目（批次 I：变更导航器） */
  changeFiles: ChangeFileEntry[];
}

export function contextPanelData(
  items: ThreadItem[],
  cost?: ContextPanelCost,
  gitStatus?: GitStatus,
  stagedFiles?: StagedFile[],
): ContextPanelData {
  const changedFiles = new Map<string, ChangedFileSummary>();
  const mountedPaths = new Set<string>();
  let messageCount = 0;
  const tools: ToolSummary[] = [];
  // 批次 I：变更导航器数据
  const changeFilesMap = new Map<string, ChangeFileEntry>();

  for (const item of items) {
    // 回溯过滤（变更-11）：被回滚的轮次不再计入右栏（文件已还原、上下文已截断），
    // 与线程的 reverted 淡化语义一致
    if ('reverted' in item && item.reverted) continue;

    if (item.kind === 'user' || item.kind === 'assistant') messageCount += 1;

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

      // 批次 I：生成变更行条目
      const changeFile = changeFilesMap.get(item.diff.path) ?? {
        path: item.diff.path,
        lines: [],
        added: 0,
        removed: 0,
      };

      for (const hunk of item.diff.hunks) {
        let lineOffset = hunk.newStart;
        for (const line of hunk.lines) {
          if (line.kind === 'add') {
            current.added += 1;
            changeFile.added += 1;
            changeFile.lines.push({
              path: item.diff.path,
              lineNumber: lineOffset,
              kind: 'add',
              text: line.text.slice(0, 80), // 截断显示
              toolId: item.id,
            });
            lineOffset++;
          } else if (line.kind === 'del') {
            current.removed += 1;
            changeFile.removed += 1;
            changeFile.lines.push({
              path: item.diff.path,
              lineNumber: lineOffset,
              kind: 'del',
              text: line.text.slice(0, 80),
              toolId: item.id,
            });
          } else {
            // ctx 行
            changeFile.lines.push({
              path: item.diff.path,
              lineNumber: lineOffset,
              kind: 'ctx',
              text: line.text.slice(0, 80),
              toolId: item.id,
            });
            lineOffset++;
          }
        }
      }

      changedFiles.set(item.diff.path, current);
      changeFilesMap.set(item.diff.path, changeFile);
    }
  }

  const freshInput = Math.max(
    0,
    (cost?.inputTokens ?? 0) - (cost?.cachedInputTokens ?? 0) - (cost?.cacheWriteInputTokens ?? 0),
  );
  const cacheRead = Math.max(0, cost?.cachedInputTokens ?? 0);
  const cacheWrite = Math.max(0, cost?.cacheWriteInputTokens ?? 0);
  const output = Math.max(0, cost?.outputTokens ?? 0);
  const billingTotal = freshInput + cacheRead + cacheWrite + output;
  const contextTokens = typeof cost?.contextTokens === 'number' ? cost.contextTokens : undefined;
  const maxTokens =
    typeof cost?.contextWindow === 'number' && cost.contextWindow > 0
      ? cost.contextWindow
      : undefined;
  const usedRatio = maxTokens && contextTokens != null ? Math.min(1, contextTokens / maxTokens) : 0;
  const usageLevel =
    contextTokens == null || !maxTokens
      ? 'none'
      : usedRatio >= 0.95
        ? 'danger'
        : usedRatio >= 0.8
          ? 'warning'
          : 'normal';

  return {
    changedFiles: Array.from(changedFiles.values()),
    messageCount,
    historicalAttachments: Array.from(mountedPaths.values()),
    mountedPaths: Array.from(mountedPaths.values()),
    contextWindow: {
      usedTokens: contextTokens ?? 0,
      ...(maxTokens ? { maxTokens } : {}),
      usedRatio,
      mountedPathCount: mountedPaths.size,
      fileTokenDetailAvailable: false,
    },
    tools,
    contextUsage: {
      ...(contextTokens != null ? { tokens: contextTokens } : {}),
      ...(maxTokens ? { maxTokens } : {}),
      ...(contextTokens != null && maxTokens ? { ratio: usedRatio } : {}),
      level: usageLevel,
    },
    billing: {
      freshInput,
      cacheWrite,
      cacheRead,
      output,
      total: billingTotal,
      ...(billingTotal > 0 ? { cacheReadShare: cacheRead / billingTotal } : {}),
    },
    gitStatus,
    stagedFiles,
    changeFiles: Array.from(changeFilesMap.values()),
  };
}
