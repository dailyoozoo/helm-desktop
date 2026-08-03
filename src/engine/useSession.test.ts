import { afterEach, describe, expect, it, vi } from 'vitest';
import type { AgentEvent, AgentEventEnvelope } from '@helm/protocol';
import {
  applyEnvelopeToLiveRegistry,
  classifyClientErrorKind,
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
  submitApprovalTransaction,
  shouldConsumeAgentEvent,
  subscribeLiveSessions,
  type SessionState,
} from './useSession';

describe('Tauri 命令错误分类', () => {
  it('把会话创建阶段的无效工作目录识别为 cwd_invalid', () => {
    expect(
      classifyClientErrorKind(
        '工作目录不存在：D:\\missing。请重新选择一个有效目录（系统找不到指定的路径）',
      ),
    ).toBe('cwd_invalid');
    expect(
      classifyClientErrorKind(
        '工作目录不存在或不是文件夹：D:\\work\\file.txt。请重新选择一个有效目录',
      ),
    ).toBe('cwd_invalid');
  });

  it('识别模型不可用错误', () => {
    expect(
      classifyClientErrorKind(
        "There's an issue with the selected model. It may not exist or you may not have access to it.",
      ),
    ).toBe('model_unavailable');
  });

  it('把 Codex 受保护工具面拒绝渲染为版本不兼容，而不是裸错误码', () => {
    for (const code of [
      'codex_probe_tool_surface_unmanaged',
      'codex_probe_tool_surface_incomplete',
      'codex_probe_tool_surface_timed_out',
      'codex_probe_tool_surface_unavailable',
      'codex_probe_tool_surface_unrecognized',
    ]) {
      expect(classifyClientErrorKind(`[${code}] codex 0.144.1 probe rejected`)).toBe(
        'version_incompatible',
      );
    }
  });

  it('不把未知命令错误误判为 Codex 版本问题', () => {
    expect(classifyClientErrorKind('未知后端错误')).toBeUndefined();
  });
});

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

describe('逐 Turn 模型证据', () => {
  it('keeps the requested preference separate from the Runtime-observed model', () => {
    const next = reduceSessionEvent(sessionState({ model: 'requested-model' }), {
      type: 'session_started',
      sessionId: 'native-1',
      engine: 'claude-code',
      model: 'routed-model',
      cwd: 'D:\\work\\demo',
      ts: 1,
    });
    expect(next.model).toBe('requested-model');
    expect(next.runtimeModel).toBe('routed-model');
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
        availableDecisions: ['allow', 'deny'],
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
        availableDecisions: ['allow', 'deny'],
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

  it('累加计费 token，但替换最近一次上下文占用', () => {
    let next = reduceSessionEvent(baseState(), {
      type: 'token_usage',
      sessionId: 'cli-current',
      inputTokens: 100,
      cachedInputTokens: 70,
      cacheWriteInputTokens: 20,
      outputTokens: 10,
      costUsd: 0.01,
    });
    next = reduceSessionEvent(next, {
      type: 'context_usage',
      sessionId: 'cli-current',
      contextTokens: 80,
      contextWindow: 200,
    });
    next = reduceSessionEvent(next, {
      type: 'context_usage',
      sessionId: 'cli-current',
      contextTokens: 120,
      contextWindow: 200,
    });
    expect(next.cost).toMatchObject({
      inputTokens: 100,
      cachedInputTokens: 70,
      cacheWriteInputTokens: 20,
      outputTokens: 10,
      contextTokens: 120,
      contextWindow: 200,
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
      availableDecisions: ['allow', 'deny'],
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
      availableDecisions: ['allow', 'deny'],
    });

    expect(waiting.openThinkingId).toBeNull();
    expect(waiting.items.find((item) => item.kind === 'thinking')).toMatchObject({ done: true });
    expect(waiting.turnActivity).toMatchObject({ stage: 'waiting_approval' });
  });

  it('updates a replayed approval request in place instead of appending a duplicate card', () => {
    const first = reduceSessionEvent(sessionState(), {
      type: 'approval_request',
      sessionId: 'cli-current',
      id: 'approval-1',
      action: 'Bash',
      detail: 'ls',
      availableDecisions: ['allow', 'deny'],
    });

    const replayed = reduceSessionEvent(first, {
      type: 'approval_request',
      sessionId: 'cli-current',
      id: 'approval-1',
      action: 'Bash',
      detail: 'ls -la',
      availableDecisions: ['allow', 'deny'],
    });

    expect(replayed.items.filter((item) => item.kind === 'approval')).toEqual([
      {
        kind: 'approval',
        id: 'approval-1',
        action: 'Bash',
        detail: 'ls -la',
        status: 'pending',
        availableDecisions: ['allow', 'deny'],
        persistentLabel: undefined,
        matcherSummary: undefined,
      },
    ]);
  });

  it('keeps a terminal approval terminal when its request event is replayed', () => {
    const resolved = sessionState({
      items: [
        {
          kind: 'approval',
          id: 'approval-1',
          action: 'Bash',
          detail: 'ls',
          status: 'resolved',
          availableDecisions: [],
        },
      ],
    });

    const replayed = reduceSessionEvent(resolved, {
      type: 'approval_request',
      sessionId: 'cli-current',
      id: 'approval-1',
      action: 'Bash',
      detail: 'ls',
      availableDecisions: ['allow', 'deny'],
    });

    expect(replayed.items.find((item) => item.kind === 'approval')).toMatchObject({
      id: 'approval-1',
      status: 'resolved',
    });
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

  it('ends the current turn on recoverable session errors', () => {
    const current = { stage: 'retrying' as const, since: 9_000, retryAttempt: 2 };
    const next = reduceSessionEvent(sessionState({ turnActivity: current }), {
      type: 'error',
      sessionId: 'cli-current',
      message: '网络抖动',
      recoverable: true,
    });

    expect(next.status).toBe('idle');
    expect(next.turnActivity).toBeNull();
    expect(next.turnStartedAt).toBeNull();
  });

  it.each([
    { type: 'select_engine' as const, engine: 'codex' as const, model: 'gpt-5' },
    {
      type: 'resume_handle' as const,
      handleId: 'handle-2',
      historyId: 'history-2',
      working: true,
    },
  ])('$type clears activity inherited from another thread', (action) => {
    const next = reduceSessionAction(
      {
        ...sessionState(),
        items: [{ kind: 'assistant', id: 'old', text: '旧消息' }],
        cost: { inputTokens: 2, outputTokens: 3, costUsd: 0.01 },
      },
      action,
    );
    expect(next.turnActivity).toBeNull();
    expect(next.turnStartedAt).toBeNull();
    if (action.type === 'resume_handle') return;
    expect(next.items).toEqual([]);
    expect(next.cost).toEqual({ inputTokens: 0, outputTokens: 0, costUsd: 0 });
  });

  it('keeps the current Session and history when changing the next-turn model preference', () => {
    const current = {
      ...sessionState(),
      handleId: 'handle-1',
      historyId: 'history-1',
      items: [{ kind: 'assistant' as const, id: 'old', text: '旧消息' }],
    };
    const next = reduceSessionAction(current, {
      type: 'select_model',
      model: 'claude-opus-4.1',
    });
    expect(next.handleId).toBe('handle-1');
    expect(next.historyId).toBe('history-1');
    expect(next.items).toEqual(current.items);
    expect(next.model).toBe('claude-opus-4.1');
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
        { role: 'user', text: '改一下配置', ts: 1000, turnId: 'turn-1' },
        { role: 'assistant', text: '先看文件', ts: 2000 },
        { role: 'assistant', text: '改完了', ts: 5000 },
      ],
      toolCalls: [
        { id: 't-read', name: 'Read', status: 'success', input: {}, output: 'ok', ts: 3000 },
        { id: 't-edit', name: 'Edit', status: 'success', input: {}, output: 'ok', ts: 4000 },
      ],
      checkpoints: [{ id: 'c-1', label: '改动前：config.ts', ts: 3500 }],
      approvals: [],
      turns: [
        {
          id: 'turn-1',
          epoch: 1,
          mode: 'build',
          permissionProfile: 'auto',
          status: 'succeeded',
          startedAt: 1100,
          endedAt: 5100,
        },
      ],
    });

    expect(items.map((it) => it.kind)).toEqual([
      'user', // 1000
      'assistant', // 2000
      'tool', // 3000 Read
      'checkpoint', // 3500
      'tool', // 4000 Edit
      'assistant', // 5000
    ]);
    expect(items[0]).toMatchObject({
      kind: 'user',
      mode: 'build',
      permissionProfile: 'auto',
    });
    expect(items[1]).toMatchObject({ kind: 'assistant', turnId: 'turn-1' });
    expect(items[5]).toMatchObject({ kind: 'assistant', turnId: 'turn-1' });
  });

  it('只在唯一已结束 Turn 的时间区间内恢复旧 assistant 归属', () => {
    const items = itemsFromHistory({
      id: 'legacy-turn-links',
      cliSessionId: null,
      title: '旧历史',
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
        { role: 'assistant', text: '第一轮完成', ts: 1900 },
        { role: 'assistant', text: '区间外旧消息', ts: 2500 },
        { role: 'assistant', text: '第二轮完成', ts: 3900 },
      ],
      toolCalls: [],
      checkpoints: [],
      approvals: [],
      turns: [
        {
          id: 'turn-1',
          epoch: 1,
          mode: 'build',
          permissionProfile: 'standard',
          status: 'succeeded',
          startedAt: 1000,
          endedAt: 2000,
        },
        {
          id: 'turn-2',
          epoch: 2,
          mode: 'build',
          permissionProfile: 'standard',
          status: 'failed',
          startedAt: 3000,
          endedAt: 4000,
        },
      ],
    });

    expect(items[0]).toMatchObject({ kind: 'assistant', turnId: 'turn-1' });
    expect(items[1]).toMatchObject({ kind: 'assistant' });
    expect(items[1]).not.toHaveProperty('turnId');
    expect(items[2]).toMatchObject({
      kind: 'assistant',
      turnId: 'turn-2',
      turnStatus: 'failed',
    });
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
      status: 'pending',
      error: undefined,
      availableDecisions: ['allow', 'deny'],
      decision: undefined,
      persistentLabel: undefined,
      matcherSummary: undefined,
    });
    expect(items).toContainEqual({
      kind: 'approval',
      id: 'appr-2',
      action: 'Write',
      detail: 'x.txt',
      status: 'resolved',
      error: undefined,
      availableDecisions: [],
      decision: undefined,
      persistentLabel: undefined,
      matcherSummary: undefined,
    });

    expect(items).toContainEqual({
      kind: 'tool',
      id: 'tool-1',
      name: 'Edit',
      input: { file_path: 'demo.ts' },
      status: 'success',
      output: 'Updated',
      startedAt: 2,
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
      restorable: false,
      fileCount: 0,
      reason: undefined,
    });
  });
});

describe('submitApprovalTransaction', () => {
  it('stays applying until the backend confirms the decision', async () => {
    const actions: Array<{ type: string }> = [];
    let confirm!: () => void;
    const backend = new Promise<void>((resolve) => {
      confirm = resolve;
    });

    const pending = submitApprovalTransaction({
      approvalId: 'approval-1',
      decision: 'always',
      respond: () => backend,
      dispatch: (action) => actions.push(action),
      onResolved: vi.fn(),
      onFailed: vi.fn(),
    });

    expect(actions).toEqual([{ type: 'approval_applying', approvalId: 'approval-1' }]);
    confirm();
    await pending;
    expect(actions).toEqual([
      { type: 'approval_applying', approvalId: 'approval-1' },
      { type: 'approval_resolved', approvalId: 'approval-1', decision: 'always' },
    ]);
  });

  it('marks a backend failure retryable and exposes the error', async () => {
    const actions: Array<{ type: string; error?: string }> = [];
    const onFailed = vi.fn();

    await submitApprovalTransaction({
      approvalId: 'approval-1',
      decision: 'allow',
      respond: async () => {
        throw new Error('审批恢复失败');
      },
      dispatch: (action) => actions.push(action),
      onResolved: vi.fn(),
      onFailed,
    });

    expect(actions.at(-1)).toEqual({
      type: 'approval_failed',
      approvalId: 'approval-1',
      error: '审批恢复失败',
    });
    expect(onFailed).toHaveBeenCalledWith('审批恢复失败');
  });
});
