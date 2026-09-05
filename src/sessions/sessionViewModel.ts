import type { EngineId } from '@helm/protocol';
import type { SessionStatus, SessionSummary } from './sessionTypes';

export type SessionEngineFilter = EngineId | 'all';
export type SessionStatusFilter =
  | SessionStatus
  | 'all'
  | 'waiting_approval'
  | 'running'
  | 'failed'
  | 'archived';
export type SessionSortKey = 'recent' | 'messages' | 'tokens' | 'cost' | 'change';
export type SortDirection = 'asc' | 'desc';

export interface SessionFilters {
  query: string;
  engine: SessionEngineFilter;
  status: SessionStatusFilter;
}

export interface SessionStats {
  totalSessions: number;
  activeEngines: number;
  totalTokens: number;
  totalCostUsd: number;
}

export type RecentSortMode = 'time' | 'folder';

export interface RecentTaskGroup {
  cwd: string;
  label: string;
  sessions: SessionSummary[];
}

const STATUS_LABELS: Record<SessionStatus, string> = {
  active: '活跃',
  idle: '空闲',
  done: '已完成',
  waiting_approval: '等待审批',
};

const DERIVED_STATUS_LABELS: Record<string, string> = {
  waiting_approval: '等审批',
  running: '运行中',
  failed: '失败',
  archived: '已归档',
};

/** 派生状态键（切片7 · F1）：等审批/运行中/失败/已归档由真实字段推导，否则回落 status。 */
export type DerivedStatusKey =
  | 'waiting_approval'
  | 'running'
  | 'failed'
  | 'archived'
  | SessionStatus;

/** 返回会话的派生状态键，供药丸 class 和标签共用。 */
export function derivedStatusKey(session: SessionSummary): DerivedStatusKey {
  if (session.archived) return 'archived';
  if (session.pendingApproval || session.status === 'waiting_approval') return 'waiting_approval';
  if (session.lastTurnFailed) return 'failed';
  if (session.status === 'active' && session.currentTool) return 'running';
  return session.status;
}

/** 返回会话的派生状态中文标签。 */
export function derivedStatusLabelForSession(session: SessionSummary): string {
  const key = derivedStatusKey(session);
  return DERIVED_STATUS_LABELS[key] ?? STATUS_LABELS[key as SessionStatus] ?? key;
}

const TOOL_VERBS: Record<string, string> = {
  Write: '写文件',
  Edit: '编辑文件',
  Bash: '执行命令',
  Read: '读取文件',
  Glob: '搜索文件',
  Grep: '搜索内容',
  LS: '列出目录',
  ClaudeNotebook: '运行笔记本',
};

export function filterSessions(
  sessions: SessionSummary[],
  filters: SessionFilters,
): SessionSummary[] {
  const query = filters.query.trim().toLowerCase();
  return sessions.filter((session) => {
    const matchesQuery =
      !query ||
      session.title.toLowerCase().includes(query) ||
      session.cwd.toLowerCase().includes(query) ||
      session.model.toLowerCase().includes(query) ||
      // 摘要搜索（P3-5）：fast model 生成的摘要参与匹配
      (session.summary ?? '').toLowerCase().includes(query);
    const matchesEngine = filters.engine === 'all' || session.engine === filters.engine;
    const matchesStatus = statusMatches(session, filters.status);
    return matchesQuery && matchesEngine && matchesStatus;
  });
}

/** 派生状态匹配（切片7 · F1）：等审批/运行中/失败/已归档由真实字段推导，而非伪造状态。 */
function statusMatches(session: SessionSummary, status: SessionStatusFilter): boolean {
  if (status === 'all') return true;
  if (status === 'archived') return !!session.archived;
  if (status === 'failed') return !!session.lastTurnFailed;
  if (status === 'running') return session.status === 'active' && !!session.currentTool;
  if (status === 'waiting_approval')
    return !!session.pendingApproval || session.status === 'waiting_approval';
  return session.status === status;
}

export function sortSessions(
  sessions: SessionSummary[],
  key: SessionSortKey,
  direction: SortDirection,
): SessionSummary[] {
  const factor = direction === 'asc' ? 1 : -1;
  return [...sessions].sort((a, b) => {
    // 置顶会话始终排最前（不受排序方向影响）
    const aPinned = a.pinned ? 1 : 0;
    const bPinned = b.pinned ? 1 : 0;
    if (aPinned !== bPinned) return bPinned - aPinned;
    return (sortValue(a, key) - sortValue(b, key)) * factor;
  });
}

export function sessionStats(sessions: SessionSummary[]): SessionStats {
  return {
    totalSessions: sessions.length,
    activeEngines: new Set(sessions.map((session) => session.engine)).size,
    totalTokens: sessions.reduce(
      (sum, session) => sum + session.inputTokens + session.outputTokens,
      0,
    ),
    totalCostUsd: Number(sessions.reduce((sum, session) => sum + session.costUsd, 0).toFixed(2)),
  };
}

function prepareRecentTasks(sessions: SessionSummary[], sort: RecentSortMode): SessionSummary[] {
  const activeSessions = sessions.filter((session) => !session.archived);
  const sortedSessions = [...activeSessions].sort((a, b) => {
    const pinnedDelta = Number(b.pinned ?? false) - Number(a.pinned ?? false);
    if (pinnedDelta !== 0) return pinnedDelta;
    if (sort === 'folder') {
      const cwdDelta = a.cwd.localeCompare(b.cwd, 'zh-CN');
      if (cwdDelta !== 0) return cwdDelta;
    }
    return b.updatedAt - a.updatedAt;
  });
  return sortedSessions;
}

/** 最近任务视图模型（S0）：归档退出最近任务；cwd 由后端 canonicalize 后作为排序/分组真值。 */
export function listRecentTasks(
  sessions: SessionSummary[],
  sort: RecentSortMode,
): SessionSummary[] {
  return prepareRecentTasks(sessions, sort);
}

/** 最近任务视图模型（S0）：同名目录按完整 canonical cwd 分开。 */
export function groupRecentTasksByCwd(
  sessions: SessionSummary[],
  sort: RecentSortMode,
): RecentTaskGroup[] {
  const sortedSessions = prepareRecentTasks(sessions, sort);
  const groupsByCwd = new Map<string, RecentTaskGroup>();
  for (const session of sortedSessions) {
    let group = groupsByCwd.get(session.cwd);
    if (!group) {
      group = { cwd: session.cwd, label: folderLabel(session.cwd), sessions: [] };
      groupsByCwd.set(session.cwd, group);
    }
    group.sessions.push(session);
  }
  return [...groupsByCwd.values()];
}

/** S1 导出：最近任务目录组展示名（末级目录名）复用同一实现。 */
export function folderLabel(path: string): string {
  const segments = path.replace(/[\\/]+$/, '').split(/[\\/]/);
  return segments[segments.length - 1] || path;
}

export function statusLabel(status: SessionStatus): string {
  return STATUS_LABELS[status];
}

/** 派生状态中文（切片7 · F1）：等审批/运行中/失败/已归档。 */
export function derivedStatusLabel(status: string): string {
  return DERIVED_STATUS_LABELS[status] ?? status;
}

/** 会话卡「当前在做什么」（切片7 · F2）：如「正在写文件 auth.ts」；无进行中工具返回 null。 */
export function currentActionText(session: SessionSummary): string | null {
  if (!session.currentTool) return null;
  const verb = TOOL_VERBS[session.currentTool] ?? session.currentTool;
  if (session.currentTarget) return `${verb} ${session.currentTarget}`;
  return verb;
}

/** 变更规模文本（切片7 · F2）：`+N -M`；无 diff 数据返回 null（显示暂无）。 */
export function changeScaleText(session: SessionSummary): string | null {
  const additions = session.changeAdditions ?? 0;
  const deletions = session.changeDeletions ?? 0;
  if (additions === 0 && deletions === 0) return null;
  return `+${additions} -${deletions}`;
}

export function engineLabel(engine: EngineId): string {
  return engine === 'codex' ? 'Codex' : 'Claude Code';
}

export function tokenText(tokens: number): string {
  if (tokens >= 1_000_000) return `${trimFixed(tokens / 1_000_000)}M`;
  if (tokens >= 1000) return `${trimFixed(tokens / 1000)}K`;
  return `${tokens}`;
}

export function costText(costUsd: number): string {
  return `$${costUsd.toFixed(2)}`;
}

export function relativeTimeText(tsSeconds: number, nowMs = Date.now()): string {
  const now = new Date(nowMs);
  const startOfToday = new Date(now.getFullYear(), now.getMonth(), now.getDate()).getTime() / 1000;
  const diffSeconds = Math.max(0, Math.floor(nowMs / 1000) - tsSeconds);
  if (diffSeconds < 60) return '刚刚';
  if (diffSeconds < 3600) return `${Math.floor(diffSeconds / 60)} 分钟前`;
  if (tsSeconds >= startOfToday) return `${Math.floor(diffSeconds / 3600)} 小时前`;
  if (tsSeconds >= startOfToday - 86_400) return '昨天';
  return new Date(tsSeconds * 1000).toLocaleDateString('zh-CN');
}

export type SessionTimeGroup = '置顶' | '今天' | '昨天' | '本周' | '更早';

/** 侧栏时间分组（变更-12，对齐原型 sgroup）：置顶最前，其余按 updatedAt 归组 */
export function sessionTimeGroup(session: SessionSummary, nowMs = Date.now()): SessionTimeGroup {
  if (session.pinned) return '置顶';
  const now = new Date(nowMs);
  const startOfToday = new Date(now.getFullYear(), now.getMonth(), now.getDate()).getTime() / 1000;
  const updated = session.updatedAt;
  if (updated >= startOfToday) return '今天';
  if (updated >= startOfToday - 86_400) return '昨天';
  if (updated >= startOfToday - 6 * 86_400) return '本周';
  return '更早';
}

function sortValue(session: SessionSummary, key: SessionSortKey): number {
  if (key === 'messages') return session.messageCount;
  if (key === 'tokens') return session.inputTokens + session.outputTokens;
  if (key === 'cost') return session.costUsd;
  if (key === 'change') return (session.changeAdditions ?? 0) + (session.changeDeletions ?? 0);
  return session.updatedAt;
}

function trimFixed(value: number): string {
  return value.toFixed(1).replace(/\.0$/, '');
}
