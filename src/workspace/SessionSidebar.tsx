import { useEffect, useMemo, useState } from 'react';
import { Icon } from '../shell/icons';
import type { SessionState } from '../engine/useSession';
import type { EngineId } from '@helm/protocol';
import type { WorkspaceEngineOption } from './workspaceViewModel';
import type { SessionSummary } from '../sessions/sessionTypes';
import {
  engineLabel as sessionEngineLabel,
  filterSessions,
  groupSessionsByTime,
  relativeTimeText,
  tokenText,
} from '../sessions/sessionViewModel';

interface SessionMenuState {
  sessionId: string;
  x: number;
  y: number;
  pinned: boolean;
}

export function SessionSidebar({
  state,
  engineOptions,
  activeOption,
  sessions,
  sessionError,
  resumingId,
  runningIds,
  approvalIds = [],
  worktreeEnabled,
  creatingWorktree,
  onNew,
  onNewWorktree,
  onSelectEngine,
  onOpenSession,
  onRenameSession,
  onDeleteSession,
  onTogglePinned,
  isSessionActive,
}: {
  state: SessionState;
  engineOptions: WorkspaceEngineOption[];
  activeOption?: WorkspaceEngineOption;
  sessions: SessionSummary[];
  sessionError: string | null;
  resumingId: string | null;
  /** 仍有存活后端句柄的会话（并行运行中，P3-3） */
  runningIds: string[];
  /** 有待处理审批的会话（变更-12：黄色徽标） */
  approvalIds?: string[];
  worktreeEnabled: boolean;
  creatingWorktree: boolean;
  onNew: () => void;
  onNewWorktree: () => void;
  onSelectEngine: (engine: EngineId) => void;
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
  const groups = useMemo(() => groupSessionsByTime(visibleSessions), [visibleSessions]);

  // 右键菜单：点击任意处 / Esc 关闭
  useEffect(() => {
    if (!menu) return;
    const close = () => setMenu(null);
    const onKey = (event: KeyboardEvent) => {
      if (event.key === 'Escape') setMenu(null);
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
        {session.model ? <span className="mono">{session.model}</span> : null}
        <span className="mono">{tokenText(session.inputTokens + session.outputTokens)}</span>
        {resumingId === session.id ? <span>恢复中</span> : null}
        {approvalIds.includes(session.id) ? (
          <span className="ws-approval-chip">待审批</span>
        ) : runningIds.includes(session.id) && !isSessionActive(session) ? (
          <span className="ws-run-chip">运行中</span>
        ) : null}
      </div>
    </button>
  );

  return (
    <aside className="sbar">
      <div className="sbar__head">
        <div className="menu-wrap" style={{ display: 'block' }}>
          <button
            className="btn btn--primary"
            style={{ width: '100%' }}
            onClick={onNew}
            type="button"
          >
            <Icon name="plus" /> 新建会话
          </button>
          <div className="menu ws-menu ws-menu--wide">
            <div className="menu__label">选择引擎开始新会话</div>
            {engineOptions.map((option) => (
              <button
                key={option.engine.id}
                className="menu__item"
                onClick={() => {
                  onSelectEngine(option.engine.id);
                  onNew();
                }}
                type="button"
              >
                <Icon name={option.engine.id === 'codex' ? 'cpu' : 'zap'} />
                <span className="ws-menu__text">
                  {option.engine.name}
                  <small>
                    {option.provider?.name ?? '未绑定服务商'} ·{' '}
                    {option.binding?.primaryModel ?? '未设置模型'}
                  </small>
                </span>
              </button>
            ))}
            {engineOptions.length === 0 && <div className="ws-menu__empty">还没有可用引擎</div>}
            {worktreeEnabled ? (
              <>
                <div className="menu__label">并行会话</div>
                <button
                  className="menu__item"
                  disabled={creatingWorktree}
                  onClick={onNewWorktree}
                  type="button"
                >
                  <Icon name="gitbranch" />
                  <span className="ws-menu__text">
                    {creatingWorktree ? '正在创建 worktree…' : '在 Git worktree 中新建'}
                    <small>隔离目录并行跑，不影响当前工作区（当前会话继续后台运行）</small>
                  </span>
                </button>
              </>
            ) : null}
          </div>
        </div>
        <div className="search">
          <Icon name="search" />
          <input
            type="text"
            placeholder="搜索会话…"
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
              {state.model && <span className="mono">{state.model}</span>}
            </div>
          </button>
        ) : null}
        {sessions.length ? (
          groups.map((group) => (
            <div key={group.label}>
              <div className="sgroup__label">{group.label}</div>
              {group.sessions.map(renderItem)}
            </div>
          ))
        ) : !started ? (
          <div className="sbar-empty">还没有会话。在右侧输入框发送一条消息即可开始。</div>
        ) : null}
        {sessions.length > 0 && visibleSessions.length === 0 ? (
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
