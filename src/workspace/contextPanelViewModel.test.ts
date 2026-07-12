import { describe, expect, it } from 'vitest';
import type { ThreadItem } from '../engine/useSession';
import { contextPanelData } from './contextPanelViewModel';

describe('contextPanelData', () => {
  it('derives changed files, terminal output, tool summary, and context window usage from real thread items', () => {
    const items: ThreadItem[] = [
      {
        kind: 'tool',
        id: 'edit-1',
        name: 'Edit',
        input: {},
        status: 'success',
        diff: {
          path: 'src/app.ts',
          hunks: [
            {
              oldStart: 1,
              newStart: 1,
              lines: [
                { kind: 'del', text: 'old' },
                { kind: 'add', text: 'new' },
                { kind: 'ctx', text: 'same' },
              ],
            },
          ],
        },
      },
      {
        kind: 'user',
        id: 'user-1',
        text: '分析这些文件',
        attachments: ['D:\\work\\app.ts', 'D:\\work\\docs'],
      },
      {
        kind: 'tool',
        id: 'bash-1',
        name: 'Bash',
        input: { command: 'npm test' },
        status: 'success',
        output: 'PASS 1 test',
      },
    ];

    expect(
      contextPanelData(items, { inputTokens: 1_000, outputTokens: 250, contextWindow: 2_000 }),
    ).toEqual({
      changedFiles: [{ path: 'src/app.ts', added: 1, removed: 1, edits: 1 }],
      mountedPaths: ['D:\\work\\app.ts', 'D:\\work\\docs'],
      contextWindow: {
        usedTokens: 1_250,
        maxTokens: 2_000,
        usedRatio: 0.625,
        mountedPathCount: 2,
        fileTokenDetailAvailable: false,
      },
      terminalOutputs: [
        { id: 'bash-1', command: 'npm test', status: 'success', output: 'PASS 1 test' },
      ],
      tools: [
        { id: 'edit-1', name: 'Edit', status: 'success' },
        { id: 'bash-1', name: 'Bash', status: 'success' },
      ],
    });
  });

  it('excludes reverted items from the panel（变更-11：回溯后右栏不再显示被回滚变更）', () => {
    const items: ThreadItem[] = [
      {
        kind: 'tool',
        id: 'edit-kept',
        name: 'Edit',
        input: {},
        status: 'success',
        diff: {
          path: 'kept.ts',
          hunks: [{ oldStart: 1, newStart: 1, lines: [{ kind: 'add', text: 'x' }] }],
        },
      },
      {
        kind: 'tool',
        id: 'edit-rolled',
        name: 'Edit',
        input: {},
        status: 'success',
        reverted: true,
        diff: {
          path: 'rolled.ts',
          hunks: [{ oldStart: 1, newStart: 1, lines: [{ kind: 'add', text: 'y' }] }],
        },
      },
    ];

    const data = contextPanelData(items);
    expect(data.changedFiles.map((file) => file.path)).toEqual(['kept.ts']);
    expect(data.tools.map((tool) => tool.id)).toEqual(['edit-kept']);
  });

  it('accumulates line counts across repeated edits of the same file and reports edit count', () => {
    const edit = (id: string): ThreadItem => ({
      kind: 'tool',
      id,
      name: 'Edit',
      input: {},
      status: 'success',
      diff: {
        path: 'same.ts',
        hunks: [
          {
            oldStart: 1,
            newStart: 1,
            lines: [
              { kind: 'del', text: 'a' },
              { kind: 'add', text: 'b' },
            ],
          },
        ],
      },
    });
    const data = contextPanelData([edit('e1'), edit('e2'), edit('e3')]);
    expect(data.changedFiles).toEqual([{ path: 'same.ts', added: 3, removed: 3, edits: 3 }]);
  });
});
