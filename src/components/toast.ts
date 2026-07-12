// 全局通知层的数据通道（P2-2）：模块级单例，替代各页面自制的局部 toast。
// 任何模块（含非 React 代码）都可以直接调用 showToast；ToastLayer 负责渲染。

export type ToastKind = 'info' | 'success' | 'error';

export interface ToastEntry {
  id: number;
  message: string;
  kind: ToastKind;
  /** 自动消失的毫秒数 */
  duration: number;
}

type ToastListener = (toasts: ToastEntry[]) => void;

const DEFAULT_DURATION: Record<ToastKind, number> = {
  info: 3200,
  success: 3200,
  // 错误需要更长的阅读时间
  error: 5600,
};

let nextId = 1;
let toasts: ToastEntry[] = [];
const listeners = new Set<ToastListener>();

function emit() {
  const snapshot = toasts.slice();
  for (const listener of listeners) listener(snapshot);
}

export function subscribeToasts(listener: ToastListener): () => void {
  listeners.add(listener);
  listener(toasts.slice());
  return () => {
    listeners.delete(listener);
  };
}

export function dismissToast(id: number): void {
  const before = toasts.length;
  toasts = toasts.filter((toast) => toast.id !== id);
  if (toasts.length !== before) emit();
}

export function showToast(
  message: string,
  kind: ToastKind = 'info',
  duration = DEFAULT_DURATION[kind],
): number {
  const id = nextId++;
  // 同文案的连续提示只保留一条，避免批量失败时刷屏
  toasts = toasts.filter((toast) => toast.message !== message || toast.kind !== kind);
  toasts = [...toasts, { id, message, kind, duration }];
  emit();
  return id;
}

/** 测试用：清空所有 toast 与订阅状态 */
export function resetToastsForTest(): void {
  toasts = [];
  listeners.clear();
  nextId = 1;
}

/**
 * 按文案推断提示级别：迁移旧页面 notify(message) 调用时使用。
 * 旧实现成功/失败共用一个绿色样式，这里让失败类文案自动落到 error 级别。
 */
export function showResultToast(message: string): number {
  const isError = /失败|错误|无法|不支持|已损坏|超时|拒绝/.test(message);
  return showToast(message, isError ? 'error' : 'success');
}
