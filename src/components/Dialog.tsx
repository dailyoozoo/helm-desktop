import React from 'react';
import { Icon } from '../shell/icons';
import { useDialogBehavior } from './useDialogBehavior';

/** 通用模态外壳（modal-overlay/modal-panel），Esc 关闭、焦点管理由 useDialogBehavior 提供。 */
export function Dialog({
  title,
  children,
  footer,
  onClose,
}: {
  title: string;
  children: React.ReactNode;
  footer?: React.ReactNode;
  onClose: () => void;
}) {
  const dialogRef = useDialogBehavior(onClose);
  return (
    <div className="modal-overlay" role="dialog" aria-modal="true" aria-label={title}>
      <div className="modal-panel" ref={dialogRef} tabIndex={-1}>
        <div className="modal-panel__head">
          <b>{title}</b>
          <button className="btn-icon sm" onClick={onClose} aria-label="关闭">
            <Icon name="x" />
          </button>
        </div>
        <div className="modal-panel__body">{children}</div>
        {footer ? <div className="modal-panel__foot">{footer}</div> : null}
      </div>
    </div>
  );
}
