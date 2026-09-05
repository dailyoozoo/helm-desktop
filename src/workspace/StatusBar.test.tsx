import { describe, expect, it } from 'vitest';
import type { ThreadItem, TurnActivity } from '../engine/useSession';
import { statusBarLabel, statusBarModel } from './StatusBar';

const tool = (
  id: string,
  overrides: Partial<Extract<ThreadItem, { kind: 'tool' }>> = {},
): ThreadItem => ({
  kind: 'tool',
  id,
  name: 'Read',
  input: {},
  status: 'success',
  ...overrides,
});

describe('statusBarModel', () => {
  it('统计本轮工具数与无 diff 时的零改动', () => {
    const model = statusBarModel([tool('t1'), tool('t2'), { kind: 'user', id: 'u1', text: 'hi' }]);
    expect(model.tools).toBe(2);
    expect(model.files).toBe(0);
    expect(model.additions).toBe(0);
    expect(model.deletions).toBe(0);
  });

  it('从真实 diff 聚合文件数与 ±行数', () => {
    const items = [
      tool('t1', {
        diff: {
          path: 'src/a.ts',
          hunks: [
            {
              oldStart: 1,
              newStart: 1,
              lines: [
                { kind: 'add', text: '+x' },
                { kind: 'add', text: '+y' },
                { kind: 'del', text: '-z' },
              ],
            },
          ],
        },
      }),
      tool('t2', {
        diff: {
          path: 'src/b.ts',
          hunks: [
            {
              oldStart: 1,
              newStart: 1,
              lines: [{ kind: 'add', text: '+a' }],
            },
          ],
        },
      }),
      tool('t3', { diff: { path: 'src/empty.ts', hunks: [] } }),
    ];
    const model = statusBarModel(items);
    expect(model.tools).toBe(3);
    expect(model.files).toBe(2);
    expect(model.additions).toBe(3);
    expect(model.deletions).toBe(1);
  });

  it('按 activeTurnId 过滤：只计入本轮工具，不混入上一轮', () => {
    const items = [
      tool('t1', {
        turnId: 'turn-1',
        diff: {
          path: 'a.ts',
          hunks: [{ oldStart: 1, newStart: 1, lines: [{ kind: 'add', text: '+x' }] }],
        },
      }),
      tool('t2', { turnId: 'turn-1' }),
      tool('t3', {
        turnId: 'turn-2',
        diff: {
          path: 'b.ts',
          hunks: [{ oldStart: 1, newStart: 1, lines: [{ kind: 'del', text: '-y' }] }],
        },
      }),
    ];
    const model = statusBarModel(items, 'turn-2');
    expect(model.tools).toBe(1);
    expect(model.files).toBe(1);
    expect(model.additions).toBe(0);
    expect(model.deletions).toBe(1);
  });

  it('无 activeTurnId 时退化为全量统计（向后兼容）', () => {
    const items = [tool('t1', { turnId: 'turn-1' }), tool('t2', { turnId: 'turn-2' })];
    const model = statusBarModel(items);
    expect(model.tools).toBe(2);
  });
});

describe('statusBarLabel', () => {
  const cases: Array<[TurnActivity | null, string]> = [
    [null, '正在准备…'],
    [{ stage: 'preparing', since: 1 }, '正在执行…'],
    [{ stage: 'reasoning', since: 1 }, '正在思考…'],
    [{ stage: 'using_tool', since: 1, toolName: 'Edit' }, '正在执行 Edit'],
    [{ stage: 'using_tool', since: 1, toolName: 'Edit', target: 'src/a.ts' }, 'Edit · src/a.ts'],
    [{ stage: 'using_tool', since: 1 }, '正在执行工具…'],
    [{ stage: 'waiting_approval', since: 1 }, '等待审批…'],
    [{ stage: 'responding', since: 1 }, '正在回复…'],
    [{ stage: 'finalizing', since: 1 }, '正在收尾…'],
    [{ stage: 'stalled', since: 1 }, '执行受阻，等待处理…'],
  ];
  it.each(cases)('stage=%s 给出真实中文动作', (activity, expected) => {
    expect(statusBarLabel(activity)).toBe(expected);
  });
});

describe('StatusBar 组件已按原型移除（无 execbar）', () => {
  it('StatusBar 不再作为组件导出，保留派生 helper 供 workstrip 复用', () => {
    // 原型没有常驻执行状态条；statusBarModel/statusBarLabel 仍被 workstrip 显示当前动作复用。
    expect(typeof statusBarModel).toBe('function');
    expect(typeof statusBarLabel).toBe('function');
  });
});
