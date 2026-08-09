import { describe, expect, it } from 'vitest';
import type { AgentEvent, AgentEventEnvelope } from '@helm/protocol';
import {
  applyEnvelopeToLiveRegistry,
  livePendingApprovalSessionIds,
  liveWorkingSessionIds,
  registerLiveSession,
  reduceSessionAction,
  reduceSessionEvent,
  resetLiveSessionsForTests,
  resetSessionState,
  type SessionState,
} from '../engine/useSession';
import { activityPresentationParts } from './activityViewModel';
import { resolveEnterAction } from './slashCommands';
import { itemsFromHistory } from '../engine/useSession';

function state(overrides: Partial<SessionState> = {}): SessionState {
  return {
    handleId: 'history-1',
    historyId: 'history-1',
    sessionId: 'cli-1',
    engine: 'claude-code',
    model: 'claude-sonnet-4.6',
    cwd: 'C:/repo',
    status: 'idle',
    items: [],
    openAssistantId: null,
    openThinkingId: null,
    cost: { inputTokens: 0, outputTokens: 0, costUsd: 0 },
    startedAt: null,
    turnActivity: null,
    turnStartedAt: null,
    disabledMcp: [],
    ...overrides,
  };
}

function event(event: AgentEvent): AgentEvent {
  return event;
}

function envelope(historyId: string, value: AgentEvent, seq = 1): AgentEventEnvelope {
  return { historyId, eventSeq: seq, event: value };
}

describe('基础对话场景 DLG-01 至 DLG-12', () => {
  it('DLG-01 新建空 Session 后发送 Ask 进入 working', () => {
    const next = reduceSessionAction(
      resetSessionState(state(), { engine: 'claude-code', cwd: 'C:/repo' }),
      {
        type: 'send',
        id: 'user-1',
        text: '解释这个项目',
        mode: 'ask',
        permissionProfile: 'standard',
      },
    );
    expect(next.status).toBe('working');
    expect(next.items[0]).toMatchObject({ kind: 'user', text: '解释这个项目', mode: 'ask' });
  });

  it('DLG-02 流式 delta 到 complete 只保留一条 Assistant 消息', () => {
    let next = state({ status: 'working' });
    next = reduceSessionEvent(
      next,
      event({ type: 'message_delta', sessionId: 'cli-1', role: 'assistant', text: '你好' }),
    );
    next = reduceSessionEvent(
      next,
      event({ type: 'message_delta', sessionId: 'cli-1', role: 'assistant', text: '，Helm' }),
    );
    next = reduceSessionEvent(
      next,
      event({
        type: 'message_complete',
        sessionId: 'cli-1',
        role: 'assistant',
        text: '你好，Helm',
      }),
    );
    expect(next.items.filter((item) => item.kind === 'assistant')).toHaveLength(1);
    expect(next.items[0]).toMatchObject({ kind: 'assistant', text: '你好，Helm' });
  });

  it('DLG-03 Runtime error 进入唯一错误终态，不伪造成功回复', () => {
    let next = reduceSessionAction(state(), { type: 'send', id: 'u-1', text: '运行检查' });
    next = reduceSessionEvent(
      next,
      event({
        type: 'error',
        sessionId: 'cli-1',
        message: '模型不可用',
        recoverable: true,
        kind: 'model_unavailable',
      }),
    );
    expect(next.status).toBe('idle');
    expect(next.items.filter((item) => item.kind === 'error')).toHaveLength(1);
    expect(next.items.some((item) => item.kind === 'assistant')).toBe(false);
  });

  it('DLG-04 working 时 Enter 进入队列动作而不替代 Stop', () => {
    expect(
      resolveEnterAction({
        shiftKey: false,
        isComposing: false,
        working: true,
        menuOpen: false,
        hasMenuMatches: false,
        unknownCommand: false,
      }),
    ).toBe('queue');
  });

  it('DLG-05 Stop 对半截 Assistant 标记 interrupted 并回到 idle', () => {
    let next = reduceSessionEvent(
      state({ status: 'working' }),
      event({ type: 'message_delta', sessionId: 'cli-1', role: 'assistant', text: '进行中' }),
    );
    next = reduceSessionEvent(
      next,
      event({ type: 'turn_complete', sessionId: 'cli-1', stopReason: 'interrupted' }),
    );
    expect(next.status).toBe('idle');
    expect(next.items[0]).toMatchObject({ kind: 'assistant', interrupted: true });
  });

  it('DLG-06 失败事件保留用户消息并产生可见错误', () => {
    const next = reduceSessionEvent(
      reduceSessionAction(state(), { type: 'send', id: 'u-2', text: '排队消息' }),
      event({ type: 'error', sessionId: 'cli-1', message: '发送失败', recoverable: true }),
    );
    expect(next.items[0]).toMatchObject({ kind: 'user', text: '排队消息' });
    expect(next.items.at(-1)).toMatchObject({ kind: 'error', message: '发送失败' });
  });

  it('DLG-07 切换 Engine/Model 清空旧身份和历史', () => {
    const next = reduceSessionAction(
      state({ items: [{ kind: 'user', id: 'u', text: '旧消息' }] }),
      { type: 'select_engine', engine: 'codex', model: 'gpt-5' },
    );
    expect(next.engine).toBe('codex');
    expect(next.model).toBe('gpt-5');
    expect(next.historyId).toBeNull();
    expect(next.handleId).toBeNull();
    expect(next.items).toEqual([]);
  });

  it('DLG-08 Approval request 进入 waiting_approval 且重复事件不重复卡片', () => {
    const request = event({
      type: 'approval_request',
      sessionId: 'cli-1',
      id: 'a-1',
      action: 'Bash',
      detail: 'git status',
      availableDecisions: ['allow', 'session', 'deny'],
      persistentLabel: '本会话总是允许',
      matcherSummary: 'Bash + git status',
    });
    let next = reduceSessionEvent(state({ status: 'working' }), request);
    next = reduceSessionEvent(next, request);
    expect(next.turnActivity?.stage).toBe('waiting_approval');
    expect(next.items.filter((item) => item.kind === 'approval')).toHaveLength(1);
  });

  it('DLG-09 Plan/Ask 发送保存轮次模式和权限档位，不在 reducer 中静默提升', () => {
    const next = reduceSessionAction(state(), {
      type: 'send',
      id: 'u-3',
      text: '规划',
      mode: 'plan',
      permissionProfile: 'full_access',
    });
    expect(next.items[0]).toMatchObject({ mode: 'plan', permissionProfile: 'full_access' });
  });

  it('DLG-10 历史恢复将消息和工具按真实历史重建', () => {
    const detail = {
      id: 'h-1',
      cliSessionId: 'cli-1',
      title: '历史',
      engine: 'claude-code' as const,
      model: 'm',
      cwd: 'C:/repo',
      status: 'done' as const,
      messageCount: 2,
      inputTokens: 1,
      outputTokens: 2,
      costUsd: 0,
      createdAt: 1,
      updatedAt: 2,
      messages: [
        { role: 'user' as const, text: '问题', ts: 1 },
        { role: 'assistant' as const, text: '回答', ts: 2 },
      ],
      toolCalls: [],
      checkpoints: [],
      approvals: [],
    };
    const items = itemsFromHistory(detail);
    expect(items.map((item) => item.kind)).toEqual(['user', 'assistant']);
    expect(items[1]).toMatchObject({ text: '回答' });
  });

  it('DLG-11 两个后台 Session 的 working/approval 状态按 historyId 隔离', () => {
    resetLiveSessionsForTests();
    registerLiveSession('h-1', 'handle-1', false);
    registerLiveSession('h-2', 'handle-2', false);
    applyEnvelopeToLiveRegistry(
      envelope('h-1', { type: 'turn_stage', sessionId: 'cli-1', stage: 'reasoning', ts: 1 }),
    );
    applyEnvelopeToLiveRegistry(
      envelope('h-2', {
        type: 'approval_request',
        sessionId: 'cli-2',
        id: 'a-2',
        action: 'Bash',
        detail: 'pwd',
        availableDecisions: ['allow', 'deny'],
      }),
    );
    expect(liveWorkingSessionIds()).toEqual(['h-1', 'h-2']);
    expect(livePendingApprovalSessionIds()).toEqual(['h-2']);
    resetLiveSessionsForTests();
  });

  it('DLG-12 stalled 有明确活动文案，避免用定时器伪造进度', () => {
    expect(
      activityPresentationParts(
        state({ status: 'working', turnActivity: { stage: 'stalled', since: 1000 } }),
        5000,
      )?.label,
    ).toContain('长时间没有新活动');
  });
});
