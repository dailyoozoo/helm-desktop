import { memo, useEffect, useMemo, useState, type RefObject } from 'react';
import type { ThreadItem } from '../engine/useSession';
import type { SessionTurn } from '../sessions/api';

/**
 * D-10（可靠性检查-工作区对话页-差异清单，用户裁决「跟原型一致」）：轮次刻度轨道。
 * 1:1 对齐原型 workspace-rail.js + .turn-rail（原型 workspace.css L7-36）：>3 轮显示；
 * 刻度作为一组在对话区垂直居中、紧凑等距排布（步进 16–30px，不随滚动高度铺满）；
 * 一条短横线对应一个稳定 Turn；悬停展开轨道并弹出「第 N 轮 · 状态 · 时间 +
 * 提问摘要（≤42 字符）」；点击定位轮次开头并闪烁。
 * 「回到最新」不再挂轨道（用户裁决 B 形态）：改由 Thread 的底部中央渐隐浮层
 * （.ws-jumplatest）承担，任何会话长度均可用。
 * 位置/状态全部来自真实 Turn/Ledger 与滚动位置，不使用均匀百分比或定时器伪造进度
 * （术语表 ThreadNav 红线）。
 */

const SUMMARY_MAX = 42;

export interface TurnRailMarker {
  turnId: string;
  index: number;
  status: SessionTurn['status'] | 'active';
  startedAt?: number;
  question: string;
}

/** 状态文案 + 气泡状态字色（原型 pr-st.is-*）；刻度线本身不着色，对齐原型。 */
const STATUS_META: Record<TurnRailMarker['status'], { label: string; cls: string }> = {
  active: { label: '运行中', cls: ' is-run' },
  running: { label: '运行中', cls: ' is-run' },
  stalled: { label: '受阻', cls: ' is-wait' },
  waiting_approval: { label: '等审批', cls: ' is-wait' },
  succeeded: { label: '已完成', cls: '' },
  failed: { label: '失败', cls: ' is-fail' },
  interrupted: { label: '已中断', cls: ' is-fail' },
};

export function turnRailMarkers(turns: SessionTurn[] | null | undefined): TurnRailMarker[] {
  if (!turns || turns.length === 0) return [];
  return turns.map((turn, index) => ({
    turnId: turn.id,
    index: index + 1,
    status: turn.status,
    ...(turn.startedAt ? { startedAt: turn.startedAt } : {}),
    question: '',
  }));
}

export function firstQuestionByTurn(items: ThreadItem[]): Map<string, string> {
  const map = new Map<string, string>();
  for (const item of items) {
    if (item.kind !== 'user') continue;
    if (item.turnId && !map.has(item.turnId)) map.set(item.turnId, item.text);
  }
  return map;
}

const fmtTime = (ts?: number) =>
  ts ? new Date(ts).toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit' }) : '';

export const ThreadTurnRail = memo(function ThreadTurnRail({
  scrollRef,
  turns,
  items,
}: {
  scrollRef: RefObject<HTMLDivElement | null>;
  turns?: SessionTurn[] | null;
  items: ThreadItem[];
}) {
  const [current, setCurrent] = useState(-1);
  const [hover, setHover] = useState<number | null>(null);
  const [zoneHover, setZoneHover] = useState(false);
  // 紧凑等距步进（原型 buildRail）：初始 26px，挂载后按滚动区高度计算
  const [gap, setGap] = useState(26);
  const questions = useMemo(() => firstQuestionByTurn(items), [items]);
  const markers = useMemo(
    () => turnRailMarkers(turns).map((m) => ({ ...m, question: questions.get(m.turnId) ?? '' })),
    [turns, questions],
  );
  const show = markers.length > 3;

  // 原型 L44-45：步进 = clamp(16, 30, min(滚动区高 55%, 440px) / 轮数)，
  // 少量轮次不会被拉开过远；随滚动区尺寸变化重算（ResizeObserver）。
  useEffect(() => {
    const el = scrollRef.current;
    if (!el || !show) return;
    const compute = () =>
      setGap(Math.max(16, Math.min(30, Math.min(el.clientHeight * 0.55, 440) / markers.length)));
    compute();
    const ro = new ResizeObserver(compute);
    ro.observe(el);
    return () => ro.disconnect();
  }, [scrollRef, markers.length, show]);

  // 当前轮高亮（原型 updateRail）：来自真实滚动位置
  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const onScroll = () => {
      const ticks = Array.from(el.querySelectorAll<HTMLElement>('[data-turn-id]'));
      let found = -1;
      for (let i = 0; i < ticks.length; i += 1) {
        if (
          ticks[i].getBoundingClientRect().top - el.getBoundingClientRect().top <=
          80 + el.scrollTop
        )
          found = i;
      }
      setCurrent(found);
    };
    onScroll();
    el.addEventListener('scroll', onScroll, { passive: true });
    return () => el.removeEventListener('scroll', onScroll);
  }, [scrollRef, markers.length]);

  if (!show) return null;
  const jumpTo = (turnId: string) => {
    const el = scrollRef.current;
    if (!el) return;
    const target = el.querySelector<HTMLElement>('[data-turn-id="' + turnId + '"]');
    if (!target) return;
    target.scrollIntoView({ behavior: 'smooth', block: 'start' });
    target.classList.add('ws-turn-jump');
    window.setTimeout(() => target.classList.remove('ws-turn-jump'), 1100);
  };
  const popTop = hover != null ? Math.round(hover * gap + gap / 2 - 30) : 0;
  const hoverMarker = hover != null ? markers[hover] : null;
  return (
    <div className={'ws-turnrail is-on' + (zoneHover ? ' is-hover' : '')} aria-label="轮次导航">
      {/* 32px 热区：鼠标进入时轨道轻微展开（原型 railZone，is-hover） */}
      <div
        className="ws-turnrail__zone"
        onMouseEnter={() => setZoneHover(true)}
        onMouseLeave={() => setZoneHover(false)}
      />
      {markers.map((m, i) => {
        const title =
          '第 ' +
          m.index +
          ' 轮 · ' +
          STATUS_META[m.status].label +
          (fmtTime(m.startedAt) ? ' · ' + fmtTime(m.startedAt) : '');
        return (
          <button
            key={m.turnId}
            type="button"
            className={'ws-turnrail__item' + (i === current ? ' is-current' : '')}
            style={{ height: Math.round(gap) + 'px' }}
            title={title}
            aria-label={title}
            onMouseEnter={() => setHover(i)}
            onFocus={() => setHover(i)}
            onMouseLeave={() => setHover(null)}
            onBlur={() => setHover(null)}
            onClick={() => jumpTo(m.turnId)}
          >
            <span className="ws-turnrail__line" aria-hidden="true" />
          </button>
        );
      })}
      {hoverMarker ? (
        <div className="ws-turnrail__pop is-on" role="tooltip" style={{ top: popTop + 'px' }}>
          <span className="pr-no">第 {hoverMarker.index} 轮</span> ·{' '}
          <span className={'pr-st' + STATUS_META[hoverMarker.status].cls}>
            {STATUS_META[hoverMarker.status].label}
          </span>{' '}
          · <span className="pr-no">{fmtTime(hoverMarker.startedAt) || '刚刚'}</span>
          {hoverMarker.question ? (
            <span className="pr-tx">
              {hoverMarker.question.slice(0, SUMMARY_MAX)}
              {hoverMarker.question.length > SUMMARY_MAX ? '…' : ''}
            </span>
          ) : null}
        </div>
      ) : null}
    </div>
  );
});
