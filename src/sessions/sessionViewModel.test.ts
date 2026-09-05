import { describe, expect, it } from 'vitest';
import type { SessionSummary } from './sessionTypes';
import {
  changeScaleText,
  currentActionText,
  derivedStatusKey,
  derivedStatusLabelForSession,
  filterSessions,
  groupRecentTasksByCwd,
  listRecentTasks,
  relativeTimeText,
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
    currentTool: 'Write',
    currentTarget: 'auth.ts',
    changeAdditions: 42,
    changeDeletions: 7,
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
    lastTurnFailed: true,
  },
  {
    id: 's3',
    title: '归档的旧会话',
    engine: 'claude-code',
    model: 'claude-sonnet-4.6',
    cwd: 'D:/work/legacy',
    status: 'done',
    messageCount: 1,
    inputTokens: 10,
    outputTokens: 5,
    costUsd: 0,
    createdAt: 500,
    updatedAt: 1500,
    cliSessionId: null,
    archived: true,
  },
  {
    id: 's4',
    title: '等待审批的会话',
    engine: 'codex',
    model: 'gpt-5-codex',
    cwd: 'D:/work/pipeline',
    status: 'waiting_approval',
    messageCount: 3,
    inputTokens: 500,
    outputTokens: 100,
    costUsd: 0.02,
    createdAt: 800,
    updatedAt: 2500,
    cliSessionId: null,
    pendingApproval: true,
  },
];

describe('session view model', () => {
  it('excludes archived sessions and sorts recent tasks', () => {
    const view = listRecentTasks(sessions, 'time');
    expect(view.map((session) => session.id)).toEqual(['s1', 's4', 's2']);
  });

  it('groups recent tasks by canonical cwd and separates same folder names', () => {
    const workspaceA = sessions[0];
    const workspaceB = { ...sessions[1], id: 's2b', cwd: 'E:/team/work/acme-web' };
    const view = groupRecentTasksByCwd([workspaceB, workspaceA], 'time');
    expect(view.map((group) => [group.cwd, group.label])).toEqual([
      ['D:/work/acme-web', 'acme-web'],
      ['E:/team/work/acme-web', 'acme-web'],
    ]);
    expect(view.flatMap((group) => group.sessions.map((session) => session.id))).toEqual([
      's1',
      's2b',
    ]);
  });

  it('sorts recent tasks by time or directory without mutating input', () => {
    const input = [sessions[1], sessions[3], sessions[0]];
    expect(listRecentTasks(input, 'time').map((session) => session.id)).toEqual(['s1', 's4', 's2']);
    const groupedByFolder = groupRecentTasksByCwd(input, 'folder');
    expect(groupedByFolder.map((group) => group.cwd)).toEqual([
      'D:/work/acme-web',
      'D:/work/data-pipeline',
      'D:/work/pipeline',
    ]);
    expect(input.map((session) => session.id)).toEqual(['s2', 's4', 's1']);
  });

  it('keeps pinned sessions before all other recent tasks', () => {
    const pinnedLegacy = { ...sessions[1], id: 'pinned-legacy', pinned: true };
    const view = groupRecentTasksByCwd([sessions[0], pinnedLegacy], 'folder');
    expect(view.flatMap((group) => group.sessions.map((session) => session.id))).toEqual([
      'pinned-legacy',
      's1',
    ]);
  });

  it('filters by search text, engine and status', () => {
    expect(filterSessions(sessions, { query: 'auth', engine: 'all', status: 'all' })).toEqual([
      sessions[0],
    ]);
    expect(filterSessions(sessions, { query: '', engine: 'codex', status: 'done' })).toEqual([
      sessions[1],
    ]);
  });

  it('filters by derived session states (slice7 F1)', () => {
    expect(
      filterSessions(sessions, { query: '', engine: 'all', status: 'running' }).map((s) => s.id),
    ).toEqual(['s1']);
    expect(
      filterSessions(sessions, { query: '', engine: 'all', status: 'failed' }).map((s) => s.id),
    ).toEqual(['s2']);
    expect(
      filterSessions(sessions, { query: '', engine: 'all', status: 'archived' }).map((s) => s.id),
    ).toEqual(['s3']);
    expect(
      filterSessions(sessions, { query: '', engine: 'all', status: 'waiting_approval' }).map(
        (s) => s.id,
      ),
    ).toEqual(['s4']);
  });

  it('formats current action and change scale (slice7 F2)', () => {
    expect(currentActionText(sessions[0])).toBe('写文件 auth.ts');
    expect(currentActionText(sessions[1])).toBeNull();
    expect(changeScaleText(sessions[0])).toBe('+42 -7');
    expect(changeScaleText(sessions[1])).toBeNull();
  });

  it('sorts by change scale', () => {
    expect(sortSessions(sessions, 'change', 'desc').map((s) => s.id)).toEqual([
      's1',
      's2',
      's3',
      's4',
    ]);
    expect(sortSessions(sessions, 'change', 'asc').map((s) => s.id)).toEqual([
      's2',
      's3',
      's4',
      's1',
    ]);
  });

  it('builds aggregate stats and compact labels', () => {
    expect(sessionStats(sessions)).toEqual({
      totalSessions: 4,
      activeEngines: 2,
      totalTokens: 30_115,
      totalCostUsd: 0.81,
    });
    expect(statusLabel('active')).toBe('活跃');
    expect(tokenText(1500)).toBe('1.5K');
  });

  it('derives status key from session fields', () => {
    // s1: active + currentTool → running
    expect(derivedStatusKey(sessions[0])).toBe('running');
    expect(derivedStatusLabelForSession(sessions[0])).toBe('运行中');
    // s2: done + lastTurnFailed → failed
    expect(derivedStatusKey(sessions[1])).toBe('failed');
    expect(derivedStatusLabelForSession(sessions[1])).toBe('失败');
    // s3: archived → archived
    expect(derivedStatusKey(sessions[2])).toBe('archived');
    expect(derivedStatusLabelForSession(sessions[2])).toBe('已归档');
    // s4: waiting_approval → waiting_approval
    expect(derivedStatusKey(sessions[3])).toBe('waiting_approval');
    expect(derivedStatusLabelForSession(sessions[3])).toBe('等审批');
  });

  it('sorts pinned sessions first regardless of key/direction', () => {
    const withPinned: SessionSummary[] = [
      { ...sessions[1] }, // updatedAt 2000, not pinned
      { ...sessions[0], pinned: true }, // updatedAt 3000, pinned
      { ...sessions[2] }, // updatedAt 1500, not pinned
    ];
    // desc by recent: pinned s1 (3000) first, then s2 (2000), s3 (1500)
    expect(sortSessions(withPinned, 'recent', 'desc').map((s) => s.id)).toEqual(['s1', 's2', 's3']);
    // asc by recent: pinned s1 still first, then s3 (1500), s2 (2000)
    expect(sortSessions(withPinned, 'recent', 'asc').map((s) => s.id)).toEqual(['s1', 's3', 's2']);
  });

  it('relativeTimeText uses date boundary for "昨天"', () => {
    // Simulate "now" at 2024-01-15 10:00 (1736922000)
    const nowMs = new Date(2024, 0, 15, 10, 0, 0).getTime();
    const startOfToday = new Date(2024, 0, 15).getTime() / 1000;
    const startOfYesterday = startOfToday - 86_400;

    // 30 min ago → "刚刚" (actually 30 min ago → "30 分钟前")
    expect(relativeTimeText(nowMs / 1000 - 1800, nowMs)).toBe('30 分钟前');
    // 2 hours ago today → "2 小时前"
    expect(relativeTimeText(nowMs / 1000 - 7200, nowMs)).toBe('2 小时前');
    // Yesterday 22:00 → "昨天"
    expect(relativeTimeText(startOfYesterday + 79_200, nowMs)).toBe('昨天');
    // Day before yesterday → formatted date, not "昨天"
    const twoDaysAgo = startOfToday - 2 * 86_400 + 3600;
    expect(relativeTimeText(twoDaysAgo, nowMs)).not.toBe('昨天');
  });
});
