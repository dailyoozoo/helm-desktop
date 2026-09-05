import type { ThreadItem } from '../engine/useSession';
import type { GitStatus, StagedFile } from '../engine/transport';

/** S3（2026-08-22 路线图）：右栏常驻 tab 冻结为「修改记录 / 全部文件」，标签与原型一致。 */
export type ContextPanelFixedTab = 'changes' | 'files';

/** 常驻 tab 渲染顺序。 */
export const CONTEXT_PANEL_FIXED_TABS = ['changes', 'files'] as const;

export const CONTEXT_PANEL_FIXED_TAB_LABELS: Record<ContextPanelFixedTab, string> = {
  changes: '修改记录',
  files: '全部文件',
};

/** 右栏默认激活的常驻 tab（S3 验收：默认 tab 与原型一致）。 */
export const CONTEXT_PANEL_DEFAULT_TAB: ContextPanelFixedTab = 'changes';

export function isContextPanelFixedTab(tab: string): tab is ContextPanelFixedTab {
  return tab === 'changes' || tab === 'files';
}

/**
 * S3：交付物区动态 tab 标识（按需打开、可关闭）。
 * 「上下文」不再是右栏 tab —— 上下文/计费只从 Composer 圆环 popover 进入；
 * 「活动」自常驻降为动态 tab（真实能力保留，经 tabbar 入口按钮打开）。
 */
export type ArtifactPaneTab = 'plan' | 'term' | 'preview' | 'tasks' | 'log' | 'tools';

/** 动态 tab 标签（单一来源；preview 当前无真实 dev server 能力，不提供打开入口）。 */
export const DYN_TAB_LABELS: Record<ArtifactPaneTab, string> = {
  plan: '计划',
  term: '终端',
  preview: '预览',
  tasks: '任务',
  log: '活动',
  tools: '工具',
};

/** 变更-34 · A4：动态 tab 打开/关闭的纯状态机（供单测，UI 层直接投影）。 */
export interface DynTabsState {
  open: ArtifactPaneTab[];
  active: ArtifactPaneTab | null;
}

export function openDynTab(state: DynTabsState, tab: ArtifactPaneTab): DynTabsState {
  return {
    open: state.open.includes(tab) ? state.open : [...state.open, tab],
    active: tab,
  };
}

export function closeDynTab(state: DynTabsState, tab: ArtifactPaneTab): DynTabsState {
  const open = state.open.filter((t) => t !== tab);
  // 关闭当前动态 tab 后 active 置空；UI 层把 null 绑定到默认常驻 tab「修改记录」（S3）。
  return {
    open,
    active: state.active === tab ? null : state.active,
  };
}

export interface ChangedFileSummary {
  path: string;
  added: number;
  removed: number;
  /** 对同一文件的编辑次数：行数是多次编辑的累计值，UI 需标注「累计」（变更-11） */
  edits: number;
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
}

/** 变更-34 · D4/E2：由累计计费 token 派生展示口径（四账 + 缓存读取占比），ContextRing 与右栏共用。 */
export function billingSummary(cost?: ContextPanelCost): BillingTokenSummary {
  const freshInput = Math.max(
    0,
    (cost?.inputTokens ?? 0) - (cost?.cachedInputTokens ?? 0) - (cost?.cacheWriteInputTokens ?? 0),
  );
  const cacheRead = Math.max(0, cost?.cachedInputTokens ?? 0);
  const cacheWrite = Math.max(0, cost?.cacheWriteInputTokens ?? 0);
  const output = Math.max(0, cost?.outputTokens ?? 0);
  const billingTotal = freshInput + cacheRead + cacheWrite + output;
  return {
    freshInput,
    cacheWrite,
    cacheRead,
    output,
    total: billingTotal,
    ...(billingTotal > 0 ? { cacheReadShare: cacheRead / billingTotal } : {}),
  };
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

      for (const hunk of item.diff.hunks) {
        for (const line of hunk.lines) {
          if (line.kind === 'add') current.added += 1;
          else if (line.kind === 'del') current.removed += 1;
        }
      }

      changedFiles.set(item.diff.path, current);
    }
  }

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
  const billing = billingSummary(cost);

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
    billing,
    gitStatus,
    stagedFiles,
  };
}

/** S3「全部文件」列表行：目录淡色 + 文件名 + 真实暂存状态徽标（仅真实 Git 事实）。 */
export interface WorkspaceFileRow {
  /** 相对 cwd 的路径（正斜杠） */
  path: string;
  /** 目录前缀（含结尾斜杠；根目录文件为空串） */
  dir: string;
  /** 文件名 */
  base: string;
  /** 暂存区状态首字母（A/M/D/R…），非暂存文件无徽标 */
  badge?: string;
}

/** 把 search_workspace_files 的真实结果与暂存区状态合成为「全部文件」行。 */
export function workspaceFileRows(files: string[], stagedFiles?: StagedFile[]): WorkspaceFileRow[] {
  const badgeByPath = new Map<string, string>();
  for (const file of stagedFiles ?? []) {
    badgeByPath.set(file.path.replace(/\\/g, '/'), file.status.charAt(0).toUpperCase());
  }
  // 目录条目（尾斜杠，供新任务页文件中心选择目录用）不进「全部文件」：S3 冻结契约是文件列表。
  return files
    .filter((path) => !path.endsWith('/'))
    .map((path) => {
      const normalized = path.replace(/\\/g, '/');
      const cut = normalized.lastIndexOf('/');
      const badge = badgeByPath.get(normalized);
      return {
        path,
        dir: cut >= 0 ? normalized.slice(0, cut + 1) : '',
        base: normalized.slice(cut + 1) || normalized,
        ...(badge != null ? { badge } : {}),
      };
    });
}
