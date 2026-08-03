import { describe, expect, it } from 'vitest';
import type { ThreadItem } from '../engine/useSession';
import { activityLogGroups } from './activityLog';

describe('activityLogGroups', () => {
  it('优先按稳定 turnId 分组，并过滤回溯活动', () => {
    const items: ThreadItem[] = [
      { kind: 'tool', id: 'a', name: 'Read', input: {}, status: 'success', turnId: 'turn-1' },
      { kind: 'tool', id: 'b', name: 'Edit', input: {}, status: 'success', turnId: 'turn-2' },
      {
        kind: 'tool',
        id: 'c',
        name: 'Bash',
        input: {},
        status: 'error',
        turnId: 'turn-2',
        reverted: true,
      },
    ];
    const groups = activityLogGroups(items);
    expect(groups.map((group) => group.items.map((item) => item.id))).toEqual([['a'], ['b']]);
  });
});
