import { describe, expect, it } from 'vitest';
import {
  addReviewNote,
  countReviewNotesForFile,
  mergeReviewNotes,
  removeReviewNote,
  reviewNotesForFile,
  reviewNotesToText,
  sameReviewLine,
} from './reviewNoteViewModel';

const noteA = { file: 'src/a.ts', line: 3, text: '未使用变量', fromAi: false };
const noteB = { file: 'src/a.ts', line: 3, text: '这里可能空指针', fromAi: true };
const noteC = { file: 'src/b.ts', line: 10, text: '命名不直观', fromAi: false };

describe('reviewNoteViewModel', () => {
  it('sameReviewLine 按字符串语义比对（diff 行号键可能是字符串）', () => {
    expect(sameReviewLine(3, '3')).toBe(true);
    expect(sameReviewLine(3, 4)).toBe(false);
  });

  it('addReviewNote 追加且不就地改原数组', () => {
    const next = addReviewNote([], noteA);
    expect(next).toEqual([noteA]);
  });

  it('removeReviewNote 只删指定文件同行的意见', () => {
    const notes = [noteA, noteB, noteC];
    expect(removeReviewNote(notes, 'src/a.ts', 3)).toEqual([noteC]);
    expect(removeReviewNote(notes, 'src/b.ts', 10)).toEqual([noteA, noteB]);
  });

  it('mergeReviewNotes 去重同一文件同一行，返回新增数量', () => {
    const merged = mergeReviewNotes([noteA], [noteB, noteC]);
    expect(merged.next).toEqual([noteA, noteC]);
    expect(merged.added).toBe(1);
  });

  it('reviewNotesForFile / countReviewNotesForFile 按文件过滤', () => {
    const notes = [noteA, noteB, noteC];
    expect(reviewNotesForFile(notes, 'src/a.ts')).toEqual([noteA, noteB]);
    expect(countReviewNotesForFile(notes, 'src/a.ts')).toBe(2);
    expect(countReviewNotesForFile(notes, 'src/b.ts')).toBe(1);
  });

  it('reviewNotesToText 生成带条数前缀与 file:line 的回灌文本', () => {
    const text = reviewNotesToText([noteA, noteC]);
    expect(text).toBe('审阅意见 · 2 条\n\nsrc/a.ts:3 — 未使用变量\nsrc/b.ts:10 — 命名不直观');
  });

  it('reviewNotesToText 折叠意见内换行为单行', () => {
    const text = reviewNotesToText([{ ...noteA, text: '第一行\n第二行\n' }]);
    expect(text).toContain('src/a.ts:3 — 第一行 第二行');
  });
});
