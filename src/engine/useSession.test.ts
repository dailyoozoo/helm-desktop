import { afterEach, describe, expect, it, vi } from 'vitest';
import type { AgentEvent, AgentEventEnvelope } from '@helm/protocol';
import {
  applyEnvelopeToLiveRegistry,
  itemsFromHistory,
  liveSessionHandle,
  liveSessionActivity,
  liveSessionWorking,
  liveWorkingSessionIds,
  registerLiveSession,
  resetLiveSessionsForTests,
  resetSessionState,
  reduceSessionAction,
  reduceSessionEvent,
  shouldConsumeAgentEvent,
  subscribeLiveSessions,
  type SessionState,
} from './useSession';

function sessionState(overrides: Partial<SessionState> = {}): SessionState {
  return {
    handleId: 'handle-1',
    historyId: 'handle-1',
    sessionId: 'cli-current',
    engine: 'claude-code',
    model: 'claude-sonnet-4.6',
    cwd: 'D:\\work\\demo',
    status: 'working',
    items: [],
    openAssistantId: null,
    openThinkingId: null,
    cost: { inputTokens: 0, outputTokens: 0, costUsd: 0 },
    startedAt: 1,
    turnActivity: { stage: 'preparing', since: 1 },
    turnStartedAt: 1,
    disabledMcp: [],
    ...overrides,
  };
}

afterEach(() => {
  vi.useRealTimers();
});

function envelope(historyId: string, event: AgentEvent): AgentEventEnvelope {
  return { historyId, event };
}

describe('shouldConsumeAgentEvent（historyId 路由，变更-06）', () => {
  const started: AgentEvent = {
    type: 'session_started',
    sessionId: 'cli-current',
    engine: 'claude-code',
    model: 'claude-sonnet-4.6',
    cwd: 'D:\\work\\demo',
    ts: 1,
  };

  it('consumes events whose envelope historyId matches the current thread', () => {
    expect(shouldConsumeAgentEvent('h-1', envelope('h-1', started))).toBe(true);
  });

  it('routes by historyId even when the CLI session id changes between turns (Codex 多轮)', () => {
    const secondTurn: AgentEvent = {
      type: 'message_complete',
      sessionId: 'codex-pid-turn2',
      role: 'assistant',
      text: '第二轮回复',
    };
    expect(shouldConsumeAgentEvent('h-1', envelope('h-1', secondTurn))).toBe(true);
  });

  it('rejects events belonging to another thread', () => {
    expect(shouldConsumeAgentEvent('h-1', envelope('h-2', started))).toBe(false);
  });

  it('rejects everything when the view is not bound to a thread（无认领竞态）', () => {
    expect(shouldConsumeAgentEvent(null, envelope('h-1', started))).toBe(false);
    expect(
      shouldConsumeAgentEvent(
        null,
        envelope('h-1', { type: 'error', message: '任意错误', recoverable: false }),
      ),
    ).toBe(false);
  });
});

describe('并行会话注册表（变更-06）', () => {
  it('marks the session idle on turn_complete（运行中标记不再永久误报）', () => {
    resetLiveSessionsForTests();
    registerLiveSession('h-1', 's-1', true);
    expect(liveWorkingSessionIds()).toEqual(['h-1']);
    applyEnvelopeToLiveRegistry(
      envelope('h-1', { type: 'turn_complete', sessionId: 'cli-x', stopReason: 'end' }),
    );
    expect(liveWorkingSessionIds()).toEqual([]);
    // 句柄保留，供重开复用
    expect(liveSessionHandle('h-1')).toBe('s-1');
  });

  it('marks the session idle on non-recoverable error', () => {
    resetLiveSessionsForTests();
    registerLiveSession('h-1', 's-1', true);
    applyEnvelopeToLiveRegistry(
      envelope('h-1', { type: 'error', message: '进程崩溃', recoverable: false }),
    );
    expect(liveSessionWorking('h-1')).toBe(false);
  });

  it('self-heals to working when deltas arrive for a session marked idle', () => {
    resetLiveSessionsForTests();
    registerLiveSession('h-1', 's-1', false);
    applyEnvelopeToLiveRegistry(
      envelope('h-1', {
        type: 'message_delta',
        sessionId: 'cli-x',
        role: 'assistant',
        text: '流式中',
      }),
    );
    expect(liveSessionWorking('h-1')).toBe(true);
  });

  it('treats turn_stage as evidence that the session is working', () => {
    resetLiveSessionsForTests();
    registerLiveSession('h-1', 's-1', false);
    applyEnvelopeToLiveRegistry(
      envelope('h-1', {
        type: 'turn_stage',
        sessionId: 'cli-x',
        stage: 'waiting_model',
        ts: 123,
      }),
    );
    expect(liveSessionWorking('h-1')).toBe(true);
  });

  it('tracks truthful activity for background sessions and clears it on terminal events', () => {
    resetLiveSessionsForTests();
    registerLiveSession('h-1', 's-1', true);

    applyEnvelopeToLiveRegistry(
      envelope('h-1', {
        type: 'turn_stage',
        sessionId: 'cli-x',
        stage: 'waiting_model',
        ts: 100,
      }),
    );
    expect(liveSessionActivity('h-1')).toEqual({ stage: 'waiting_model', since: 100 });

    applyEnvelopeToLiveRegistry(
      envelope('h-1', {
        type: 'tool_call',
        sessionId: 'cli-x',
        id: 'tool-1',
        name: 'Read',
        input: { file_path: 'README.md' },
        status: 'pending',
      }),
    );
    expect(liveSessionActivity('h-1')).toMatchObject({
      stage: 'using_tool',
      toolName: 'Read',
      target: 'README.md',
    });

    applyEnvelopeToLiveRegistry(
      envelope('h-1', {
        type: 'approval_request',
        sessionId: 'cli-x',
        id: 'approval-1',
        action: 'Bash',
        detail: 'npm test',
      }),
    );
    expect(liveSessionActivity('h-1')).toMatchObject({ stage: 'waiting_approval' });

    applyEnvelopeToLiveRegistry(
      envelope('h-1', { type: 'turn_complete', sessionId: 'cli-x', stopReason: 'end' }),
    );
    expect(liveSessionActivity('h-1')).toBeNull();
  });

  it('keeps activity when the same live handle is registered and restores it on resume', () => {
    resetLiveSessionsForTests();
    const activity = { stage: 'reasoning' as const, since: 123 };
    registerLiveSession('h-1', 's-1', true, activity);
    registerLiveSession('h-1', 's-1', true);

    expect(liveSessionActivity('h-1')).toEqual(activity);
    const resumed = reduceSessionAction(sessionState(), {
      type: 'resume_handle',
      handleId: 's-1',
      historyId: 'h-1',
      working: true,
      activity: liveSessionActivity('h-1'),
    });
    expect(resumed.turnActivity).toEqual(activity);
    expect(resumed.turnStartedAt).toBe(123);
  });

  it('does not notify live snapshot subscribers for activity-only delta changes', () => {
    resetLiveSessionsForTests();
    registerLiveSession('h-1', 's-1', true);
    let notifications = 0;
    const unsubscribe = subscribeLiveSessions(() => {
      notifications += 1;
    });

    applyEnvelopeToLiveRegistry(
      envelope('h-1', {
        type: 'message_delta',
        sessionId: 'cli-x',
        role: 'assistant',
        text: '第一段',
      }),
    );
    applyEnvelopeToLiveRegistry(
      envelope('h-1', {
        type: 'message_delta',
        sessionId: 'cli-x',
        role: 'assistant',
        text: '第二段',
      }),
    );
    applyEnvelopeToLiveRegistry(
      envelope('h-1', {
        type: 'thinking_delta',
        sessionId: 'cli-x',
        text: '继续分析',
      }),
    );

    unsubscribe();
    expect(notifications).toBe(0);
    expect(liveSessionActivity('h-1')).toMatchObject({ stage: 'reasoning' });
  });

  it('notifies live snapshot subscribers when working or approval snapshots change', () => {
    resetLiveSessionsForTests();
    registerLiveSession('h-1', 's-1', false);
    let notifications = 0;
    const unsubscribe = subscribeLiveSessions(() => {
      notifications += 1;
    });

    applyEnvelopeToLiveRegistry(
      envelope('h-1', {
        type: 'message_delta',
        sessionId: 'cli-x',
        role: 'assistant',
        text: '开始回复',
      }),
    );
    expect(notifications).toBe(1);

    applyEnvelopeToLiveRegistry(
      envelope('h-1', {
        type: 'approval_request',
        sessionId: 'cli-x',
        id: 'approval-1',
        action: 'Bash',
        detail: 'npm test',
      }),
    );
    expect(notifications).toBe(2);

    applyEnvelopeToLiveRegistry(
      envelope('h-1', {
        type: 'message_delta',
        sessionId: 'cli-x',
        role: 'assistant',
        text: '审批后恢复',
      }),
    );
    unsubscribe();
    expect(notifications).toBe(3);
  });

  it('ignores events for sessions the registry does not know', () => {
    resetLiveSessionsForTests();
    applyEnvelopeToLiveRegistry(
      envelope('h-unknown', { type: 'turn_complete', sessionId: 'cli-x', stopReason: 'end' }),
    );
    expect(liveWorkingSessionIds()).toEqual([]);
  });
});

describe('reduceSessionEvent plan_update', () => {
  function baseState(): SessionState {
    return sessionState();
  }

  it('adds and updates a single plan item from plan_update events', () => {
    const first = reduceSessionEvent(baseState(), {
      type: 'plan_update',
      sessionId: 'cli-current',
      steps: [
        { text: '读取需求', status: 'done' },
        { text: '修改代码', status: 'active' },
      ],
    });

    expect(first.items).toEqual([
      {
        kind: 'plan',
        id: 'plan-cli-current',
        steps: [
          { text: '读取需求', status: 'done' },
          { text: '修改代码', status: 'active' },
        ],
      },
    ]);

    const second = reduceSessionEvent(first, {
      type: 'plan_update',
      sessionId: 'cli-current',
      steps: [
        { text: '读取需求', status: 'done' },
        { text: '修改代码', status: 'done' },
        { text: '运行验证', status: 'active' },
      ],
    });

    expect(second.items).toHaveLength(1);
    expect(second.items[0]).toEqual({
      kind: 'plan',
      id: 'plan-cli-current',
      steps: [
        { text: '读取需求', status: 'done' },
        { text: '修改代码', status: 'done' },
        { text: '运行验证', status: 'active' },
      ],
    });
  });

  it('accumulates and completes a single thinking item from thinking events', () => {
    const first = reduceSessionEvent(baseState(), {
      type: 'thinking_delta',
      sessionId: 'cli-current',
      text: '先读',
    });
    const second = reduceSessionEvent(first, {
      type: 'thinking_delta',
      sessionId: 'cli-current',
      text: '文件',
    });
    const third = reduceSessionEvent(second, {
      type: 'thinking_complete',
      sessionId: 'cli-current',
      text: '先读文件',
    });

    expect(third.items).toEqual([
      {
        kind: 'thinking',
        id: expect.any(String),
        text: '先读文件',
        done: true,
      },
    ]);
  });

  it('does not duplicate thinking when text deltas closed the stream first（变更-09 S2）', () => {
    // thinking 流式 → 正文 delta 开始（thinking 被关闭）→ 轮末 ThinkingComplete 重放
    let s = reduceSessionEvent(baseState(), {
      type: 'thinking_delta',
      sessionId: 'cli-current',
      text: '思考中',
    });
    s = reduceSessionEvent(s, {
      type: 'message_delta',
      sessionId: 'cli-current',
      role: 'assistant',
      text: '正文',
    });
    // 正文开始时思考项应已落定（不再显示「正在思考」）
    const thinkingAfterText = s.items.filter((it) => it.kind === 'thinking');
    expect(thinkingAfterText).toHaveLength(1);
    expect(thinkingAfterText[0]).toMatchObject({ done: true });

    s = reduceSessionEvent(s, {
      type: 'thinking_complete',
      sessionId: 'cli-current',
      text: '思考中',
    });
    // 轮末重放不追加第二条 thinking
    expect(s.items.filter((it) => it.kind === 'thinking')).toHaveLength(1);
  });

  it('marks the streaming assistant message as interrupted on turn_complete{interrupted}', () => {
    let s = reduceSessionEvent(baseState(), {
      type: 'message_delta',
      sessionId: 'cli-current',
      role: 'assistant',
      text: '写到一半',
    });
    s = reduceSessionEvent(s, {
      type: 'turn_complete',
      sessionId: 'cli-current',
      stopReason: 'interrupted',
    });
    expect(s.items[0]).toMatchObject({ kind: 'assistant', text: '写到一半', interrupted: true });
    expect(s.status).toBe('idle');
  });

  it('dedupes the turn-end assistant replay when streaming already delivered it', () => {
    let s = reduceSessionEvent(baseState(), {
      type: 'message_delta',
      sessionId: 'cli-current',
      role: 'assistant',
      text: '完整回复',
    });
    // 工具调用关闭了 openAssistantId
    s = reduceSessionEvent(s, {
      type: 'tool_call',
      sessionId: 'cli-current',
      id: 't1',
      name: 'Read',
      input: {},
      status: 'pending',
    });
    s = reduceSessionEvent(s, {
      type: 'message_complete',
      sessionId: 'cli-current',
      role: 'assistant',
      text: '完整回复',
    });
    // 轮末重放与最后一条 assistant 相同 → 不追加？（此处 last 是 tool，因此会追加——验证只有紧邻重复才去重）
    expect(s.items.filter((it) => it.kind === 'assistant')).toHaveLength(2);
  });

  it('keeps context window size from token_usage events', () => {
    const next = reduceSessionEvent(baseState(), {
      type: 'token_usage',
      sessionId: 'cli-current',
      inputTokens: 800,
      outputTokens: 200,
      costUsd: 0.02,
      contextWindow: 2000,
    } as AgentEvent);

    expect(next.cost).toEqual({
      inputTokens: 800,
      outputTokens: 200,
      costUsd: 0.02,
      contextWindow: 2000,
    });
  });
});

describe('轮次活动追踪', () => {
  it('local send immediately records a truthful preparing stage and turn start time', () => {
    vi.useFakeTimers();
    vi.setSystemTime(10_000);
    const next = reduceSessionAction(
      sessionState({ status: 'idle', turnActivity: null, turnStartedAt: null }),
      { type: 'send', id: 'u-1', text: '检查项目' },
    );

    expect(next.turnActivity).toEqual({ stage: 'preparing', since: 10_000 });
    expect(next.turnStartedAt).toBe(10_000);
  });

  it('turn_stage replaces the current activity with protocol fields and timestamp', () => {
    const next = reduceSessionEvent(sessionState(), {
      type: 'turn_stage',
      sessionId: 'cli-current',
      stage: 'retrying',
      ts: 2_000,
      retryAttempt: 3,
      engineReportedTtftMs: 640,
    });

    expect(next.turnActivity).toEqual({
      stage: 'retrying',
      since: 2_000,
      retryAttempt: 3,
      engineReportedTtftMs: 640,
    });
    expect(next.turnStartedAt).toBe(1);
  });

  it('derives reasoning from thinking events while the turn is active', () => {
    vi.useFakeTimers();
    vi.setSystemTime(3_000);
    const first = reduceSessionEvent(sessionState(), {
      type: 'thinking_delta',
      sessionId: 'cli-current',
      text: '先读文件',
    });
    vi.setSystemTime(4_000);
    const second = reduceSessionEvent(first, {
      type: 'thinking_complete',
      sessionId: 'cli-current',
      text: '先读文件',
    });

    expect(first.turnActivity).toEqual({ stage: 'reasoning', since: 3_000 });
    expect(second.turnActivity).toEqual({ stage: 'reasoning', since: 3_000 });
  });

  it('tool_call overrides reasoning with the real tool name and a safe target', () => {
    vi.useFakeTimers();
    vi.setSystemTime(5_000);
    const next = reduceSessionEvent(
      sessionState({ turnActivity: { stage: 'reasoning', since: 3_000 } }),
      {
        type: 'tool_call',
        sessionId: 'cli-current',
        id: 'tool-1',
        name: 'Read',
        input: { file_path: 'src/workspace/Thread.tsx', ignored: { secret: true } },
        status: 'pending',
      },
    );

    expect(next.turnActivity).toEqual({
      stage: 'using_tool',
      since: 5_000,
      toolName: 'Read',
      target: 'src/workspace/Thread.tsx',
    });
  });

  it.each([
    'curl https://example.com -H "Authorization: Bearer secret-token"',
    'deploy --password super-secret',
  ])('never copies a Bash command into turnActivity.target: %s', (command) => {
    vi.useFakeTimers();
    vi.setSystemTime(5_000);
    const next = reduceSessionEvent(sessionState(), {
      type: 'tool_call',
      sessionId: 'cli-current',
      id: 'tool-bash',
      name: 'Bash',
      input: { command },
      status: 'pending',
    });

    expect(next.turnActivity).toEqual({
      stage: 'using_tool',
      since: 5_000,
      toolName: 'Bash',
    });
  });

  it('derives responding and waiting_approval from real events', () => {
    vi.useFakeTimers();
    vi.setSystemTime(6_000);
    const responding = reduceSessionEvent(sessionState(), {
      type: 'message_delta',
      sessionId: 'cli-current',
      role: 'assistant',
      text: '处理中',
    });
    vi.setSystemTime(7_000);
    const waiting = reduceSessionEvent(responding, {
      type: 'approval_request',
      sessionId: 'cli-current',
      id: 'approval-1',
      action: 'Bash',
      detail: 'npm test',
    });

    expect(responding.turnActivity).toEqual({ stage: 'responding', since: 6_000 });
    expect(waiting.turnActivity).toEqual({ stage: 'waiting_approval', since: 7_000 });
  });

  it('closes active thinking before entering waiting_approval', () => {
    vi.useFakeTimers();
    vi.setSystemTime(8_000);
    const thinking = reduceSessionEvent(sessionState(), {
      type: 'thinking_delta',
      sessionId: 'cli-current',
      text: '分析真实上下文',
    });
    const waiting = reduceSessionEvent(thinking, {
      type: 'approval_request',
      sessionId: 'cli-current',
      id: 'approval-1',
      action: 'Bash',
      detail: 'npm test',
    });

    expect(waiting.openThinkingId).toBeNull();
    expect(waiting.items.find((item) => item.kind === 'thinking')).toMatchObject({ done: true });
    expect(waiting.turnActivity).toMatchObject({ stage: 'waiting_approval' });
  });

  it('clears activity on turn completion and fatal errors', () => {
    const completed = reduceSessionEvent(sessionState(), {
      type: 'turn_complete',
      sessionId: 'cli-current',
      stopReason: 'end',
    });
    const failed = reduceSessionEvent(sessionState(), {
      type: 'error',
      sessionId: 'cli-current',
      message: '进程崩溃',
      recoverable: false,
    });

    expect(completed).toMatchObject({ turnActivity: null, turnStartedAt: null });
    expect(failed).toMatchObject({ turnActivity: null, turnStartedAt: null });
  });

  it('keeps the current activity on recoverable errors', () => {
    const current = { stage: 'retrying' as const, since: 9_000, retryAttempt: 2 };
    const next = reduceSessionEvent(sessionState({ turnActivity: current }), {
      type: 'error',
      sessionId: 'cli-current',
      message: '网络抖动',
      recoverable: true,
    });

    expect(next.turnActivity).toEqual(current);
    expect(next.turnStartedAt).toBe(1);
  });

  it.each([
    { type: 'select_engine' as const, engine: 'codex' as const, model: 'gpt-5' },
    { type: 'select_model' as const, model: 'claude-opus-4.1' },
    {
      type: 'resume_handle' as const,
      handleId: 'handle-2',
      historyId: 'history-2',
      working: true,
    },
  ])('$type clears activity inherited from another thread', (action) => {
    const next = reduceSessionAction(sessionState(), action);
    expect(next.turnActivity).toBeNull();
    expect(next.turnStartedAt).toBeNull();
  });
});

describe('resetSessionState', () => {
  it('uses settings defaults instead of carrying over the previous session selection', () => {
    const previous: SessionState = {
      handleId: 'handle-1',
      historyId: 'handle-1',
      sessionId: 'cli-current',
      engine: 'claude-code',
      model: 'claude-sonnet-4.6',
      cwd: 'D:\\old',
      status: 'idle',
      items: [{ kind: 'assistant', id: 'a-1', text: '旧会话' }],
      openAssistantId: null,
      openThinkingId: null,
      cost: { inputTokens: 10, outputTokens: 5, costUsd: 0.01 },
      startedAt: 1,
      turnActivity: { stage: 'responding', since: 2 },
      turnStartedAt: 2,
      disabledMcp: [],
    };

    expect(resetSessionState(previous, { engine: 'codex', cwd: 'D:\\new' })).toMatchObject({
      handleId: null,
      sessionId: null,
      engine: 'codex',
      model: '',
      cwd: 'D:\\new',
      items: [],
      cost: { inputTokens: 0, outputTokens: 0, costUsd: 0 },
      startedAt: null,
      turnActivity: null,
      turnStartedAt: null,
    });
  });
});

describe('itemsFromHistory', () => {
  it('interleaves messages, tools and checkpoints by timestamp（变更-10）', () => {
    const items = itemsFromHistory({
      id: 'local-2',
      cliSessionId: null,
      title: '穿插排序',
      engine: 'claude-code',
      model: 'claude-sonnet-4.6',
      cwd: 'D:\\work\\demo',
      status: 'done',
      createdAt: 1,
      updatedAt: 2,
      messageCount: 3,
      inputTokens: 0,
      outputTokens: 0,
      costUsd: 0,
      messages: [
        { role: 'user', text: '改一下配置', ts: 1000 },
        { role: 'assistant', text: '先看文件', ts: 2000 },
        { role: 'assistant', text: '改完了', ts: 5000 },
      ],
      toolCalls: [
        { id: 't-read', name: 'Read', status: 'success', input: {}, output: 'ok', ts: 3000 },
        { id: 't-edit', name: 'Edit', status: 'success', input: {}, output: 'ok', ts: 4000 },
      ],
      checkpoints: [{ id: 'c-1', label: '改动前：config.ts', ts: 3500 }],
      approvals: [],
    });

    expect(items.map((it) => it.kind)).toEqual([
      'user', // 1000
      'assistant', // 2000
      'tool', // 3000 Read
      'checkpoint', // 3500
      'tool', // 4000 Edit
      'assistant', // 5000
    ]);
  });

  it('restores tool calls with diff from session history', () => {
    const items = itemsFromHistory({
      id: 'local-1',
      cliSessionId: 'cli-current',
      title: '测试会话',
      engine: 'claude-code',
      model: 'claude-sonnet-4.6',
      cwd: 'D:\\work\\demo',
      status: 'done',
      createdAt: 1,
      updatedAt: 2,
      messageCount: 1,
      inputTokens: 0,
      outputTokens: 0,
      costUsd: 0,
      messages: [{ role: 'assistant', text: '完成', ts: 1 }],
      toolCalls: [
        {
          id: 'tool-1',
          name: 'Edit',
          status: 'success',
          input: { file_path: 'demo.ts' },
          output: 'Updated',
          ts: 2,
          diff: {
            path: 'demo.ts',
            hunks: [
              {
                oldStart: 1,
                newStart: 1,
                lines: [
                  { kind: 'del', text: 'old' },
                  { kind: 'add', text: 'new' },
                ],
              },
            ],
          },
        },
      ],
      checkpoints: [{ id: 'ckpt-1', label: '改动前：demo.ts', ts: 1_717_171_703_000 }],
      approvals: [
        { id: 'appr-1', action: 'Bash', detail: 'pnpm test', status: 'pending', ts: 2 },
        { id: 'appr-2', action: 'Write', detail: 'x.txt', status: 'expired', ts: 3 },
      ],
    });

    expect(items).toContainEqual({
      kind: 'approval',
      id: 'appr-1',
      action: 'Bash',
      detail: 'pnpm test',
      resolved: false,
    });
    expect(items).toContainEqual({
      kind: 'approval',
      id: 'appr-2',
      action: 'Write',
      detail: 'x.txt',
      resolved: true,
    });

    expect(items).toContainEqual({
      kind: 'tool',
      id: 'tool-1',
      name: 'Edit',
      input: { file_path: 'demo.ts' },
      status: 'success',
      output: 'Updated',
      diff: {
        path: 'demo.ts',
        hunks: [
          {
            oldStart: 1,
            newStart: 1,
            lines: [
              { kind: 'del', text: 'old' },
              { kind: 'add', text: 'new' },
            ],
          },
        ],
      },
    });
    expect(items).toContainEqual({
      kind: 'checkpoint',
      id: 'ckpt-1',
      label: '改动前：demo.ts',
      ts: 1_717_171_703_000,
      restored: false,
    });
  });
});
