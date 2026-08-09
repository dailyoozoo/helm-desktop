import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import type { EngineId } from '@helm/protocol';
import { getSessionHistory, listFolders, listSessions, resumeSession } from './api';
import { deleteSession, renameSession, setSessionPinned } from './api';
import { liveSessionHandle } from '../engine/useSession';
import { Chip } from '../components/Chip';
import { ConfirmDialog } from '../components/ConfirmDialog';
import { Dialog } from '../components/Dialog';
import { Icon } from '../shell/icons';
import type { SessionFolder, SessionStatus, SessionSummary } from './sessionTypes';
import { publishResume } from './resumeBridge';
import {
  costText,
  engineLabel,
  filterSessions,
  relativeTimeText,
  sessionStats,
  sortSessions,
  statusLabel,
  tokenText,
  type SessionSortKey,
  type SortDirection,
} from './sessionViewModel';
import './sessions.css';

interface SessionsPageProps {
  onOpenWorkspace: () => void;
  onNewSession: () => void;
}

export function SessionsPage({ onOpenWorkspace, onNewSession }: SessionsPageProps) {
  const [sessions, setSessions] = useState<SessionSummary[]>([]);
  const [folders, setFolders] = useState<SessionFolder[]>([]);
  const [folderId, setFolderId] = useState('all');
  const [query, setQuery] = useState('');
  const [engine, setEngine] = useState<EngineId | 'all'>('all');
  const [status, setStatus] = useState<SessionStatus | 'all'>('all');
  const [sortKey, setSortKey] = useState<SessionSortKey>('recent');
  const [sortDirection, setSortDirection] = useState<SortDirection>('desc');
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
      })
      .catch((err: unknown) => {
        if (!mountedRef.current || requestId !== requestRef.current) return;
        setError(err instanceof Error ? err.message : '无法读取会话历史');
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
    return sortSessions(
      folderId === 'all' ? filtered : filtered.filter((session) => session.folderId === folderId),
      sortKey,
      sortDirection,
    );
  }, [engine, folderId, query, sessions, sortDirection, sortKey, status]);

  const stats = useMemo(() => sessionStats(sessions), [sessions]);

  const changeSort = (key: SessionSortKey) => {
    if (sortKey === key) {
      setSortDirection((current) => (current === 'desc' ? 'asc' : 'desc'));
    } else {
      setSortKey(key);
      setSortDirection('desc');
    }
  };

  const openSession = async (sessionId: string) => {
    // 恢复期间全表禁点（B1-4）：并发触发第二次 resume 会为同一 CLI 会话拉起双运行时
    if (resumingId) return;
    setResumingId(sessionId);
    try {
      const session = await getSessionHistory(sessionId);
      // 复用存活句柄（可靠性检查 A5）：后台仍在跑的会话不再启动第二个运行时，
      // 否则同一 CLI 会话会出现双进程且旧轮次永久失控。
      const live = liveSessionHandle(sessionId);
      const handleId = live ?? (await resumeSession(sessionId));
      publishResume({ handleId, session });
      onOpenWorkspace();
    } catch (err) {
      setRowErrors((current) => ({
        ...current,
        [sessionId]: err instanceof Error ? err.message : '恢复会话失败',
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
      setRowErrors((current) => ({ ...current, [session.id]: '' }));
    } catch (err) {
      setRowErrors((current) => ({
        ...current,
        [session.id]: err instanceof Error ? err.message : '更新置顶状态失败',
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
      setDeleteTarget(null);
    } catch (err) {
      setRowErrors((current) => ({
        ...current,
        [deleteTarget.id]: err instanceof Error ? err.message : '删除会话失败',
      }));
      setDeleteTarget(null);
    }
  };

  return (
    <main className="main">
      <div className="page scroll">
        <div className="page__head">
          <div>
            <div className="page__title">会话历史</div>
            <div className="page__sub">跨项目、引擎与模型的全部会话，可搜索、可恢复。</div>
          </div>
          <button className="btn btn--primary" type="button" onClick={onNewSession}>
            新建会话
          </button>
        </div>

        <div className="sessions-wrap">
          <div className="sessions-stats">
            <Stat label="会话" value={String(stats.totalSessions)} detail="全部历史" />
            <Stat
              label="活跃引擎"
              value={String(stats.activeEngines)}
              detail="Claude Code · Codex"
            />
            <Stat label="计费 token" value={tokenText(stats.totalTokens)} detail="含缓存重读" />
            <Stat label="花费" value={costText(stats.totalCostUsd)} detail="累计成本" />
          </div>
          <p className="faint" style={{ fontSize: '11.5px', margin: '-12px 0 18px' }}>
            统计为全部会话的累计口径，不随下方筛选变化。
          </p>

          <div className="sessions-filterbar">
            <div className="search sessions-search">
              <Icon name="search" />
              <input
                value={query}
                placeholder="搜索会话、模型或路径..."
                onChange={(event) => setQuery(event.target.value)}
              />
            </div>
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
            <div className="grow" />
            <Chip
              className="sessions-chip"
              active={status === 'all'}
              onClick={() => setStatus('all')}
            >
              全部
            </Chip>
            <Chip
              className="sessions-chip"
              active={status === 'active'}
              onClick={() => setStatus('active')}
            >
              活跃
            </Chip>
            <Chip
              className="sessions-chip"
              active={status === 'idle'}
              onClick={() => setStatus('idle')}
            >
              空闲
            </Chip>
            <Chip
              className="sessions-chip"
              active={status === 'done'}
              onClick={() => setStatus('done')}
            >
              已完成
            </Chip>
          </div>
          <div className="sessions-folder-filters" aria-label="按文件夹筛选">
            <Chip
              className="sessions-chip"
              active={folderId === 'all'}
              onClick={() => setFolderId('all')}
            >
              全部文件夹
            </Chip>
            {folders.map((folder) => (
              <Chip
                key={folder.id}
                active={folderId === folder.id}
                onClick={() => setFolderId(folder.id)}
                title={folder.cwd ?? folder.name}
              >
                <Icon name="folder" /> {folder.name}
              </Chip>
            ))}
          </div>

          {error ? (
            <div className="sessions-error" role="alert">
              <span>{error}</span>
              <button className="btn btn--subtle btn--sm" type="button" onClick={loadSessions}>
                重试
              </button>
            </div>
          ) : null}

          <div className="card sessions-table-card">
            <table className="table sessions-table">
              <thead>
                <tr>
                  <th>会话</th>
                  <th>引擎</th>
                  <th className="sessions-col-hide">模型</th>
                  <th className="sessions-col-hide">
                    <SortButton
                      active={sortKey === 'messages'}
                      direction={sortKey === 'messages' ? sortDirection : 'desc'}
                      onClick={() => changeSort('messages')}
                    >
                      消息
                    </SortButton>
                  </th>
                  <th>
                    <SortButton
                      active={sortKey === 'tokens'}
                      direction={sortKey === 'tokens' ? sortDirection : 'desc'}
                      onClick={() => changeSort('tokens')}
                    >
                      计费 token
                    </SortButton>
                  </th>
                  <th className="sessions-col-hide">
                    <SortButton
                      active={sortKey === 'cost'}
                      direction={sortKey === 'cost' ? sortDirection : 'desc'}
                      onClick={() => changeSort('cost')}
                    >
                      花费
                    </SortButton>
                  </th>
                  <th>
                    <SortButton
                      active={sortKey === 'recent'}
                      direction={sortKey === 'recent' ? sortDirection : 'desc'}
                      onClick={() => changeSort('recent')}
                    >
                      最近活跃
                    </SortButton>
                  </th>
                  <th />
                </tr>
              </thead>
              <tbody>
                {loading ? (
                  <tr>
                    <td colSpan={8} className="sessions-empty" aria-live="polite">
                      正在读取会话历史…
                    </td>
                  </tr>
                ) : visibleSessions.length ? (
                  visibleSessions.map((session) => (
                    <tr className="sessions-row" key={session.id}>
                      <td>
                        <button
                          className="sessions-title sessions-open"
                          type="button"
                          onClick={() => void openSession(session.id)}
                          disabled={resumingId !== null}
                          title={
                            resumingId && resumingId !== session.id
                              ? '正在恢复另一个会话，请稍候'
                              : undefined
                          }
                        >
                          <b>{session.title}</b>
                          <small className="sessions-title-meta">
                            <span>
                              <Icon name="folder" />
                              {folders.find((folder) => folder.id === session.folderId)?.name ??
                                '默认'}
                            </span>
                            <span>{session.cwd || '未设置工作目录'}</span>
                          </small>
                          {rowErrors[session.id] ? (
                            <small className="sessions-row-error" role="alert">
                              {rowErrors[session.id]}
                            </small>
                          ) : null}
                        </button>
                      </td>
                      <td>
                        <span className="row gap-sm">
                          <Icon
                            name={session.engine === 'codex' ? 'cpu' : 'zap'}
                            style={{
                              width: 14,
                              height: 14,
                              color: session.engine === 'codex' ? 'var(--fg-2)' : 'var(--warn)',
                            }}
                          />
                          <span>{engineLabel(session.engine)}</span>
                        </span>
                      </td>
                      <td className="sessions-col-hide mono">{session.model || '默认模型'}</td>
                      <td className="sessions-col-hide mono">{session.messageCount}</td>
                      <td className="mono" title="跨轮累计计费 token，包含缓存读取和缓存写入">
                        {tokenText(session.inputTokens + session.outputTokens)}
                      </td>
                      <td className="sessions-col-hide mono">{costText(session.costUsd)}</td>
                      <td>{relativeTimeText(session.updatedAt)}</td>
                      <td className="sessions-action">
                        <div className="sessions-actions">
                          <button
                            className="btn btn--subtle btn--sm sessions-resume"
                            type="button"
                            disabled={resumingId !== null}
                            title={
                              resumingId && resumingId !== session.id
                                ? '正在恢复另一个会话，请稍候'
                                : undefined
                            }
                            onClick={() => void openSession(session.id)}
                          >
                            {resumingId === session.id ? '恢复中…' : '恢复'}
                          </button>
                          <span className={`sessions-pill is-${session.status}`}>
                            {statusLabel(session.status)}
                          </span>
                          <div className="sessions-menu-wrap">
                            <button
                              className="btn-icon sm"
                              type="button"
                              aria-label={`管理会话 ${session.title}`}
                              aria-expanded={menuSessionId === session.id}
                              onClick={() =>
                                setMenuSessionId((current) =>
                                  current === session.id ? null : session.id,
                                )
                              }
                            >
                              <Icon name="more" />
                            </button>
                            {menuSessionId === session.id ? (
                              <div className="menu sessions-row-menu">
                                <button type="button" onClick={() => void togglePinned(session)}>
                                  <Icon name="flag" /> {session.pinned ? '取消置顶' : '置顶'}
                                </button>
                                <button
                                  type="button"
                                  onClick={() => {
                                    setMenuSessionId(null);
                                    setRenameTarget(session);
                                  }}
                                >
                                  <Icon name="edit" /> 重命名
                                </button>
                                <button
                                  className="is-danger"
                                  type="button"
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
                      </td>
                    </tr>
                  ))
                ) : (
                  <tr>
                    <td colSpan={8} className="sessions-empty">
                      没有符合筛选条件的会话
                      <br />
                      <button
                        className="btn btn--sm"
                        type="button"
                        style={{ marginTop: 10 }}
                        onClick={() => {
                          setQuery('');
                          setEngine('all');
                          setStatus('all');
                          setFolderId('all');
                        }}
                      >
                        清除筛选
                      </button>
                    </td>
                  </tr>
                )}
              </tbody>
            </table>
          </div>
          <p className="sessions-count">
            显示 {visibleSessions.length} / {sessions.length} 个会话
          </p>
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
          title="删除会话"
          body={`确定删除「${deleteTarget.title}」吗？消息、用量与检查点将一并删除，此操作不可撤销。`}
          confirmLabel="删除"
          onCancel={() => setDeleteTarget(null)}
          onConfirm={confirmDelete}
        />
      ) : null}
    </main>
  );
}

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
      title="重命名会话"
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
        aria-label="会话标题"
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

function Stat({ label, value, detail }: { label: string; value: string; detail: string }) {
  return (
    <div className="card sessions-stat">
      <div className="sessions-stat-label">{label}</div>
      <div className="sessions-stat-value">{value}</div>
      <div className="sessions-stat-detail">{detail}</div>
    </div>
  );
}

function SortButton({
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
      className={'sessions-sort' + (active ? ' is-active' : '')}
      type="button"
      onClick={onClick}
    >
      {children}
      <span style={active && direction === 'asc' ? { transform: 'rotate(180deg)' } : undefined}>
        ↓
      </span>
    </button>
  );
}
