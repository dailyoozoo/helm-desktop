import { useEffect, useMemo, useState } from 'react';
import { Icon } from '../shell/icons';
import type { SessionState } from '../engine/useSession';
import type { WorkspaceEngineOption } from './workspaceViewModel';
import type { SessionFolder, SessionSummary } from '../sessions/sessionTypes';
import {
  engineLabel as sessionEngineLabel,
  costText,
  filterSessions,
  relativeTimeText,
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
    return !normalizedQuery || folderMatches || items.length ? [{ folder, items }] : [];
  });
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
  isSessionActive: (session: SessionSummary) => boolean;
}) {
  const [query, setQuery] = useState('');
  const [menu, setMenu] = useState<SessionMenuState | null>(null);
  const started = state.sessionId !== null || state.items.length > 0;
  const currentSessionInHistory = sessions.some(isSessionActive);
  const engineLabel =
    activeOption?.engine.name ?? (state.engine === 'codex' ? 'Codex' : 'Claude Code');
  const providerLabel = activeOption?.provider?.name ?? '未绑定服务商';

  const visibleSessions = useMemo(
    () => filterSessions(sessions, { query, engine: 'all', status: 'all' }),
    [query, sessions],
  );
  const folderEntries = useMemo(() => {
    return sidebarFolderEntries(folders, sessions, visibleSessions, query);
  }, [folders, query, sessions, visibleSessions]);

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

  const renderItem = (session: SessionSummary) => (
    <button
      className={'sitem' + (isSessionActive(session) ? ' is-active' : '')}
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
          <Icon name="flag" style={{ width: 12, height: 12, color: 'var(--accent-hi)' }} />
        ) : null}
        <span className="sitem__title">{session.title}</span>
        <span className="sitem__time">{relativeTimeText(session.updatedAt)}</span>
      </div>
      <div className="sitem__sub">
        <Icon name={session.engine === 'codex' ? 'cpu' : 'zap'} style={{ width: 13, height: 13 }} />
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
      <span
        className="btn-icon sm sitem__kebab"
        role="button"
        tabIndex={0}
        title="会话操作"
        aria-label={`会话操作：${session.title}`}
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

  return (
    <aside className="sbar">
      <div className="sbar__head">
        <button
          className="btn btn--primary"
          style={{ width: '100%' }}
          onClick={() => onNew()}
          type="button"
        >
          <Icon name="plus" /> 新建会话
        </button>
        <div className="search">
          <Icon name="search" />
          <input
            type="text"
            placeholder="搜索会话、项目或文件夹…"
            value={query}
            onChange={(event) => setQuery(event.target.value)}
          />
        </div>
      </div>
      <div className="sbar__scroll">
        {sessionError ? <div className="sbar-error">{sessionError}</div> : null}
        {started && !currentSessionInHistory ? (
          <button className="sitem is-active" type="button">
            <div className="sitem__top">
              <span className="sitem__title">未命名会话</span>
            </div>
            <div className="sitem__sub">
              <Icon
                name={state.engine === 'codex' ? 'cpu' : 'zap'}
                style={{ width: 13, height: 13 }}
              />
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
                    <div className="ws-folder__empty">暂无会话</div>
                  )
                ) : null}
              </section>
            );
          })
        ) : !started ? (
          <div className="sbar-empty">还没有会话。在右侧输入框发送一条消息即可开始。</div>
        ) : null}
        {sessions.length > 0 && folderEntries.length === 0 ? (
          <div className="sbar-empty">没有匹配的会话</div>
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
