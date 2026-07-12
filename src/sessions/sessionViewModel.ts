import type { EngineId } from '@helm/protocol';
import type { SessionStatus, SessionSummary } from './sessionTypes';

export type SessionEngineFilter = EngineId | 'all';
export type SessionStatusFilter = SessionStatus | 'all';
export type SessionSortKey = 'recent' | 'messages' | 'tokens' | 'cost';
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

const STATUS_LABELS: Record<SessionStatus, string> = {
  active: '活跃',
  idle: '空闲',
  done: '已完成',
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
    const matchesStatus = filters.status === 'all' || session.status === filters.status;
    return matchesQuery && matchesEngine && matchesStatus;
  });
}

export function sortSessions(
  sessions: SessionSummary[],
  key: SessionSortKey,
  direction: SortDirection,
): SessionSummary[] {
  const factor = direction === 'asc' ? 1 : -1;
  return [...sessions].sort((a, b) => (sortValue(a, key) - sortValue(b, key)) * factor);
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

export function statusLabel(status: SessionStatus): string {
  return STATUS_LABELS[status];
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
  const diffSeconds = Math.max(0, Math.floor(nowMs / 1000) - tsSeconds);
  if (diffSeconds < 60) return '刚刚';
  if (diffSeconds < 3600) return `${Math.floor(diffSeconds / 60)} 分钟前`;
  if (diffSeconds < 86400) return `${Math.floor(diffSeconds / 3600)} 小时前`;
  if (diffSeconds < 172800) return '昨天';
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

/** 把（已排序的）会话列表切成分组保持原顺序 */
export function groupSessionsByTime(
  sessions: SessionSummary[],
  nowMs = Date.now(),
): Array<{ label: SessionTimeGroup; sessions: SessionSummary[] }> {
  const groups: Array<{ label: SessionTimeGroup; sessions: SessionSummary[] }> = [];
  for (const session of sessions) {
    const label = sessionTimeGroup(session, nowMs);
    const last = groups[groups.length - 1];
    if (last && last.label === label) {
      last.sessions.push(session);
    } else {
      groups.push({ label, sessions: [session] });
    }
  }
  return groups;
}

function sortValue(session: SessionSummary, key: SessionSortKey): number {
  if (key === 'messages') return session.messageCount;
  if (key === 'tokens') return session.inputTokens + session.outputTokens;
  if (key === 'cost') return session.costUsd;
  return session.updatedAt;
}

function trimFixed(value: number): string {
  return value.toFixed(1).replace(/\.0$/, '');
}
