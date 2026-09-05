import type { EngineId } from '@helm/protocol';
import type { SessionSummary } from '../sessions/sessionTypes';
import {
  filterSessions,
  type SessionFilters,
  type SessionStatusFilter,
} from '../sessions/sessionViewModel';

/** 设置页「全部任务」筛选（S8）：在 sessionViewModel 真实筛选上增加目录维度。 */
export interface TaskFilters {
  query: string;
  /** canonical cwd；空串 = 全部目录 */
  directory: string;
  engine: EngineId | 'all';
  status: SessionStatusFilter;
}

export const DEFAULT_TASK_FILTERS: TaskFilters = {
  query: '',
  directory: '',
  engine: 'all',
  status: 'all',
};

export interface TaskDirectoryOption {
  cwd: string;
  label: string;
  count: number;
}

function cwdLabel(cwd: string): string {
  const segments = cwd.replace(/[\\/]+$/, '').split(/[\\/]/);
  return segments[segments.length - 1] || cwd;
}

/** 目录下拉选项来自真实 Session 的 canonical cwd，按名称排序并带计数。 */
export function listTaskDirectories(sessions: SessionSummary[]): TaskDirectoryOption[] {
  const counts = new Map<string, number>();
  for (const session of sessions) {
    counts.set(session.cwd, (counts.get(session.cwd) ?? 0) + 1);
  }
  return [...counts.entries()]
    .map(([cwd, count]) => ({ cwd, label: cwdLabel(cwd), count }))
    .sort((a, b) => a.label.localeCompare(b.label, 'zh-CN'));
}

/** 组合筛选：查询/引擎/状态复用全局 sessionViewModel（同一套派生状态语义）。 */
export function filterTasks(sessions: SessionSummary[], filters: TaskFilters): SessionSummary[] {
  const base: SessionFilters = {
    query: filters.query,
    engine: filters.engine,
    status: filters.status,
  };
  const matched = filterSessions(sessions, base);
  if (!filters.directory) return matched;
  return matched.filter((session) => session.cwd === filters.directory);
}

/** 当前激活的非默认筛选项，供「筛选 token」与空态清除按钮展示。 */
export function activeTaskFilterTokens(
  filters: TaskFilters,
  directories: TaskDirectoryOption[],
): { key: keyof TaskFilters; label: string }[] {
  const tokens: { key: keyof TaskFilters; label: string }[] = [];
  if (filters.query.trim()) {
    tokens.push({ key: 'query', label: `搜索：${filters.query.trim()}` });
  }
  if (filters.directory) {
    const option = directories.find((entry) => entry.cwd === filters.directory);
    tokens.push({ key: 'directory', label: `目录：${option?.label ?? filters.directory}` });
  }
  if (filters.engine !== 'all') {
    tokens.push({
      key: 'engine',
      label: `引擎：${filters.engine === 'codex' ? 'Codex' : 'Claude Code'}`,
    });
  }
  if (filters.status !== 'all') {
    const labels: Record<string, string> = {
      waiting_approval: '等审批',
      running: '运行中',
      failed: '失败',
      archived: '已归档',
      active: '活跃',
      idle: '空闲',
      done: '已完成',
    };
    tokens.push({ key: 'status', label: `状态：${labels[filters.status] ?? filters.status}` });
  }
  return tokens;
}
