import { describe, expect, it } from 'vitest';
import type { Diff, DiffLine } from '@helm/protocol';
import type { ThreadItem } from '../engine/useSession';
import { contextPanelData } from './contextPanelViewModel';
import { contextSnapshot } from './contextSnapshotViewModel';
import { diffStats } from './diffStats';
import { statusBarModel } from './StatusBar';
import { summarizeTurn } from './turnSummary';
import type { ThreadRenderEntry } from './threadGroups';

describe('immutable diff statistics cache', () => {
  it('caches by diff identity, not file path or hunk identity', () => {
    const diff: Diff = {
      path: 'same.ts',
      hunks: [{ oldStart: 1, newStart: 1, lines: [{ kind: 'add', text: 'new' }] }],
    };
    const original = diffStats(diff);
    expect(original).toEqual({ added: 1, removed: 0 });
    expect(diffStats(diff)).toBe(original);
    const changed: Diff = {
      ...diff,
      hunks: [{ ...diff.hunks[0], lines: [{ kind: 'del', text: 'old' }] }],
    };
    expect(diffStats(changed)).toEqual({ added: 0, removed: 1 });
    expect(diffStats(changed)).not.toBe(original);
    expect(diffStats(diff)).toBe(original);
  });

  it('reads 100,000 historical lines once across panel, status and repeated streaming summaries', () => {
    let lineReads = 0;
    const kinds: DiffLine['kind'][] = ['add', 'del', 'ctx'];
    const tools: Extract<ThreadItem, { kind: 'tool' }>[] = Array.from(
      { length: 200 },
      (_, toolIndex) => ({
        kind: 'tool',
        id: `tool-${toolIndex}`,
        name: 'Edit',
        input: {},
        status: 'success',
        turnId: 'completed-turn',
        turnStatus: 'succeeded',
        diff: {
          path: `file-${toolIndex}.ts`,
          hunks: [
            {
              oldStart: 1,
              newStart: 1,
              lines: Array.from({ length: 500 }, (_, lineIndex) => ({
                get kind() {
                  lineReads += 1;
                  return kinds[lineIndex % kinds.length];
                },
                text: 'line',
              })),
            },
          ],
        },
      }),
    );
    const entries = (): ThreadRenderEntry[] => tools.map((item) => ({ kind: 'item', item }));
    expect(contextSnapshot({ items: tools }).messageCount).toBe(0);
    expect(lineReads).toBe(0);
    const summary = summarizeTurn(entries(), 1);
    expect(summary).toMatchObject({ toolCount: 200, added: 33_400, removed: 33_400 });
    expect(lineReads).toBe(100_000);
    for (let delta = 0; delta < 64; delta += 1) {
      const items: ThreadItem[] = [
        ...tools,
        { kind: 'assistant', id: 'stream', text: 'x'.repeat(delta + 1), turnId: 'active-turn' },
      ];
      expect(contextSnapshot({ items }).messageCount).toBe(1);
      expect(contextPanelData(items).changedFiles).toHaveLength(200);
      expect(statusBarModel(items)).toEqual({
        tools: 200,
        files: 200,
        additions: 33_400,
        deletions: 33_400,
      });
      expect(summarizeTurn(entries(), 1)).toBe(summary);
    }
    expect(lineReads).toBe(100_000);
  });
});
