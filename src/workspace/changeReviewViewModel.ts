import type { ThreadItem } from '../engine/useSession';
import type { Diff } from '@helm/protocol';

/** 变更审阅文件状态：a=新增 / m=修改 / d=删除。 */
export type ChangeStatus = 'a' | 'm' | 'd';

export interface ReviewChangeLine {
  kind: 'add' | 'del' | 'ctx';
  /** 旧文件行号（add 行为空） */
  oldNo: number | null;
  /** 新文件行号（del 行为空） */
  newNo: number | null;
  text: string;
}

export interface ReviewHunk {
  /** 文件内稳定标识（折叠状态的键） */
  key: string;
  oldStart: number;
  newStart: number;
  /** 本 hunk 之前被折叠的未变更行数（真实 Diff 未携带该区间内容） */
  skip: number;
  lines: ReviewChangeLine[];
}

export interface ReviewChangeFile {
  path: string;
  status: ChangeStatus;
  added: number;
  removed: number;
  /** 对同一文件的编辑次数（行数是多次编辑的累计值） */
  edits: number;
  hunks: ReviewHunk[];
}

export interface ChangeReviewModel {
  files: ReviewChangeFile[];
  totalAdded: number;
  totalRemoved: number;
}

export interface HunkTarget {
  path: string;
  hunkKey: string;
}

function countAdded(diff: Diff): number {
  return diff.hunks.reduce(
    (total, hunk) => total + hunk.lines.filter((line) => line.kind === 'add').length,
    0,
  );
}

function countRemoved(diff: Diff): number {
  return diff.hunks.reduce(
    (total, hunk) => total + hunk.lines.filter((line) => line.kind === 'del').length,
    0,
  );
}

function inferStatus(file: ReviewChangeFile): ChangeStatus {
  if (file.hunks.length === 0) return 'm';
  const allLines = file.hunks.flatMap((hunk) => hunk.lines);
  if (allLines.length > 0 && allLines.every((line) => line.kind === 'add')) return 'a';
  if (allLines.length > 0 && allLines.every((line) => line.kind === 'del')) return 'd';
  return 'm';
}

/**
 * 变更-34 · A1：从 TurnLedger 的工具项聚合出变更审阅模型。
 * 每个带 diff 的工具调用按文件聚合成一个条目；±行数为多次编辑累计值，
 * hunk 之间按真实行号计算可折叠的未变更行数（内容未随 Diff 记录，只展示行区间）。
 */
export function changeReviewFiles(items: ThreadItem[]): ChangeReviewModel {
  const byPath = new Map<string, ReviewChangeFile>();
  let totalAdded = 0;
  let totalRemoved = 0;

  for (const item of items) {
    if (item.kind !== 'tool' || !item.diff) continue;
    if ('reverted' in item && item.reverted) continue;
    const diff = item.diff;
    if (diff.hunks.length === 0) continue;

    const file = byPath.get(diff.path) ?? {
      path: diff.path,
      status: 'm' as ChangeStatus,
      added: 0,
      removed: 0,
      edits: 0,
      hunks: [],
    };
    file.edits += 1;

    // 同一工具调用内部的 hunk 之间计算折叠区间；跨工具调用不推断间隔（各自独立编辑）
    let lastNew = 0;
    diff.hunks.forEach((hunk, hunkIndex) => {
      let oldNo = hunk.oldStart;
      let newNo = hunk.newStart;
      const lines: ReviewChangeLine[] = hunk.lines.map((line) => {
        if (line.kind === 'add') {
          const number = newNo;
          newNo += 1;
          return { kind: 'add' as const, oldNo: null, newNo: number, text: line.text };
        }
        if (line.kind === 'del') {
          const number = oldNo;
          oldNo += 1;
          return { kind: 'del' as const, oldNo: number, newNo: null, text: line.text };
        }
        const oldNumber = oldNo;
        const newNumber = newNo;
        oldNo += 1;
        newNo += 1;
        return { kind: 'ctx' as const, oldNo: oldNumber, newNo: newNumber, text: line.text };
      });
      const skip =
        hunkIndex === 0 ? Math.max(0, hunk.newStart - 1) : Math.max(0, hunk.newStart - lastNew - 1);
      file.hunks.push({
        key: `${diff.path}@${file.hunks.length}`,
        oldStart: hunk.oldStart,
        newStart: hunk.newStart,
        skip,
        lines,
      });
      lastNew = hunk.newStart + lines.filter((line) => line.kind !== 'del').length - 1;
    });

    file.added += countAdded(diff);
    file.removed += countRemoved(diff);
    byPath.set(diff.path, file);
  }

  const files = Array.from(byPath.values()).map((file) => ({
    ...file,
    status: inferStatus(file),
  }));
  for (const file of files) {
    totalAdded += file.added;
    totalRemoved += file.removed;
  }

  return { files, totalAdded, totalRemoved };
}

/** 把文件的 hunk 拍平成一维目标表，供跨文件「上/下一处变更」导航。 */
export function flattenHunkTargets(files: ReviewChangeFile[]): HunkTarget[] {
  const targets: HunkTarget[] = [];
  for (const file of files) {
    for (const hunk of file.hunks) {
      targets.push({ path: file.path, hunkKey: hunk.key });
    }
  }
  return targets;
}
