import { memo } from 'react';
import type { Decision } from '@helm/protocol';
import type { ThreadItem } from '../../engine/useSession';

type ApprovalItem = Extract<ThreadItem, { kind: 'approval' }>;

interface Props {
  item: ApprovalItem;
  onRespond: (approvalId: string, decision: Decision) => void;
  className?: string;
}

export const ApprovalCard = memo(function ApprovalCard({ item, onRespond, className }: Props) {
  return (
    <div className={className ? `item ${className}` : 'item'}>
      <div className="item__gut" />
      <div className="item__main">
        <div className={`approve${item.resolved ? ' resolved' : ''}`}>
          <div className="approve__t">
            <svg
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              strokeWidth="2"
              width="16"
              height="16"
            >
              <path d="M10.29 3.86L1.82 18a2 2 0 001.71 3h16.94a2 2 0 001.71-3L13.71 3.86a2 2 0 00-3.42 0z" />
              <line x1="12" y1="9" x2="12" y2="13" />
              <line x1="12" y1="17" x2="12.01" y2="17" />
            </svg>
            {item.action}
          </div>
          <div className="prose" style={{ fontSize: 13, color: 'var(--fg-3)', marginBottom: 8 }}>
            Helm 想要执行以下操作，请先审核再允许。
          </div>
          <div className="approve__cmd">{item.detail || item.action}</div>
          <div className="approve__acts">
            <button
              className="btn btn--primary btn--sm"
              onClick={() => onRespond(item.id, 'allow')}
              disabled={item.resolved}
            >
              仅允许一次
            </button>
            <button
              className="btn btn--subtle btn--sm"
              onClick={() => onRespond(item.id, 'always')}
              disabled={item.resolved}
            >
              始终允许
            </button>
            <button
              className="btn btn--danger btn--sm"
              onClick={() => onRespond(item.id, 'deny')}
              disabled={item.resolved}
            >
              拒绝
            </button>
          </div>
          <div className="approve__done">
            <svg
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              strokeWidth="2"
              width="15"
              height="15"
            >
              <polyline points="20 6 9 17 4 12" />
            </svg>
            <span style={{ color: 'var(--success)' }}>已处理</span>
          </div>
        </div>
      </div>
    </div>
  );
});
