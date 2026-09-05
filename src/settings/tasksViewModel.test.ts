import { describe, expect, it } from 'vitest';
import type { SessionSummary } from '../sessions/sessionTypes';
import {
  activeTaskFilterTokens,
  DEFAULT_TASK_FILTERS,
  filterTasks,
  listTaskDirectories,
} from './tasksViewModel';

function session(
  overrides: Partial<SessionSummary> & Pick<SessionSummary, 'id' | 'cwd'>,
): SessionSummary {
  return {
    cliSessionId: null,
    title: `任务 ${overrides.id}`,
    engine: 'claude-code',
    model: 'claude-sonnet-4',
    status: 'idle',
    messageCount: 2,
    inputTokens: 10,
    outputTokens: 5,
    costUsd: 0.01,
    createdAt: 1_000,
    updatedAt: 2_000,
    summary: null,
    pinned: false,
    folderId: 'folder-default',
    archived: false,
    lastContextTokens: null,
    lastContextWindow: null,
    preferredModel: null,
    preferredReasoningEffort: null,
    titleManual: false,
    ...overrides,
  } as SessionSummary;
}

const sessions: SessionSummary[] = [
  session({ id: 'a', cwd: 'D:\\Projects\\helm', engine: 'claude-code', archived: false }),
  session({ id: 'b', cwd: 'D:\\Projects\\helm', engine: 'codex', status: 'done' }),
  session({ id: 'c', cwd: 'D:\\Other\\demo', engine: 'claude-code', archived: true }),
];

describe('listTaskDirectories', () => {
  it('从真实会话聚合目录并带计数、按名称排序', () => {
    const directories = listTaskDirectories(sessions);
    expect(directories).toEqual([
      { cwd: 'D:\\Other\\demo', label: 'demo', count: 1 },
      { cwd: 'D:\\Projects\\helm', label: 'helm', count: 2 },
    ]);
  });
});

describe('filterTasks', () => {
  it('目录筛选只保留该 canonical cwd 的会话', () => {
    const result = filterTasks(sessions, {
      ...DEFAULT_TASK_FILTERS,
      directory: 'D:\\Projects\\helm',
    });
    expect(result.map((item) => item.id)).toEqual(['a', 'b']);
  });

  it('查询/引擎/状态沿用全局视图模型语义（含派生状态）', () => {
    const archived = filterTasks(sessions, {
      ...DEFAULT_TASK_FILTERS,
      status: 'archived',
    });
    expect(archived.map((item) => item.id)).toEqual(['c']);

    const codex = filterTasks(sessions, { ...DEFAULT_TASK_FILTERS, engine: 'codex' });
    expect(codex.map((item) => item.id)).toEqual(['b']);

    const query = filterTasks(sessions, { ...DEFAULT_TASK_FILTERS, query: '任务 c' });
    expect(query.map((item) => item.id)).toEqual(['c']);
  });
});

describe('activeTaskFilterTokens', () => {
  it('只为非默认筛选生成 token，目录显示短名', () => {
    const tokens = activeTaskFilterTokens(
      { query: '登录', directory: 'D:\\Projects\\helm', engine: 'codex', status: 'failed' },
      [{ cwd: 'D:\\Projects\\helm', label: 'helm', count: 2 }],
    );
    expect(tokens.map((token) => token.label)).toEqual([
      '搜索：登录',
      '目录：helm',
      '引擎：Codex',
      '状态：失败',
    ]);
  });

  it('默认筛选没有 token', () => {
    expect(activeTaskFilterTokens(DEFAULT_TASK_FILTERS, [])).toEqual([]);
  });
});
