export const DEFAULT_THREAD_WINDOW = 200;

export function threadWindow<T>(items: readonly T[], visibleCount: number) {
  const count = Math.max(0, visibleCount);
  const hiddenCount = Math.max(0, items.length - count);
  return { hiddenCount, visibleItems: items.slice(hiddenCount) };
}

export function expandThreadWindow(visibleCount: number, hiddenCount: number): number {
  return visibleCount + Math.min(DEFAULT_THREAD_WINDOW, Math.max(0, hiddenCount));
}
