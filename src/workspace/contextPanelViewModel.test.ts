import { describe, expect, it } from 'vitest';
import type { ThreadItem } from '../engine/useSession';
import { contextPanelData } from './contextPanelViewModel';

describe('contextPanelData', () => {
  it('derives changed files, tool summary, and context window usage from real thread items', () => {
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
      messageCount: 1,
      historicalAttachments: ['D:\\work\\app.ts', 'D:\\work\\docs'],
      mountedPaths: ['D:\\work\\app.ts', 'D:\\work\\docs'],
      contextWindow: {
        usedTokens: 0,
        maxTokens: 2_000,
        usedRatio: 0,
        mountedPathCount: 2,
        fileTokenDetailAvailable: false,
      },
      tools: [
        { id: 'edit-1', name: 'Edit', status: 'success' },
        { id: 'bash-1', name: 'Bash', status: 'success' },
      ],
      contextUsage: { maxTokens: 2_000, level: 'none' },
      billing: {
        freshInput: 1_000,
        cacheWrite: 0,
        cacheRead: 0,
        output: 250,
        total: 1_250,
        cacheReadShare: 0,
      },
      changeFiles: [
        {
          path: 'src/app.ts',
          lines: [
            { path: 'src/app.ts', lineNumber: 1, kind: 'del', text: 'old', toolId: 'edit-1' },
            { path: 'src/app.ts', lineNumber: 1, kind: 'add', text: 'new', toolId: 'edit-1' },
            { path: 'src/app.ts', lineNumber: 2, kind: 'ctx', text: 'same', toolId: 'edit-1' },
          ],
          added: 1,
          removed: 1,
        },
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

  it.each([
    [1560, 'normal'],
    [1700, 'warning'],
    [1920, 'danger'],
  ] as const)('按真实上下文占用应用阈值：%s → %s', (contextTokens, level) => {
    const data = contextPanelData([], {
      inputTokens: 9_000,
      outputTokens: 100,
      contextTokens,
      contextWindow: 2_000,
    });
    expect(data.contextUsage.level).toBe(level);
    expect(data.contextUsage.ratio).toBe(contextTokens / 2_000);
  });

  it('四分账从总输入中扣除缓存读取和写入，并标记读取占比', () => {
    const data = contextPanelData([], {
      inputTokens: 1_000,
      cachedInputTokens: 700,
      cacheWriteInputTokens: 200,
      outputTokens: 100,
    });
    expect(data.billing).toEqual({
      freshInput: 100,
      cacheWrite: 200,
      cacheRead: 700,
      output: 100,
      total: 1_100,
      cacheReadShare: 700 / 1_100,
    });
  });
});
