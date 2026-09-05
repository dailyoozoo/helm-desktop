import { describe, expect, it } from 'vitest';
import type { ThreadItem } from '../engine/useSession';
import {
  billingSummary,
  closeDynTab,
  CONTEXT_PANEL_DEFAULT_TAB,
  CONTEXT_PANEL_FIXED_TABS,
  CONTEXT_PANEL_FIXED_TAB_LABELS,
  contextPanelData,
  DYN_TAB_LABELS,
  isContextPanelFixedTab,
  openDynTab,
  workspaceFileRows,
} from './contextPanelViewModel';

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

  it('billingSummary 纯函数与 contextPanelData 口径一致（ContextRing 复用）', () => {
    const summary = billingSummary({
      inputTokens: 1_000,
      cachedInputTokens: 700,
      cacheWriteInputTokens: 200,
      outputTokens: 100,
    });
    expect(summary).toEqual({
      freshInput: 100,
      cacheWrite: 200,
      cacheRead: 700,
      output: 100,
      total: 1_100,
      cacheReadShare: 700 / 1_100,
    });
    expect(billingSummary(undefined)).toEqual({
      freshInput: 0,
      cacheWrite: 0,
      cacheRead: 0,
      output: 0,
      total: 0,
    });
  });
});

describe('交付物区动态 tab 状态机（变更-34 · A4）', () => {
  it('打开动态 tab：追加到打开列表并置为当前', () => {
    const next = openDynTab({ open: [], active: null }, 'plan');
    expect(next.open).toEqual(['plan']);
    expect(next.active).toBe('plan');
  });

  it('重复打开同一 tab：不重复追加，仍置为当前', () => {
    const next = openDynTab({ open: ['plan', 'term'], active: 'term' }, 'plan');
    expect(next.open).toEqual(['plan', 'term']);
    expect(next.active).toBe('plan');
  });

  it('打开第二个 tab：保留已有 tab（不移除）', () => {
    const next = openDynTab({ open: ['plan'], active: 'plan' }, 'term');
    expect(next.open).toEqual(['plan', 'term']);
    expect(next.active).toBe('term');
  });

  it('关闭非当前 tab：只移除，当前不变', () => {
    const next = closeDynTab({ open: ['plan', 'term'], active: 'term' }, 'plan');
    expect(next.open).toEqual(['term']);
    expect(next.active).toBe('term');
  });

  it('关闭当前 tab：active 置空，由 UI 回退到常驻「修改记录」', () => {
    const next = closeDynTab({ open: ['plan', 'term'], active: 'term' }, 'term');
    expect(next.open).toEqual(['plan']);
    expect(next.active).toBeNull();
  });

  it('S3：「活动」并入动态 tab 后照常打开/关闭', () => {
    const opened = openDynTab({ open: [], active: null }, 'log');
    expect(opened.open).toEqual(['log']);
    expect(opened.active).toBe('log');
    const closed = closeDynTab(opened, 'log');
    expect(closed.open).toEqual([]);
    expect(closed.active).toBeNull();
  });
});

describe('S3 · 右栏固定 tab 模型（修改记录 / 全部文件）', () => {
  it('常驻 tab 只有 changes/files，顺序与原型一致', () => {
    expect([...CONTEXT_PANEL_FIXED_TABS]).toEqual(['changes', 'files']);
  });

  it('标签与原型一致：修改记录 / 全部文件', () => {
    expect(CONTEXT_PANEL_FIXED_TAB_LABELS).toEqual({
      changes: '修改记录',
      files: '全部文件',
    });
  });

  it('默认激活「修改记录」', () => {
    expect(CONTEXT_PANEL_DEFAULT_TAB).toBe('changes');
  });

  it('isContextPanelFixedTab 只认常驻标识；动态标识（含活动）不算常驻', () => {
    expect(isContextPanelFixedTab('changes')).toBe(true);
    expect(isContextPanelFixedTab('files')).toBe(true);
    expect(isContextPanelFixedTab('log')).toBe(false);
    expect(isContextPanelFixedTab('context')).toBe(false);
  });

  it('动态 tab 标签单一来源：活动并入动态、上下文不再存在', () => {
    expect(DYN_TAB_LABELS.log).toBe('活动');
    expect(DYN_TAB_LABELS.tools).toBe('工具');
    expect(Object.keys(DYN_TAB_LABELS)).not.toContain('context');
    expect(Object.keys(DYN_TAB_LABELS)).not.toContain('files');
  });
});

describe('S3 · workspaceFileRows（全部文件真实行）', () => {
  it('拆分目录/文件名并按真实暂存状态打徽标', () => {
    const rows = workspaceFileRows(
      ['src/app.ts', 'README.md'],
      [
        { path: 'src/app.ts', status: 'Modified' },
        { path: 'new.ts', status: 'Added' },
      ],
    );
    expect(rows).toEqual([
      { path: 'src/app.ts', dir: 'src/', base: 'app.ts', badge: 'M' },
      { path: 'README.md', dir: '', base: 'README.md' },
    ]);
  });

  it('无暂存数据时无徽标字段；反斜杠路径归一化匹配', () => {
    const rows = workspaceFileRows(
      ['docs\\guide.md'],
      [{ path: 'docs/guide.md', status: 'Deleted' }],
    );
    expect(rows).toEqual([{ path: 'docs\\guide.md', dir: 'docs/', base: 'guide.md', badge: 'D' }]);
    expect(workspaceFileRows([], []).map((row) => row.badge ?? null)).toEqual([]);
  });
});
