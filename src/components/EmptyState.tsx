import type { ReactNode } from 'react';
import { Icon, type IconName } from '../shell/icons';

/**
 * 带下一步动作的空态（P2-3）：图标 + 标题 + 说明 + 可选 CTA。
 * 供扩展中心、用量页等列表空态复用；纯文本空态请继续用 `.empty`。
 */
export function EmptyState({
  icon,
  title,
  hint,
  action,
}: {
  icon: IconName;
  title: string;
  hint?: ReactNode;
  action?: { label: string; onClick: () => void };
}) {
  return (
    <div className="empty empty--cta">
      <span className="empty__ic">
        <Icon name={icon} />
      </span>
      <b>{title}</b>
      {hint ? <p>{hint}</p> : null}
      {action ? (
        <button className="btn btn--primary btn--sm" onClick={action.onClick} type="button">
          {action.label}
        </button>
      ) : null}
    </div>
  );
}
