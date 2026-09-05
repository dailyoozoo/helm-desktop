import { Icon } from '../shell/icons';

/** 变更-34 · A1：上/下一处变更（跨文件跳转）。 */
export function DiffNavigation({
  total,
  current,
  onPrev,
  onNext,
}: {
  total: number;
  current: number;
  onPrev: () => void;
  onNext: () => void;
}) {
  const position = total === 0 ? '—' : `${Math.min(current + 1, total)} / ${total}`;
  return (
    <span className="anav">
      <button
        type="button"
        className="btn-icon sm"
        title="上一处变更"
        aria-label="上一处变更"
        disabled={total === 0 || current <= 0}
        onClick={onPrev}
      >
        <Icon name="up" />
      </button>
      <span className="pos">{position}</span>
      <button
        type="button"
        className="btn-icon sm"
        title="下一处变更"
        aria-label="下一处变更"
        disabled={total === 0 || current >= total - 1}
        onClick={onNext}
      >
        <Icon name="down" />
      </button>
    </span>
  );
}
