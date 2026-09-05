import React from 'react';
import {
  Dialog as ShadcnDialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogFooter,
} from '@/components/ui/dialog';

interface DialogProps {
  title: string;
  children: React.ReactNode;
  footer?: React.ReactNode;
  onClose: () => void;
  /** 弹窗宽度档：md=620（默认，对齐 cm-modal）/ sm=560 / xs=400（确认卡，对齐 wsconfirm）。 */
  size?: 'md' | 'sm' | 'xs';
}

const SIZE_CLASS: Record<NonNullable<DialogProps['size']>, string> = {
  md: '',
  sm: 'cm-modal--sm',
  xs: 'cm-modal--xs',
};

/** 通用模态外壳，基于 shadcn Dialog（Radix UI）。Esc 关闭、焦点管理由 Radix 提供。 */
export function Dialog({ title, children, footer, onClose, size = 'md' }: DialogProps) {
  return (
    <ShadcnDialog
      open
      onOpenChange={(open) => {
        if (!open) onClose();
      }}
    >
      {/* 无正文描述时向 Radix 显式声明 aria-describedby 缺省，避免无意义警告。 */}
      <DialogContent showClose aria-describedby={undefined} className={SIZE_CLASS[size]}>
        <DialogHeader>
          <DialogTitle>{title}</DialogTitle>
        </DialogHeader>
        {children}
        {footer ? <DialogFooter>{footer}</DialogFooter> : null}
      </DialogContent>
    </ShadcnDialog>
  );
}
