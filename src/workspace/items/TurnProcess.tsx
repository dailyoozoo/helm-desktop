import { useEffect, useMemo, useState, type ReactNode } from 'react';
import { Icon } from '../../shell/icons';
import type { ThreadRenderEntry } from '../threadGroups';
import type { TurnSummaryMeta } from '../turnSummary';
import { TurnSummaryHead } from '../TurnSummaryHead';

/**
 * AI 轮次容器（渲染形态 B / WorkBuddy 模式，ADR 0019）：
 * 一轮一个头像 + ai-head（名字「Helm」+ 轮次摘要胶囊 turn__lite）。
 * 过程条目（思考/工具/工具组/子代理）收进可折叠的 .turn-process 过程体，
 * 最终回答、待办审批、失败卡、压缩/派生标记等可见条目常驻直接可见（children）。
 * 折叠语义（对齐 WorkBuddy）：完成后默认折叠为单行摘要，运行中/失败终态/被定位时展开；
 * 失败工具由 ToolBlock 提成 .fail 就地展开，不依赖整轮折叠。
 */
export function TurnProcess({
  id,
  turnId,
  entries,
  completed,
  terminalStatus,
  waitingApproval,
  locateTarget,
  summary,
  process,
  swch,
  children,
}: {
  id: string;
  /** D-10：稳定 Turn id，供轮次刻度轨道定位（data-turn-id）。 */
  turnId?: string;
  entries: ThreadRenderEntry[];
  completed: boolean;
  terminalStatus?: 'succeeded' | 'failed' | 'interrupted';
  waitingApproval: boolean;
  locateTarget?: { id: string; request: number } | null;
  /** 变更-34/35 · B2：轮次摘要（第N轮/模型/耗时/工具数/±行数），缺省不显示胶囊。 */
  summary?: TurnSummaryMeta;
  /** 过程体内容（思考/工具/工具组/子代理等过程条目）。 */
  process?: ReactNode;
  /** 模型切换标记行（原型 .swch，批次①裁决补上），渲染在过程体之后。 */
  swch?: ReactNode;
  /** 轮次可见条目（最终回答/审批卡/失败卡/压缩标记等）。 */
  children: ReactNode;
}) {
  const itemIds = useMemo(
    () =>
      entries.flatMap((entry) =>
        entry.kind === 'tool-group' || entry.kind === 'subagent'
          ? entry.items.map((item) => item.id)
          : [entry.item.id],
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
      (entry.kind === 'tool-group' || entry.kind === 'subagent'
        ? entry.items.filter((item) => isActualFailure(item)).length
        : isActualFailure(entry.item)
          ? 1
          : 0),
    0,
  );
  const failed = failedCount > 0;
  // 渲染形态 B（ADR 0019，对齐 WorkBuddy 实测截图 2026-08-31）：
  // 完成后整轮过程默认折叠为单行摘要（「已完成 · 工具 N · 耗时」），最终答案/审批/
  // 失败卡等 children 常驻可见——与 WorkBuddy 折叠态一致（摘要行 › + 最终答案）。
  // 点摘要头展开后，思考/工具各自折叠成单行、按真实时序显示（对应 WorkBuddy 展开态 ∨）。
  // 运行中(!completed)/终态失败/被定位(located)时默认展开；
  // 失败工具由 ToolBlock 提成 .fail 就地展开，不依赖整轮折叠。
  const [manualOpen, setManualOpen] = useState<boolean | null>(() => (located ? true : null));

  useEffect(() => {
    if (located) setManualOpen(true);
  }, [located, locateTarget?.request]);

  const defaultOpen = located || !completed;
  const open = manualOpen ?? defaultOpen;
  const collapsed = !open;

  const status = waitingApproval
    ? '等待审批'
    : terminalStatus === 'interrupted'
      ? '已中断'
      : terminalStatus === 'failed'
        ? '执行失败'
        : !completed
          ? failed
            ? '执行失败'
            : '进行中'
          : '已完成';
  // 运行态呼吸点仅在真正进行中（未终态、未等审批）时出现；终态失败不显示 live。
  const live = !completed && terminalStatus == null && !waitingApproval;
  const hasProcess = process != null;

  return (
    <div className="item ai-turn" data-turn-process-id={id} data-turn-id={turnId}>
      <div className="item__gut">
        <div className="ava-bot" aria-hidden="true">
          <Icon name="helm" />
        </div>
      </div>
      <div className="item__main">
        <div className="ai-head">
          <span className="ai-head__name">Helm</span>
          {summary ? (
            <TurnSummaryHead
              summary={summary}
              open={open}
              onToggle={() => setManualOpen((value) => !(value ?? defaultOpen))}
              status={status}
              live={live}
              failed={!completed && failed}
              trailing={completed && failedCount ? `${failedCount} 次失败后恢复` : undefined}
            />
          ) : null}
        </div>
        {hasProcess ? (
          <div className={'turn-process' + (collapsed ? ' is-collapsed' : '')}>
            <div className="turn-process__body">{process}</div>
          </div>
        ) : null}
        {swch}
        {children}
      </div>
    </div>
  );
}
