import { useEffect, useState } from 'react';
import { dismissToast, subscribeToasts, type ToastEntry, type ToastKind } from './toast';

const ICONS: Record<ToastKind, string> = {
  // 提示/成功/错误共用一套线性图标路径（stroke 风格与页面一致）
  info: 'M12 8v5m0 3v.01M12 21a9 9 0 1 1 0-18 9 9 0 0 1 0 18Z',
  success: 'm5 13 4 4L19 7',
  error: 'M12 8v5m0 3v.01M12 21a9 9 0 1 1 0-18 9 9 0 0 1 0 18Z',
};

function ToastItem({ toast }: { toast: ToastEntry }) {
  useEffect(() => {
    const timer = window.setTimeout(() => dismissToast(toast.id), toast.duration);
    return () => window.clearTimeout(timer);
  }, [toast.id, toast.duration]);

  return (
    <div className={`toast show toast-item toast-item--${toast.kind}`} role="status">
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
        <path d={ICONS[toast.kind]} strokeLinecap="round" strokeLinejoin="round" />
      </svg>
      <span>{toast.message}</span>
      <button
        type="button"
        className="toast-item__close"
        aria-label="关闭提示"
        onClick={() => dismissToast(toast.id)}
      >
        ×
      </button>
    </div>
  );
}

/** 全局通知层（P2-2）：挂在 App 根部，渲染 showToast 推入的所有提示。 */
export function ToastLayer() {
  const [toasts, setToasts] = useState<ToastEntry[]>([]);

  useEffect(() => subscribeToasts(setToasts), []);

  if (toasts.length === 0) return null;
  return (
    <div className="toast-layer">
      {toasts.map((toast) => (
        <ToastItem key={toast.id} toast={toast} />
      ))}
    </div>
  );
}
