import { memo, useEffect, useState } from 'react';
import { Icon } from '../../shell/icons';
import type { ThreadItem } from '../../engine/useSession';

type ToolItem = Extract<ThreadItem, { kind: 'tool' }>;

function isDenied(item: ToolItem): boolean {
  return (
    item.outcome === 'auto_review_unavailable' ||
    item.outcome === 'auto_review_parse_error' ||
    item.outcome === 'auto_review_blocked' ||
    item.outcome === 'runtime_denied'
  );
}

function target(item: ToolItem): string {
  if (!item.input || typeof item.input !== 'object') return '';
  const value = item.input as Record<string, unknown>;
  const path = value.file_path ?? value.path ?? value.pattern;
  if (typeof path === 'string') return path;
  if (item.name === 'Bash' && typeof value.command === 'string') return value.command;
  return '';
}

function resultMeta(item: ToolItem): string {
  if (item.status === 'pending') return '运行中';
  if (item.outcome === 'auto_review_unavailable' || item.outcome === 'auto_review_parse_error') {
    return '自动审查不可用，工具未执行';
  }
  if (item.outcome === 'auto_review_blocked' || item.outcome === 'runtime_denied') {
    return '已拒绝，工具未执行';
  }
  if (item.status === 'error') return '失败';
  if (item.diff) {
    let added = 0;
    let removed = 0;
    item.diff.hunks.forEach((hunk) =>
      hunk.lines.forEach((line) => {
        if (line.kind === 'add') added += 1;
        if (line.kind === 'del') removed += 1;
      }),
    );
    return `diff +${added}/-${removed}`;
  }
  if (item.output) return `${item.output.split(/\r?\n/).length} 行输出`;
  return '完成';
}

function terminalLabel(item: ToolItem): string {
  if (item.status === 'pending') return '运行中';
  if (item.outcome === 'auto_review_unavailable' || item.outcome === 'auto_review_parse_error') {
    return '未执行';
  }
  if (item.outcome === 'auto_review_blocked' || item.outcome === 'runtime_denied') {
    return '已拒绝';
  }
  if (item.status === 'error' && item.output?.includes('[turn_interrupted]')) return '已中断';
  return item.status === 'error' ? '失败' : '成功';
}

function duration(item: ToolItem): string {
  if (!item.startedAt || !item.endedAt) return '';
  return `${Math.max(0, (item.endedAt - item.startedAt) / 1000).toFixed(1)}s`;
}

export const ToolGroup = memo(function ToolGroup({
  items,
  locateTarget,
}: {
  items: ToolItem[];
  locateTarget?: { id: string; request: number } | null;
}) {
  const denied = items.filter(isDenied).length;
  const failed = items.filter((item) => item.status === 'error' && !isDenied(item)).length;
  const running = items.some((item) => item.status === 'pending');
  const locatedItemId = items.some((item) => item.id === locateTarget?.id)
    ? locateTarget?.id
    : undefined;
  const [manualOpen, setManualOpen] = useState<boolean | null>(() => (locatedItemId ? true : null));
  const [openRows, setOpenRows] = useState<Set<string>>(() =>
    locatedItemId ? new Set([locatedItemId]) : new Set(),
  );
  const open = manualOpen ?? running;
  useEffect(() => {
    if (!locatedItemId) return;
    setManualOpen(true);
    setOpenRows((current) => {
      if (current.has(locatedItemId)) return current;
      const next = new Set(current);
      next.add(locatedItemId);
      return next;
    });
  }, [locatedItemId, locateTarget?.request]);
  const filesChanged = items.filter((item) => item.diff).length;
  const starts = items.flatMap((item) => (item.startedAt ? [item.startedAt] : []));
  const ends = items.flatMap((item) => (item.endedAt ? [item.endedAt] : []));
  const totalMs = starts.length && ends.length ? Math.max(...ends) - Math.min(...starts) : 0;
  return (
    /* 批次①：工具组不再自带 .item 头像壳，由所在轮次 .ai-turn 统一承担 */
    <div
      className={items.every((item) => item.reverted) ? 'rolled' : undefined}
      data-thread-item-id={items[0].id}
      data-kind="tgrp"
    >
      <div className={`tgrp${open ? '' : ' collapsed'}`}>
        <button
          className="tgrp__head"
          type="button"
          aria-expanded={open}
          onClick={() => setManualOpen(!open)}
        >
          <span className="tgrp__ic">
            <Icon name="layers" />
          </span>
          <span className="tgrp__t">
            {running ? `正在执行第 ${items.length} 个工具…` : `执行了 ${items.length} 个工具`}
          </span>
          <span className="tgrp__meta">
            {[
              filesChanged ? `${filesChanged} 个文件变更` : '',
              failed ? `${failed} 失败` : '',
              denied ? `${denied} 未执行/拒绝` : '',
              totalMs ? `${(totalMs / 1000).toFixed(1)}s` : '',
            ]
              .filter(Boolean)
              .join(' · ')}
          </span>
          {failed ? (
            <span className="pill pill--danger">{failed} 失败</span>
          ) : denied ? (
            <span className="pill pill--warn">{denied} 未执行/拒绝</span>
          ) : running ? (
            <span className="pill pill--warn">运行中</span>
          ) : null}
          <span className="tool__chev">
            <Icon name="down" />
          </span>
        </button>
        {open ? (
          <div className="tgrp__body">
            {items.map((item) => (
              <div
                className={`trow${item.status === 'error' && !isDenied(item) ? ' is-err' : ''}`}
                key={item.id}
                data-thread-item-id={item.id}
              >
                <button
                  className="trow__line"
                  type="button"
                  title="展开工具输出"
                  aria-expanded={openRows.has(item.id)}
                  onClick={() =>
                    setOpenRows((current) => {
                      const next = new Set(current);
                      if (next.has(item.id)) next.delete(item.id);
                      else next.add(item.id);
                      return next;
                    })
                  }
                >
                  <Icon
                    name={
                      item.name === 'Bash'
                        ? 'terminal'
                        : item.name === 'Grep' || item.name === 'Glob'
                          ? 'search'
                          : 'file'
                    }
                  />
                  <span className="trow__name">
                    <b>{item.name}</b>
                    {target(item) ? ` · ${target(item)}` : ''}
                  </span>
                  <span className="trow__meta">{resultMeta(item)}</span>
                  <span className="trow__dur">{duration(item)}</span>
                  <span
                    className={`trow__st ${item.status === 'pending' ? 'is-run' : item.status === 'error' && item.output?.includes('[turn_interrupted]') ? 'is-stop' : item.status === 'error' ? '' : ''}`}
                  >
                    {item.status === 'pending' ? (
                      <>
                        <i />
                        运行中
                      </>
                    ) : (
                      terminalLabel(item)
                    )}
                  </span>
                </button>
                {openRows.has(item.id) ? (
                  <pre className="trow__out">
                    {item.output || JSON.stringify(item.input, null, 2)}
                  </pre>
                ) : null}
              </div>
            ))}
          </div>
        ) : null}
      </div>
    </div>
  );
});
