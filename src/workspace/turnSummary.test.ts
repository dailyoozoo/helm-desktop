import { describe, expect, it } from 'vitest';
import type { ThreadItem } from '../engine/useSession';
import type { ThreadRenderEntry } from './threadGroups';
import { formatTurnDuration, summarizeTurn, turnDiffStats } from './turnSummary';

function toolItem(
  overrides: Partial<Extract<ThreadItem, { kind: 'tool' }>> = {},
): Extract<ThreadItem, { kind: 'tool' }> {
  return {
    kind: 'tool' as const,
    id: 't1',
    name: 'Edit',
    input: {},
    status: 'success' as const,
    ...overrides,
  };
}

function diffHunk(lines: Array<{ kind: 'add' | 'del' | 'ctx'; text: string }>) {
  return { oldStart: 1, newStart: 1, lines };
}

describe('turnDiffStats', () => {
  it('counts tools and diff lines across groups and standalone items', () => {
    const entries: ThreadRenderEntry[] = [
      {
        kind: 'tool-group',
        id: 'g1',
        items: [
          toolItem({
            id: 'a',
            diff: {
              path: 'a.ts',
              hunks: [
                diffHunk([
                  { kind: 'add', text: '+' },
                  { kind: 'del', text: '-' },
                ]),
              ],
            },
          }),
          toolItem({ id: 'b' }),
        ],
      },
      {
        kind: 'item',
        item: toolItem({
          id: 'c',
          diff: { path: 'b.ts', hunks: [diffHunk([{ kind: 'add', text: '+' }])] },
        }),
      },
    ];
    expect(turnDiffStats(entries)).toEqual({ added: 2, removed: 1, toolCount: 3 });
  });

  it('returns zeros for turns without tools', () => {
    expect(turnDiffStats([])).toEqual({ added: 0, removed: 0, toolCount: 0 });
  });
});

describe('summarizeTurn', () => {
  it('carries model from TurnLedger and duration from turn boundaries', () => {
    const summary = summarizeTurn([{ kind: 'item', item: toolItem({ id: 'a' }) }], 2, {
      id: 'turn-1',
      epoch: 1,
      mode: 'build',
      permissionProfile: 'standard',
      status: 'succeeded',
      startedAt: 1000,
      endedAt: 6500,
      routedModelId: 'claude-sonnet-4.6',
    });
    expect(summary).toMatchObject({
      turnNumber: 2,
      model: 'claude-sonnet-4.6',
      durationSec: 5.5,
      toolCount: 1,
    });
  });

  it('omits missing fields instead of placeholder values', () => {
    const summary = summarizeTurn([], 1, null);
    expect(summary).toEqual({ turnNumber: 1, toolCount: 0 });
    expect(summary.model).toBeUndefined();
    expect(summary.durationSec).toBeUndefined();
    expect(summary.added).toBeUndefined();
    expect(summary.removed).toBeUndefined();
  });

  it('derives duration from item timestamps when turn ledger lacks endedAt', () => {
    const entries: ThreadRenderEntry[] = [
      {
        kind: 'item',
        item: toolItem({ id: 'a', startedAt: 1000, endedAt: 2000 }),
      },
      {
        kind: 'item',
        item: toolItem({ id: 'b', startedAt: 1000, endedAt: 4000 }),
      },
    ];
    expect(summarizeTurn(entries, 1, null).durationSec).toBe(3);
  });

  it('does not estimate duration while the turn is still running', () => {
    const entries: ThreadRenderEntry[] = [
      { kind: 'item', item: toolItem({ id: 'a', status: 'pending', startedAt: 1000 }) },
    ];
    expect(summarizeTurn(entries, 1, null).durationSec).toBeUndefined();
  });

  it('omits zero-valued add/removed counts', () => {
    const summary = summarizeTurn([{ kind: 'item', item: toolItem({ id: 'a' }) }], 1, null);
    expect(summary.added).toBeUndefined();
    expect(summary.removed).toBeUndefined();
  });

  it('keeps a completed summary reference stable without rereading timestamps', () => {
    let timestampReads = 0;
    const tool = toolItem({
      turnStatus: 'succeeded',
      endedAt: 2_000,
    });
    Object.defineProperty(tool, 'startedAt', {
      get: () => {
        timestampReads += 1;
        return 1_000;
      },
    });
    const entries = (): ThreadRenderEntry[] => [{ kind: 'tool-group', id: 'group', items: [tool] }];
    const first = summarizeTurn(entries(), 1);
    const readsAfterFirst = timestampReads;
    expect(first.durationSec).toBe(1);
    for (let delta = 0; delta < 1_000; delta += 1) expect(summarizeTurn(entries(), 1)).toBe(first);
    expect(timestampReads).toBe(readsAfterFirst);
  });

  it('invalidates when an earlier item, ordinal or ledger snapshot changes', () => {
    const first = toolItem({ id: 'first', turnStatus: 'succeeded' });
    const last: ThreadItem = {
      kind: 'assistant',
      id: 'answer',
      text: 'done',
      turnStatus: 'succeeded',
    };
    const entries: ThreadRenderEntry[] = [
      { kind: 'item', item: first },
      { kind: 'item', item: last },
    ];
    const summary = summarizeTurn(entries, 1);
    const changed = [
      {
        kind: 'item' as const,
        item: {
          ...first,
          diff: { path: 'file', hunks: [diffHunk([{ kind: 'add', text: 'added' }])] },
        },
      },
      entries[1],
    ];
    const refreshed = summarizeTurn(changed, 1);
    expect(refreshed).not.toBe(summary);
    expect(refreshed.added).toBe(1);
    expect(summarizeTurn(changed, 2)).toMatchObject({ turnNumber: 2, added: 1 });
    const ledger = {
      id: 'turn',
      epoch: 1,
      mode: 'build' as const,
      permissionProfile: 'standard' as const,
      status: 'succeeded' as const,
      startedAt: 1_000,
      endedAt: 2_000,
      routedModelId: 'model-a',
    };
    const routed = summarizeTurn(changed, 2, ledger);
    expect(summarizeTurn(changed, 2, ledger)).toBe(routed);
    const rerouted = summarizeTurn(changed, 2, { ...ledger, routedModelId: 'model-b' });
    expect(rerouted).not.toBe(routed);
    expect(rerouted.model).toBe('model-b');
  });

  it('does not cache an active turn as completed', () => {
    const tool = toolItem({ status: 'pending', startedAt: 1_000 });
    const entries: ThreadRenderEntry[] = [{ kind: 'item', item: tool }];
    expect(summarizeTurn(entries, 1)).not.toBe(summarizeTurn(entries, 1));
    expect(summarizeTurn(entries, 1).durationSec).toBeUndefined();
  });
});

describe('formatTurnDuration', () => {
  it('formats seconds and minutes', () => {
    expect(formatTurnDuration(42)).toBe('42秒');
    expect(formatTurnDuration(61)).toBe('1分1秒');
    expect(formatTurnDuration(120)).toBe('2分0秒');
  });
});
