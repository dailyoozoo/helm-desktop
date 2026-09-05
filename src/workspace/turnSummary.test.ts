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
});

describe('formatTurnDuration', () => {
  it('formats seconds and minutes', () => {
    expect(formatTurnDuration(42)).toBe('42秒');
    expect(formatTurnDuration(61)).toBe('1分1秒');
    expect(formatTurnDuration(120)).toBe('2分0秒');
  });
});
