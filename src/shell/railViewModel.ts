import type { SessionFolder, SessionSummary } from '../sessions/sessionTypes';
import {
  folderLabel,
  groupRecentTasksByCwd,
  listRecentTasks,
  relativeTimeText,
} from '../sessions/sessionViewModel';

/** 主侧栏「选择工作目录」弹框的候选项：真实会话 cwd 去重 + 自动 Folder 命名优先。 */
export interface DirectoryOption {
  /** canonical 工作目录（去重键与保存值）。 */
  cwd: string;
  /** 展示名：自动 Folder 命名优先，否则末级目录名。 */
  label: string;
}

/** 主侧栏「最近任务」分组方式（2026-08-23 五次反馈用户规格）：按目录（默认）/ 按列表平铺。 */
export type RailGrouping = 'folder' | 'list';
/** 排序方式：最近更新（默认，时间倒排）/ 手动排序（用户拖拽前后调整，顺序持久化）。置顶始终最前。 */
export type RailSort = 'recent' | 'manual';

export interface RailRecentFilters {
  query: string;
  grouping: RailGrouping;
  sort: RailSort;
  /** 手动排序的既知全序（sessionId 有序数组）；缺失者按最近活跃追加在已排序者之后。 */
  manualOrder?: string[];
}

/** 单行任务视图模型：相对时间文案在此统一派生，Rail 只负责渲染。 */
export interface RailTaskRow {
  session: SessionSummary;
  timeLabel: string;
}

/** 主侧栏任务行状态徽标（2026-08-25 用户规格）。
 *  等审批 > 运行中 > 处理完成（本轮结束后尚未查看）；点开看过即回到时间展示。
 *  「处理完成」不区分成功/失败/中断 —— 只提醒“有新结果没看”；
 *  看过之后同一任务又跑完新一轮（updatedAt 越过 seenAt），会重新算未看。 */
export type RailTaskChip = 'running' | 'waiting_approval' | 'done_unseen';

const TERMINAL_TURN_STATUSES = new Set(['succeeded', 'failed', 'interrupted']);

export interface RailTaskChipInput {
  /** 本机记录的最近一次打开时间（epoch ms）；null/undefined = 从未在本机打开过（视为已看，避免首启刷屏）。 */
  seenAt?: number | null;
  /** 该任务当前是否正打开在工作台（正在盯着看的行不再标「处理完成」）。 */
  isActive?: boolean;
}

/** 派生该任务行应展示的状态徽标；null = 无徽标，正常显示时间。全部由真实字段推导，无任何猜测态。 */
export function railTaskChip(
  session: SessionSummary,
  input: RailTaskChipInput = {},
): RailTaskChip | null {
  if (session.archived) return null;
  if (session.pendingApproval || session.status === 'waiting_approval') return 'waiting_approval';
  if (session.lastTurnStatus === 'running') return 'running';
  // 兜底：旧数据缺 lastTurnStatus 时退回「活跃且有未完成工具」的保守判定
  if (!session.lastTurnStatus && session.status === 'active' && session.currentTool) {
    return 'running';
  }
  if (
    session.lastTurnStatus &&
    TERMINAL_TURN_STATUSES.has(session.lastTurnStatus) &&
    input.seenAt != null &&
    session.updatedAt * 1000 > input.seenAt &&
    !input.isActive
  ) {
    return 'done_unseen';
  }
  return null;
}

export interface RailRecentGroupVM {
  /** 分组 canonical cwd；平铺模式为空串。 */
  cwd: string;
  /** 目录行展示名：末级目录名（自动 Folder 命名优先），完整路径放 title。 */
  label: string;
  rows: RailTaskRow[];
}

/** 侧栏每组默认最多展示的任务行数：超出的收进「显示全部 N 条」折叠行（2026-09-04 用户规格）。 */
export const RAIL_VISIBLE_ROW_LIMIT = 10;

/** 平铺列表（按列表分组）的展开/收起键：目录组用 canonical cwd，平铺组用该哨兵值。 */
export const RAIL_FLAT_EXPAND_KEY = '__all__';

/** 把一组行拆成默认可见的前 10 条与折叠余量；不超过上限时全部可见。 */
export function splitRailRows(rows: RailTaskRow[]): {
  visible: RailTaskRow[];
  hidden: RailTaskRow[];
} {
  if (rows.length <= RAIL_VISIBLE_ROW_LIMIT) return { visible: rows, hidden: [] };
  return {
    visible: rows.slice(0, RAIL_VISIBLE_ROW_LIMIT),
    hidden: rows.slice(RAIL_VISIBLE_ROW_LIMIT),
  };
}

/** 搜索过滤（S1）：标题 + canonical cwd 子串匹配，大小写不敏感；归档退出最近任务。 */
export function filterRecentSessions(sessions: SessionSummary[], query: string): SessionSummary[] {
  const q = query.trim().toLowerCase();
  const active = sessions.filter((session) => !session.archived);
  if (!q) return active;
  return active.filter((session) => `${session.title} ${session.cwd}`.toLowerCase().includes(q));
}

function decorate(session: SessionSummary): RailTaskRow {
  return {
    session,
    timeLabel: relativeTimeText(session.updatedAt),
  };
}

/** 自动 Folder 的命名可作目录组展示名；canonical cwd 仍是分组真值（S0 冻结）。 */
export function folderNameByCwd(folders: SessionFolder[]): Map<string, string> {
  const map = new Map<string, string>();
  for (const folder of folders) {
    if (!folder.cwd) continue;
    if (!map.has(folder.cwd)) map.set(folder.cwd, folder.name);
  }
  return map;
}

/** 「选择工作目录」弹框候选（2026-08-23 二次反馈）：真实会话 cwd 按最近活跃去重排序，query 子串过滤。 */
export function directoryOptions(
  sessions: SessionSummary[],
  folders: SessionFolder[],
  query: string,
): DirectoryOption[] {
  const nameByCwd = folderNameByCwd(folders);
  const seen = new Set<string>();
  const rows: DirectoryOption[] = [];
  for (const session of [...sessions].sort((a, b) => b.updatedAt - a.updatedAt)) {
    if (seen.has(session.cwd)) continue;
    seen.add(session.cwd);
    rows.push({ cwd: session.cwd, label: nameByCwd.get(session.cwd) ?? folderLabel(session.cwd) });
  }
  const q = query.trim().toLowerCase();
  if (!q) return rows;
  return rows.filter((row) => (row.label + ' ' + row.cwd).toLowerCase().includes(q));
}

/**
 * 手动排序合成（2026-08-23 五次反馈）：
 * 置顶始终最前（S0 规则不变），其余按 manualOrder 中的先后；
 * 未登记的新会话按最近活跃追加在已排序者之后。
 */
export function applyManualOrder(
  sessions: SessionSummary[],
  manualOrder: string[] = [],
): SessionSummary[] {
  const rank = new Map<string, number>();
  manualOrder.forEach((id, index) => {
    if (!rank.has(id)) rank.set(id, index);
  });
  const byRecent = [...sessions].sort((a, b) => b.updatedAt - a.updatedAt);
  return byRecent.sort((a, b) => {
    if (a.pinned !== b.pinned) return a.pinned ? -1 : 1;
    const ra = rank.get(a.id);
    const rb = rank.get(b.id);
    if (ra !== undefined && rb !== undefined) return ra - rb;
    if (ra !== undefined) return -1;
    if (rb !== undefined) return 1;
    return 0; // byRecent 已保证同层时间倒排（Array#sort 稳定）
  });
}

/**
 * 在当前可见序列里把 dragId 移动到 overId 的位置（其余项依次让位）。
 * 返回新的完整顺序数组；任一 id 不在序列中时原样返回。
 */
export function reorderVisibleIds(visibleIds: string[], dragId: string, overId: string): string[] {
  if (dragId === overId) return visibleIds;
  const from = visibleIds.indexOf(dragId);
  const to = visibleIds.indexOf(overId);
  if (from === -1 || to === -1) return visibleIds;
  const next = visibleIds.slice();
  next.splice(from, 1);
  next.splice(to, 0, dragId);
  return next;
}

/** 最近任务视图组装：搜索 → 归档剔除 → 排序（最近/手动）→ 平铺或按 cwd 分组。 */
export function buildRailRecentGroups(
  sessions: SessionSummary[],
  filters: RailRecentFilters,
  folders: SessionFolder[] = [],
): RailRecentGroupVM[] {
  const filtered = filterRecentSessions(sessions, filters.query);
  const nameByCwd = folderNameByCwd(folders);
  const ordered =
    filters.sort === 'manual' ? applyManualOrder(filtered, filters.manualOrder) : filtered;
  if (filters.grouping === 'folder') {
    const groups =
      filters.sort === 'manual'
        ? groupByCwdPreservingOrder(ordered)
        : groupRecentTasksByCwd(ordered, 'time');
    return groups.map((group) => ({
      cwd: group.cwd,
      label: nameByCwd.get(group.cwd) ?? folderLabel(group.cwd),
      rows: group.sessions.map(decorate),
    }));
  }
  const rows =
    filters.sort === 'manual'
      ? ordered.map(decorate)
      : listRecentTasks(filtered, 'time').map(decorate);
  return [{ cwd: '', label: '', rows }];
}

interface CwdGroup {
  cwd: string;
  sessions: SessionSummary[];
}

/**
 * 工作区上报的当前会话身份（2026-09-04 用户报告「发起了新对话，左栏还选中老对话」）：
 * 三个 id 分别对应「已落库的会话」「运行时句柄」「CLI 原生会话」，任一命中即算当前会话。
 * 全部为空 = 工作区是新建未落库的空会话，此时左栏不应有任何选中行。
 */
export interface ActiveSessionIds {
  historyId?: string | null;
  handleId?: string | null;
  cliSessionId?: string | null;
}

/**
 * 把工作区上报的会话身份落到侧栏真实行 id；尚未进入列表（刚创建、列表未刷新）时返回 null，
 * 调用方保留上一次的乐观选中，等列表刷新后重新派生。
 */
export function activeRailTaskId(
  sessions: SessionSummary[],
  ids: ActiveSessionIds | null,
): string | null {
  if (!ids) return null;
  const hasAny = Boolean(ids.historyId || ids.handleId || ids.cliSessionId);
  if (!hasAny) return null;
  const byId = (id: string | null | undefined): string | null => {
    if (!id) return null;
    return sessions.some((session) => session.id === id) ? id : null;
  };
  const direct = byId(ids.historyId) ?? byId(ids.handleId);
  if (direct) return direct;
  if (ids.cliSessionId) {
    const hit = sessions.find((session) => session.cliSessionId === ids.cliSessionId);
    if (hit) return hit.id;
  }
  return null;
}

/** 手动排序下的目录分组：组间顺序跟随首次出现位置，组内保持手动顺序。 */
function groupByCwdPreservingOrder(sessions: SessionSummary[]): CwdGroup[] {
  const groups = new Map<string, SessionSummary[]>();
  for (const session of sessions) {
    const bucket = groups.get(session.cwd);
    if (bucket) bucket.push(session);
    else groups.set(session.cwd, [session]);
  }
  return Array.from(groups, ([cwd, members]) => ({ cwd, sessions: members }));
}
