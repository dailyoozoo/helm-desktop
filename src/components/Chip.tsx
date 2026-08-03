import React from 'react';

/** 通用筛选药丸按钮；页面专属样式通过 className 传入页面前缀类（如 sessions-chip）。 */
export function Chip({
  active,
  onClick,
  children,
  className = 'chip',
  title,
}: {
  active: boolean;
  onClick: () => void;
  children: React.ReactNode;
  className?: string;
  title?: string;
}) {
  return (
    <button
      type="button"
      className={className + (active ? ' is-active' : '')}
      onClick={onClick}
      title={title}
    >
      {children}
    </button>
  );
}
