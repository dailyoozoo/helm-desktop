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
  return (
    <div className={className ? `item ${className}` : 'item'}>
      <div className="item__gut" />
      <div className="item__main">
        <div className="plan">
          <div className="plan__t">
            <Icon name="flag" />
            <span>计划</span>
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
    </div>
  );
});
