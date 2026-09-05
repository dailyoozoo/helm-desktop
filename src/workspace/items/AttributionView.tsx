import type { IconName } from '../../shell/icons';
import { Icon } from '../../shell/icons';
import { attributionTip, ATTRIBUTION_EMPTY, type AttributionEntry } from '../attributionViewModel';

/**
 * 变更-34/35 · E2：占用归因（把「上下文 31%」拆成「谁占的」）。
 * 只列 Runtime 真实报告过规模的来源；无逐项数据整体显示「暂无」，
 * 不拿累计计费值反推每一项（AGENTS.md 红线）。
 * 视觉对齐原型 workspace.html 的 `.attrow` / `.atttip`。
 */
export function AttributionView({
  entries,
  emptyText = ATTRIBUTION_EMPTY,
}: {
  entries: AttributionEntry[];
  emptyText?: string;
}) {
  if (entries.length === 0) {
    return <div className="attview__empty">{emptyText}</div>;
  }
  const tip = attributionTip(entries);
  return (
    <div className="attview">
      {entries.map((entry) => (
        <div key={entry.label} className={'attrow' + (entry.isHot ? ' is-hot' : '')}>
          {entry.icon ? <Icon name={entry.icon as IconName} /> : null}
          <span className="nm">
            {entry.label}
            {entry.sublabel ? <small>{entry.sublabel}</small> : null}
          </span>
          <span className="v">{entry.value}</span>
        </div>
      ))}
      {tip ? (
        <div className="atttip">
          <Icon name="info" />
          <span>{tip}</span>
        </div>
      ) : null}
    </div>
  );
}
