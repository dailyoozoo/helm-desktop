import { describe, expect, it, vi } from 'vitest';
import { applyMcpDisabledTransaction, McpDisabledSyncQueue } from './useSession';

function deferred() {
  let resolve!: () => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<void>((done, fail) => {
    resolve = done;
    reject = fail;
  });
  return { promise, resolve, reject };
}

describe('applyMcpDisabledTransaction', () => {
  it('下发失败时 toast 并回滚到原状态', async () => {
    const states: string[][] = [];
    const onFailed = vi.fn();

    const applied = await applyMcpDisabledTransaction({
      handle: 'handle-1',
      next: ['server-a'],
      rollback: [],
      sync: async () => {
        throw new Error('IPC failed');
      },
      dispatch: (disabled) => states.push(disabled),
      onFailed,
    });

    expect(applied).toBe(false);
    expect(states).toEqual([['server-a'], []]);
    expect(onFailed).toHaveBeenCalledWith('IPC failed');
  });

  it('下发成功时保留新状态', async () => {
    const states: string[][] = [];
    const applied = await applyMcpDisabledTransaction({
      handle: 'handle-1',
      next: ['server-a'],
      rollback: [],
      sync: async () => {},
      dispatch: (disabled) => states.push(disabled),
      onFailed: vi.fn(),
    });

    expect(applied).toBe(true);
    expect(states).toEqual([['server-a']]);
  });

  it('串行下发快速切换，旧请求失败不会回滚更新的选择', async () => {
    const first = deferred();
    const calls: string[][] = [];
    const states: string[][] = [];
    const onFailed = vi.fn();
    const queue = new McpDisabledSyncQueue(
      'handle-1',
      [],
      async (_handle, disabled) => {
        calls.push(disabled);
        if (calls.length === 1) await first.promise;
      },
      (disabled) => states.push([...disabled]),
      onFailed,
    );

    const oldRequest = queue.update(['server-a']);
    const latestRequest = queue.update(['server-a', 'server-b']);
    expect(calls).toEqual([]);
    await Promise.resolve();
    expect(calls).toEqual([['server-a']]);

    first.reject(new Error('old failed'));
    expect(await oldRequest).toBe(false);
    expect(await latestRequest).toBe(true);
    expect(calls).toEqual([['server-a'], ['server-a', 'server-b']]);
    expect(states).toEqual([['server-a'], ['server-a', 'server-b']]);
    expect(queue.current()).toEqual(['server-a', 'server-b']);
    expect(onFailed).toHaveBeenCalledWith('old failed');
  });

  it('最新请求失败时回滚到最后一次后端确认状态', async () => {
    const states: string[][] = [];
    const queue = new McpDisabledSyncQueue(
      'handle-1',
      [],
      async () => {
        throw new Error('latest failed');
      },
      (disabled) => states.push([...disabled]),
      vi.fn(),
    );

    expect(await queue.update(['server-a'])).toBe(false);
    expect(states).toEqual([['server-a'], []]);
    expect(queue.current()).toEqual([]);
  });
});
