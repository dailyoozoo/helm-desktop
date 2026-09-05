import { useEffect, useMemo, useState } from 'react';
import { Icon } from '../shell/icons';
import { EngineBrand } from '../shell/EngineBrand';
import type { SessionState } from '../engine/useSession';
import type { WorkspaceEngineOption } from './workspaceViewModel';
import type { SessionFolder, SessionSummary } from '../sessions/sessionTypes';
import {
  engineLabel as sessionEngineLabel,
  changeScaleText,
  costText,
  currentActionText,
  filterSessions,
  relativeTimeText,
  type SessionStatusFilter,
} from '../sessions/sessionViewModel';

interface SessionMenuState {
  sessionId: string;
  x: number;
  y: number;
  pinned: boolean;
}

export function sessionProjectName(cwd: string): string {
  const normalized = cwd.replace(/[\\/]+$/, '');
  return normalized.split(/[\\/]/).filter(Boolean).pop() || cwd || '未设置项目';
}

export function sidebarFolderEntries(
  folders: SessionFolder[],
  sessions: SessionSummary[],
  visibleSessions: SessionSummary[],
  query: string,
): Array<{ folder: SessionFolder; items: SessionSummary[] }> {
  const normalizedQuery = query.trim().toLowerCase();
  return folders.flatMap((folder) => {
    const folderMatches = Boolean(
      normalizedQuery && folder.name.toLowerCase().includes(normalizedQuery),
    );
    const source = folderMatches ? sessions : visibleSessions;
    const items = source.filter((session) => (session.folderId ?? 'folder-default') === folder.id);
    // 新安装时后端仍保留兼容用的 folder-default，但不在空侧栏中展示这个占位项。
    if (folder.id === 'folder-default' && items.length === 0) return [];
    return !normalizedQuery || folderMatches || items.length ? [{ folder, items }] : [];
  });
}

/** 工作区侧栏各状态筛选 chip 的计数（切片C · F1）。
 *  归档会话只计入「已归档」，不计入「全部」；非归档会话按 pendingApproval / active+currentTool / lastTurnFailed 派生。 */
export interface SidebarStatusCounts {
  all: number;
  waiting_approval: number;
  running: number;
  failed: number;
  archived: number;
}

export function sidebarStatusCounts(sessions: SessionSummary[]): SidebarStatusCounts {
  const counts: SidebarStatusCounts = {
    all: 0,
    waiting_approval: 0,
    running: 0,
    failed: 0,
    archived: 0,
  };
  for (const session of sessions) {
    if (session.archived) {
      counts.archived += 1;
      continue;
    }
    counts.all += 1;
    if (session.pendingApproval || session.status === 'waiting_approval')
      counts.waiting_approval += 1;
    if (session.status === 'active' && session.currentTool) counts.running += 1;
    if (session.lastTurnFailed) counts.failed += 1;
  }
  return counts;
}

export function SessionSidebar({
  state,
  activeOption,
  sessions,
  folders,
  sessionError,
  resumingId,
  runningIds,
  approvalIds = [],
  onNew,
  onToggleFolder,
  onOpenSession,
  onRenameSession,
  onDeleteSession,
  onTogglePinned,
  onToggleArchived,
  isSessionActive,
}: {
  state: SessionState;
  activeOption?: WorkspaceEngineOption;
  sessions: SessionSummary[];
  folders: SessionFolder[];
  sessionError: string | null;
  resumingId: string | null;
  /** 仍有存活后端句柄的会话（并行运行中，P3-3） */
  runningIds: string[];
  /** 有待处理审批的会话（变更-12：黄色徽标） */
  approvalIds?: string[];
  onNew: () => void;
  onToggleFolder: (folder: SessionFolder) => void;
  onOpenSession: (sessionId: string) => void;
  onRenameSession?: (session: SessionSummary) => void;
  onDeleteSession?: (session: SessionSummary) => void;
  onTogglePinned?: (session: SessionSummary) => void;
  onToggleArchived?: (session: SessionSummary) => void;
  isSessionActive: (session: SessionSummary) => boolean;
}) {
  const [query, setQuery] = useState('');
  const [statusFilter, setStatusFilter] = useState<SessionStatusFilter>('all');
  const [menu, setMenu] = useState<SessionMenuState | null>(null);
  const started = state.sessionId !== null || state.items.length > 0;
  const currentSessionInHistory = sessions.some(isSessionActive);
  const engineLabel =
    activeOption?.engine.name ?? (state.engine === 'codex' ? 'Codex' : 'Claude Code');
  const providerLabel = activeOption?.provider?.name ?? '未绑定服务商';

  const visibleSessions = useMemo(
    () => filterSessions(sessions, { query, engine: 'all', status: statusFilter }),
    [query, sessions, statusFilter],
  );
  const folderEntries = useMemo(() => {
    return sidebarFolderEntries(folders, sessions, visibleSessions, query);
  }, [folders, query, sessions, visibleSessions]);

  // 各派生状态计数（切片C · F1 工作区侧栏复用会话历史页同一套派生逻辑）
  const statusCounts = useMemo(() => sidebarStatusCounts(sessions), [sessions]);

  // 右键菜单：点击任意处 / Esc 关闭
  useEffect(() => {
    if (!menu) return;
    const close = () => setMenu(null);
    const onKey = (event: KeyboardEvent) => {
      if (event.key === 'Escape') close();
    };
    window.addEventListener('click', close);
    window.addEventListener('contextmenu', close);
    window.addEventListener('keydown', onKey);
    return () => {
      window.removeEventListener('click', close);
      window.removeEventListener('contextmenu', close);
      window.removeEventListener('keydown', onKey);
    };
  }, [menu]);

  const menuSession = menu ? sessions.find((session) => session.id === menu.sessionId) : undefined;

  // hover kebab（B1-5）：与右键共用同一个会话菜单，锚定在 kebab 左下角（原型 r.left / r.bottom+4）
  const openKebabMenu = (session: SessionSummary, anchor: Element) => {
    const rect = anchor.getBoundingClientRect();
    setMenu((current) =>
      current?.sessionId === session.id
        ? null
        : {
            sessionId: session.id,
            x: rect.left,
            y: rect.bottom + 4,
            pinned: Boolean(session.pinned),
          },
    );
  };

  const renderItem = (session: SessionSummary) => {
    const action = currentActionText(session);
    const changes = changeScaleText(session);
    const hasDoRow = Boolean(action || changes);
    return (
      <button
        className={
          'sitem' +
          (isSessionActive(session) ? ' is-active' : '') +
          (session.archived ? ' is-archived' : '')
        }
        disabled={resumingId === session.id}
        key={session.id}
        onClick={() => onOpenSession(session.id)}
        onContextMenu={(event) => {
          event.preventDefault();
          event.stopPropagation();
          setMenu({
            sessionId: session.id,
            x: event.clientX,
            y: event.clientY,
            pinned: Boolean(session.pinned),
          });
        }}
        type="button"
      >
        <div className="sitem__top">
          {session.pinned ? (
            <Icon
              name="flag"
              className="h-3 w-3"
              style={{ width: 12, height: 12, color: 'var(--accent-hi)' }}
            />
          ) : null}
          <span className="sitem__title">{session.title}</span>
          <span className="sitem__time">{relativeTimeText(session.updatedAt)}</span>
        </div>
        <div className="sitem__sub">
          <EngineBrand engine={session.engine} size={13} />
          <span>{sessionEngineLabel(session.engine)}</span>
          <span title={session.cwd}>{sessionProjectName(session.cwd)}</span>
          {session.model ? <span className="mono">{session.model}</span> : null}
          <span className="mono">{costText(session.costUsd)}</span>
          {resumingId === session.id ? <span>恢复中</span> : null}
          {approvalIds.includes(session.id) ? (
            <span className="ws-approval-chip">待审批</span>
          ) : runningIds.includes(session.id) && !isSessionActive(session) ? (
            <span className="ws-run-chip">运行中</span>
          ) : null}
        </div>
        {hasDoRow ? (
          <div className="sitem__do">
            {action ? (
              <>
                <Icon
                  name="dot"
                  className="h-2.5 w-2.5"
                  style={{ width: 11, height: 11, color: 'var(--accent-hi)' }}
                />
                <span className="sitem__do-tx">{action}</span>
              </>
            ) : (
              <span className="sitem__do-tx" />
            )}
            {changes ? (
              <span className="sitem__do-dd">
                <span className="sitem__do-a">{changes.split(' ')[0]}</span>{' '}
                <span className="sitem__do-d">{changes.split(' ')[1]}</span>
              </span>
            ) : null}
          </div>
        ) : null}
        <span
          className="btn-icon sm sitem__kebab"
          role="button"
          tabIndex={0}
          title="任务操作"
          aria-label={`任务操作：${session.title}`}
          aria-haspopup="menu"
          onClick={(event) => {
            event.stopPropagation();
            openKebabMenu(session, event.currentTarget);
          }}
          onKeyDown={(event) => {
            if (event.key !== 'Enter' && event.key !== ' ') return;
            event.preventDefault();
            event.stopPropagation();
            openKebabMenu(session, event.currentTarget);
          }}
        >
          <Icon name="more" />
        </span>
      </button>
    );
  };

  return (
    <aside className="sbar" aria-label="任务列表">
      <div className="sbar__toolbar">
        <span>
          <b>任务</b>
          <small>{statusCounts.all} 个活跃任务</small>
        </span>
        <button
          className="btn-icon sm"
          onClick={() => onNew()}
          type="button"
          title="新建任务"
          aria-label="新建任务"
        >
          <Icon name="plus" />
        </button>
      </div>
      <div className="sbar__head">
        <div className="search">
          <Icon name="search" />
          <input
            type="text"
            placeholder="搜索任务、项目或文件夹…"
            value={query}
            onChange={(event) => setQuery(event.target.value)}
          />
        </div>
        <div className="sfilter" role="tablist" aria-label="按状态筛选任务">
          {(
            [
              { key: 'all', label: '全部', count: statusCounts.all },
              { key: 'waiting_approval', label: '等审批', count: statusCounts.waiting_approval },
              { key: 'running', label: '运行中', count: statusCounts.running },
              { key: 'failed', label: '失败', count: statusCounts.failed },
              { key: 'archived', label: '已归档', count: statusCounts.archived },
            ] as const
          ).map((chip) => (
            <button
              key={chip.key}
              type="button"
              role="tab"
              aria-selected={statusFilter === chip.key}
              className={'fchip' + (statusFilter === chip.key ? ' is-on' : '')}
              onClick={() => setStatusFilter(chip.key)}
            >
              {chip.label}
              {chip.count > 0 ? <span className="fchip__n">{chip.count}</span> : null}
            </button>
          ))}
        </div>
      </div>
      <div className="sbar__scroll">
        {sessionError ? <div className="sbar-error">{sessionError}</div> : null}
        {started && !currentSessionInHistory ? (
          <button className="sitem is-active" type="button">
            <div className="sitem__top">
              <span className="sitem__title">未命名任务</span>
            </div>
            <div className="sitem__sub">
              <EngineBrand engine={state.engine} size={13} />
              <span>
                {engineLabel} · {providerLabel}
              </span>
              <span title={state.cwd}>{sessionProjectName(state.cwd)}</span>
              {state.model && <span className="mono">{state.model}</span>}
            </div>
          </button>
        ) : null}
        {folderEntries.length ? (
          folderEntries.map(({ folder, items }) => {
            const collapsed = folder.collapsed && !query.trim();
            return (
              <section className="ws-folder" key={folder.id}>
                <div className="ws-folder__head">
                  <button
                    className="ws-folder__toggle"
                    type="button"
                    aria-expanded={!collapsed}
                    title={folder.cwd ?? folder.name}
                    onClick={() => onToggleFolder(folder)}
                  >
                    <Icon name={collapsed ? 'right' : 'down'} />
                    <Icon name="folder" />
                    <span>{folder.name}</span>
                    <small>{items.length}</small>
                  </button>
                </div>
                {!collapsed ? (
                  items.length ? (
                    items.map(renderItem)
                  ) : (
                    <div className="ws-folder__empty">暂无任务</div>
                  )
                ) : null}
              </section>
            );
          })
        ) : !started ? (
          <div className="sbar-empty">还没有任务。在右侧输入框发送一条消息即可开始。</div>
        ) : null}
        {sessions.length > 0 && folderEntries.length === 0 ? (
          <div className="sbar-empty">没有匹配的任务</div>
        ) : null}
      </div>
      {menu && menuSession ? (
        <div
          className="ws-ctxmenu menu"
          style={{ left: menu.x, top: menu.y }}
          role="menu"
          onClick={(event) => event.stopPropagation()}
        >
          <button
            type="button"
            className="menu__item"
            role="menuitem"
            onClick={() => {
              setMenu(null);
              onTogglePinned?.(menuSession);
            }}
          >
            <Icon name="flag" /> {menu.pinned ? '取消置顶' : '置顶'}
          </button>
          <div className="menu__sep" />
          <button
            type="button"
            className="menu__item"
            role="menuitem"
            onClick={() => {
              setMenu(null);
              onRenameSession?.(menuSession);
            }}
          >
            <Icon name="edit" /> 重命名
          </button>
          {onToggleArchived ? (
            <button
              type="button"
              className="menu__item"
              role="menuitem"
              onClick={() => {
                setMenu(null);
                onToggleArchived(menuSession);
              }}
            >
              <Icon name="archive" /> {menuSession.archived ? '取消归档' : '归档'}
            </button>
          ) : null}
          <button
            type="button"
            className="menu__item ws-ctxmenu__danger"
            role="menuitem"
            onClick={() => {
              setMenu(null);
              onDeleteSession?.(menuSession);
            }}
          >
            <Icon name="x" /> 删除会话…
          </button>
        </div>
      ) : null}
    </aside>
  );
}
