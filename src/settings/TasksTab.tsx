import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import type { EngineId } from '@helm/protocol';
import { Icon } from '../shell/icons';
import { EngineBrand } from '../shell/EngineBrand';
import { ConfirmDialog } from '../components/ConfirmDialog';
import { Dialog } from '../components/Dialog';
import {
  deleteSession,
  getSessionHistory,
  listSessions,
  renameSession,
  resumeSession,
  setSessionArchived,
} from '../sessions/api';
import { liveSessionHandle } from '../engine/useSession';
import { discardHistoryPreview, publishHistoryOnly, publishResume } from '../sessions/resumeBridge';
import type { SessionSummary } from '../sessions/sessionTypes';
import {
  derivedStatusLabelForSession,
  derivedStatusKey,
  engineLabel,
  filterSessions,
  relativeTimeText,
  sortSessions,
} from '../sessions/sessionViewModel';
import {
  activeTaskFilterTokens,
  DEFAULT_TASK_FILTERS,
  filterTasks,
  listTaskDirectories,
  type TaskFilters,
} from './tasksViewModel';

const STATUS_OPTIONS: { value: TaskFilters['status']; label: string }[] = [
  { value: 'all', label: '全部' },
  { value: 'running', label: '运行中' },
  { value: 'waiting_approval', label: '待处理' },
  { value: 'done', label: '已完成' },
  { value: 'failed', label: '失败' },
  { value: 'archived', label: '已归档' },
];

/** 状态点颜色（对齐原型 settings.html STATUS_DOT）。 */
const STATUS_DOT: Record<string, string> = {
  all: 'transparent',
  running: 'var(--accent)',
  waiting_approval: 'var(--warn)',
  done: 'var(--success)',
  failed: 'var(--danger)',
  archived: 'var(--fg-4)',
};

const ENGINE_OPTIONS: { value: 'all' | EngineId; label: string }[] = [
  { value: 'all', label: '全部引擎' },
  { value: 'claude-code', label: 'Claude Code' },
  { value: 'codex', label: 'Codex' },
];

/** 把派生状态键映射成原型 cm-status-pill 的 class（待处理/运行中/失败/已归档/已完成）。 */
function statusPillClass(session: SessionSummary): string {
  const key = derivedStatusKey(session);
  if (session.archived) return 'cm-status-pill is-ready';
  if (key === 'waiting_approval') return 'cm-status-pill is-warn';
  if (key === 'failed') return 'cm-status-pill is-danger';
  if (key === 'running') return 'cm-status-pill';
  return 'cm-status-pill is-ready';
}

/** 千 token 简写（K）。 */
function formatTokens(n: number): string {
  return n >= 1000 ? (n / 1000).toFixed(1).replace(/\.0$/, '') + 'K' : String(n);
}

/** 美元成本两位小数。 */
function formatCost(n: number): string {
  return '$' + n.toFixed(2);
}

/**
 * 设置页「全部任务」（S8）：跨项目、引擎和状态的搜索筛选列表。
 * 数据全部来自真实 list_sessions；打开/重命名/归档/删除复用既有真实命令。
 */
export function TasksTab({ onOpenWorkspace }: { onOpenWorkspace?: () => void }) {
  const [sessions, setSessions] = useState<SessionSummary[]>([]);
  const [filters, setFilters] = useState<TaskFilters>(DEFAULT_TASK_FILTERS);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [resumingId, setResumingId] = useState<string | null>(null);
  const [menuSessionId, setMenuSessionId] = useState<string | null>(null);
  const [openMenu, setOpenMenu] = useState<'dir' | 'eng' | 'st' | null>(null);
  const [dirQuery, setDirQuery] = useState('');
  const [renameTarget, setRenameTarget] = useState<SessionSummary | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<SessionSummary | null>(null);
  const [rowErrors, setRowErrors] = useState<Record<string, string>>({});
  const requestRef = useRef(0);
  const mountedRef = useRef(true);

  const loadSessions = useCallback(() => {
    setLoading(true);
    setError(null);
    const requestId = ++requestRef.current;
    listSessions()
      .then((next) => {
        if (!mountedRef.current || requestId !== requestRef.current) return;
        setSessions(next);
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
      if (!(event.target instanceof Element) || !event.target.closest('.st-task-menu-wrap')) {
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

  useEffect(() => {
    if (!openMenu) return;
    const close = (event: MouseEvent) => {
      if (!(event.target instanceof Element) || !event.target.closest('.st-dd')) {
        setOpenMenu(null);
      }
    };
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') setOpenMenu(null);
    };
    document.addEventListener('mousedown', close);
    document.addEventListener('keydown', onKeyDown);
    return () => {
      document.removeEventListener('mousedown', close);
      document.removeEventListener('keydown', onKeyDown);
    };
  }, [openMenu]);

  const directories = useMemo(() => listTaskDirectories(sessions), [sessions]);
  const visible = useMemo(
    () => sortSessions(filterTasks(sessions, filters), 'recent', 'desc'),
    [filters, sessions],
  );
  const tokens = useMemo(
    () => activeTaskFilterTokens(filters, directories),
    [directories, filters],
  );

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
        onOpenWorkspace?.();
        return;
      }
      // B 方案历史先行：先跳工作区并渲染线程，CLI 在后台重建；
      // 重建失败回滚先行渲染并把错误留在本页行内。
      publishHistoryOnly({ session });
      onOpenWorkspace?.();
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
      setMenuSessionId(null);
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
    } catch (err) {
      setRowErrors((current) => ({
        ...current,
        [renameTarget.id]: err instanceof Error ? err.message : '重命名失败',
      }));
    } finally {
      setRenameTarget(null);
    }
  };

  const confirmDelete = async () => {
    if (!deleteTarget) return;
    try {
      await deleteSession(deleteTarget.id);
      setSessions((current) => current.filter((item) => item.id !== deleteTarget.id));
      clearRowError(deleteTarget.id);
    } catch (err) {
      setRowErrors((current) => ({
        ...current,
        [deleteTarget.id]: err instanceof Error ? err.message : '删除任务失败',
      }));
    } finally {
      setDeleteTarget(null);
    }
  };

  const setFilter = <K extends keyof TaskFilters>(key: K, value: TaskFilters[K]) =>
    setFilters((current) => ({ ...current, [key]: value }));

  const dirMenuItems = useMemo(() => {
    const all = { cwd: '', label: '全部目录', count: sessions.length };
    return [all, ...directories];
  }, [sessions, directories]);
  const filteredDirItems = useMemo(() => {
    const q = dirQuery.trim().toLowerCase();
    if (!q) return dirMenuItems;
    return dirMenuItems.filter(
      (entry) => entry.label.toLowerCase().includes(q) || entry.cwd.toLowerCase().includes(q),
    );
  }, [dirMenuItems, dirQuery]);
  const dirLabel = filters.directory
    ? (directories.find((entry) => entry.cwd === filters.directory)?.label ?? '目录')
    : '全部';
  const engLabel =
    filters.engine === 'all' ? '全部' : filters.engine === 'codex' ? 'Codex' : 'Claude Code';
  const stLabel = STATUS_OPTIONS.find((option) => option.value === filters.status)?.label ?? '全部';
  const countForStatus = (value: TaskFilters['status']) =>
    value === 'all'
      ? sessions.length
      : filterSessions(sessions, { query: '', engine: 'all', status: value }).length;
  const countForEngine = (value: 'all' | EngineId) =>
    value === 'all'
      ? sessions.length
      : sessions.filter((session) => session.engine === value).length;

  return (
    <section>
      <div className="cm-section">
        <div className="cm-section__head">
          <div>
            <h2>
              <Icon name="inbox" /> 任务列表
            </h2>
            <p>跨项目、引擎和模型的全部任务 — 可搜索、可筛选、按最近活跃排列。</p>
          </div>
        </div>

        <div className="st-find-bar">
          <div className="st-toolbar">
            <label className="cm-search st-search">
              <Icon name="search" />
              <input
                value={filters.query}
                placeholder="搜索任务、项目或路径…"
                aria-label="搜索任务"
                onChange={(event) => setFilter('query', event.target.value)}
              />
            </label>
            <div className="st-dd" data-dd="dir">
              <button
                className={'st-dd__trig' + (openMenu === 'dir' ? ' is-on' : '')}
                type="button"
                aria-haspopup="listbox"
                aria-expanded={openMenu === 'dir'}
                onClick={() => setOpenMenu(openMenu === 'dir' ? null : 'dir')}
              >
                <span className="st-dd__k">目录</span>
                <span className="st-dd__val">{dirLabel}</span>
                <Icon name="chevrondown" />
              </button>
              <div className="st-dd__menu" hidden={openMenu !== 'dir'}>
                <div className="st-dd__search">
                  <Icon name="search" />
                  <input
                    className="st-dd__q"
                    value={dirQuery}
                    placeholder="筛选目录…"
                    aria-label="筛选目录"
                    onChange={(event) => setDirQuery(event.target.value)}
                  />
                </div>
                <div className="st-dd__list">
                  {filteredDirItems.map((entry) => (
                    <button
                      key={entry.cwd}
                      className={'st-dd__item' + (filters.directory === entry.cwd ? ' is-on' : '')}
                      type="button"
                      onClick={() => {
                        setFilter('directory', entry.cwd);
                        setOpenMenu(null);
                      }}
                    >
                      <span className="st-dd__lead">
                        <Icon name={filters.directory === entry.cwd ? 'check' : 'folder'} />
                      </span>
                      <span className="st-dd__name">{entry.label}</span>
                      <span className="st-dd__count">{entry.count}</span>
                    </button>
                  ))}
                </div>
              </div>
            </div>
            <div className="st-dd" data-dd="eng">
              <button
                className={'st-dd__trig' + (openMenu === 'eng' ? ' is-on' : '')}
                type="button"
                aria-haspopup="listbox"
                aria-expanded={openMenu === 'eng'}
                onClick={() => setOpenMenu(openMenu === 'eng' ? null : 'eng')}
              >
                <span className="st-dd__k">引擎</span>
                <span className="st-dd__val">{engLabel}</span>
                <Icon name="chevrondown" />
              </button>
              <div className="st-dd__menu" hidden={openMenu !== 'eng'}>
                <div className="st-dd__list">
                  {ENGINE_OPTIONS.map((option) => (
                    <button
                      key={option.value}
                      className={'st-dd__item' + (filters.engine === option.value ? ' is-on' : '')}
                      type="button"
                      onClick={() => {
                        setFilter('engine', option.value);
                        setOpenMenu(null);
                      }}
                    >
                      <span className="st-dd__lead">
                        {filters.engine === option.value ? (
                          <Icon name="check" />
                        ) : option.value === 'all' ? (
                          <Icon name="layers" />
                        ) : (
                          <EngineBrand engine={option.value} size={13} />
                        )}
                      </span>
                      <span className="st-dd__name">{option.label}</span>
                      <span className="st-dd__count">{countForEngine(option.value)}</span>
                    </button>
                  ))}
                </div>
              </div>
            </div>
            <div className="st-dd" data-dd="st">
              <button
                className={'st-dd__trig' + (openMenu === 'st' ? ' is-on' : '')}
                type="button"
                aria-haspopup="listbox"
                aria-expanded={openMenu === 'st'}
                onClick={() => setOpenMenu(openMenu === 'st' ? null : 'st')}
              >
                <span className="st-dd__k">状态</span>
                <span className="st-dd__val">{stLabel}</span>
                <Icon name="chevrondown" />
              </button>
              <div className="st-dd__menu" hidden={openMenu !== 'st'}>
                <div className="st-dd__list">
                  {STATUS_OPTIONS.map((option) => (
                    <button
                      key={option.value}
                      className={'st-dd__item' + (filters.status === option.value ? ' is-on' : '')}
                      type="button"
                      onClick={() => {
                        setFilter('status', option.value);
                        setOpenMenu(null);
                      }}
                    >
                      <span className="st-dd__lead">
                        <span
                          className="st-dd__dot"
                          style={{
                            background:
                              option.value === 'all' ? 'transparent' : STATUS_DOT[option.value],
                            boxShadow:
                              option.value === 'all' ? 'inset 0 0 0 1.5px var(--fg-4)' : undefined,
                          }}
                        />
                      </span>
                      <span className="st-dd__name">{option.label}</span>
                      <span className="st-dd__count">{countForStatus(option.value)}</span>
                    </button>
                  ))}
                </div>
              </div>
            </div>
          </div>
        </div>

        {tokens.length ? (
          <div className="st-tokens">
            {tokens.map((token) => (
              <button
                key={token.key}
                className="st-token"
                type="button"
                title="点击移除该筛选"
                onClick={() => setFilter(token.key, DEFAULT_TASK_FILTERS[token.key] as never)}
              >
                {token.label}
                <Icon name="x" />
              </button>
            ))}
            <button
              className="st-token st-token--clear"
              type="button"
              onClick={() => setFilters(DEFAULT_TASK_FILTERS)}
            >
              清除全部
            </button>
          </div>
        ) : null}

        {error ? (
          <div className="settings-inline-error" role="alert">
            <span>{error}</span>
            <button className="btn btn--subtle btn--sm" type="button" onClick={loadSessions}>
              重试
            </button>
          </div>
        ) : null}

        <div className="cm-detail-card st-feed-card">
          <div className="st-feed">
            {loading && sessions.length === 0 ? (
              <div className="empty" aria-live="polite">
                正在读取任务历史…
              </div>
            ) : visible.length ? (
              visible.map((session) => (
                <div className="tcard" key={session.id} data-session-id={session.id}>
                  <button
                    className="tcard__main"
                    type="button"
                    disabled={resumingId === session.id}
                    onClick={() => void openSession(session.id)}
                  >
                    <div className="tcard__top">
                      <b className="tcard__title">{session.title}</b>
                    </div>
                    <div className="tcard__sub">
                      <span className="eng">
                        <EngineBrand engine={session.engine} size={12} />
                        {engineLabel(session.engine)}
                      </span>
                      <span className="dot">·</span>
                      <span>{session.model || '—'}</span>
                      <span className="dot">·</span>
                      <span>{session.cwd}</span>
                      <span className="dot">·</span>
                      <span>最近 {relativeTimeText(session.updatedAt)}</span>
                      <span className="dot">·</span>
                      <span className="spend">
                        {formatCost(session.costUsd)} ·{' '}
                        {formatTokens(session.inputTokens + session.outputTokens)}
                      </span>
                    </div>
                    {rowErrors[session.id] ? (
                      <small className="st-task__error" role="alert">
                        {rowErrors[session.id]}
                      </small>
                    ) : null}
                  </button>
                  <span className={statusPillClass(session)}>
                    {derivedStatusLabelForSession(session)}
                  </span>
                  <div className="tcard__actions">
                    <div className="st-task-menu-wrap">
                      <button
                        className="btn-icon sm"
                        type="button"
                        aria-label={session.title + ' 更多操作'}
                        aria-haspopup="menu"
                        aria-expanded={menuSessionId === session.id}
                        onClick={() =>
                          setMenuSessionId(menuSessionId === session.id ? null : session.id)
                        }
                      >
                        <Icon name="more" />
                      </button>
                      {menuSessionId === session.id ? (
                        <div className="st-task-menu" role="menu">
                          <button
                            role="menuitem"
                            type="button"
                            onClick={() => void openSession(session.id)}
                          >
                            <Icon name="play" /> 恢复
                          </button>
                          <button
                            role="menuitem"
                            type="button"
                            onClick={() => {
                              setMenuSessionId(null);
                              setRenameTarget(session);
                            }}
                          >
                            <Icon name="edit" /> 重命名
                          </button>
                          <button
                            role="menuitem"
                            type="button"
                            onClick={() => void toggleArchived(session)}
                          >
                            <Icon name="archive" /> {session.archived ? '恢复任务' : '归档'}
                          </button>
                          <button
                            role="menuitem"
                            type="button"
                            className="is-danger"
                            onClick={() => {
                              setMenuSessionId(null);
                              setDeleteTarget(session);
                            }}
                          >
                            <Icon name="x" /> 删除
                          </button>
                        </div>
                      ) : null}
                    </div>
                  </div>
                </div>
              ))
            ) : (
              <div className="st-empty">
                <b>没有匹配的任务</b>
                <p>调整搜索词或筛选后再试。</p>
                <button
                  className="cm-action"
                  type="button"
                  onClick={() => setFilters(DEFAULT_TASK_FILTERS)}
                >
                  清除筛选
                </button>
              </div>
            )}
          </div>
        </div>
      </div>

      {renameTarget ? (
        <Dialog title="重命名任务" size="xs" onClose={() => setRenameTarget(null)}>
          <RenameForm
            initial={renameTarget.title}
            onSubmit={(title) => void confirmRename(title)}
            onCancel={() => setRenameTarget(null)}
          />
        </Dialog>
      ) : null}
      {deleteTarget ? (
        <ConfirmDialog
          title="删除任务"
          body={'确定删除「' + deleteTarget.title + '」吗？历史记录与用量会一并删除，且无法恢复。'}
          confirmLabel="删除"
          onConfirm={() => void confirmDelete()}
          onCancel={() => setDeleteTarget(null)}
        />
      ) : null}
    </section>
  );
}

function RenameForm({
  initial,
  onSubmit,
  onCancel,
}: {
  initial: string;
  onSubmit: (title: string) => void;
  onCancel: () => void;
}) {
  const [value, setValue] = useState(initial);
  return (
    <form
      className="row gap-sm"
      onSubmit={(event) => {
        event.preventDefault();
        const trimmed = value.trim();
        if (trimmed) onSubmit(trimmed);
      }}
    >
      <input
        className="input grow"
        value={value}
        autoFocus
        aria-label="任务名称"
        onChange={(event) => setValue(event.target.value)}
      />
      <button className="btn btn--subtle btn--sm" type="button" onClick={onCancel}>
        取消
      </button>
      <button className="btn btn--primary btn--sm" type="submit" disabled={!value.trim()}>
        保存
      </button>
    </form>
  );
}
