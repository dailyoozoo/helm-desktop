import { useEffect, useMemo, useState, type ReactNode } from 'react';
import { Icon } from '../../shell/icons';
import type { ThreadRenderEntry } from '../threadGroups';

export function TurnProcess({
  id,
  entries,
  completed,
  terminalStatus,
  waitingApproval,
  locateTarget,
  children,
}: {
  id: string;
  entries: ThreadRenderEntry[];
  completed: boolean;
  terminalStatus?: 'succeeded' | 'failed' | 'interrupted';
  waitingApproval: boolean;
  locateTarget?: { id: string; request: number } | null;
  children: ReactNode;
}) {
  const itemIds = useMemo(
    () =>
      entries.flatMap((entry) =>
        entry.kind === 'tool-group' ? entry.items.map((item) => item.id) : [entry.item.id],
      ),
    [entries],
  );
  const located = Boolean(locateTarget && itemIds.includes(locateTarget.id));
  const isActualFailure = (item: Extract<ThreadRenderEntry, { kind: 'item' }>['item']) =>
    item.kind === 'tool' &&
    item.status === 'error' &&
    item.outcome !== 'auto_review_unavailable' &&
    item.outcome !== 'auto_review_parse_error' &&
    item.outcome !== 'auto_review_blocked' &&
    item.outcome !== 'runtime_denied';
  const failedCount = entries.reduce(
    (count, entry) =>
      count +
      (entry.kind === 'tool-group'
        ? entry.items.filter((item) => isActualFailure(item)).length
        : isActualFailure(entry.item)
          ? 1
          : 0),
    0,
  );
  const failed = failedCount > 0;
  const running = entries.some((entry) =>
    entry.kind === 'tool-group'
      ? entry.items.some((item) => item.status === 'pending')
      : entry.item.kind === 'thinking' && !entry.item.done,
  );
  const terminalFailed = terminalStatus === 'failed' || terminalStatus === 'interrupted';
  const automaticOpen = !completed || running || waitingApproval || terminalFailed;
  const [manualOpen, setManualOpen] = useState<boolean | null>(() => (located ? true : null));
  const open = located || automaticOpen || manualOpen === true;

  useEffect(() => {
    if (located) setManualOpen(true);
  }, [located, locateTarget?.request]);

  const toolCount = entries.reduce(
    (count, entry) =>
      count +
      (entry.kind === 'tool-group' ? entry.items.length : entry.item.kind === 'tool' ? 1 : 0),
    0,
  );
  const processCount = entries.filter(
    (entry) => entry.kind === 'item' && entry.item.kind !== 'tool',
  ).length;
  const status = waitingApproval
    ? '等待审批'
    : running
      ? '进行中'
      : terminalStatus === 'interrupted'
        ? '已中断'
        : terminalStatus === 'failed'
          ? '执行失败'
          : completed
            ? '已完成'
            : failed
              ? '执行失败'
              : '未完成';

  return (
    <div className="item turn-process" data-turn-process-id={id}>
      <div className="item__gut" />
      <div className="item__main">
        <div className={`turn-process__box${open ? '' : ' collapsed'}`}>
          <button
            type="button"
            className="turn-process__head"
            aria-expanded={Boolean(open)}
            onClick={() => setManualOpen(!open)}
          >
            <span className="turn-process__icon">
              <Icon name="layers" />
            </span>
            <span className="turn-process__title">轮次过程</span>
            <span className="turn-process__meta">
              {processCount ? `${processCount} 段过程` : ''}
              {processCount && toolCount ? ' · ' : ''}
              {toolCount ? `${toolCount} 个工具` : ''}
              {completed && failedCount ? ` · ${failedCount} 次失败后恢复` : ''}
            </span>
            <span
              className={`pill${terminalFailed || (!completed && failed) ? ' pill--danger' : running || waitingApproval || failedCount ? ' pill--warn' : ''}`}
            >
              {status}
            </span>
            <span className="tool__chev">
              <Icon name="down" />
            </span>
          </button>
          {open ? <div className="turn-process__body">{children}</div> : null}
        </div>
      </div>
    </div>
  );
}
