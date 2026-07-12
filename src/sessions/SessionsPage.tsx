import { useEffect, useMemo, useState } from 'react';
import type { EngineId } from '@helm/protocol';
import { getSessionHistory, listSessions, resumeSession } from './api';
import { liveSessionHandle } from '../engine/useSession';
import type { SessionStatus, SessionSummary } from './sessionTypes';
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
  const [query, setQuery] = useState('');
  const [engine, setEngine] = useState<EngineId | 'all'>('all');
  const [status, setStatus] = useState<SessionStatus | 'all'>('all');
  const [sortKey, setSortKey] = useState<SessionSortKey>('recent');
  const [sortDirection, setSortDirection] = useState<SortDirection>('desc');
  const [error, setError] = useState<string | null>(null);
  const [resumingId, setResumingId] = useState<string | null>(null);

  useEffect(() => {
    let active = true;
    listSessions()
      .then((next) => {
        if (!active) return;
        setSessions(next);
        setError(null);
      })
      .catch((err: unknown) => {
        if (!active) return;
        setError(err instanceof Error ? err.message : '无法读取会话历史');
      });
    return () => {
      active = false;
    };
  }, []);

  const visibleSessions = useMemo(() => {
    const filtered = filterSessions(sessions, { query, engine, status });
    return sortSessions(filtered, sortKey, sortDirection);
  }, [engine, query, sessions, sortDirection, sortKey, status]);

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
      setError(err instanceof Error ? err.message : '恢复会话失败');
    } finally {
      setResumingId(null);
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
            <Stat label="Token" value={tokenText(stats.totalTokens)} detail="累计用量" />
            <Stat label="花费" value={costText(stats.totalCostUsd)} detail="累计成本" />
          </div>

          <div className="sessions-filterbar">
            <div className="search sessions-search">
              <span>⌕</span>
              <input
                value={query}
                placeholder="搜索会话、模型或路径..."
                onChange={(event) => setQuery(event.target.value)}
              />
            </div>
            <Chip active={engine === 'all'} onClick={() => setEngine('all')}>
              全部引擎
            </Chip>
            <Chip active={engine === 'claude-code'} onClick={() => setEngine('claude-code')}>
              Claude Code
            </Chip>
            <Chip active={engine === 'codex'} onClick={() => setEngine('codex')}>
              Codex
            </Chip>
            <div className="grow" />
            <Chip active={status === 'all'} onClick={() => setStatus('all')}>
              全部
            </Chip>
            <Chip active={status === 'active'} onClick={() => setStatus('active')}>
              活跃
            </Chip>
            <Chip active={status === 'idle'} onClick={() => setStatus('idle')}>
              空闲
            </Chip>
            <Chip active={status === 'done'} onClick={() => setStatus('done')}>
              已完成
            </Chip>
          </div>

          {error ? <div className="sessions-error">{error}</div> : null}

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
                      onClick={() => changeSort('messages')}
                    >
                      消息
                    </SortButton>
                  </th>
                  <th>
                    <SortButton active={sortKey === 'tokens'} onClick={() => changeSort('tokens')}>
                      Token
                    </SortButton>
                  </th>
                  <th className="sessions-col-hide">
                    <SortButton active={sortKey === 'cost'} onClick={() => changeSort('cost')}>
                      花费
                    </SortButton>
                  </th>
                  <th>
                    <SortButton active={sortKey === 'recent'} onClick={() => changeSort('recent')}>
                      最近活跃
                    </SortButton>
                  </th>
                  <th />
                </tr>
              </thead>
              <tbody>
                {visibleSessions.length ? (
                  visibleSessions.map((session) => (
                    <tr className="sessions-row" key={session.id}>
                      <td>
                        <button
                          className="sessions-title sessions-open"
                          type="button"
                          onClick={() => void openSession(session.id)}
                          disabled={resumingId === session.id}
                        >
                          <b>{session.title}</b>
                          <small>{session.cwd || '未设置工作目录'}</small>
                        </button>
                      </td>
                      <td>{engineLabel(session.engine)}</td>
                      <td className="sessions-col-hide mono">{session.model || '默认模型'}</td>
                      <td className="sessions-col-hide mono">{session.messageCount}</td>
                      <td className="mono">
                        {tokenText(session.inputTokens + session.outputTokens)}
                      </td>
                      <td className="sessions-col-hide mono">{costText(session.costUsd)}</td>
                      <td>{relativeTimeText(session.updatedAt)}</td>
                      <td className="sessions-action">
                        <span className={`sessions-pill is-${session.status}`}>
                          {resumingId === session.id ? '恢复中' : statusLabel(session.status)}
                        </span>
                      </td>
                    </tr>
                  ))
                ) : (
                  <tr>
                    <td colSpan={8} className="sessions-empty">
                      暂无符合条件的会话
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
    </main>
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

function Chip({
  active,
  children,
  onClick,
}: {
  active: boolean;
  children: React.ReactNode;
  onClick: () => void;
}) {
  return (
    <button
      className={'sessions-chip' + (active ? ' is-active' : '')}
      type="button"
      onClick={onClick}
    >
      {children}
    </button>
  );
}

function SortButton({
  active,
  children,
  onClick,
}: {
  active: boolean;
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
      <span>↓</span>
    </button>
  );
}
