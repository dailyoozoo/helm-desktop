import { setImmediate } from 'node:timers';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { EngineId, ReasoningEffortCapability } from '@helm/protocol';
import { PROVIDER_CONFIG_CHANGED_EVENT } from '../providers/api';
import { getReasoningEffortCapability } from './transport';
import {
  resetReasoningCapabilityCacheForTests,
  useReasoningEffortCapability,
} from './useReasoningEffortCapability';

const hooks = vi.hoisted(() => ({ useState: vi.fn(), useEffect: vi.fn() }));

vi.mock('react', () => hooks);
vi.mock('./transport', () => ({ getReasoningEffortCapability: vi.fn() }));

const supported: ReasoningEffortCapability = {
  support: 'supported',
  options: ['auto', 'low', 'high'],
  source: 'engine-probe',
};
const unsupported: ReasoningEffortCapability = {
  support: 'unsupported',
  options: ['auto'],
  source: 'engine-probe',
};

type Query = { engine: EngineId; model: string; providerId?: string };
type Effect = { deps: unknown[]; cleanup?: () => void };
const mounted = new Set<() => void>();

function mountCapability(initial: Query) {
  const states: unknown[] = [];
  const effects: Effect[] = [];
  let query = initial;
  let updates = 0;
  let disposed = false;

  function useRender(next = query) {
    query = next;
    let stateIndex = 0;
    let effectIndex = 0;
    const pendingEffects: (() => void)[] = [];
    hooks.useState.mockImplementation((initialValue: unknown) => {
      const index = stateIndex++;
      if (index === states.length) states.push(initialValue);
      return [
        states[index],
        (value: unknown) => {
          states[index] = typeof value === 'function' ? value(states[index]) : value;
          updates += 1;
        },
      ];
    });
    hooks.useEffect.mockImplementation((effect: () => (() => void) | void, deps: unknown[]) => {
      const index = effectIndex++;
      const previous = effects[index];
      if (previous && deps.every((value, at) => Object.is(value, previous.deps[at]))) return;
      pendingEffects.push(() => {
        previous?.cleanup?.();
        effects[index] = { deps, cleanup: effect() ?? undefined };
      });
    });
    const result = useReasoningEffortCapability(query.engine, query.model, query.providerId);
    for (const runEffect of pendingEffects) runEffect();
    return result;
  }

  function unmount() {
    if (disposed) return;
    disposed = true;
    for (const effect of effects) effect.cleanup?.();
    mounted.delete(unmount);
  }

  mounted.add(unmount);
  const harness = { read: useRender, unmount, updates: () => updates };
  harness.read();
  return harness;
}

function deferred() {
  let resolve!: (capability: ReasoningEffortCapability) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<ReasoningEffortCapability>((done, fail) => {
    resolve = done;
    reject = fail;
  });
  return { promise, resolve, reject };
}

function settleRequests(): Promise<void> {
  return new Promise((resolve) => setImmediate(resolve));
}

beforeEach(() => {
  vi.stubGlobal('window', new EventTarget());
  vi.mocked(getReasoningEffortCapability).mockReset();
  resetReasoningCapabilityCacheForTests();
});

afterEach(() => {
  for (const unmount of [...mounted]) unmount();
  resetReasoningCapabilityCacheForTests();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe('推理能力在途合并与失效', () => {
  it('同身份只合并在途请求，成功结果不永久缓存', async () => {
    vi.mocked(getReasoningEffortCapability).mockResolvedValue(supported);
    const query = { engine: 'codex', model: 'official-model', providerId: 'provider' } as const;
    const first = mountCapability(query);
    const second = mountCapability(query);
    expect(getReasoningEffortCapability).toHaveBeenCalledTimes(1);
    await settleRequests();
    expect(first.read()).toEqual({ capability: supported, loading: false, error: null });
    expect(second.read()).toEqual(first.read());

    const next = mountCapability(query);
    expect(getReasoningEffortCapability).toHaveBeenCalledTimes(2);
    await settleRequests();
    expect(next.read().capability).toEqual(supported);
  });

  it('引擎、服务商和模型分别隔离，带冒号的 ID 不会碰撞', () => {
    vi.mocked(getReasoningEffortCapability).mockImplementation(() => deferred().promise);
    mountCapability({ engine: 'codex', model: 'model', providerId: 'provider:part' });
    mountCapability({ engine: 'codex', model: 'part:model', providerId: 'provider' });
    mountCapability({ engine: 'claude-code', model: 'model', providerId: 'provider:part' });
    mountCapability({ engine: 'codex', model: 'another', providerId: 'provider:part' });
    expect(getReasoningEffortCapability).toHaveBeenCalledTimes(4);
  });

  it.each([PROVIDER_CONFIG_CHANGED_EVENT, 'focus'])(
    '%s 一次刷新所有订阅者，旧请求的 finally 不会移除新请求',
    async (eventName) => {
      const oldRequest = deferred();
      const nextRequest = deferred();
      vi.mocked(getReasoningEffortCapability)
        .mockReturnValueOnce(oldRequest.promise)
        .mockReturnValueOnce(nextRequest.promise);
      const query = { engine: 'codex', model: 'model', providerId: 'provider' } as const;
      const first = mountCapability(query);
      const second = mountCapability(query);
      window.dispatchEvent(new Event(eventName));
      expect(getReasoningEffortCapability).toHaveBeenCalledTimes(2);

      oldRequest.resolve(supported);
      await settleRequests();
      expect(first.read()).toEqual({ capability: null, loading: true, error: null });
      expect(second.read()).toEqual(first.read());

      const third = mountCapability(query);
      expect(getReasoningEffortCapability).toHaveBeenCalledTimes(2);
      nextRequest.resolve(unsupported);
      await settleRequests();
      for (const hook of [first, second, third]) {
        expect(hook.read()).toEqual({ capability: unsupported, loading: false, error: null });
      }
    },
  );

  it('旧请求失败不会覆盖刷新后的状态，也不会删除新在途请求', async () => {
    const oldRequest = deferred();
    const nextRequest = deferred();
    vi.mocked(getReasoningEffortCapability)
      .mockReturnValueOnce(oldRequest.promise)
      .mockReturnValueOnce(nextRequest.promise);
    const query = { engine: 'claude-code', model: 'model', providerId: 'provider' } as const;
    const first = mountCapability(query);
    window.dispatchEvent(new Event(PROVIDER_CONFIG_CHANGED_EVENT));
    oldRequest.reject(new Error('stale probe failed'));
    await settleRequests();
    expect(first.read()).toEqual({ capability: null, loading: true, error: null });
    mountCapability(query);
    expect(getReasoningEffortCapability).toHaveBeenCalledTimes(2);
    nextRequest.resolve(supported);
    await settleRequests();
    expect(first.read()).toEqual({ capability: supported, loading: false, error: null });
  });

  it('模型切换与卸载后，迟到的结果不再写入当前状态', async () => {
    const oldRequest = deferred();
    const nextRequest = deferred();
    vi.mocked(getReasoningEffortCapability)
      .mockReturnValueOnce(oldRequest.promise)
      .mockReturnValueOnce(nextRequest.promise);
    const hook = mountCapability({ engine: 'codex', model: 'first', providerId: 'provider' });
    hook.read({ engine: 'codex', model: 'second', providerId: 'provider' });
    oldRequest.resolve(supported);
    await settleRequests();
    expect(hook.read()).toEqual({ capability: null, loading: true, error: null });
    hook.unmount();
    const updates = hook.updates();
    nextRequest.resolve(unsupported);
    await settleRequests();
    expect(hook.updates()).toBe(updates);
  });

  it('当前失败可重试；刷新时先清除旧错误和能力', async () => {
    vi.mocked(getReasoningEffortCapability)
      .mockRejectedValueOnce(new Error('probe failed'))
      .mockResolvedValueOnce(supported);
    const hook = mountCapability({ engine: 'codex', model: 'model' });
    await settleRequests();
    expect(hook.read()).toEqual({ capability: null, loading: false, error: 'probe failed' });
    window.dispatchEvent(new Event('focus'));
    expect(hook.read()).toEqual({ capability: null, loading: true, error: null });
    await settleRequests();
    expect(hook.read()).toEqual({ capability: supported, loading: false, error: null });
    expect(getReasoningEffortCapability).toHaveBeenLastCalledWith('codex', 'model', undefined);
  });

  it('空模型不探测，切到空模型后旧结果也被丢弃', async () => {
    const pending = deferred();
    vi.mocked(getReasoningEffortCapability).mockReturnValue(pending.promise);
    const hook = mountCapability({ engine: 'codex', model: '  ' });
    expect(getReasoningEffortCapability).not.toHaveBeenCalled();
    expect(hook.read()).toEqual({ capability: null, loading: false, error: null });
    hook.read({ engine: 'codex', model: 'model' });
    hook.read({ engine: 'codex', model: '' });
    pending.resolve(supported);
    await settleRequests();
    expect(hook.read()).toEqual({ capability: null, loading: false, error: null });
  });

  it('共享事件监听在最后一个使用者卸载后移除，不遗留刷新任务', () => {
    const addListener = vi.spyOn(window, 'addEventListener');
    const removeListener = vi.spyOn(window, 'removeEventListener');
    vi.mocked(getReasoningEffortCapability).mockImplementation(() => deferred().promise);
    const query = { engine: 'codex', model: 'model' } as const;
    const first = mountCapability(query);
    const second = mountCapability(query);
    expect(addListener).toHaveBeenCalledTimes(2);
    first.unmount();
    expect(removeListener).not.toHaveBeenCalled();
    second.unmount();
    expect(removeListener).toHaveBeenCalledTimes(2);
    window.dispatchEvent(new Event('focus'));
    window.dispatchEvent(new Event(PROVIDER_CONFIG_CHANGED_EVENT));
    expect(getReasoningEffortCapability).toHaveBeenCalledTimes(1);
  });

  it('全部卸载后不保留旧在途探测，重新进入页面使用新请求', async () => {
    const oldRequest = deferred();
    const nextRequest = deferred();
    vi.mocked(getReasoningEffortCapability)
      .mockReturnValueOnce(oldRequest.promise)
      .mockReturnValueOnce(nextRequest.promise);
    const query = { engine: 'codex', model: 'model' } as const;
    mountCapability(query).unmount();
    window.dispatchEvent(new Event(PROVIDER_CONFIG_CHANGED_EVENT));
    const next = mountCapability(query);
    expect(getReasoningEffortCapability).toHaveBeenCalledTimes(2);
    oldRequest.resolve(supported);
    await settleRequests();
    expect(next.read()).toEqual({ capability: null, loading: true, error: null });
    nextRequest.resolve(unsupported);
    await settleRequests();
    expect(next.read().capability).toEqual(unsupported);
  });
});
