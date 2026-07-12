import { describe, expect, it, vi } from 'vitest';
import { LatestSerialSaver } from './latestSerialSaver';

function deferred() {
  let resolve!: () => void;
  const promise = new Promise<void>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}

describe('LatestSerialSaver', () => {
  it('debounces rapid changes and saves only the latest snapshot', async () => {
    vi.useFakeTimers();
    const saved: number[] = [];
    const saver = new LatestSerialSaver<number>(async (value) => {
      saved.push(value);
    }, 400);

    saver.schedule(1);
    saver.schedule(2);
    saver.schedule(3);
    await vi.advanceTimersByTimeAsync(400);
    await saver.whenIdle();

    expect(saved).toEqual([3]);
    vi.useRealTimers();
  });

  it('serializes saves so an older request cannot finish after a newer request', async () => {
    vi.useFakeTimers();
    const first = deferred();
    const calls: number[] = [];
    const saver = new LatestSerialSaver<number>(async (value) => {
      calls.push(value);
      if (value === 1) await first.promise;
    }, 100);

    saver.schedule(1);
    await vi.advanceTimersByTimeAsync(100);
    saver.schedule(2);
    await vi.advanceTimersByTimeAsync(100);
    expect(calls).toEqual([1]);

    first.resolve();
    await saver.whenIdle();
    expect(calls).toEqual([1, 2]);
    vi.useRealTimers();
  });
});
