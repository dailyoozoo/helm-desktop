import { memo } from 'react';
import type { Decision } from '@helm/protocol';
import type { ThreadItem } from '../../engine/useSession';
import { Icon } from '../../shell/icons';

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

/**
 * 审批卡（批次①对齐原型 .approve，ws.js L123-126）：警示左缘 + 标题 + 审核提示 +
 * 命令框 + 决定按钮 + 指纹注释。availableDecisions 仍是后端权威决定集合
 * （AGENTS 红线），按钮族按其动态渲染，不写死决定集合。
 */
export const ApprovalCard = memo(function ApprovalCard({ item, onRespond, className }: Props) {
  const disabled = item.status === 'applying' || item.status === 'resolved';
  const retrying = item.status === 'failed';
  // 渲染形态 B（对齐 WorkBuddy）：已处理完的审批收成轻量行——无需再操作，
  // 保留完整警示卡片只会让折叠后的过程区显得层层嵌套；待处理审批保持卡片。
  if (item.status === 'resolved') {
    const denied = item.decision === 'deny';
    return (
      <div className={className} data-kind="approve">
        <div className="approve-lite">
          <Icon name={denied ? 'close' : 'check'} />
          <span className="approve-lite__act">{item.detail || item.action}</span>
          <span
            className="approve-lite__res"
            style={{ color: denied ? 'var(--danger)' : 'var(--success)' }}
          >
            {item.decision ? RESOLVED_LABELS[item.decision] : '审批已失效'}
          </span>
        </div>
      </div>
    );
  }
  return (
    /* 批次①：审批卡不再自带 .item 头像壳，由所在轮次 .ai-turn 统一承担 */
    <div className={className} data-kind="approve">
      <div className="approve">
        <div className="approve__t">
          <Icon name="alert" style={{ width: 16, height: 16 }} />
          {item.action}
        </div>
        <div className="prose">Helm 想要运行一条命令。请先审核再允许。</div>
        <div className="approve__cmd">{item.detail || item.action}</div>
        <div className="approve__note">
          选择&quot;总是允许&quot;后，本会话执行该程序不再逐条确认。
        </div>
        {item.status === 'failed' && item.error ? (
          <div className="approve__error">{item.error}</div>
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
              {decision === 'allow' ? <Icon name="check" /> : null}
              {retrying ? `重试${DECISION_LABELS[decision]}` : DECISION_LABELS[decision]}
            </button>
          ))}
        </div>
        {item.status === 'applying' ? <div className="approve__applying">正在应用审批…</div> : null}
      </div>
    </div>
  );
});
