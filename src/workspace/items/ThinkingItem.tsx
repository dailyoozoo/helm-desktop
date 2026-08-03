import { memo, useEffect, useId, useRef, useState } from 'react';
import { Markdown } from '../../lib/markdown';
import { Icon } from '../../shell/icons';
import type { ThreadItem } from '../../engine/useSession';
import { thinkingOpenAfterItemUpdate } from '../activityViewModel';

type ThinkingThreadItem = Extract<ThreadItem, { kind: 'thinking' }>;

export const ThinkingItem = memo(function ThinkingItem({
  item,
  className,
  locateTarget,
}: {
  item: ThinkingThreadItem;
  className?: string;
  locateTarget?: { id: string; request: number } | null;
}) {
  const located = locateTarget?.id === item.id;
  const [open, setOpen] = useState(() => !item.done || Boolean(located));
  const previousDone = useRef(item.done);
  const bodyId = useId();
  const duration =
    item.startedAt && item.endedAt
      ? Math.max(0, Math.round((item.endedAt - item.startedAt) / 1000))
      : null;

  useEffect(() => {
    setOpen((current) => thinkingOpenAfterItemUpdate(current, previousDone.current, item.done));
    previousDone.current = item.done;
  }, [item.done]);

  useEffect(() => {
    if (located) setOpen(true);
  }, [located, locateTarget?.request]);

  return (
    <div className={className ? `item ${className}` : 'item'} data-thread-item-id={item.id}>
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
            <span>
              {item.done ? (duration == null ? '分析过程' : `思考了 ${duration} 秒`) : '正在分析'}
            </span>
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
