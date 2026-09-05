import { describe, expect, it } from 'vitest';
import type { ThreadItem } from '../engine/useSession';
import { changeReviewFiles, flattenHunkTargets } from './changeReviewViewModel';

type ToolItem = Extract<ThreadItem, { kind: 'tool' }>;

function toolItem(partial: Partial<ToolItem> & { id: string }): ToolItem {
  return { kind: 'tool', name: 'Edit', input: {}, status: 'success', ...partial };
}

describe('changeReviewFiles', () => {
  it('全新增 diff 归类为新增文件并统计 +N 与行号', () => {
    const items: ThreadItem[] = [
      toolItem({
        id: 'write-1',
        name: 'Write',
        diff: {
          path: 'src/new.ts',
          hunks: [
            {
              oldStart: 1,
              newStart: 1,
              lines: [
                { kind: 'add', text: 'export const a = 1;' },
                { kind: 'add', text: '' },
                { kind: 'add', text: 'export const b = 2;' },
              ],
            },
          ],
        },
      }),
    ];

    const model = changeReviewFiles(items);
    expect(model.files).toHaveLength(1);
    const file = model.files[0];
    expect(file.status).toBe('a');
    expect(file.added).toBe(3);
    expect(file.removed).toBe(0);
    expect(file.edits).toBe(1);
    expect(file.hunks[0].lines.map((line) => line.newNo)).toEqual([1, 2, 3]);
    expect(model.totalAdded).toBe(3);
    expect(model.totalRemoved).toBe(0);
  });

  it('修改 diff 归为修改，行号按 add/del/ctx 分别推进', () => {
    const items: ThreadItem[] = [
      toolItem({
        id: 'edit-1',
        diff: {
          path: 'src/app.ts',
          hunks: [
            {
              oldStart: 5,
              newStart: 4,
              lines: [
                { kind: 'ctx', text: 'const x = 1' },
                { kind: 'del', text: 'old line' },
                { kind: 'add', text: 'new line' },
                { kind: 'ctx', text: 'const y = 2' },
              ],
            },
          ],
        },
      }),
    ];

    const file = changeReviewFiles(items).files[0];
    expect(file.status).toBe('m');
    expect(file.added).toBe(1);
    expect(file.removed).toBe(1);
    const lines = file.hunks[0].lines;
    expect(lines[0]).toMatchObject({ kind: 'ctx', oldNo: 5, newNo: 4 });
    expect(lines[1]).toMatchObject({ kind: 'del', oldNo: 6, newNo: null });
    expect(lines[2]).toMatchObject({ kind: 'add', oldNo: null, newNo: 5 });
    // del 已占用旧行号 6，后续 ctx 的旧行号应为 7
    expect(lines[3]).toMatchObject({ kind: 'ctx', oldNo: 7, newNo: 6 });
  });

  it('全删除 diff 归为删除文件', () => {
    const items: ThreadItem[] = [
      toolItem({
        id: 'del-1',
        diff: {
          path: 'src/obsolete.ts',
          hunks: [
            {
              oldStart: 1,
              newStart: 1,
              lines: [
                { kind: 'del', text: 'old' },
                { kind: 'del', text: 'gone' },
              ],
            },
          ],
        },
      }),
    ];

    const file = changeReviewFiles(items).files[0];
    expect(file.status).toBe('d');
    expect(file.removed).toBe(2);
  });

  it('同一文件多次编辑累计 ± 行数与编辑次数，hunk 依次追加', () => {
    const items: ThreadItem[] = [
      toolItem({
        id: 'edit-1',
        diff: {
          path: 'src/a.ts',
          hunks: [
            {
              oldStart: 1,
              newStart: 1,
              lines: [{ kind: 'add', text: 'one' }],
            },
          ],
        },
      }),
      toolItem({
        id: 'edit-2',
        diff: {
          path: 'src/a.ts',
          hunks: [
            {
              oldStart: 1,
              newStart: 1,
              lines: [
                { kind: 'del', text: 'old' },
                { kind: 'add', text: 'two' },
              ],
            },
          ],
        },
      }),
    ];

    const file = changeReviewFiles(items).files[0];
    expect(file.edits).toBe(2);
    expect(file.added).toBe(2);
    expect(file.removed).toBe(1);
    expect(file.hunks).toHaveLength(2);
    expect(file.hunks[1].lines).toHaveLength(2);
  });

  it('被回滚（reverted）的工具项不计入变更', () => {
    const items: ThreadItem[] = [
      toolItem({
        id: 'edit-1',
        reverted: true,
        diff: {
          path: 'src/rolled.ts',
          hunks: [{ oldStart: 1, newStart: 1, lines: [{ kind: 'add', text: 'x' }] }],
        },
      }),
    ];

    expect(changeReviewFiles(items).files).toHaveLength(0);
  });

  it('首 hunk 之前按真实行号计算折叠区间，同一工具调用内 hunk 之间也计算', () => {
    const items: ThreadItem[] = [
      toolItem({
        id: 'edit-1',
        diff: {
          path: 'src/auth/token.ts',
          hunks: [
            {
              oldStart: 12,
              newStart: 44,
              lines: [
                { kind: 'del', text: 'old' },
                { kind: 'add', text: 'new' },
              ],
            },
            {
              oldStart: 90,
              newStart: 100,
              lines: [
                { kind: 'ctx', text: 'ctx' },
                { kind: 'add', text: 'tail' },
              ],
            },
          ],
        },
      }),
    ];

    const file = changeReviewFiles(items).files[0];
    expect(file.hunks[0].skip).toBe(43);
    // 第一个 hunk 的新行号只有 44（del 不占新行，add 落到 44）；
    // 第二个 hunk newStart=100 → 中间折叠 100-44-1=55 行
    expect(file.hunks[1].skip).toBe(55);
  });

  it('空 hunk 的 diff 不产生变更文件', () => {
    const items: ThreadItem[] = [
      toolItem({ id: 'noop-1', diff: { path: 'src/noop.ts', hunks: [] } }),
    ];
    expect(changeReviewFiles(items).files).toHaveLength(0);
  });

  it('不带 diff 的工具项不参与', () => {
    const items: ThreadItem[] = [
      toolItem({ id: 'bash-1', name: 'Bash', input: { command: 'npm test' } }),
      { kind: 'user', id: 'user-1', text: 'hi' },
    ];
    const model = changeReviewFiles(items);
    expect(model.files).toHaveLength(0);
    expect(model.totalAdded).toBe(0);
  });
});

describe('flattenHunkTargets', () => {
  it('按文件顺序拍平全部 hunk，供跨文件导航', () => {
    const files = changeReviewFiles([
      toolItem({
        id: 'e1',
        diff: {
          path: 'a.ts',
          hunks: [
            { oldStart: 1, newStart: 1, lines: [{ kind: 'add', text: 'x' }] },
            { oldStart: 9, newStart: 9, lines: [{ kind: 'add', text: 'y' }] },
          ],
        },
      }),
      toolItem({
        id: 'e2',
        diff: {
          path: 'b.ts',
          hunks: [{ oldStart: 1, newStart: 1, lines: [{ kind: 'del', text: 'z' }] }],
        },
      }),
    ]).files;

    const targets = flattenHunkTargets(files);
    expect(targets.map((target) => target.path)).toEqual(['a.ts', 'a.ts', 'b.ts']);
    expect(targets[0].hunkKey).toBe('a.ts@0');
    expect(targets[1].hunkKey).toBe('a.ts@1');
    expect(targets[2].hunkKey).toBe('b.ts@0');
  });
});
