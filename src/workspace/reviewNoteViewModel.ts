// 变更-34 · A2/A3：行级审阅意见（ReviewNote）。
// 用户在 diff 上逐行标注 → 攒批 → 一次性回灌给 Agent；自评审产出 fromAi 标注。
// 纯函数集合，供 ChangeReview / DiffView / 单测共享；意见只挂 UI 层，不落盘、不回滚磁盘。

export interface ReviewNote {
  /** diff.path，即变更审阅模型的文件路径 */
  file: string;
  /** 行号标记：add/ctx 行为新文件行号，del 行为旧文件行号（与 DiffView 的 data-line 对齐） */
  line: number;
  text: string;
  /** 是否来自「让 Helm 自评审」（渲染用 .rnote.is-ai 区分） */
  fromAi: boolean;
}

/** 编辑中的草稿位置（一次只开一行）。 */
export interface ReviewDraft {
  file: string;
  line: number;
}

/** 统一的行号比对：diff 行号是字符串键，这里按字符串语义匹配。 */
export function sameReviewLine(a: number | string, b: number | string): boolean {
  return String(a) === String(b);
}

export function addReviewNote(notes: ReviewNote[], note: ReviewNote): ReviewNote[] {
  return [...notes, note];
}

export function removeReviewNote(
  notes: ReviewNote[],
  file: string,
  line: number | string,
): ReviewNote[] {
  return notes.filter((note) => !(note.file === file && sameReviewLine(note.line, line)));
}

/** 合并一批意见（自评审结果去重：同一文件同一行不再重复出现）。返回是否有新增。 */
export function mergeReviewNotes(
  notes: ReviewNote[],
  incoming: ReviewNote[],
): {
  next: ReviewNote[];
  added: number;
} {
  const added: ReviewNote[] = incoming.filter(
    (candidate) =>
      !notes.some(
        (note) => note.file === candidate.file && sameReviewLine(note.line, candidate.line),
      ),
  );
  return { next: [...notes, ...added], added: added.length };
}

export function reviewNotesForFile(notes: ReviewNote[], file: string): ReviewNote[] {
  return notes.filter((note) => note.file === file);
}

export function countReviewNotesForFile(notes: ReviewNote[], file: string): number {
  return notes.filter((note) => note.file === file).length;
}

/**
 * 回灌文本（A2）：攒批意见一次性作为一条真实用户消息发出。
 * 行首保留「审阅意见 · N 条」，线程里即可识别；逐条按 file:line — 意见 换行。
 */
export function reviewNotesToText(notes: ReviewNote[]): string {
  const lines = notes.map(
    (note) => `${note.file}:${note.line} — ${note.text.replace(/\s*\n+/g, ' ').trim()}`,
  );
  return `审阅意见 · ${notes.length} 条\n\n${lines.join('\n')}`;
}
