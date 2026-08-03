import { describe, expect, it } from 'vitest';
import type { ThreadItem } from '../engine/useSession';
import { groupThreadItems, layoutThreadItems } from './threadGroups';

const tool = (id: string, turnId = 'turn-1'): Extract<ThreadItem, { kind: 'tool' }> => ({
  kind: 'tool',
  id,
  name: 'Read',
  input: {},
  status: 'success',
  turnId,
});

describe('groupThreadItems', () => {
  it('聚合同一轮相邻工具，并由正文切断', () => {
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

  it('Diff 和终端工具始终独立，并切断前后工具组', () => {
    const diff = tool('diff');
    diff.name = 'Edit';
    diff.diff = { path: 'a.ts', hunks: [] };
    const terminal = tool('terminal');
    terminal.name = 'Bash';
    terminal.input = { command: 'npm test' };
    const result = groupThreadItems([tool('a'), tool('b'), diff, tool('c'), tool('d'), terminal]);
    expect(result.map((entry) => entry.kind)).toEqual(['tool-group', 'item', 'tool-group', 'item']);
    expect(result[1]).toMatchObject({ kind: 'item', item: { id: 'diff' } });
    expect(result[3]).toMatchObject({ kind: 'item', item: { id: 'terminal' } });
  });

  it('写入工具即使没有 diff 也作为交付物独立展示', () => {
    const write = tool('write');
    write.name = 'Write';
    write.input = { file_path: 'new.ts', content: 'export {}' };
    const result = groupThreadItems([tool('a'), tool('b'), write, tool('c'), tool('d')]);
    expect(result.map((entry) => entry.kind)).toEqual(['tool-group', 'item', 'tool-group']);
    expect(result[1]).toMatchObject({ kind: 'item', item: { id: 'write' } });
  });

  it('Turn 过程容器保持 thinking、过程正文和多个 ToolGroup 的原始顺序', () => {
    const items: ThreadItem[] = [
      { kind: 'user', id: 'user', text: '处理', turnId: 'turn-1' },
      { kind: 'thinking', id: 'thinking', text: '分析', done: true, turnId: 'turn-1' },
      tool('a'),
      tool('b'),
      { kind: 'assistant', id: 'progress', text: '阶段结论', turnId: 'turn-1' },
      tool('c'),
      tool('d'),
      { kind: 'assistant', id: 'final', text: '完成', turnId: 'turn-1' },
    ];
    const result = layoutThreadItems(items);
    expect(result.map((entry) => entry.kind)).toEqual(['item', 'turn-process', 'item']);
    const process = result[1];
    expect(process.kind).toBe('turn-process');
    if (process.kind !== 'turn-process') return;
    expect(
      process.entries.flatMap((entry) =>
        entry.kind === 'tool-group' ? entry.items.map((item) => item.id) : [entry.item.id],
      ),
    ).toEqual(['thinking', 'a', 'b', 'progress', 'c', 'd']);
    expect(process.completed).toBe(true);
  });

  it('独立交付物穿插时同一稳定 Turn 仍只有一个过程容器', () => {
    const terminal = tool('terminal');
    terminal.name = 'Bash';
    terminal.input = { command: 'npm test' };
    const write = tool('write');
    write.name = 'Edit';
    write.diff = { path: 'src/app.ts', hunks: [] };
    const result = layoutThreadItems([
      { kind: 'thinking', id: 'thinking', text: '分析', done: true, turnId: 'turn-1' },
      tool('a'),
      tool('b'),
      terminal,
      { kind: 'assistant', id: 'progress', text: '继续处理', turnId: 'turn-1' },
      tool('c'),
      tool('d'),
      write,
      {
        kind: 'checkpoint',
        id: 'checkpoint',
        label: '改动前',
        ts: 1,
        restored: false,
        restorable: true,
        fileCount: 1,
        turnId: 'turn-1',
      },
      { kind: 'assistant', id: 'final', text: '完成', turnId: 'turn-1' },
    ]);
    expect(result.filter((entry) => entry.kind === 'turn-process')).toHaveLength(1);
    expect(result.filter((entry) => entry.kind === 'item').map((entry) => entry.item.id)).toEqual([
      'terminal',
      'write',
      'checkpoint',
      'final',
    ]);
  });

  it('单个普通失败工具也进入 Turn 过程容器并由最终答复完成收口', () => {
    const failed = tool('web');
    failed.name = 'WebSearch';
    failed.status = 'error';
    failed.output = '当前服务商不支持网络搜索';
    const result = layoutThreadItems([
      { kind: 'thinking', id: 'thinking', text: '查询天气', done: true, turnId: 'turn-1' },
      failed,
      { kind: 'assistant', id: 'final', text: '无法联网查询', turnId: 'turn-1' },
    ]);
    expect(result.map((entry) => entry.kind)).toEqual(['turn-process', 'item']);
    expect(result[0]).toMatchObject({
      kind: 'turn-process',
      completed: true,
      entries: [
        { kind: 'item', item: { id: 'thinking' } },
        { kind: 'item', item: { id: 'web', status: 'error' } },
      ],
    });
  });

  it('以权威 Turn 终态覆盖最终回复存在性，失败 Turn 不会被当成成功', () => {
    const result = layoutThreadItems([
      { kind: 'thinking', id: 'thinking', text: '执行', done: true, turnId: 'turn-1' },
      { kind: 'assistant', id: 'final', text: '未能完成', turnId: 'turn-1', turnStatus: 'failed' },
    ]);
    expect(result[0]).toMatchObject({
      kind: 'turn-process',
      completed: false,
      terminalStatus: 'failed',
    });
  });

  it('回溯轮次仍把最终答复留在过程容器外', () => {
    const firstTool = tool('a');
    const secondTool = tool('b');
    firstTool.reverted = true;
    secondTool.reverted = true;
    const result = layoutThreadItems([
      {
        kind: 'thinking',
        id: 'thinking',
        text: '分析',
        done: true,
        turnId: 'turn-1',
        reverted: true,
      },
      firstTool,
      secondTool,
      { kind: 'assistant', id: 'final', text: '完成', turnId: 'turn-1', reverted: true },
    ]);
    expect(result.map((entry) => entry.kind)).toEqual(['turn-process', 'item']);
    expect(result[1]).toMatchObject({ kind: 'item', item: { id: 'final' } });
  });
});
