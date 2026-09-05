import { useEffect, useState } from 'react';
import { dismissToast, subscribeToasts, type ToastEntry, type ToastKind } from './toast';
import { cn } from '@/lib/cn';
import { CheckCircle, Info, AlertCircle } from 'lucide-react';

const ICON_MAP: Record<ToastKind, typeof Info> = {
  info: Info,
  success: CheckCircle,
  error: AlertCircle,
};

function ToastItem({ toast }: { toast: ToastEntry }) {
  useEffect(() => {
    const timer = window.setTimeout(() => dismissToast(toast.id), toast.duration);
    return () => window.clearTimeout(timer);
  }, [toast.id, toast.duration]);

  const Icon = ICON_MAP[toast.kind];
  return (
    <div
      className={cn(
        'flex items-center gap-2.5 rounded-lg border bg-raised px-3.5 py-2.5 shadow-[var(--shadow-pop)]',
        'data-[state=open]:animate-in data-[state=closed]:animate-out',
        toast.kind === 'error' && 'border-danger/30',
        toast.kind === 'success' && 'border-success/30',
        toast.kind === 'info' && 'border-border-2',
      )}
      role="status"
    >
      <Icon
        className={cn(
          'h-4 w-4 shrink-0',
          toast.kind === 'error' && 'text-danger',
          toast.kind === 'success' && 'text-success',
          toast.kind === 'info' && 'text-fg-3',
        )}
      />
      <span className="text-[13px] text-fg">{toast.message}</span>
    </div>
  );
}

/** 全局通知层（P2-2）：挂在 App 根部，渲染 showToast 推入的所有提示。 */
export function ToastLayer() {
  const [toasts, setToasts] = useState<ToastEntry[]>([]);

  useEffect(() => subscribeToasts(setToasts), []);

  if (toasts.length === 0) return null;
  return (
    <div className="fixed bottom-7 left-1/2 z-[400] flex -translate-x-1/2 flex-col items-center gap-2">
      {toasts.map((toast) => (
        <ToastItem key={toast.id} toast={toast} />
      ))}
    </div>
  );
}
