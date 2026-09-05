import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { listen } from '@tauri-apps/api/event';
import { PRIMARY_RAIL_ENTRIES, type PageId } from './navigation';
import { Icon } from './icons';
import { EngineBrand } from './EngineBrand';
import {
  deleteSession,
  getBackgroundOperation,
  listFolders,
  listSessions,
  renameSession,
  setSessionArchived,
  startSessionBranch,
} from '../sessions/api';
import type { SessionFolder, SessionSummary } from '../sessions/sessionTypes';
import { forkTrace } from '../diag/forkTrace';
import {
  activeRailTaskId,
  buildRailRecentGroups,
  directoryOptions,
  RAIL_FLAT_EXPAND_KEY,
  railTaskChip,
  splitRailRows,
  type ActiveSessionIds,
  reorderVisibleIds,
  type RailGrouping,
  type RailRecentGroupVM,
  type RailSort,
  type RailTaskChip as RailTaskChipKey,
  type RailTaskRow,
} from './railViewModel';
import { markRailTaskSeen, railTaskSeenAt } from './railSeen';

/** 任务行状态徽标展示映射：文案与提示语对齐原型 cm-recent-item__state。 */
const RAIL_CHIP_META: Record<RailTaskChipKey, { cls: string; label: string; tip: string }> = {
  running: { cls: 'run', label: '运行中', tip: '正在执行 · 点击查看实时进度' },
  waiting_approval: { cls: 'wait', label: '等审批', tip: '等待人工确认 · 点击去处理' },
  done_unseen: { cls: 'done', label: '处理完成', tip: '本轮已完成 · 打开查看结果' },
};
import { dismissToast, showToast } from '../components/toast';
import { ConfirmDialog } from '../components/ConfirmDialog';
import { Dialog } from '../components/Dialog';
import { selectDirectory } from '../settings/api';
import { Button } from '@/components/ui/button';

export type { PageId };

/* 主侧栏（S1 任务型文字侧栏）：顶部 4 个主入口 + 中段最近任务滚动区 + 底部设置。
   视觉真值 prototype/assets/commercial.js 的 cm-sidebar；数据全部来自真实 session/folder 命令。 */

interface MenuAnchor {
  x: number;
  y: number;
}

interface TaskMenuState extends MenuAnchor {
  sessionId: string;
}

/** 视图选项持久化（2026-08-23 五次反馈）：纯 UI 偏好，localStorage 存放，不进业务设置。 */
interface RailViewPrefs {
  grouping: RailGrouping;
  sort: RailSort;
  /** 已折叠目录组（canonical cwd）。 */
  collapsed: string[];
  /** 已展开「显示全部」的组键：目录组为 canonical cwd，平铺列表为 RAIL_FLAT_EXPAND_KEY。 */
  expanded: string[];
  /** 手动排序全序（sessionId 有序数组）。 */
  order: string[];
}

const RAIL_VIEW_KEY = 'helm.railView.v1';
const RAIL_VIEW_DEFAULT: RailViewPrefs = {
  grouping: 'folder',
  sort: 'recent',
  collapsed: [],
  expanded: [],
  order: [],
};

function loadRailViewPrefs(): RailViewPrefs {
  try {
    const raw = window.localStorage.getItem(RAIL_VIEW_KEY);
    if (!raw) return RAIL_VIEW_DEFAULT;
    const parsed = JSON.parse(raw) as Partial<RailViewPrefs>;
    return {
      grouping: parsed.grouping === 'list' ? 'list' : 'folder',
      sort: parsed.sort === 'manual' ? 'manual' : 'recent',
      collapsed: Array.isArray(parsed.collapsed)
        ? parsed.collapsed.filter((c) => typeof c === 'string')
        : [],
      expanded: Array.isArray(parsed.expanded)
        ? parsed.expanded.filter((c) => typeof c === 'string')
        : [],
      order: Array.isArray(parsed.order) ? parsed.order.filter((o) => typeof o === 'string') : [],
    };
  } catch {
    return RAIL_VIEW_DEFAULT;
  }
}

function saveRailViewPrefs(prefs: RailViewPrefs): void {
  try {
    window.localStorage.setItem(RAIL_VIEW_KEY, JSON.stringify(prefs));
  } catch {
    // 隐私模式等场景写入失败可接受：视图偏好不构成业务数据
  }
}

const FORK_POLL_MS = 750;
const FORK_POLL_MAX = 120; // ~90s：与工作台交接轮询同量级

/** Tauri invoke 失败以字符串 reject，`instanceof Error` 会吞掉后端真实原因（十一次反馈）。 */
function forkErrorMessage(err: unknown): string {
  if (typeof err === 'string' && err.trim()) return '分叉失败：' + err;
  if (err instanceof Error && err.message) return err.message;
  if (err != null && typeof err !== 'object') return '分叉失败：' + String(err);
  return '创建分叉任务失败';
}

async function followForkOperation(operationId: string): Promise<string> {
  for (let attempt = 0; attempt < FORK_POLL_MAX; attempt += 1) {
    const operation = await getBackgroundOperation(operationId);
    if (!operation) throw new Error('交接任务状态缺失');
    if (operation.status === 'succeeded') {
      const targetSessionId =
        operation.result && typeof operation.result === 'object'
          ? (operation.result as { targetSessionId?: unknown }).targetSessionId
          : null;
      if (typeof targetSessionId !== 'string' || !targetSessionId) {
        throw new Error('交接任务完成，但目标 Session 身份缺失');
      }
      return targetSessionId;
    }
    if (operation.status === 'failed' || operation.status === 'cancelled') {
      throw new Error(
        operation.errorCode ? '分叉失败：' + operation.errorCode : '分叉任务失败或已取消',
      );
    }
    await new Promise((resolve) => setTimeout(resolve, FORK_POLL_MS));
  }
  throw new Error('分叉任务超时，请稍后在「全部任务」中查看结果');
}

function openTaskInWorkspace(sessionId: string) {
  window.dispatchEvent(new CustomEvent('helm:open-session', { detail: { sessionId } }));
}

/* 最近任务行（纯展示）：标题 / 目录或分叉来源 / 时间 / kebab；手动排序时按住行拖拽换位。
   2026-08-23 六次反馈根因：HTML5 drag&drop 在 Tauri Windows（WebView2 默认开启
   dragDropEnabled 劫持原生拖放）下完全不触发——浏览器探针能拖、装进壳就失灵。
   改用 Pointer Events 自研拖拽：按下后位移超 5px 激活，命中行实时换位，
   浏览器与 Tauri 同一套行为；跨目录组命中不换位（会话归属真实 cwd）。 */
export function RailTaskRows({
  rows,
  activeTaskId,
  groupId = '',
  manualSort = false,
  onReorder,
  onOpenTask,
  onOpenMenu,
}: {
  rows: RailTaskRow[];
  activeTaskId: string | null;
  /** 所在目录组 canonical cwd（平铺为空串）：拖拽命中检测的同组约束键。 */
  groupId?: string;
  manualSort?: boolean;
  onReorder?: (dragId: string, overId: string) => void;
  onOpenTask: (sessionId: string) => void;
  onOpenMenu: (row: RailTaskRow, anchor: Element) => void;
}) {
  const dragState = useRef<{
    id: string;
    groupId: string;
    x: number;
    y: number;
    active: boolean;
  } | null>(null);
  const [draggingId, setDraggingId] = useState<string | null>(null);
  const suppressClickUntil = useRef(0);
  const reorderRef = useRef(onReorder);
  reorderRef.current = onReorder;

  useEffect(() => {
    if (!manualSort) return;
    const hitRowId = (x: number, y: number, dragGroupId: string): string | null => {
      for (const el of document.querySelectorAll<HTMLElement>(
        '[data-rail-recent] [data-task-id]',
      )) {
        const rect = el.getBoundingClientRect();
        if (x < rect.left || x > rect.right || y < rect.top || y > rect.bottom) continue;
        return el.dataset.groupId === dragGroupId ? (el.dataset.taskId ?? null) : null;
      }
      return null;
    };
    const onMove = (event: PointerEvent) => {
      const drag = dragState.current;
      if (!drag) return;
      if (!drag.active) {
        if (Math.hypot(event.clientX - drag.x, event.clientY - drag.y) < 5) return;
        drag.active = true;
        setDraggingId(drag.id);
        document.body.classList.add('is-rail-dragging');
      }
      event.preventDefault();
      const overId = hitRowId(event.clientX, event.clientY, drag.groupId);
      if (overId && overId !== drag.id) reorderRef.current?.(drag.id, overId);
    };
    const endDrag = () => {
      const drag = dragState.current;
      if (drag?.active) {
        suppressClickUntil.current = Date.now() + 240;
        document.body.classList.remove('is-rail-dragging');
        setDraggingId(null);
      }
      dragState.current = null;
    };
    window.addEventListener('pointermove', onMove, { passive: false });
    window.addEventListener('pointerup', endDrag);
    window.addEventListener('pointercancel', endDrag);
    return () => {
      window.removeEventListener('pointermove', onMove);
      window.removeEventListener('pointerup', endDrag);
      window.removeEventListener('pointercancel', endDrag);
      document.body.classList.remove('is-rail-dragging');
    };
  }, [manualSort]);

  const recentlyDragged = () => Date.now() < suppressClickUntil.current;

  return (
    <>
      {rows.map(({ session, timeLabel }) => {
        const chip = railTaskChip(session, {
          seenAt: railTaskSeenAt(session.id),
          isActive: session.id === activeTaskId,
        });
        const chipMeta = chip ? RAIL_CHIP_META[chip] : null;
        return (
          <div
            className={
              'rail-task' +
              (session.id === activeTaskId ? ' is-active' : '') +
              (draggingId === session.id ? ' is-dragging' : '') +
              (chip === 'waiting_approval' ? ' is-wait' : '')
            }
            data-task-id={session.id}
            data-group-id={groupId}
            key={session.id}
            onPointerDown={(event) => {
              if (!manualSort || event.button !== 0) return;
              dragState.current = {
                id: session.id,
                groupId,
                x: event.clientX,
                y: event.clientY,
                active: false,
              };
            }}
          >
            <button
              className="rail-task__link"
              type="button"
              title={session.title + ' · ' + session.cwd}
              onClick={() => {
                if (recentlyDragged()) return;
                markRailTaskSeen(session.id);
                onOpenTask(session.id);
              }}
            >
              <span className="rail-task__icon">
                <EngineBrand engine={session.engine} size={14} />
              </span>
              <span className="rail-task__main">
                <b>{session.title}</b>
                {session.forkedFrom ? (
                  <small
                    className="rail-task__from"
                    title={'分叉自「' + session.forkedFrom + '」· 原任务完整保留'}
                  >
                    <Icon name="gitbranch" />
                    分叉自 {session.forkedFrom}
                  </small>
                ) : (
                  <small>{session.cwd}</small>
                )}
              </span>
              {chipMeta ? (
                <span
                  className={'rail-task__state rail-task__state--' + chipMeta.cls}
                  title={chipMeta.tip}
                >
                  {chip === 'running' ? <i aria-hidden="true" /> : null}
                  {chipMeta.label}
                </span>
              ) : (
                <span className="rail-task__time tnum">{timeLabel}</span>
              )}
            </button>
            <button
              className="rail-task__more"
              type="button"
              title="更多操作"
              aria-label={'更多操作：' + session.title}
              aria-haspopup="menu"
              onClick={(event) => {
                if (recentlyDragged()) return;
                event.stopPropagation();
                onOpenMenu({ session, timeLabel }, event.currentTarget);
              }}
            >
              <Icon name="more" />
            </button>
          </div>
        );
      })}
    </>
  );
}

/* 最近任务滚动区主体：加载 / 错误 / 空态与分组列表。
   每组（目录组 / 平铺列表）默认最多展示 RAIL_VISIBLE_ROW_LIMIT 条最新任务，
   超出的收进「显示全部 N 条」折叠行，点击展开/收起（2026-09-04 用户规格）；
   搜索态（query 非空）不截断——搜出来的结果藏起来反而找不到任务。 */
export function RailRecentBody({
  loading,
  error,
  groups,
  hasAnySession,
  activeTaskId,
  collapsedDirs,
  onToggleGroup,
  expandedGroups,
  onToggleExpanded,
  truncate = true,
  manualSort,
  onReorder,
  onRetry,
  onOpenTask,
  onOpenMenu,
}: {
  loading: boolean;
  error: string | null;
  groups: RailRecentGroupVM[];
  hasAnySession: boolean;
  activeTaskId: string | null;
  /** 已折叠目录组（canonical cwd）。 */
  collapsedDirs?: string[];
  onToggleGroup?: (cwd: string) => void;
  /** 已展开「显示全部」的组键（目录组 = cwd，平铺 = RAIL_FLAT_EXPAND_KEY）。 */
  expandedGroups?: string[];
  onToggleExpanded?: (key: string) => void;
  /** 是否按每组上限截断行数；搜索态传 false 全量展示。 */
  truncate?: boolean;
  /** 手动排序开启时行可拖拽。 */
  manualSort?: boolean;
  onReorder?: (dragId: string, overId: string) => void;
  onRetry: () => void;
  onOpenTask: (sessionId: string) => void;
  onOpenMenu: (row: RailTaskRow, anchor: Element) => void;
}) {
  if (error) {
    return (
      <div className="rail-recent__error" role="alert">
        <span>{error}</span>
        <button type="button" onClick={onRetry}>
          重试
        </button>
      </div>
    );
  }
  if (loading && !hasAnySession) {
    return <div className="rail-recent__empty">正在加载最近任务…</div>;
  }
  const totalRows = groups.reduce((sum, group) => sum + group.rows.length, 0);
  if (totalRows === 0) {
    return (
      <div className="rail-recent__empty">{hasAnySession ? '没有匹配的任务' : '暂无最近任务'}</div>
    );
  }
  return (
    <>
      {groups.map((group) => {
        const isCollapsed = group.cwd !== '' && (collapsedDirs ?? []).includes(group.cwd);
        const expandKey = group.cwd || RAIL_FLAT_EXPAND_KEY;
        const { visible, hidden } = truncate
          ? splitRailRows(group.rows)
          : {
              visible: group.rows,
              hidden: [],
            };
        const isExpanded = (expandedGroups ?? []).includes(expandKey);
        const shown = isExpanded ? [...visible, ...hidden] : visible;
        return (
          <div key={group.cwd || '__flat__'}>
            {group.cwd ? (
              <button
                type="button"
                className={'rail-group' + (isCollapsed ? ' is-collapsed' : '')}
                title={group.cwd}
                aria-expanded={!isCollapsed}
                onClick={() => onToggleGroup?.(group.cwd)}
              >
                <Icon name="chevrondown" className="rail-group__chev" />
                <Icon name="folder" />
                <span>{group.label}</span>
                <small className="tnum">{group.rows.length}</small>
              </button>
            ) : null}
            {!isCollapsed ? (
              <>
                <RailTaskRows
                  rows={shown}
                  activeTaskId={activeTaskId}
                  groupId={group.cwd || ''}
                  manualSort={manualSort}
                  onReorder={onReorder}
                  onOpenTask={onOpenTask}
                  onOpenMenu={onOpenMenu}
                />
                {hidden.length > 0 ? (
                  <button
                    type="button"
                    className={'rail-more' + (isExpanded ? ' is-open' : '')}
                    aria-expanded={isExpanded}
                    onClick={() => onToggleExpanded?.(expandKey)}
                  >
                    <Icon name="chevrondown" className="rail-more__chev" />
                    {isExpanded ? '收起' : `显示全部 ${visible.length + hidden.length} 条`}
                  </button>
                ) : null}
              </>
            ) : null}
          </div>
        );
      })}
    </>
  );
}

export function Rail({
  active,
  onSelect,
  onSetDefaultDirectory,
}: {
  active: PageId;
  onSelect: (page: PageId) => void;
  /** 「选择工作目录」弹框落点：写入 App 设置的 defaultDirectory（真实持久化链）。 */
  onSetDefaultDirectory: (path: string) => void;
}) {
  const [sessions, setSessions] = useState<SessionSummary[]>([]);
  const [folders, setFolders] = useState<SessionFolder[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [searching, setSearching] = useState(false);
  const [query, setQuery] = useState('');
  const [viewPrefs, setViewPrefs] = useState<RailViewPrefs>(loadRailViewPrefs);
  const grouping = viewPrefs.grouping;
  const sort = viewPrefs.sort;
  const [taskMenu, setTaskMenu] = useState<TaskMenuState | null>(null);
  const [viewMenu, setViewMenu] = useState<MenuAnchor | null>(null);
  const [renameTarget, setRenameTarget] = useState<SessionSummary | null>(null);
  const [renameValue, setRenameValue] = useState('');
  const [deleteTarget, setDeleteTarget] = useState<SessionSummary | null>(null);
  const [forkBusy, setForkBusy] = useState(false);
  // 选中态唯一真值 = 工作区当前正在跑的会话（2026-09-04 用户报告）：
  // activeIds 由 Workspace 在会话身份变化时上报；pickedTaskId 只是「点完行 → 工作区上报抵达」
  // 之间的乐观高亮，上报一到就以工作区为准（新建会话会上报全空，高亮随即清空）。
  const [pickedTaskId, setPickedTaskId] = useState<string | null>(null);
  const [activeIds, setActiveIds] = useState<ActiveSessionIds | null>(null);
  const activeTaskId = activeRailTaskId(sessions, activeIds) ?? pickedTaskId;
  const [dirPickerOpen, setDirPickerOpen] = useState(false);
  const [dirQuery, setDirQuery] = useState('');
  const forkSeq = useRef(0);

  // 真实数据源：list_sessions / list_folders；helm-sessions-changed 与窗口聚焦触发刷新。
  const refresh = useCallback(async () => {
    try {
      const [nextSessions, nextFolders] = await Promise.all([listSessions(), listFolders()]);
      setSessions(nextSessions);
      setFolders(nextFolders);
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : '读取任务列表失败');
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | null = null;
    void listen('helm-sessions-changed', () => {
      void refresh();
    })
      .then((stop) => {
        if (disposed) stop();
        else unlisten = stop;
      })
      .catch(() => {
        // 浏览器预览没有 Tauri 事件桥；忽略
      });
    const onVisible = () => {
      if (document.visibilityState === 'visible') void refresh();
    };
    document.addEventListener('visibilitychange', onVisible);
    return () => {
      disposed = true;
      document.removeEventListener('visibilitychange', onVisible);
      unlisten?.();
    };
  }, [refresh]);

  // 「选择工作目录」弹框：就地选择已有目录或系统选目录，经 App 持久化链写入
  // general.defaultDirectory；2026-08-23 五次反馈决策——选完即跳新任务页，
  // 页面以该目录为选中态直接就绪。
  const closeDirPicker = () => {
    setDirPickerOpen(false);
    setDirQuery('');
  };
  const chooseDirectory = (path: string) => {
    onSetDefaultDirectory(path);
    showToast('已设为默认工作目录，已跳转新任务');
    closeDirPicker();
    onSelect('home');
  };
  const browseDirectory = async () => {
    try {
      const dir = await selectDirectory();
      if (dir) chooseDirectory(dir);
    } catch {
      showToast('目录选择器不可用，请在设置中配置默认目录', 'error');
    }
  };
  const dirRows = directoryOptions(sessions, folders, dirQuery);

  // 打开任务的高亮同步：本组件发起 + 其他入口（全部任务页 / 命令面板）派发的事件都算数。
  useEffect(() => {
    const onOpenSession = (event: Event) => {
      const sessionId = (event as CustomEvent<{ sessionId?: string }>).detail?.sessionId;
      if (sessionId) setPickedTaskId(sessionId);
    };
    const onActiveSession = (event: Event) => {
      const detail = (event as CustomEvent<ActiveSessionIds | null>).detail ?? null;
      setActiveIds(detail);
      setPickedTaskId(null);
    };
    window.addEventListener('helm:open-session', onOpenSession);
    window.addEventListener('helm:session-active', onActiveSession);
    return () => {
      window.removeEventListener('helm:open-session', onOpenSession);
      window.removeEventListener('helm:session-active', onActiveSession);
    };
  }, []);

  // 浮层关闭：点击任意处 / Esc（Esc 同时退出搜索态）。
  // 四次反馈根因：打开菜单的同一次点击仍会冒泡到 window；若不排除触发钮与菜单内部，
  // 菜单会在开启的同一手势里被立刻关掉——表现为「点了没反应」。
  useEffect(() => {
    if (!taskMenu && !viewMenu) return;
    const close = () => {
      setTaskMenu(null);
      setViewMenu(null);
    };
    const onClick = (event: MouseEvent) => {
      const target = event.target;
      if (target instanceof Element && target.closest('[role="menu"],[aria-haspopup="menu"]')) {
        return; // 触发钮的开关语义由按钮自身处理；菜单内点击由容器 stopPropagation 处理。
      }
      close();
    };
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== 'Escape') return;
      close();
      setSearching(false);
      setQuery('');
    };
    window.addEventListener('click', onClick);
    window.addEventListener('keydown', onKey);
    return () => {
      window.removeEventListener('click', onClick);
      window.removeEventListener('keydown', onKey);
    };
  }, [taskMenu, viewMenu]);

  const groups = useMemo(
    () =>
      buildRailRecentGroups(
        sessions,
        { query, grouping, sort, manualOrder: viewPrefs.order },
        folders,
      ),
    [folders, grouping, query, sessions, sort, viewPrefs.order],
  );

  const updateViewPrefs = (patch: Partial<RailViewPrefs>) => {
    setViewPrefs((prev) => {
      const next = { ...prev, ...patch };
      saveRailViewPrefs(next);
      return next;
    });
  };

  const toggleGroupCollapsed = (cwd: string) => {
    setViewPrefs((prev) => {
      const collapsed = prev.collapsed.includes(cwd)
        ? prev.collapsed.filter((item) => item !== cwd)
        : [...prev.collapsed, cwd];
      const next = { ...prev, collapsed };
      saveRailViewPrefs(next);
      return next;
    });
  };

  // 「显示全部 / 收起」展开态：目录组键 = canonical cwd，平铺列表键 = RAIL_FLAT_EXPAND_KEY。
  const toggleGroupExpanded = (key: string) => {
    setViewPrefs((prev) => {
      const expanded = prev.expanded.includes(key)
        ? prev.expanded.filter((item) => item !== key)
        : [...prev.expanded, key];
      const next = { ...prev, expanded };
      saveRailViewPrefs(next);
      return next;
    });
  };

  // 手动排序：在当前可见序列内把 drag 行移动到 over 行位置，并把全序持久化。
  // 按目录模式下仅允许同目录组内换位：会话归属真实 cwd，跨组拖拽视为未命中。
  const handleReorder = (dragId: string, overId: string) => {
    if (sort !== 'manual') return;
    const groupOf = new Map<string, string>();
    for (const group of groups)
      for (const row of group.rows) groupOf.set(row.session.id, group.cwd);
    if (groupOf.get(dragId) !== groupOf.get(overId)) return;
    const visibleIds: string[] = [];
    for (const group of groups) for (const row of group.rows) visibleIds.push(row.session.id);
    const nextOrder = reorderVisibleIds(visibleIds, dragId, overId);
    if (nextOrder === visibleIds) return;
    updateViewPrefs({ order: nextOrder });
  };
  const menuSession = taskMenu
    ? sessions.find((session) => session.id === taskMenu.sessionId)
    : undefined;

  const anchorFor = (rect: DOMRect, estimatedHeight: number): MenuAnchor => ({
    x: rect.left,
    y: Math.max(8, Math.min(rect.bottom + 6, window.innerHeight - estimatedHeight)),
  });

  const openTaskMenu = (row: RailTaskRow, anchor: Element) => {
    const state = anchorFor(anchor.getBoundingClientRect(), 210);
    setViewMenu(null);
    setTaskMenu((current) =>
      current?.sessionId === row.session.id ? null : { ...state, sessionId: row.session.id },
    );
  };

  const openViewMenu = (anchor: Element) => {
    const state = anchorFor(anchor.getBoundingClientRect(), 250);
    setTaskMenu(null);
    setViewMenu((current) =>
      current && current.x === state.x && current.y === state.y ? null : state,
    );
  };

  const handleRenameConfirm = async () => {
    if (!renameTarget) return;
    const title = renameValue.trim();
    if (!title || title === renameTarget.title) return;
    try {
      await renameSession(renameTarget.id, title);
      showToast('已重命名为「' + title + '」');
      setRenameTarget(null);
      void refresh();
    } catch (err) {
      showToast('重命名失败：' + (err instanceof Error ? err.message : String(err)), 'error');
    }
  };

  const handleDeleteConfirm = async () => {
    if (!deleteTarget) return;
    try {
      await deleteSession(deleteTarget.id);
      showToast('任务已删除');
      if (activeTaskId === deleteTarget.id) setPickedTaskId(null);
      setDeleteTarget(null);
      void refresh();
    } catch (err) {
      showToast('删除失败：' + (err instanceof Error ? err.message : String(err)), 'error');
    }
  };

  const handleToggleArchived = async (session: SessionSummary) => {
    try {
      await setSessionArchived(session.id, !session.archived);
      showToast(session.archived ? '已取消归档' : '已归档 · 可在「全部任务」的已归档筛选中找回');
      void refresh();
    } catch (err) {
      showToast('归档失败：' + (err instanceof Error ? err.message : String(err)), 'error');
    }
  };

  // 分叉（S1 更多菜单）：无损分支优先——同引擎且 CLI 支持 --fork-session 时即时
  // 开新会话，完整历史随首条消息复制（同业体验）；否则回退摘要派生并轮询。
  // 原任务完整保留。
  const handleFork = async (session: SessionSummary) => {
    if (forkBusy) return;
    const seq = ++forkSeq.current;
    setForkBusy(true);
    forkTrace('rail_fork_click', `source=${session.id}`);
    // 即时反馈：摘要派生是真实 CLI 调用，可能持续几十秒；全程挂一条进行中提示。
    const progressToastId = showToast(
      '正在创建分叉任务 · 优先无损派生，CLI 不支持时回退交接摘要（原任务完整保留）',
      'info',
      120_000,
    );
    try {
      const outcome = await startSessionBranch(session.id);
      if (outcome.mode === 'lossless') {
        dismissToast(progressToastId);
        if (seq !== forkSeq.current) {
          forkTrace('rail_lossless_stale_seq', `target=${outcome.sessionId}`);
          return;
        }
        // 契约防御（2026-09-04 埋点实证）：后端序列化错位曾让 sessionId=undefined，
        // 事件带着空值派发 → App 静默丢弃 →「点了没反应」。坏载荷就地暴露。
        if (!outcome.sessionId) {
          forkTrace('rail_lossless_bad_payload', `raw=${JSON.stringify(outcome)}`);
          showToast('分叉已创建，但返回载荷缺少会话标识（前端契约错误，请反馈）', 'error');
          void refresh();
          return;
        }
        showToast('已创建无损分支 · 完整历史将随首条消息一并携带');
        setPickedTaskId(outcome.sessionId);
        forkTrace('rail_lossless_dispatch', `target=${outcome.sessionId}`);
        openTaskInWorkspace(outcome.sessionId);
        void refresh();
        return;
      }
      const targetSessionId = await followForkOperation(outcome.operation.id);
      dismissToast(progressToastId);
      if (seq !== forkSeq.current) {
        forkTrace('rail_summary_stale_seq', `target=${targetSessionId}`);
        return;
      }
      showToast('已通过交接摘要创建新任务 · 原任务完整保留');
      setPickedTaskId(targetSessionId);
      forkTrace('rail_summary_dispatch', `target=${targetSessionId}`);
      openTaskInWorkspace(targetSessionId);
      void refresh();
    } catch (err) {
      dismissToast(progressToastId);
      if (seq === forkSeq.current) {
        forkTrace(
          'rail_fork_error',
          `source=${session.id} err=${err instanceof Error ? err.message : String(err)}`,
        );
        showToast(forkErrorMessage(err), 'error');
      }
    } finally {
      if (seq === forkSeq.current) setForkBusy(false);
    }
  };

  // workspace 是工作区详情态（S0 协议）：不映射为一级导航激活项。
  const activeNav = active === 'workspace' ? null : active;

  return (
    <nav className="rail" aria-label="主导航">
      <div className="rail-nav">
        {PRIMARY_RAIL_ENTRIES.map((entry) => (
          <button
            className={'rail-nav__item' + (entry.id === activeNav ? ' is-active' : '')}
            aria-label={entry.label}
            aria-current={entry.id === activeNav ? 'page' : undefined}
            type="button"
            key={entry.id}
            onClick={() => onSelect(entry.id)}
          >
            <span className="rail-nav__icon">
              <Icon name={entry.icon} />
            </span>
            <span className="rail-nav__label">{entry.label}</span>
          </button>
        ))}
      </div>
      <div className={'rail-recent' + (searching ? ' is-searching' : '')}>
        <div className="rail-recent__toolbar">
          <div className="rail-recent__head">
            <span>最近任务</span>
          </div>
          <div className="rail-recent__actions">
            <button
              className="rail-icon-btn"
              type="button"
              aria-label="搜索任务"
              title="搜索任务"
              onClick={() => {
                setSearching(true);
                setTaskMenu(null);
                setViewMenu(null);
              }}
            >
              <Icon name="search" />
            </button>
            <button
              className="rail-icon-btn"
              type="button"
              aria-label="视图选项"
              title="视图选项"
              aria-haspopup="menu"
              onClick={(event) => openViewMenu(event.currentTarget)}
            >
              <Icon name="slidersh" />
            </button>
            <button
              className="rail-icon-btn"
              type="button"
              aria-label="添加工作目录"
              title="添加工作目录"
              onClick={() => {
                setDirPickerOpen(true);
                setTaskMenu(null);
                setViewMenu(null);
              }}
            >
              <Icon name="folder" />
            </button>
          </div>
          <div className="rail-recent__search">
            <Icon name="search" />
            <input
              type="text"
              placeholder="输入名称匹配任务…"
              aria-label="搜索最近任务"
              value={query}
              onChange={(event) => setQuery(event.target.value)}
              onKeyDown={(event) => {
                if (event.key !== 'Escape') return;
                event.stopPropagation();
                setSearching(false);
                setQuery('');
              }}
            />
            <button
              className="rail-recent__search-close"
              type="button"
              aria-label="关闭搜索"
              title="关闭搜索"
              onClick={() => {
                setSearching(false);
                setQuery('');
              }}
            >
              <Icon name="x" />
            </button>
          </div>
        </div>
        <div
          className="rail-recent__body"
          data-rail-recent=""
          data-manual={sort === 'manual' ? 'true' : undefined}
        >
          <RailRecentBody
            loading={loading}
            error={error}
            groups={groups}
            hasAnySession={sessions.length > 0}
            activeTaskId={activeTaskId}
            collapsedDirs={viewPrefs.collapsed}
            onToggleGroup={toggleGroupCollapsed}
            expandedGroups={viewPrefs.expanded}
            onToggleExpanded={toggleGroupExpanded}
            truncate={query.trim() === ''}
            manualSort={sort === 'manual'}
            onReorder={handleReorder}
            onRetry={() => void refresh()}
            onOpenTask={(sessionId) => {
              setPickedTaskId(sessionId);
              openTaskInWorkspace(sessionId);
            }}
            onOpenMenu={openTaskMenu}
          />
        </div>
      </div>
      <div className="rail-footer">
        <button
          className={'rail-nav__item' + (active === 'settings' ? ' is-active' : '')}
          aria-label="设置"
          aria-current={active === 'settings' ? 'page' : undefined}
          type="button"
          onClick={() => onSelect('settings')}
        >
          <span className="rail-nav__icon">
            <Icon name="settings2" />
          </span>
          <span className="rail-nav__label">设置</span>
        </button>
      </div>

      {taskMenu && menuSession
        ? createPortal(
            <div
              className="menu rail-menu"
              role="menu"
              style={{ left: taskMenu.x, top: taskMenu.y }}
              onClick={(event) => event.stopPropagation()}
            >
              <button
                type="button"
                className="menu__item"
                role="menuitem"
                onClick={() => {
                  setRenameValue(menuSession.title);
                  setRenameTarget(menuSession);
                  setTaskMenu(null);
                }}
              >
                <Icon name="edit" /> 重命名
              </button>
              <button
                type="button"
                className="menu__item"
                role="menuitem"
                onClick={() => {
                  setTaskMenu(null);
                  void handleToggleArchived(menuSession);
                }}
              >
                <Icon name="archive" /> {menuSession.archived ? '取消归档' : '归档任务'}
              </button>
              <button
                type="button"
                className="menu__item"
                role="menuitem"
                disabled={forkBusy}
                onClick={() => {
                  setTaskMenu(null);
                  void handleFork(menuSession);
                }}
              >
                <Icon name="gitbranch" /> 分叉任务
                <small>从此任务派生新任务 · 原任务完整保留</small>
              </button>
              <div className="menu__sep" />
              <button
                type="button"
                className="menu__item rail-menu__danger"
                role="menuitem"
                onClick={() => {
                  setDeleteTarget(menuSession);
                  setTaskMenu(null);
                }}
              >
                <Icon name="trash" /> 删除任务
              </button>
            </div>,
            document.body,
          )
        : null}

      {viewMenu
        ? createPortal(
            <div
              className="menu rail-menu"
              role="menu"
              style={{ left: viewMenu.x, top: viewMenu.y }}
              onClick={(event) => event.stopPropagation()}
            >
              <div className="menu__label">分组方式</div>
              <button
                type="button"
                className={'menu__item' + (grouping === 'folder' ? ' is-on' : '')}
                role="menuitemradio"
                aria-checked={grouping === 'folder'}
                onClick={() => {
                  updateViewPrefs({ grouping: 'folder' });
                  setViewMenu(null);
                }}
              >
                <Icon name="folderopen" /> 按目录
                <span className="check">
                  <Icon name="check" />
                </span>
              </button>
              <button
                type="button"
                className={'menu__item' + (grouping === 'list' ? ' is-on' : '')}
                role="menuitemradio"
                aria-checked={grouping === 'list'}
                onClick={() => {
                  updateViewPrefs({ grouping: 'list' });
                  setViewMenu(null);
                }}
              >
                <Icon name="rows" /> 按列表
                <span className="check">
                  <Icon name="check" />
                </span>
              </button>
              <div className="menu__label">排序方式</div>
              <button
                type="button"
                className={'menu__item' + (sort === 'recent' ? ' is-on' : '')}
                role="menuitemradio"
                aria-checked={sort === 'recent'}
                onClick={() => {
                  updateViewPrefs({ sort: 'recent' });
                  setViewMenu(null);
                }}
              >
                <Icon name="clock" /> 最近更新
                <span className="check">
                  <Icon name="check" />
                </span>
              </button>
              <button
                type="button"
                className={'menu__item' + (sort === 'manual' ? ' is-on' : '')}
                role="menuitemradio"
                aria-checked={sort === 'manual'}
                onClick={() => {
                  updateViewPrefs({ sort: 'manual' });
                  setViewMenu(null);
                }}
              >
                <Icon name="filter" /> 手动排序
                <small>按住任务行拖动调整先后</small>
                <span className="check">
                  <Icon name="check" />
                </span>
              </button>
            </div>,
            document.body,
          )
        : null}

      {renameTarget ? (
        <Dialog
          title="重命名任务"
          size="xs"
          onClose={() => setRenameTarget(null)}
          footer={
            <>
              <Button variant="ghost" type="button" onClick={() => setRenameTarget(null)}>
                取消
              </Button>
              <Button
                variant="primary"
                type="button"
                disabled={!renameValue.trim() || renameValue.trim() === renameTarget.title}
                onClick={() => void handleRenameConfirm()}
              >
                保存
              </Button>
            </>
          }
        >
          <input
            className="rail-rename-input"
            type="text"
            aria-label="任务名称"
            value={renameValue}
            autoFocus
            onChange={(event) => setRenameValue(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === 'Enter') void handleRenameConfirm();
            }}
          />
        </Dialog>
      ) : null}

      {deleteTarget ? (
        <ConfirmDialog
          title="删除任务"
          body={'删除任务「' + deleteTarget.title + '」？该操作会一并清理其消息与用量记录。'}
          confirmLabel="删除"
          onCancel={() => setDeleteTarget(null)}
          onConfirm={() => handleDeleteConfirm()}
        />
      ) : null}

      {dirPickerOpen ? (
        <Dialog title="选择工作目录" onClose={closeDirPicker}>
          <div className="rail-dir">
            <div className="rail-dir__search">
              <Icon name="search" />
              <input
                type="text"
                value={dirQuery}
                placeholder="筛选已有目录…"
                aria-label="筛选已有目录"
                onChange={(event) => setDirQuery(event.target.value)}
              />
            </div>
            <div className="rail-dir__list" role="listbox" aria-label="已有工作目录">
              {dirRows.map((row) => (
                <button
                  key={row.cwd}
                  type="button"
                  role="option"
                  aria-selected="false"
                  className="rail-dir__row"
                  title={row.cwd}
                  onClick={() => chooseDirectory(row.cwd)}
                >
                  <Icon name="folder" />
                  <span className="rail-dir__main">
                    <b>{row.label}</b>
                    <small>{row.cwd}</small>
                  </span>
                </button>
              ))}
              {dirRows.length === 0 ? <div className="rail-dir__empty">没有匹配的目录</div> : null}
            </div>
            <button
              type="button"
              className="rail-dir__browse"
              onClick={() => void browseDirectory()}
            >
              <Icon name="folderopen" />
              <span>从电脑选择…</span>
            </button>
          </div>
        </Dialog>
      ) : null}
    </nav>
  );
}
