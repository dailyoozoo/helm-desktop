import type { Diff } from '@helm/protocol';

const cachedStats = new WeakMap<Diff, Readonly<{ added: number; removed: number }>>();

export function diffStats(diff: Diff): Readonly<{ added: number; removed: number }> {
  const cached = cachedStats.get(diff);
  if (cached) return cached;
  let added = 0;
  let removed = 0;
  for (const hunk of diff.hunks) {
    for (const line of hunk.lines) {
      const kind = line.kind;
      if (kind === 'add') added += 1;
      else if (kind === 'del') removed += 1;
    }
  }
  const stats = { added, removed };
  cachedStats.set(diff, stats);
  return stats;
}
