import { describe, expect, it } from 'vitest';
import { isAgentEvent, type Diff } from '@helm/protocol';
import { parseClaudeLine } from '../src/parse';

function diffFromContent(content: unknown): Diff | undefined {
  const events = parseClaudeLine(
    JSON.stringify({
      type: 'user',
      session_id: 'diff-session',
      message: {
        content: [{ type: 'tool_result', tool_use_id: 'diff-tool', content }],
      },
    }),
  );
  expect(events).toHaveLength(1);
  expect(isAgentEvent(events[0])).toBe(true);
  const event = events[0];
  if (event.type !== 'tool_result') throw new Error('Expected a tool result');
  return event.diff;
}

function parseDiff(oldLines: string[], newLines: string[]): Diff | undefined {
  return diffFromContent([
    {
      type: 'diff',
      path: 'demo.txt',
      old_string: oldLines.length === 0 ? '' : `${oldLines.join('\n')}\n`,
      new_string: newLines.length === 0 ? '' : `${newLines.join('\n')}\n`,
    },
  ]);
}

function expectRebuild(oldLines: string[], newLines: string[], diff: Diff | undefined): void {
  let oldIndex = 0;
  const rebuilt: string[] = [];
  for (const hunk of diff?.hunks ?? []) {
    const start = hunk.oldStart - 1;
    expect(start).toBeGreaterThanOrEqual(oldIndex);
    rebuilt.push(...oldLines.slice(oldIndex, start));
    oldIndex = start;
    expect(hunk.newStart).toBe(rebuilt.length + 1);
    for (const line of hunk.lines) {
      if (line.kind !== 'add') {
        expect(line.text).toBe(oldLines[oldIndex]);
        oldIndex += 1;
      }
      if (line.kind !== 'del') rebuilt.push(line.text);
    }
  }
  rebuilt.push(...oldLines.slice(oldIndex));
  expect(rebuilt).toEqual(newLines);
}

function lcsLength(oldLines: string[], newLines: string[]): number {
  let previous = Array<number>(newLines.length + 1).fill(0);
  for (const oldLine of oldLines) {
    const current = Array<number>(newLines.length + 1).fill(0);
    for (let index = 0; index < newLines.length; index += 1) {
      current[index + 1] =
        oldLine === newLines[index]
          ? previous[index] + 1
          : Math.max(previous[index + 1], current[index]);
    }
    previous = current;
  }
  return previous[newLines.length];
}

function deterministicRandom(seed: number): (limit: number) => number {
  let state = seed;
  return (limit) => {
    state = (Math.imul(state, 1_664_525) + 1_013_904_223) >>> 0;
    return state % limit;
  };
}

function editedLines(
  original: string[],
  count: number,
  random: (limit: number) => number,
): string[] {
  const result = original.slice();
  for (let editIndex = 0; editIndex < count; editIndex += 1) {
    const position = random(result.length + 1);
    const operation = random(3);
    const text = `修改-${random(12)}`;
    if (operation === 0 || result.length === 0) result.splice(position, 0, text);
    else result.splice(position, 1, ...(operation === 1 ? [] : [text]));
  }
  return result;
}

describe('Claude diff reconstruction', () => {
  it('保留两处修改之间的公共行，起始行号排除公共前后缀', () => {
    const oldLines = ['header', 'before', 'keep', 'after', 'footer'];
    const newLines = ['header', 'changed-before', 'keep', 'changed-after', 'footer'];
    const diff = parseDiff(oldLines, newLines);
    expect(diff).toEqual({
      path: 'demo.txt',
      hunks: [
        {
          oldStart: 2,
          newStart: 2,
          lines: [
            { kind: 'del', text: 'before' },
            { kind: 'add', text: 'changed-before' },
            { kind: 'ctx', text: 'keep' },
            { kind: 'del', text: 'after' },
            { kind: 'add', text: 'changed-after' },
          ],
        },
      ],
    });
    expectRebuild(oldLines, newLines, diff);
  });

  it.each<{ oldLines: string[]; newLines: string[] }>([
    { oldLines: [], newLines: [] },
    { oldLines: ['same'], newLines: ['same'] },
    { oldLines: [], newLines: ['新增', '🙂'] },
    { oldLines: ['移除', '🙂'], newLines: [] },
    { oldLines: ['head', 'tail'], newLines: ['insert', 'head', 'tail'] },
    { oldLines: ['head', 'tail'], newLines: ['head', 'tail', 'append'] },
    { oldLines: ['head', 'tail'], newLines: ['tail'] },
    { oldLines: ['head', 'tail'], newLines: ['head'] },
    { oldLines: ['A', 'B', 'A', 'C'], newLines: ['B', 'A', 'B', 'D'] },
    { oldLines: [''], newLines: ['', ''] },
  ])('小例子重建 $oldLines → $newLines', ({ oldLines, newLines }) => {
    const diff = parseDiff(oldLines, newLines);
    expectRebuild(oldLines, newLines, diff);
    const changes = diff?.hunks.flatMap((hunk) => hunk.lines).filter((line) => line.kind !== 'ctx');
    expect(changes?.length ?? 0).toBe(
      oldLines.length + newLines.length - 2 * lcsLength(oldLines, newLines),
    );
    if (
      oldLines.length === newLines.length &&
      oldLines.every((text, index) => text === newLines[index])
    ) {
      expect(diff).toBeUndefined();
    }
  });

  it('遵循真实 diff 内容块，不从普通文本猜测修改', () => {
    expect(
      diffFromContent('--- guessed.txt\n+++ guessed.txt\n@@ -1 +1 @@\n-old\n+new'),
    ).toBeUndefined();
    const diff = diffFromContent([
      null,
      { message: 'unrelated metadata' },
      { type: 'text', text: '--- guessed.txt\n+++ guessed.txt' },
      {
        type: 'diff',
        path: '真实.txt',
        old_string: '头部\r\n旧🙂\r\n\r\n尾部\r\n',
        new_string: '头部\r\n新🙂\r\n\r\n尾部\r\n',
      },
    ]);
    expect(diff?.path).toBe('真实.txt');
    expect(diff?.hunks[0]?.oldStart).toBe(2);
    expectRebuild(['头部', '旧🙂', '', '尾部'], ['头部', '新🙂', '', '尾部'], diff);
  });

  it('确定性随机小变更始终可重建且编辑数最短', () => {
    const random = deterministicRandom(0x5eed1234);
    for (let caseIndex = 0; caseIndex < 128; caseIndex += 1) {
      const oldLines = Array.from({ length: random(65) }, () => `line-${random(12)}`);
      const newLines = editedLines(oldLines, 1 + random(24), random);
      const diff = parseDiff(oldLines, newLines);
      expectRebuild(oldLines, newLines, diff);
      const changes = diff?.hunks
        .flatMap((hunk) => hunk.lines)
        .filter((line) => line.kind !== 'ctx');
      expect(changes?.length ?? 0).toBe(
        oldLines.length + newLines.length - 2 * lcsLength(oldLines, newLines),
      );
    }
  });

  it.each([511, 512])('矩阵预算边界 %i 行切换为有界前瞻', (lineCount) => {
    const common = Array.from({ length: lineCount - 68 }, (_, index) => `common-${index}`);
    const removed = Array.from({ length: 65 }, (_, index) => `removed-${index}`);
    const inserted = Array.from({ length: 65 }, (_, index) => `inserted-${index}`);
    const oldLines = ['old-head', ...removed, 'anchor', ...common, 'old-foot'];
    const newLines = ['new-head', 'anchor', ...common, ...inserted, 'new-foot'];
    expect(oldLines).toHaveLength(lineCount);
    expect(newLines).toHaveLength(lineCount);
    const diff = parseDiff(oldLines, newLines);
    expectRebuild(oldLines, newLines, diff);
    expect(diff?.hunks[0].lines.some((line) => line.kind === 'ctx' && line.text === 'anchor')).toBe(
      lineCount === 511,
    );
  });

  it('超矩阵预算的确定性随机变更可重建且结果稳定', () => {
    const random = deterministicRandom(0x5eed5678);
    for (let caseIndex = 0; caseIndex < 16; caseIndex += 1) {
      const oldLines = Array.from({ length: 700 }, () => `line-${random(48)}`);
      const newLines = editedLines(oldLines, 80, random);
      newLines[0] = 'changed-first';
      newLines[newLines.length - 1] = 'changed-last';
      const diff = parseDiff(oldLines, newLines);
      expectRebuild(oldLines, newLines, diff);
      expect(parseDiff(oldLines, newLines)).toEqual(diff);
    }
  });

  it.each([63, 64, 65])('大变更前瞻 %i 行只在预算内寻找匹配', (distance) => {
    const common = Array.from({ length: 600 }, (_, index) => `common-${index}`);
    const removed = Array.from({ length: distance }, (_, index) => `removed-${index}`);
    const oldLines = ['old-head', ...removed, 'anchor', ...common, 'old-foot'];
    const newLines = ['new-head', 'anchor', ...common, 'new-foot'];
    const diff = parseDiff(oldLines, newLines);
    expectRebuild(oldLines, newLines, diff);
    expect(diff?.hunks[0].lines.some((line) => line.kind === 'ctx' && line.text === 'anchor')).toBe(
      distance <= 64,
    );
  });

  it('3000 行稀疏修改保留公共内容且完整重建', () => {
    const oldLines = Array.from({ length: 3000 }, (_, index) => `line-${index}`);
    const newLines = oldLines.flatMap((text, index) => {
      if (index % 89 === 0) return [];
      if (index % 97 === 0) return [`changed-${index}`];
      if (index % 101 === 0) return [`inserted-${index}`, text];
      return [text];
    });
    newLines[0] = 'changed-first';
    newLines[newLines.length - 1] = 'changed-last';
    const diff = parseDiff(oldLines, newLines);
    expectRebuild(oldLines, newLines, diff);
    expect(diff?.hunks[0].lines.filter((line) => line.kind === 'ctx').length).toBeGreaterThan(2800);
    expect(diff?.hunks[0].lines.length).toBeLessThanOrEqual(oldLines.length + newLines.length);
  });

  it('3000 行全量替换走有界前瞻而非平方矩阵', () => {
    const oldLines = Array.from({ length: 3000 }, (_, index) => `old-${index}`);
    const newLines = Array.from({ length: 3000 }, (_, index) => `new-${index}`);
    const diff = parseDiff(oldLines, newLines);
    expectRebuild(oldLines, newLines, diff);
    expect(diff?.hunks[0].lines).toHaveLength(6000);
    expect(diff?.hunks[0].lines.some((line) => line.kind === 'ctx')).toBe(false);
  });
});
