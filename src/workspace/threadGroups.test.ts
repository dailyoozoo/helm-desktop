import { describe, expect, it } from 'vitest';
import type { ThreadItem } from '../engine/useSession';
import {
  groupThreadItems,
  isLiftedFailureEntry,
  layoutThreadItems,
  type ThreadRenderEntry,
} from './threadGroups';

const tool = (id: string, turnId = 'turn-1'): Extract<ThreadItem, { kind: 'tool' }> => ({
  kind: 'tool',
  id,
  name: 'Read',
  input: {},
  status: 'success',
  turnId,
});

describe('groupThreadItems', () => {
  it('聚合同一轮相邻工具，并由非工具条目（正文/思考）切断', () => {
    const result = groupThreadItems([
      tool('a'),
      tool('b'),
      { kind: 'assistant', id: 'm', text: '阶段结论', turnId: 'turn-1' },
      tool('c'),
      tool('d'),
    ]);
    expect(result.map((entry) => entry.kind)).toEqual(['tool-group', 'item', 'tool-group']);
  });

  it('不跨 Turn 聚合', () => {
    const result = groupThreadItems([tool('a', 'turn-1'), tool('b', 'turn-2')]);
    expect(result.map((entry) => entry.kind)).toEqual(['item', 'item']);
  });

  it('稳定 Turn 关联不完整时不与新数据聚合', () => {
    const legacy = tool('a');
    legacy.turnId = undefined;
    const result = groupThreadItems([legacy, tool('b', 'turn-2')]);
    expect(result.map((entry) => entry.kind)).toEqual(['item', 'item']);
  });

  it('reverted 与未 reverted 的工具不合并为同一组', () => {
    const a = tool('a');
    const b = tool('b');
    b.reverted = true;
    const result = groupThreadItems([a, b]);
    expect(result.map((entry) => entry.kind)).toEqual(['item', 'item']);
  });

  it('Diff / 终端 / 写入工具也按同 Turn 相邻合并进工具组（B：不再独立拉出）', () => {
    const diff = tool('diff');
    diff.name = 'Edit';
    diff.diff = { path: 'a.ts', hunks: [] };
    const terminal = tool('terminal');
    terminal.name = 'Bash';
    terminal.input = { command: 'npm test' };
    const write = tool('write');
    write.name = 'Write';
    write.input = { file_path: 'new.ts', content: 'export {}' };
    const result = groupThreadItems([
      tool('a'),
      tool('b'),
      diff,
      tool('c'),
      tool('d'),
      terminal,
      write,
    ]);
    // 全部同 Turn 相邻 → 合并为单个工具组（B 不再把 diff/终端/写入独立成行）
    expect(result.map((entry) => entry.kind)).toEqual(['tool-group']);
    expect(result[0]).toMatchObject({
      kind: 'tool-group',
      items: [
        { id: 'a' },
        { id: 'b' },
        { id: 'diff' },
        { id: 'c' },
        { id: 'd' },
        { id: 'terminal' },
        { id: 'write' },
      ],
    });
  });

  it('工具组只切在相邻工具之间，被思考/正文切断后重新成组', () => {
    const result = groupThreadItems([
      tool('a'),
      tool('b'),
      { kind: 'thinking', id: 't', text: '分析', done: true, turnId: 'turn-1' },
      tool('c'),
      tool('d'),
    ]);
    expect(result.map((entry) => entry.kind)).toEqual(['tool-group', 'item', 'tool-group']);
  });

  it('相邻同 Turn 的子代理工具合入一张并行子代理卡', () => {
    const taskA = tool('task-a');
    taskA.name = 'Task';
    taskA.input = { description: '改 API 层' };
    const taskB = tool('task-b');
    taskB.name = 'Task';
    taskB.input = { description: '改 UI 层' };
    const read = tool('read');
    const result = groupThreadItems([taskA, taskB, read]);
    expect(result.map((entry) => entry.kind)).toEqual(['subagent', 'item']);
    expect(result[0]).toMatchObject({
      kind: 'subagent',
      items: [{ id: 'task-a' }, { id: 'task-b' }],
    });
  });

  it('单个子代理工具也渲染为子代理卡；不跨 Turn 合并', () => {
    const solo = tool('task-a', 'turn-1');
    solo.name = 'Agent';
    const other = tool('task-b', 'turn-2');
    other.name = 'Agent';
    const result = groupThreadItems([solo, other]);
    expect(result.map((entry) => entry.kind)).toEqual(['subagent', 'subagent']);
  });
});

describe('layoutThreadItems（渲染形态 B：平铺别名）', () => {
  it('直接复用 groupThreadItems，不重排、不包裹过程容器', () => {
    const items: ThreadItem[] = [
      { kind: 'thinking', id: 'thinking', text: '分析', done: true, turnId: 'turn-1' },
      tool('a'),
      tool('b'),
      { kind: 'assistant', id: 'final', text: '完成', turnId: 'turn-1' },
    ];
    const result = layoutThreadItems(items);
    // 思考 → 工具组 → 最终回答，平铺按真实时序，无 turn-process 包裹
    expect(result.map((entry) => entry.kind)).toEqual(['item', 'tool-group', 'item']);
    expect(result).toEqual(groupThreadItems(items));
  });
});

describe('isLiftedFailureEntry（失败工具提升为 children，TurnProcess 渲染契约）', () => {
  const failedTool = (id: string, partial: Partial<Extract<ThreadItem, { kind: 'tool' }>> = {}) => {
    const item = tool(id);
    item.status = 'error';
    return { kind: 'item' as const, item: { ...item, ...partial } };
  };

  it('独立失败工具提升：status=error 且无拒绝/复核 outcome', () => {
    expect(isLiftedFailureEntry(failedTool('a'))).toBe(true);
    expect(isLiftedFailureEntry(failedTool('b', { outcome: 'tool_failed' }))).toBe(true);
  });

  it('拒绝与 auto_review 复核态不提升：保持过程区内联呈现', () => {
    expect(isLiftedFailureEntry(failedTool('a', { outcome: 'runtime_denied' }))).toBe(false);
    expect(isLiftedFailureEntry(failedTool('b', { outcome: 'auto_review_unavailable' }))).toBe(
      false,
    );
    expect(isLiftedFailureEntry(failedTool('c', { outcome: 'auto_review_parse_error' }))).toBe(
      false,
    );
    expect(isLiftedFailureEntry(failedTool('d', { outcome: 'auto_review_blocked' }))).toBe(false);
  });

  it('非失败工具不提升', () => {
    expect(isLiftedFailureEntry({ kind: 'item', item: tool('ok') })).toBe(false);
    expect(
      isLiftedFailureEntry({ kind: 'item', item: { ...tool('pending'), status: 'pending' } }),
    ).toBe(false);
  });

  it('非工具条目与组/子代理条目一律不提升', () => {
    expect(
      isLiftedFailureEntry({
        kind: 'item',
        item: { kind: 'thinking', id: 't', text: '分析', done: true, turnId: 'turn-1' },
      }),
    ).toBe(false);
    const group: ThreadRenderEntry = {
      kind: 'tool-group',
      id: 'g1',
      items: [tool('g1-a'), { ...tool('g1-b'), outcome: 'runtime_denied' }],
    };
    expect(isLiftedFailureEntry(group)).toBe(false);
  });

  it('与分组的边界配合：失败工具被正文切断后单列成 item 条目（可提升），相邻则并入组走组头 pill', () => {
    const okA = tool('ok-a');
    const okB = tool('ok-b');
    const failed = tool('failed');
    failed.status = 'error';
    const result = groupThreadItems([
      okA,
      okB,
      { kind: 'assistant', id: 'm', text: '阶段结论', turnId: 'turn-1' },
      failed,
    ]);
    expect(result.map((entry) => entry.kind)).toEqual(['tool-group', 'item', 'item']);
    expect(isLiftedFailureEntry(result[2])).toBe(true);
    // 相邻不切断时：失败工具并入工具组，失败呈现走组头「N 失败」pill，不提升
    const grouped = groupThreadItems([okA, failed]);
    expect(grouped.map((entry) => entry.kind)).toEqual(['tool-group']);
    expect(isLiftedFailureEntry(grouped[0])).toBe(false);
  });
});
