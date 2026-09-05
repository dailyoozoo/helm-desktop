import { memo } from 'react';
import type { ThreadItem } from '../../engine/useSession';
import { Icon } from '../../shell/icons';

type PlanThreadItem = Extract<ThreadItem, { kind: 'plan' }>;

function stepClass(status: PlanThreadItem['steps'][number]['status']) {
  if (status === 'done') return 'done';
  if (status === 'active') return 'doing';
  return '';
}

export const PlanItem = memo(function PlanItem({
  item,
  className,
}: {
  item: PlanThreadItem;
  className?: string;
}) {
  // 原型计划卡用真实计划标题（ws.js 计划头）；数据模型仅带 steps，取首条步骤文案作为标题，
  // 无步骤时回退「计划」。
  const title = item.steps.find((step) => step.text.trim())?.text ?? '计划';
  return (
    /* 批次①：计划卡不再自带 .item 头像壳，由所在轮次 .ai-turn 统一承担 */
    <div className={className} data-kind="plan">
      <div className="plan">
        <div className="plan__t">
          <Icon name="flag" />
          <span>{title}</span>
        </div>
        <ul>
          {item.steps.map((step, index) => (
            <li key={`${index}-${step.text}`} className={stepClass(step.status)}>
              <span className="box">
                {step.status === 'done' ? (
                  <Icon name="check" />
                ) : step.status === 'active' ? (
                  <i />
                ) : null}
              </span>
              <span>{step.text}</span>
            </li>
          ))}
        </ul>
      </div>
    </div>
  );
});
