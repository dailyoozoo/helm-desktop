import { useSyncExternalStore } from 'react';

/** 线程视图密度（变更-34/35 · B1，v3 最终态）：`std` 标准 / `lite` 专注。
 *  对齐 Claude Code focusView：标准=工具折叠为摘要；专注=过程收成每轮一行。
 *  开关在设置页「外观 · 专注模式」，工作台不放控件；只改可见性，不改数据与折叠状态。 */
export type TranscriptDensity = 'std' | 'lite';

export const TRANSCRIPT_DENSITY_LABEL: Record<TranscriptDensity, string> = {
  std: '标准',
  lite: '专注',
};

export const TRANSCRIPT_DENSITY_DESC: Record<TranscriptDensity, string> = {
  std: '工具折叠为摘要',
  lite: '过程收成每轮一行',
};

const STORAGE_KEY = 'helm:density';

function isTranscriptDensity(value: unknown): value is TranscriptDensity {
  return value === 'std' || value === 'lite';
}

function readStored(): TranscriptDensity {
  try {
    const stored = localStorage.getItem(STORAGE_KEY);
    if (isTranscriptDensity(stored)) return stored;
  } catch {
    // 非浏览器环境或隐私模式下读取失败，回落默认档
  }
  return 'std';
}

/** 循环换档：标准 ↔ 专注，供 Ctrl+O 使用。 */
export function nextTranscriptDensity(current: TranscriptDensity): TranscriptDensity {
  return current === 'std' ? 'lite' : 'std';
}

let currentDensity: TranscriptDensity = readStored();

const listeners = new Set<() => void>();

export function getTranscriptDensity(): TranscriptDensity {
  return currentDensity;
}

export function subscribeTranscriptDensity(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

export function setTranscriptDensity(density: TranscriptDensity): void {
  if (!isTranscriptDensity(density) || density === currentDensity) return;
  currentDensity = density;
  try {
    localStorage.setItem(STORAGE_KEY, density);
  } catch {
    // 存储失败不影响本次会话内的显示
  }
  listeners.forEach((listener) => listener());
}

export function useTranscriptDensity(): TranscriptDensity {
  return useSyncExternalStore(
    subscribeTranscriptDensity,
    getTranscriptDensity,
    getTranscriptDensity,
  );
}
