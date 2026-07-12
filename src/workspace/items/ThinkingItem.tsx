import { memo, useEffect, useId, useRef, useState } from 'react';
import { Markdown } from '../../lib/markdown';
import { Icon } from '../../shell/icons';
import type { ThreadItem } from '../../engine/useSession';
import { thinkingOpenAfterItemUpdate } from '../activityViewModel';

type ThinkingThreadItem = Extract<ThreadItem, { kind: 'thinking' }>;

export const ThinkingItem = memo(function ThinkingItem({
  item,
  className,
}: {
  item: ThinkingThreadItem;
  className?: string;
}) {
  const [open, setOpen] = useState(() => !item.done);
  const previousDone = useRef(item.done);
  const bodyId = useId();

  useEffect(() => {
    setOpen((current) => thinkingOpenAfterItemUpdate(current, previousDone.current, item.done));
    previousDone.current = item.done;
  }, [item.done]);

  return (
    <div className={className ? `item ${className}` : 'item'}>
      <div className="item__gut" />
      <div className="item__main">
        <div className={open ? 'think' : 'think collapsed'}>
          <button
            className="think__btn"
            type="button"
            aria-expanded={open}
            aria-controls={bodyId}
            onClick={() => setOpen((value) => !value)}
          >
            <Icon name="sparkles" />
            <span>{item.done ? '分析过程' : '正在分析'}</span>
            <Icon name="down" className="chev" />
          </button>
          <div className="think__body" id={bodyId}>
            <Markdown text={item.text} />
          </div>
        </div>
      </div>
    </div>
  );
});
