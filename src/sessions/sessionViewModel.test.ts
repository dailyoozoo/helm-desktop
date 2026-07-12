import { describe, expect, it } from 'vitest';
import type { SessionSummary } from './sessionTypes';
import {
  filterSessions,
  sessionStats,
  sortSessions,
  statusLabel,
  tokenText,
} from './sessionViewModel';

const sessions: SessionSummary[] = [
  {
    id: 's1',
    title: '修复 auth token 刷新',
    engine: 'claude-code',
    model: 'claude-sonnet-4.6',
    cwd: 'D:/work/acme-web',
    status: 'active',
    messageCount: 4,
    inputTokens: 1200,
    outputTokens: 300,
    costUsd: 0.08,
    createdAt: 1000,
    updatedAt: 3000,
    cliSessionId: 'claude-real-1',
  },
  {
    id: 's2',
    title: '重构 ETL 聚合阶段',
    engine: 'codex',
    model: 'gpt-5-codex',
    cwd: 'D:/work/data-pipeline',
    status: 'done',
    messageCount: 2,
    inputTokens: 20_000,
    outputTokens: 8000,
    costUsd: 0.71,
    createdAt: 900,
    updatedAt: 2000,
    cliSessionId: null,
  },
];

describe('session view model', () => {
  it('filters by search text, engine and status', () => {
    expect(filterSessions(sessions, { query: 'auth', engine: 'all', status: 'all' })).toEqual([
      sessions[0],
    ]);
    expect(filterSessions(sessions, { query: '', engine: 'codex', status: 'done' })).toEqual([
      sessions[1],
    ]);
  });

  it('sorts sessions by recent activity and token usage', () => {
    expect(sortSessions(sessions, 'recent', 'desc').map((session) => session.id)).toEqual([
      's1',
      's2',
    ]);
    expect(sortSessions(sessions, 'tokens', 'desc').map((session) => session.id)).toEqual([
      's2',
      's1',
    ]);
  });

  it('builds aggregate stats and compact labels', () => {
    expect(sessionStats(sessions)).toEqual({
      totalSessions: 2,
      activeEngines: 2,
      totalTokens: 29_500,
      totalCostUsd: 0.79,
    });
    expect(statusLabel('active')).toBe('活跃');
    expect(tokenText(1500)).toBe('1.5K');
  });
});
