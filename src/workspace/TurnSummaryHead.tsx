import { Icon } from '../shell/icons';
import type { TurnSummaryMeta } from './turnSummary';
import { formatTurnDuration } from './turnSummary';

/**
 * 轮次折叠胶囊（对齐原型 turnLite，ws.js L437-453）：完成态「已完成 · 思考了 N 秒 ·
 * 工具 N · 耗时」，运行态呼吸点「正在思考」+ 同一串事实；点击折叠/展开整轮过程。
 * 数据全部来自 TurnLedger 与真实条目（变更-34/35 · B2），缺项不显示、不用占位符。
 */
export function TurnSummaryHead({
  summary,
  open,
  onToggle,
  status,
  live,
  failed,
  trailing,
}: {
  summary: TurnSummaryMeta;
  open: boolean;
  onToggle: () => void;
  /** 状态文案（与 TurnProcess 同源：等待审批/进行中/已完成/执行失败…） */
  status?: string;
  /** 轮次仍活跃（呼吸点 + 强调底色，原型 is-live） */
  live?: boolean;
  /** 终态失败/中断（胶囊文字转危险色） */
  failed?: boolean;
  /** 头部右侧附加内容（如「N 次失败后恢复」），缺省不渲染 */
  trailing?: string;
}) {
  const bits: string[] = [];
  if (summary.thinkingSec != null && summary.thinkingSec > 0)
    bits.push(`思考了 ${Math.round(summary.thinkingSec)} 秒`);
  if (summary.toolCount != null && summary.toolCount > 0) bits.push(`工具 ${summary.toolCount}`);
  if (summary.durationSec != null) bits.push(formatTurnDuration(summary.durationSec));
  if (trailing) bits.push(trailing);

  return (
    <button
      type="button"
      className={'turn__lite' + (live ? ' is-live' : '') + (failed ? ' is-fail' : '')}
      aria-expanded={Boolean(open)}
      aria-label={`第 ${summary.turnNumber} 轮，点击${open ? '折叠' : '展开'}整轮`}
      onClick={onToggle}
    >
      {live ? <span className="live-dot" aria-hidden="true" /> : null}
      {status ? <span>{status}</span> : null}
      {bits.length ? <span className="mono">{bits.join(' · ')}</span> : null}
      <span className="chev" aria-hidden="true">
        <Icon name="down" />
      </span>
    </button>
  );
}
