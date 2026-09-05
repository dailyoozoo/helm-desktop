import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import type { EngineId } from '@helm/protocol';
import { getSessionHistory, listFolders, listSessions, resumeSession } from './api';
import {
  deleteSession,
  renameSession,
  setFolderCollapsed,
  setSessionArchived,
  setSessionPinned,
} from './api';
import { liveSessionHandle } from '../engine/useSession';
import { Chip } from '../components/Chip';
import { ConfirmDialog } from '../components/ConfirmDialog';
import { Dialog } from '../components/Dialog';
import { Icon } from '../shell/icons';
import { EngineBrand } from '../shell/EngineBrand';
import type { SessionFolder, SessionSummary } from './sessionTypes';
import type { SessionStatusFilter } from './sessionViewModel';
import { discardHistoryPreview, publishHistoryOnly, publishResume } from './resumeBridge';
import { sidebarStatusCounts } from '../workspace/SessionSidebar';
import {
  changeScaleText,
  costText,
  currentActionText,
  derivedStatusKey,
  derivedStatusLabelForSession,
  engineLabel,
  filterSessions,
  relativeTimeText,
  sessionStats,
  sortSessions,
  tokenText,
  type SessionSortKey,
  type SortDirection,
} from './sessionViewModel';
import './sessions.css';

interface SessionsPageProps {
  onOpenWorkspace: () => void;
}

/** 一个项目分组：folder 元信息 + 该分组下的会话列表。 */
interface ProjectGroup {
  folder: SessionFolder | null;
  sessions: SessionSummary[];
}

export function SessionsPage({ onOpenWorkspace }: SessionsPageProps) {
  const [sessions, setSessions] = useState<SessionSummary[]>([]);
  const [folders, setFolders] = useState<SessionFolder[]>([]);
  const [query, setQuery] = useState('');
  const [engine, setEngine] = useState<EngineId | 'all'>('all');
  const [status, setStatus] = useState<SessionStatusFilter>('all');
  const [sortKey, setSortKey] = useState<SessionSortKey>('recent');
  const [sortDirection, setSortDirection] = useState<SortDirection>('desc');
  const [collapsedFolders, setCollapsedFolders] = useState<Set<string>>(new Set());
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [resumingId, setResumingId] = useState<string | null>(null);
  const [menuSessionId, setMenuSessionId] = useState<string | null>(null);
  const [renameTarget, setRenameTarget] = useState<SessionSummary | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<SessionSummary | null>(null);
  const [rowErrors, setRowErrors] = useState<Record<string, string>>({});
  const requestRef = useRef(0);
  const mountedRef = useRef(true);

  const loadSessions = useCallback(() => {
    setLoading(true);
    setError(null);
    const requestId = ++requestRef.current;
    Promise.all([listSessions(), listFolders()])
      .then(([next, nextFolders]) => {
        if (!mountedRef.current || requestId !== requestRef.current) return;
        setSessions(next);
        setFolders(nextFolders);
        // 同步后端 collapsed 状态到本地 Set
        setCollapsedFolders(new Set(nextFolders.filter((f) => f.collapsed).map((f) => f.id)));
      })
      .catch((err: unknown) => {
        if (!mountedRef.current || requestId !== requestRef.current) return;
        setError(err instanceof Error ? err.message : '无法读取任务历史');
      })
      .finally(() => {
        if (mountedRef.current && requestId === requestRef.current) setLoading(false);
      });
  }, []);

  useEffect(() => {
    mountedRef.current = true;
    void loadSessions();
    return () => {
      mountedRef.current = false;
    };
  }, [loadSessions]);

  useEffect(() => {
    if (!menuSessionId) return;
    const close = (event: MouseEvent) => {
      if (!(event.target instanceof Element) || !event.target.closest('.sessions-menu-wrap')) {
        setMenuSessionId(null);
      }
    };
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') setMenuSessionId(null);
    };
    document.addEventListener('mousedown', close);
    document.addEventListener('keydown', onKeyDown);
    return () => {
      document.removeEventListener('mousedown', close);
      document.removeEventListener('keydown', onKeyDown);
    };
  }, [menuSessionId]);

  const visibleSessions = useMemo(() => {
    const filtered = filterSessions(sessions, { query, engine, status });
    return sortSessions(filtered, sortKey, sortDirection);
  }, [engine, query, sessions, sortDirection, sortKey, status]);

  const stats = useMemo(() => sessionStats(sessions), [sessions]);

  const statusCounts = useMemo(() => sidebarStatusCounts(sessions), [sessions]);

  // 按 folderId 分组——没有 folderId 的会话归入"其他"分组
  const projectGroups = useMemo<ProjectGroup[]>(() => {
    const byFolder = new Map<string, SessionSummary[]>();
    const noFolder: SessionSummary[] = [];
    for (const session of visibleSessions) {
      const fid = session.folderId;
      if (!fid) {
        noFolder.push(session);
      } else {
        const list = byFolder.get(fid);
        if (list) list.push(session);
        else byFolder.set(fid, [session]);
      }
    }
    const groups: ProjectGroup[] = folders.map((folder) => ({
      folder,
      sessions: byFolder.get(folder.id) ?? [],
    }));
    // 有会话但无对应 folder 记录的，也作为分组展示
    for (const [fid, list] of byFolder) {
      if (!folders.some((f) => f.id === fid)) {
        groups.push({
          folder: {
            id: fid,
            name: fid,
            sortOrder: 999,
            collapsed: false,
            locked: false,
            createdAt: 0,
          },
          sessions: list,
        });
      }
    }
    // 有会话但没有 folderId 的
    if (noFolder.length) {
      groups.push({ folder: null, sessions: noFolder });
    }
    return groups.filter((g) => g.sessions.length > 0);
  }, [folders, visibleSessions]);

  const changeSort = (key: SessionSortKey) => {
    if (sortKey === key) {
      setSortDirection((current) => (current === 'desc' ? 'asc' : 'desc'));
    } else {
      setSortKey(key);
      setSortDirection('desc');
    }
  };

  const clearRowError = useCallback((sessionId: string) => {
    setRowErrors((current) => {
      if (!current[sessionId]) return current;
      const next = { ...current };
      delete next[sessionId];
      return next;
    });
  }, []);

  const openSession = async (sessionId: string) => {
    if (resumingId) return;
    setResumingId(sessionId);
    try {
      const session = await getSessionHistory(sessionId);
      const live = liveSessionHandle(sessionId);
      if (live) {
        publishResume({ handleId: live, session });
        onOpenWorkspace();
        return;
      }
      // B 方案历史先行：先跳工作区并渲染线程，CLI 在后台重建；
      // 重建失败回滚先行渲染并把错误留在本页行内。
      publishHistoryOnly({ session });
      onOpenWorkspace();
      try {
        const handleId = await resumeSession(sessionId);
        publishResume({ handleId, session });
      } catch (err) {
        discardHistoryPreview(sessionId);
        setRowErrors((current) => ({
          ...current,
          [sessionId]: err instanceof Error ? err.message : '恢复任务失败',
        }));
      }
    } catch (err) {
      setRowErrors((current) => ({
        ...current,
        [sessionId]: err instanceof Error ? err.message : '恢复任务失败',
      }));
    } finally {
      setResumingId(null);
    }
  };

  const togglePinned = async (session: SessionSummary) => {
    setMenuSessionId(null);
    try {
      await setSessionPinned(session.id, !session.pinned);
      setSessions((current) =>
        current.map((item) =>
          item.id === session.id ? { ...item, pinned: !session.pinned } : item,
        ),
      );
      clearRowError(session.id);
    } catch (err) {
      setRowErrors((current) => ({
        ...current,
        [session.id]: err instanceof Error ? err.message : '更新置顶状态失败',
      }));
    }
  };

  const toggleArchived = async (session: SessionSummary) => {
    setMenuSessionId(null);
    const next = !session.archived;
    try {
      await setSessionArchived(session.id, next);
      setSessions((current) =>
        current.map((item) => (item.id === session.id ? { ...item, archived: next } : item)),
      );
      clearRowError(session.id);
    } catch (err) {
      setRowErrors((current) => ({
        ...current,
        [session.id]: err instanceof Error ? err.message : '更新归档状态失败',
      }));
    }
  };

  const confirmRename = async (title: string) => {
    if (!renameTarget) return;
    try {
      await renameSession(renameTarget.id, title);
      setSessions((current) =>
        current.map((item) => (item.id === renameTarget.id ? { ...item, title } : item)),
      );
      clearRowError(renameTarget.id);
      setRenameTarget(null);
    } catch (err) {
      setRowErrors((current) => ({
        ...current,
        [renameTarget.id]: err instanceof Error ? err.message : '重命名失败',
      }));
      setRenameTarget(null);
    }
  };

  const confirmDelete = async () => {
    if (!deleteTarget) return;
    try {
      await deleteSession(deleteTarget.id);
      setSessions((current) => current.filter((item) => item.id !== deleteTarget.id));
      clearRowError(deleteTarget.id);
      setDeleteTarget(null);
    } catch (err) {
      setRowErrors((current) => ({
        ...current,
        [deleteTarget.id]: err instanceof Error ? err.message : '删除任务失败',
      }));
      setDeleteTarget(null);
    }
  };

  const toggleFolder = useCallback(
    (folderId: string) => {
      const next = new Set(collapsedFolders);
      if (next.has(folderId)) next.delete(folderId);
      else next.add(folderId);
      setCollapsedFolders(next);
      // 同步到后端（fire-and-forget）
      void setFolderCollapsed(folderId, next.has(folderId));
    },
    [collapsedFolders],
  );

  const sortChips: { key: SessionSortKey; label: string }[] = [
    { key: 'recent', label: '最近活跃' },
    { key: 'messages', label: '消息' },
    { key: 'tokens', label: 'Token' },
    { key: 'change', label: '变更' },
    { key: 'cost', label: '花费' },
  ];

  const statusChips: { key: SessionStatusFilter; label: string; count: number }[] = [
    { key: 'all', label: '全部', count: statusCounts.all },
    { key: 'waiting_approval', label: '等审批', count: statusCounts.waiting_approval },
    { key: 'running', label: '运行中', count: statusCounts.running },
    { key: 'failed', label: '失败', count: statusCounts.failed },
    { key: 'archived', label: '已归档', count: statusCounts.archived },
  ];

  return (
    <main className="main">
      <div className="page scroll">
        <div className="sessions-page">
          {/* ── 工具栏：搜索 + 状态筛选 + 引擎筛选 + 排序 ── */}
          <div className="sessions-toolbar">
            <div className="sessions-toolbar-row">
              <div className="search sessions-search">
                <Icon name="search" />
                <input
                  value={query}
                  placeholder="搜索任务、模型或路径..."
                  onChange={(event) => setQuery(event.target.value)}
                />
              </div>
              <div className="grow" />
              <div className="sessions-sort-group">
                {sortChips.map((chip) => (
                  <SortChip
                    key={chip.key}
                    active={sortKey === chip.key}
                    direction={sortKey === chip.key ? sortDirection : 'desc'}
                    onClick={() => changeSort(chip.key)}
                  >
                    {chip.label}
                  </SortChip>
                ))}
              </div>
            </div>
            <div className="sessions-toolbar-row">
              <div className="sessions-filter-group">
                {statusChips.map((chip) => (
                  <Chip
                    key={chip.key}
                    className="sessions-chip"
                    active={status === chip.key}
                    onClick={() => setStatus(chip.key)}
                  >
                    {chip.label}
                    {chip.count > 0 ? (
                      <span className="sessions-chip-count">{chip.count}</span>
                    ) : null}
                  </Chip>
                ))}
              </div>
              <div className="grow" />
              <div className="sessions-filter-group">
                <Chip
                  className="sessions-chip"
                  active={engine === 'all'}
                  onClick={() => setEngine('all')}
                >
                  全部引擎
                </Chip>
                <Chip
                  className="sessions-chip"
                  active={engine === 'claude-code'}
                  onClick={() => setEngine('claude-code')}
                >
                  Claude Code
                </Chip>
                <Chip
                  className="sessions-chip"
                  active={engine === 'codex'}
                  onClick={() => setEngine('codex')}
                >
                  Codex
                </Chip>
              </div>
            </div>
          </div>

          {/* ── 摘要条 ── */}
          <div className="sessions-summary-bar">
            <span>
              {visibleSessions.length} / {sessions.length} 个任务
            </span>
            <span className="sessions-summary-dot" />
            <span>{statusCounts.running} 运行中</span>
            <span className="sessions-summary-dot" />
            <span>{statusCounts.waiting_approval} 待审批</span>
            <span className="sessions-summary-dot" />
            <span>{tokenText(stats.totalTokens)} token</span>
            <span className="sessions-summary-dot" />
            <span>{costText(stats.totalCostUsd)}</span>
          </div>

          {error ? (
            <div className="sessions-error" role="alert">
              <span>{error}</span>
              <button className="btn btn--subtle btn--sm" type="button" onClick={loadSessions}>
                重试
              </button>
            </div>
          ) : null}

          {/* ── 按项目分组的任务列表 ── */}
          <div className="sessions-groups">
            {loading ? (
              <div className="sessions-loading" aria-live="polite">
                正在读取任务历史…
              </div>
            ) : projectGroups.length ? (
              projectGroups.map((group) => {
                const folderId = group.folder?.id ?? '__nofolder';
                const collapsed = collapsedFolders.has(folderId);
                const folderName = group.folder?.name ?? '其他';
                const folderCwd = group.folder?.cwd ?? null;
                return (
                  <section className="sessions-group" key={folderId}>
                    <div className="sessions-group__head">
                      <button
                        className="sessions-group__toggle"
                        type="button"
                        aria-expanded={!collapsed}
                        title={folderCwd ?? folderName}
                        onClick={() => toggleFolder(folderId)}
                      >
                        <Icon name={collapsed ? 'right' : 'down'} />
                        <Icon name="folder" />
                        <span className="sessions-group__name">{folderName}</span>
                        <small className="sessions-group__count">{group.sessions.length}</small>
                      </button>
                      <div className="sessions-group__meta">
                        {folderCwd ? (
                          <span className="mono sessions-group__cwd">{folderCwd}</span>
                        ) : null}
                        <span>
                          最近活动{' '}
                          {relativeTimeText(Math.max(...group.sessions.map((s) => s.updatedAt)))}
                        </span>
                      </div>
                    </div>
                    {!collapsed ? (
                      <div className="sessions-group__body">
                        {group.sessions.map((session) => (
                          <TaskCard
                            key={session.id}
                            session={session}
                            resuming={resumingId !== null}
                            resumingThis={resumingId === session.id}
                            menuOpen={menuSessionId === session.id}
                            rowError={rowErrors[session.id]}
                            onOpen={() => void openSession(session.id)}
                            onMenu={(open) =>
                              setMenuSessionId((cur) =>
                                open ? session.id : cur === session.id ? null : cur,
                              )
                            }
                            onTogglePinned={() => void togglePinned(session)}
                            onToggleArchived={() => void toggleArchived(session)}
                            onRename={() => setRenameTarget(session)}
                            onDelete={() => setDeleteTarget(session)}
                          />
                        ))}
                      </div>
                    ) : null}
                  </section>
                );
              })
            ) : (
              <div className="sessions-empty">
                <div className="empty--cta">
                  <div className="empty__ic">
                    <Icon name="search" />
                  </div>
                  <b>没有符合条件的任务</b>
                  <p>调整搜索词或筛选条件后再试。</p>
                  <button
                    className="btn btn--sm"
                    type="button"
                    onClick={() => {
                      setQuery('');
                      setEngine('all');
                      setStatus('all');
                    }}
                  >
                    清除筛选
                  </button>
                </div>
              </div>
            )}
          </div>
        </div>
      </div>
      {renameTarget ? (
        <RenameDialog
          initialValue={renameTarget.title}
          onCancel={() => setRenameTarget(null)}
          onConfirm={confirmRename}
        />
      ) : null}
      {deleteTarget ? (
        <ConfirmDialog
          title="删除任务"
          body={`确定删除「${deleteTarget.title}」吗？消息、用量与检查点将一并删除，此操作不可撤销。`}
          confirmLabel="删除"
          onCancel={() => setDeleteTarget(null)}
          onConfirm={confirmDelete}
        />
      ) : null}
    </main>
  );
}

// ── 任务卡 ──

interface TaskCardProps {
  session: SessionSummary;
  resuming: boolean;
  resumingThis: boolean;
  menuOpen: boolean;
  rowError?: string;
  onOpen: () => void;
  onMenu: (open: boolean) => void;
  onTogglePinned: () => void;
  onToggleArchived: () => void;
  onRename: () => void;
  onDelete: () => void;
}

const TaskCard = function TaskCard({
  session,
  resuming,
  resumingThis,
  menuOpen,
  rowError,
  onOpen,
  onMenu,
  onTogglePinned,
  onToggleArchived,
  onRename,
  onDelete,
}: TaskCardProps) {
  const action = currentActionText(session);
  const changes = changeScaleText(session);
  const statusKey = derivedStatusKey(session);
  const statusLabel = derivedStatusLabelForSession(session);

  return (
    <div className={'tcard' + (session.archived ? ' is-archived' : '')}>
      <button
        className="tcard__main"
        type="button"
        onClick={onOpen}
        disabled={resuming}
        title={resuming && !resumingThis ? '正在恢复另一个任务，请稍候' : undefined}
      >
        <div className="tcard__title-row">
          {session.pinned ? (
            <Icon
              name="flag"
              className="tcard__pin"
              style={{ width: 12, height: 12, color: 'var(--accent-hi)' }}
            />
          ) : null}
          <b className="tcard__title">{session.title}</b>
          <span className={`sessions-pill is-${statusKey}`}>{statusLabel}</span>
        </div>
        <div className="tcard__sub">
          {action ? (
            <span className="tcard__action">
              <Icon name="zap" />
              {action}
            </span>
          ) : session.summary ? (
            <span className="tcard__summary">{session.summary}</span>
          ) : null}
        </div>
        <div className="tcard__meta">
          <span className="tcard__engine">
            <EngineBrand engine={session.engine} size={12} />
            {engineLabel(session.engine)}
          </span>
          {session.model ? <span className="mono tcard__model">{session.model}</span> : null}
          {changes ? <span className="mono tcard__changes">{changes}</span> : null}
          <span className="tcard__tokens" title="跨轮累计计费 token">
            {tokenText(session.inputTokens + session.outputTokens)}
          </span>
          <span className="tcard__cost">{costText(session.costUsd)}</span>
          <span className="tcard__time">{relativeTimeText(session.updatedAt)}</span>
        </div>
        {rowError ? (
          <div className="tcard__error" role="alert">
            {rowError}
          </div>
        ) : null}
      </button>
      <div className="tcard__actions">
        <button
          className="btn btn--subtle btn--sm"
          type="button"
          disabled={resuming}
          title={resuming && !resumingThis ? '正在恢复另一个任务，请稍候' : undefined}
          onClick={onOpen}
        >
          {resumingThis ? '恢复中…' : '恢复'}
        </button>
        <div className="sessions-menu-wrap">
          <button
            className="btn-icon sm"
            type="button"
            aria-label={`管理任务 ${session.title}`}
            aria-expanded={menuOpen}
            onClick={() => onMenu(!menuOpen)}
          >
            <Icon name="more" />
          </button>
          {menuOpen ? (
            <div className="menu sessions-row-menu">
              <button type="button" onClick={onTogglePinned}>
                <Icon name="flag" /> {session.pinned ? '取消置顶' : '置顶'}
              </button>
              <button type="button" onClick={onToggleArchived}>
                <Icon name="folderopen" /> {session.archived ? '取消归档' : '归档'}
              </button>
              <button type="button" onClick={onRename}>
                <Icon name="edit" /> 重命名
              </button>
              <button className="is-danger" type="button" onClick={onDelete}>
                <Icon name="x" /> 删除
              </button>
            </div>
          ) : null}
        </div>
      </div>
    </div>
  );
};

// ── 子组件 ──

function RenameDialog({
  initialValue,
  onCancel,
  onConfirm,
}: {
  initialValue: string;
  onCancel: () => void;
  onConfirm: (title: string) => Promise<void>;
}) {
  const [value, setValue] = useState(initialValue);
  return (
    <Dialog
      title="重命名任务"
      size="xs"
      onClose={onCancel}
      footer={
        <>
          <button className="btn btn--sm" type="button" onClick={onCancel}>
            取消
          </button>
          <button
            className="btn btn--primary btn--sm"
            type="button"
            disabled={!value.trim()}
            onClick={() => void onConfirm(value.trim())}
          >
            保存
          </button>
        </>
      }
    >
      <input
        className="input"
        aria-label="任务标题"
        value={value}
        onChange={(event) => setValue(event.target.value)}
        onKeyDown={(event) => {
          if (!event.nativeEvent.isComposing && event.key === 'Enter' && value.trim())
            void onConfirm(value.trim());
        }}
      />
    </Dialog>
  );
}

function SortChip({
  active,
  direction = 'desc',
  children,
  onClick,
}: {
  active: boolean;
  direction?: SortDirection;
  children: React.ReactNode;
  onClick: () => void;
}) {
  return (
    <button
      className={'sessions-sort-chip' + (active ? ' is-active' : '')}
      type="button"
      onClick={onClick}
    >
      {children}
      {active ? (
        <Icon
          name="down"
          style={{
            width: 11,
            height: 11,
            transform: direction === 'asc' ? 'rotate(180deg)' : undefined,
          }}
        />
      ) : null}
    </button>
  );
}
