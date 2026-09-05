import { memo, useEffect, useState } from 'react';
import { Icon } from '../../shell/icons';
import { copyText } from '../../lib/markdown';
import { showToast } from '../../components/toast';
import {
  classifyToolFailure,
  FAILURE_KIND_LABELS,
  failureAdvice,
  type ToolFailureSource,
} from './failureCardViewModel';

// 变更-34 · C4：失败终态卡 —— 错误分类 + 已重试次数 + 能否自愈 + 重试/复制入口。
// 关键是说清「重试能不能自愈」：环境类失败重试无意义，不该让用户空等。
// 9/4 折叠化：轮次运行中默认展开（正在跑的失败要及时看到），轮次结束后默认收起
// 为轻量单行（对齐 .tool 轻量行惯例）；收起时标题+分类药丸常驻——收起轮次下
// 「失败可见」契约（isLiftedFailureEntry / 2026-09-04 视觉矩阵）不受影响。
export const FailureCard = memo(function FailureCard({
  item,
  toolId,
  title,
  retryCount = 0,
  onRetry,
  working = false,
}: {
  item: ToolFailureSource;
  /** 对应 ThreadItem 的工具 id（item 本身不含 id，重试回调需要）。 */
  toolId?: string;
  title: string;
  /** 同一 Turn 中同名工具已重试的次数（真实 Ledger 事实）。 */
  retryCount?: number;
  /** 「重试这一步」：把失败工具作为真实用户消息发回 Agent。
   *  收工具 id、由调用方绑定，避免上层每渲染都造新闭包击穿 memo。 */
  onRetry?: (toolId: string) => void;
  /** 轮次仍在运行时禁用重试并保持展开。 */
  working?: boolean;
}) {
  const kind = classifyToolFailure(item);
  const advice = failureAdvice(kind);
  const output = item.output;
  const note = [retryCount > 0 ? `已重试 ${retryCount} 次。` : '', advice.note]
    .filter(Boolean)
    .join(' ');

  const [manualOpen, setManualOpen] = useState<boolean | null>(null);
  // 轮次一旦结束（working=false），未手动操作的卡自动收起，避免大块报错长期占据线程；
  // 手动开合后尊重用户选择。运行中保持展开。
  const open = working ? (manualOpen ?? true) : (manualOpen ?? false);
  useEffect(() => {
    if (!working) setManualOpen(null);
  }, [working]);

  return (
    <div className={'failc' + (open ? '' : ' collapsed')}>
      <button
        type="button"
        className="failc__t"
        aria-expanded={open}
        onClick={() => setManualOpen(!open)}
      >
        <Icon name="xc" />
        <span>{title}</span>
        <span className="pill pill--danger" style={{ height: 19 }}>
          {FAILURE_KIND_LABELS[kind]}
        </span>
        <span className="tool__chev">
          <Icon name="down" />
        </span>
      </button>
      {open ? (
        <>
          {output ? (
            <div className="failc__out" dir="ltr">
              {output}
            </div>
          ) : null}
          {note ? <div className="failc__note">{note}</div> : null}
          <div className="failc__acts">
            {onRetry ? (
              <button
                type="button"
                className="btn btn--subtle btn--sm fail-retry"
                disabled={working}
                onClick={() => onRetry(toolId ?? '')}
              >
                <Icon name="refresh" /> 重试这一步
              </button>
            ) : null}
            {output ? (
              <button
                type="button"
                className="btn btn--subtle btn--sm fail-copy"
                onClick={async () => {
                  if (await copyText(output)) showToast('报错已复制', 'success');
                  else showToast('复制失败', 'error');
                }}
              >
                <Icon name="copy" /> 复制报错
              </button>
            ) : null}
          </div>
        </>
      ) : null}
    </div>
  );
});
