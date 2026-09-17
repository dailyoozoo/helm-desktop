import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { AgentEventEnvelope, TurnStreamFailure } from '@helm/protocol';
import { showToast } from '../components/toast';
import {
  closeSession,
  createSession,
  getTurnSnapshot,
  interrupt,
  respondApproval,
  sendMessage,
  setSessionMcpDisabled,
} from './transport';
import {
  liveSessionHandle,
  liveSessionWorking,
  resetLiveSessionsForTests,
  useSession as runSessionHook,
} from './useSession';

const runtime = vi.hoisted(() => {
  const slots: unknown[] = [];
  let cursor = 0;
  const effects: Array<() => void> = [];
  const cleanups = new Set<() => void>();
  function nextSlot<Value>(initialize: () => Value): Value {
    const index = cursor++;
    if (!(index in slots)) slots[index] = initialize();
    return slots[index] as Value;
  }
  return {
    beginRender: () => {
      cursor = 0;
    },
    flushEffects: () => {
      for (const effect of effects.splice(0)) effect();
    },
    unmount: () => {
      for (const cleanup of cleanups) cleanup();
      cleanups.clear();
    },
    reset: () => {
      slots.length = 0;
      effects.length = 0;
      cursor = 0;
    },
    useRef: <Value>(current: Value) => nextSlot(() => ({ current })),
    useCallback: <Callback>(callback: Callback) => callback,
    useReducer: <State, Action, Initial>(
      reducer: (state: State, action: Action) => State,
      initial: Initial,
      initialize: (initial: Initial) => State,
    ) => {
      const slot = nextSlot(() => ({ state: initialize(initial) }));
      return [
        slot.state,
        (action: Action) => {
          slot.state = reducer(slot.state, action);
        },
      ];
    },
    useEffect: (effect: () => void | (() => void), dependencies?: readonly unknown[]) => {
      const slot = nextSlot(() => ({
        dependencies: undefined as readonly unknown[] | undefined,
        cleanup: undefined as (() => void) | undefined,
      }));
      if (
        dependencies &&
        slot.dependencies &&
        dependencies.length === slot.dependencies.length &&
        dependencies.every((value, index) => Object.is(value, slot.dependencies?.[index]))
      )
        return;
      slot.dependencies = dependencies;
      effects.push(() => {
        if (slot.cleanup) {
          slot.cleanup();
          cleanups.delete(slot.cleanup);
        }
        slot.cleanup = effect() || undefined;
        if (slot.cleanup) cleanups.add(slot.cleanup);
      });
    },
  };
});

const bridge = vi.hoisted(() => ({
  listener: undefined as ((envelope: AgentEventEnvelope) => void) | undefined,
  failureListener: undefined as ((failure: TurnStreamFailure) => void) | undefined,
}));

vi.mock('react', async () => ({
  ...(await vi.importActual<typeof import('react')>('react')),
  useRef: runtime.useRef,
  useCallback: runtime.useCallback,
  useReducer: runtime.useReducer,
  useEffect: runtime.useEffect,
}));

vi.mock('../components/toast', () => ({ showToast: vi.fn() }));
vi.mock('./transport', async () => ({
  ...(await vi.importActual<typeof import('./transport')>('./transport')),
  createSession: vi.fn(),
  closeSession: vi.fn(),
  sendMessage: vi.fn(),
  interrupt: vi.fn(),
  setSessionMcpDisabled: vi.fn(),
  getTurnSnapshot: vi.fn(),
  respondApproval: vi.fn(),
  onAgentEvent: vi.fn(async (listener: (envelope: AgentEventEnvelope) => void) => {
    bridge.listener = listener;
    return () => {};
  }),
  onTurnStreamFailure: vi.fn(async (listener: (failure: TurnStreamFailure) => void) => {
    bridge.failureListener = listener;
    return () => {};
  }),
}));

function deferred<Value>() {
  let resolve!: (value: Value) => void;
  let reject!: (error: Error) => void;
  const promise = new Promise<Value>((done, failed) => {
    resolve = done;
    reject = failed;
  });
  return { promise, resolve, reject };
}

function renderSession() {
  runtime.beginRender();
  const session = runSessionHook();
  runtime.flushEffects();
  return session;
}

async function flushPromises() {
  for (let microtask = 0; microtask < 8; microtask += 1) await Promise.resolve();
}

beforeEach(() => {
  runtime.reset();
  resetLiveSessionsForTests();
  vi.clearAllMocks();
  vi.stubGlobal('window', Object.assign(new EventTarget(), { setTimeout: globalThis.setTimeout }));
  vi.stubGlobal('localStorage', { getItem: () => null, setItem: vi.fn(), removeItem: vi.fn() });
  vi.mocked(createSession).mockReset().mockResolvedValue('handle');
  vi.mocked(closeSession).mockReset().mockResolvedValue(undefined);
  vi.mocked(sendMessage).mockReset().mockResolvedValue(undefined);
  vi.mocked(interrupt).mockReset().mockResolvedValue(undefined);
  vi.mocked(respondApproval).mockReset().mockResolvedValue(undefined);
  vi.mocked(setSessionMcpDisabled).mockReset().mockResolvedValue(undefined);
  vi.mocked(getTurnSnapshot).mockReset().mockResolvedValue(null);
});

afterEach(() => {
  runtime.unmount();
  resetLiveSessionsForTests();
  vi.unstubAllGlobals();
});

describe('useSession stream delivery failures', () => {
  const failure: TurnStreamFailure = {
    historyId: 'handle',
    turnId: 'turn',
    turnEpoch: 1,
    attemptNo: 1,
    runtimeGenerationId: 'runtime-1',
    message: '消息流保存失败，已请求停止执行；部分显示内容可能尚未保存。',
  };

  it('settles visible work once without losing partial content or reopening on late IPC', async () => {
    const delivery = deferred<void>();
    vi.mocked(sendMessage).mockReturnValueOnce(delivery.promise);
    const sending = renderSession().send('request');
    await flushPromises();
    for (const event of [
      { type: 'message_delta', sessionId: 'native', role: 'assistant', text: 'partial reply' },
      { type: 'thinking_delta', sessionId: 'native', text: 'public summary' },
      {
        type: 'approval_request',
        sessionId: 'native',
        id: 'approval',
        action: 'Bash',
        detail: 'command',
        availableDecisions: ['allow', 'deny'],
      },
    ] satisfies AgentEventEnvelope['event'][]) {
      bridge.listener?.({ ...failure, event });
    }
    bridge.failureListener?.(failure);
    bridge.failureListener?.(failure);
    delivery.resolve(undefined);
    await sending;
    const state = renderSession().state;
    expect(state).toMatchObject({ status: 'idle', turnActivity: null });
    expect(state.items.filter((item) => item.kind === 'error')).toHaveLength(1);
    expect(state.items.find((item) => item.kind === 'assistant')).toMatchObject({
      text: 'partial reply',
    });
    expect(state.items.find((item) => item.kind === 'thinking')).toMatchObject({ done: true });
    const approval = state.items.find((item) => item.kind === 'approval');
    expect(approval).toMatchObject({ status: 'resolved', availableDecisions: [] });
    expect(approval).not.toHaveProperty('decision');
    expect(liveSessionWorking('handle')).toBe(false);
    expect(showToast).not.toHaveBeenCalled();
    bridge.listener?.({
      ...failure,
      event: { type: 'message_delta', sessionId: 'native', role: 'assistant', text: 'late' },
    });
    expect(renderSession().state).toBe(state);
    expect(sendMessage).toHaveBeenCalledTimes(1);
  });

  it('does not let a completed turn failure cancel a new send before its first event', async () => {
    await renderSession().send('first');
    bridge.listener?.({
      ...failure,
      event: { type: 'turn_complete', sessionId: 'native', stopReason: 'end' },
    });
    await renderSession().send('second');
    bridge.failureListener?.(failure);
    expect(renderSession().state.status).toBe('working');
    expect(liveSessionWorking('handle')).toBe(true);
    expect(renderSession().state.items.some((item) => item.kind === 'error')).toBe(false);
  });

  it('reports background failures once after the workspace unmounts', async () => {
    await renderSession().send('request');
    runtime.unmount();
    bridge.failureListener?.(failure);
    bridge.failureListener?.(failure);
    expect(liveSessionWorking('handle')).toBe(false);
    expect(showToast).toHaveBeenCalledTimes(1);
    expect(showToast).toHaveBeenCalledWith(failure.message, 'error');
  });
});

describe('useSession send cancellation', () => {
  it('cancels during creation and closes the late-created handle without sending', async () => {
    const creation = deferred<string>();
    vi.mocked(createSession).mockReturnValueOnce(creation.promise);
    const sending = renderSession().send('request');
    expect(renderSession().state.status).toBe('working');
    await renderSession().stop();
    expect(renderSession().state.status).toBe('idle');
    creation.resolve('cancelled-handle');
    expect(await sending).toBe(false);
    expect(sendMessage).not.toHaveBeenCalled();
    expect(vi.mocked(closeSession).mock.calls).toEqual([['cancelled-handle']]);
    expect(renderSession().state.handleId).toBeNull();
    expect(interrupt).not.toHaveBeenCalled();
  });

  it.each(['resolve', 'reject'] as const)(
    'ignores stale creation %s and finally without clearing a newer send',
    async (outcome) => {
      const creation = deferred<string>();
      const delivery = deferred<void>();
      vi.mocked(createSession)
        .mockReturnValueOnce(creation.promise)
        .mockResolvedValueOnce('new-handle');
      vi.mocked(sendMessage).mockReturnValueOnce(delivery.promise);
      const oldSend = renderSession().send('old');
      await renderSession().stop();
      const newSend = renderSession().send('new');
      await flushPromises();
      if (outcome === 'resolve') creation.resolve('cancelled-handle');
      else creation.reject(new Error('stale creation failure'));
      expect(await oldSend).toBe(false);
      expect(await renderSession().send('duplicate while pending')).toBe(false);
      expect(createSession).toHaveBeenCalledTimes(2);
      expect(renderSession().state).toMatchObject({
        handleId: 'new-handle',
        historyId: 'new-handle',
        status: 'working',
      });
      expect(renderSession().state.items.map((item) => item.kind)).toEqual(['user', 'user']);
      expect(liveSessionHandle('new-handle')).toBe('new-handle');
      expect(liveSessionWorking('new-handle')).toBe(true);
      expect(showToast).not.toHaveBeenCalled();
      expect(closeSession).not.toHaveBeenCalledWith('new-handle');
      if (outcome === 'resolve')
        expect(vi.mocked(closeSession).mock.calls).toEqual([['cancelled-handle']]);
      delivery.resolve(undefined);
      expect(await newSend).toBe(true);
    },
  );

  it('closes an unpublished handle when stopped during MCP preparation, isolating the next send', async () => {
    const preparation = deferred<void>();
    const delivery = deferred<void>();
    vi.mocked(createSession)
      .mockResolvedValueOnce('old-handle')
      .mockResolvedValueOnce('new-handle');
    vi.mocked(setSessionMcpDisabled).mockReturnValueOnce(preparation.promise);
    vi.mocked(sendMessage).mockReturnValueOnce(delivery.promise);
    await renderSession().toggleMcpServer('server');
    const oldSend = renderSession().send('old');
    await flushPromises();
    expect(setSessionMcpDisabled).toHaveBeenCalledWith('old-handle', ['server']);
    expect(sendMessage).not.toHaveBeenCalled();
    await renderSession().stop();
    const newSend = renderSession().send('new');
    await flushPromises();
    preparation.reject(new Error('stale MCP failure'));
    expect(await oldSend).toBe(false);
    expect(vi.mocked(closeSession).mock.calls).toEqual([['old-handle']]);
    expect(renderSession().state).toMatchObject({
      handleId: 'new-handle',
      status: 'working',
      disabledMcp: ['server'],
    });
    expect(vi.mocked(sendMessage).mock.calls).toEqual([
      ['new-handle', 'new', [], 'build', undefined, '', 'auto'],
    ]);
    expect(await renderSession().send('duplicate')).toBe(false);
    expect(showToast).not.toHaveBeenCalled();
    delivery.resolve(undefined);
    expect(await newSend).toBe(true);
  });

  it('waits for pending IPC rejection before reporting a prepared send as stopped', async () => {
    const delivery = deferred<void>();
    vi.mocked(sendMessage).mockReturnValueOnce(delivery.promise);
    const sending = renderSession().send('request');
    await flushPromises();
    const stopping = renderSession().stop();
    await flushPromises();
    expect(vi.mocked(interrupt).mock.calls).toEqual([['handle']]);
    expect(renderSession().state.status).toBe('working');
    delivery.reject(new Error('dispatch cancelled'));
    expect(await sending).toBe(false);
    await stopping;
    expect(renderSession().state.status).toBe('idle');
    expect(liveSessionWorking('handle')).toBe(false);
    expect(renderSession().state.items.map((item) => item.kind)).toEqual(['user']);
    expect(getTurnSnapshot).not.toHaveBeenCalled();
    expect(showToast).not.toHaveBeenCalled();
  });

  it('settles active entities only after an accepted send has a terminal backend snapshot', async () => {
    await renderSession().send('request');
    bridge.listener?.({
      historyId: 'handle',
      turnId: 'turn',
      event: {
        type: 'tool_call',
        sessionId: 'native',
        id: 'tool',
        name: 'Bash',
        input: {},
        status: 'pending',
      },
    });
    bridge.listener?.({
      historyId: 'handle',
      turnId: 'turn',
      event: { type: 'thinking_delta', sessionId: 'native', text: 'public reasoning' },
    });
    const snapshot = deferred<Awaited<ReturnType<typeof getTurnSnapshot>>>();
    vi.mocked(getTurnSnapshot).mockReturnValueOnce(snapshot.promise);
    const stopping = renderSession().stop();
    await flushPromises();
    expect(renderSession().state.status).toBe('working');
    snapshot.resolve({
      historySessionId: 'handle',
      turnId: 'turn',
      turnEpoch: 1,
      status: 'interrupted',
      recoverable: false,
      eventSeq: 4,
      updatedAt: 4,
      mode: 'build',
      permissionProfile: 'standard',
      startedAt: 1,
    });
    await stopping;
    const state = renderSession().state;
    expect(state).toMatchObject({ status: 'idle', openThinkingId: null });
    expect(state.items.find((item) => item.kind === 'tool')).toMatchObject({
      status: 'error',
      turnStatus: 'interrupted',
    });
    expect(state.items.find((item) => item.kind === 'thinking')).toMatchObject({
      done: true,
      turnStatus: 'interrupted',
    });
    expect(liveSessionWorking('handle')).toBe(false);
  });

  it('suppresses a stale Stop failure after resetting into a newer session', async () => {
    vi.mocked(createSession)
      .mockResolvedValueOnce('old-handle')
      .mockResolvedValueOnce('new-handle');
    await renderSession().send('old');
    const cancellation = deferred<void>();
    vi.mocked(interrupt).mockReturnValueOnce(cancellation.promise);
    const stopping = renderSession().stop();
    renderSession().reset();
    expect(await renderSession().send('new')).toBe(true);
    cancellation.reject(new Error('old Stop failed'));
    await stopping;
    expect(renderSession().state).toMatchObject({ status: 'working', historyId: 'new-handle' });
    expect(showToast).not.toHaveBeenCalled();
  });

  it('does not send a late-created session after unmounting', async () => {
    const creation = deferred<string>();
    vi.mocked(createSession).mockReturnValueOnce(creation.promise);
    const sending = renderSession().send('request');
    runtime.unmount();
    creation.resolve('unmounted-handle');
    expect(await sending).toBe(false);
    expect(vi.mocked(closeSession).mock.calls).toEqual([['unmounted-handle']]);
    expect(sendMessage).not.toHaveBeenCalled();
  });

  it.each(['resolve', 'reject'] as const)(
    'does not reopen terminal approval or activity when approval IPC later %s',
    async (outcome) => {
      await renderSession().send('request');
      bridge.listener?.({
        historyId: 'handle',
        turnId: 'turn',
        event: {
          type: 'approval_request',
          sessionId: 'native',
          id: 'approval',
          action: 'Bash',
          detail: 'command',
          availableDecisions: ['allow', 'deny'],
        },
      });
      const decision = deferred<void>();
      vi.mocked(respondApproval).mockReturnValueOnce(decision.promise);
      const approving = renderSession().approve('approval', 'allow');
      bridge.listener?.({
        historyId: 'handle',
        turnId: 'turn',
        event: { type: 'error', message: 'runtime failed', recoverable: false },
      });
      if (outcome === 'resolve') decision.resolve(undefined);
      else decision.reject(new Error('late approval failure'));
      await approving;
      const state = renderSession().state;
      expect(state.status).toBe('idle');
      const approval = state.items.find((item) => item.kind === 'approval');
      expect(approval).toMatchObject({
        status: 'resolved',
        turnStatus: 'failed',
        availableDecisions: [],
      });
      expect(approval).not.toHaveProperty('decision');
      expect(liveSessionWorking('handle')).toBe(false);
      expect(showToast).not.toHaveBeenCalled();
    },
  );
});
