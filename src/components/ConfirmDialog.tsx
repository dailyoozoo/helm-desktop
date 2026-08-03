import React, { useState } from 'react';
import { Dialog } from './Dialog';

/** 通用确认对话框：onConfirm 返回 Promise 时进入 busy 态；是否关闭由调用方在 onConfirm/onCancel 中控制。 */
export function ConfirmDialog({
  title,
  body,
  confirmLabel,
  danger = true,
  onCancel,
  onConfirm,
}: {
  title: string;
  body: React.ReactNode;
  confirmLabel: string;
  danger?: boolean;
  onCancel: () => void;
  onConfirm: () => void | Promise<void>;
}) {
  const [busy, setBusy] = useState(false);
  return (
    <Dialog
      title={title}
      onClose={busy ? () => undefined : onCancel}
      footer={
        <>
          <button className="btn btn--ghost" onClick={onCancel} disabled={busy} type="button">
            取消
          </button>
          <button
            className={danger ? 'btn btn--danger' : 'btn btn--primary'}
            disabled={busy}
            type="button"
            onClick={() => {
              const result = onConfirm();
              if (result instanceof Promise) {
                setBusy(true);
                void result.finally(() => setBusy(false));
              }
            }}
          >
            {busy ? '处理中...' : confirmLabel}
          </button>
        </>
      }
    >
      <p className="modal-copy">{body}</p>
    </Dialog>
  );
}
