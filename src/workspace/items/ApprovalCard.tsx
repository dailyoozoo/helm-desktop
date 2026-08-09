import { memo } from 'react';
import type { Decision } from '@helm/protocol';
import type { ThreadItem } from '../../engine/useSession';

type ApprovalItem = Extract<ThreadItem, { kind: 'approval' }>;

interface Props {
  item: ApprovalItem;
  onRespond: (approvalId: string, decision: Decision) => void;
  className?: string;
}

const DECISION_LABELS: Record<Decision, string> = {
  allow: '当次允许',
  turn: '本轮允许',
  session: '总是允许',
  project: '此项目允许',
  always: '所有项目允许',
  deny: '拒绝',
};

const RESOLVED_LABELS: Record<Decision, string> = {
  allow: '已批准当次',
  turn: '已批准本轮',
  session: '已批准本会话',
  project: '已批准此项目',
  always: '已批准所有项目',
  deny: '已拒绝',
};

export const ApprovalCard = memo(function ApprovalCard({ item, onRespond, className }: Props) {
  const disabled = item.status === 'applying' || item.status === 'resolved';
  const retrying = item.status === 'failed';
  return (
    <div className={className ? `item ${className}` : 'item'}>
      <div className="item__gut" />
      <div className="item__main">
        <div className={`approve${item.status === 'resolved' ? ' resolved' : ''}`}>
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
          {item.matcherSummary && item.availableDecisions.includes('session') ? (
            <div className="approve__scope">
              选择&quot;总是允许&quot;后，本会话执行该程序不再逐条确认。
            </div>
          ) : null}
          {item.status === 'failed' && item.error ? (
            <div className="prose" style={{ fontSize: 12, color: 'var(--danger)', marginTop: 8 }}>
              {item.error}
            </div>
          ) : null}
          <div className="approve__acts">
            {item.availableDecisions.map((decision) => (
              <button
                key={decision}
                className={`btn btn--sm ${
                  decision === 'allow'
                    ? 'btn--primary'
                    : decision === 'deny'
                      ? 'btn--danger'
                      : 'btn--subtle'
                }`}
                onClick={() => onRespond(item.id, decision)}
                disabled={disabled}
              >
                {retrying ? `重试${DECISION_LABELS[decision]}` : DECISION_LABELS[decision]}
              </button>
            ))}
          </div>
          {item.status === 'applying' ? (
            <div className="prose" style={{ fontSize: 12, color: 'var(--fg-3)', marginTop: 8 }}>
              正在应用审批…
            </div>
          ) : null}
          {item.status === 'resolved' ? (
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
              <span
                style={{ color: item.decision === 'deny' ? 'var(--danger)' : 'var(--success)' }}
              >
                {item.decision ? RESOLVED_LABELS[item.decision] : '审批已失效'}
              </span>
            </div>
          ) : null}
        </div>
      </div>
    </div>
  );
});
