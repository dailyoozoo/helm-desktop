import { memo, useState } from 'react';
import { Markdown } from '../../lib/markdown';
import { Icon } from '../../shell/icons';
import type { ThreadItem } from '../../engine/useSession';

type ThinkingThreadItem = Extract<ThreadItem, { kind: 'thinking' }>;

/**
 * 思考卡（渲染形态 B，对齐 WorkBuddy 截图 2026-08-31）：
 * 运行中是带呼吸图标的「正在思考…」活动块；完成后默认收起为「深度思考」轻量单行
 * （WorkBuddy 展开态即此形态），点击展开全文、再点收起。
 */
export const ThinkingItem = memo(function ThinkingItem({
  item,
  className,
}: {
  item: ThinkingThreadItem;
  className?: string;
}) {
  const [open, setOpen] = useState(false);
  if (!item.done) {
    return (
      <div className={className} data-kind="think">
        <div className="think is-live">
          <span className="think__ic">
            <Icon name="sparkles" />
          </span>
          <div>
            <div className="think__lb">正在思考…</div>
            <div className="think__body">
              <Markdown text={item.text} />
            </div>
          </div>
        </div>
      </div>
    );
  }
  return (
    <div className={className} data-kind="think">
      <button
        type="button"
        className="think-lite"
        aria-expanded={open}
        onClick={() => setOpen(!open)}
      >
        <Icon name="sparkles" />
        <span>深度思考</span>
        <span className="think-lite__chev">
          <Icon name="down" />
        </span>
      </button>
      {open ? (
        <div className="think">
          <Markdown text={item.text} />
        </div>
      ) : null}
    </div>
  );
});
