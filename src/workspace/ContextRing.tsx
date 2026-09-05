import { useEffect, useRef, useState } from 'react';
import type { SessionState } from '../engine/useSession';
import type { BillingTokenSummary } from './contextPanelViewModel';
import { billingSummary } from './contextPanelViewModel';
import type { AttributionEntry } from './attributionViewModel';
import { contextRingHoverSummary, type ContextSnapshotViewModel } from './contextSnapshotViewModel';
import { Icon } from '../shell/icons';

/**
 * 变更-34/35 · D4/E2 · 切片 D（P1-02）：上下文指示器（Composer 右下角常驻圆环）。
 * 三级递进：
 *   1. 常驻圆环（最近一次调用的真实输入规模 ÷ 窗口）
 *   2. hover 摘要（文件数 / MCP 数 / 总 token 与百分比，仅真实字段，不估算）
 *   3. 点击完整 popover（占用 + 计费 token 两节，严格对齐 prototype/workspace.html 的 ctxpop 骨架）
 * 口径红线：显示的是 context_usage 事件的真实值，不是累计计费值；归因只列 Runtime 真实逐来源，无数据则空。
 */

/** 旧 ContextRingDetail 兼容入口：Workspace 仍直接传 cost/fileCount/mcpCount；
 * 切片 D 内部转换为 ContextSnapshotViewModel 派生口径，对外接口保持稳定。 */
export interface ContextRingDetail {
  cost?: SessionState['cost'];
  /** 历史附件数（「上下文中的文件」）。 */
  fileCount?: number;
  /** 已连接 MCP 服务器数。 */
  mcpCount?: number;
  /** 计费 token（累计，四账 + 缓存读取占比）。 */
  billing?: BillingTokenSummary;
  messageCount?: number;
  startedAt?: number | null;
  /** E2 归因条目；无逐项数据传空数组 → 「暂无归因数据」。 */
  attribution?: AttributionEntry[];
}

/** S3：会话上下文增删动作（由 Workspace 注入真实 list/add/remove_session_contexts 链路）。 */
export interface SessionContextEditActions {
  /** 会话就绪且非运行中时才可编辑。 */
  enabled: boolean;
  /** 对话框/IPC 往返进行中，按钮防抖。 */
  busy?: boolean;
  onAddFile: () => void;
  onAddDirectory: () => void;
  onRemove: (contextId: string) => void;
}

export interface ContextRingState {
  ratio?: number;
  percent?: number;
  level: 'none' | 'warn' | 'danger';
}

/** 圆环纯状态：真实 tokens/maxTokens 推导百分比与告警级。 */
export function contextRingState(tokens?: number, maxTokens?: number): ContextRingState {
  if (tokens == null || !maxTokens || maxTokens <= 0) {
    return { level: 'none' };
  }
  const ratio = Math.min(1, tokens / maxTokens);
  return {
    ratio,
    percent: Math.round(ratio * 100),
    level: ratio >= 0.95 ? 'danger' : ratio >= 0.8 ? 'warn' : 'none',
  };
}

/** 圆环周长（r=13，对齐原型）。 */
const RING_R = 13;
const RING_C = 2 * Math.PI * RING_R;

export function ContextRing({
  detail,
  snapshot,
  defaultOpen = false,
}: {
  detail?: ContextRingDetail;
  /** 切片 D · P1-02：统一快照（含分栏文件 / 有效 MCP / 归因），优先于 detail。 */
  snapshot?: ContextSnapshotViewModel;
  /** S3：会话上下文增删动作；不传时列表只读。 */
  sessionContextEdit?: SessionContextEditActions;
  /** 测试/SSR 用：强制 popover 初始展开。生产代码不传。 */
  defaultOpen?: boolean;
}) {
  const [open, setOpen] = useState(defaultOpen);
  const [hover, setHover] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);
  const btnRef = useRef<HTMLButtonElement>(null);
  const { cost } = detail ?? {};
  const ring = contextRingState(cost?.contextTokens, cost?.contextWindow);
  const ratio = ring.ratio ?? 0;
  const dashOffset = RING_C * (1 - ratio);
  const fmt = (n?: number) => (n == null ? '暂无' : n.toLocaleString('zh-CN'));
  const billing = snapshot?.billing ?? detail?.billing ?? (cost ? billingSummary(cost) : undefined);

  const hoverSummary = snapshot ? contextRingHoverSummary(snapshot) : null;

  useEffect(() => {
    if (!open) return;
    const onPointerDown = (event: MouseEvent) => {
      if (rootRef.current && !rootRef.current.contains(event.target as Node)) {
        setOpen(false);
      }
    };
    const onKey = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        setOpen(false);
        // 原型一致：Esc 关闭后焦点回到圆环按钮（键盘焦点通过）。
        window.requestAnimationFrame(() => btnRef.current?.focus());
      }
    };
    window.addEventListener('mousedown', onPointerDown);
    window.addEventListener('keydown', onKey);
    return () => {
      window.removeEventListener('mousedown', onPointerDown);
      window.removeEventListener('keydown', onKey);
    };
  }, [open]);

  const hasData = cost?.contextTokens != null && cost?.contextWindow;
  const hoverText = hoverSummary
    ? hoverSummary.percent == null
      ? `本会话尚无逐调用用量数据 · 文件 ${hoverSummary.files} · MCP ${hoverSummary.mcp}`
      : `上下文 ${hoverSummary.percent}% · ${fmt(hoverSummary.tokens ?? undefined)}/${fmt(hoverSummary.maxTokens ?? undefined)} · 文件 ${hoverSummary.files} · MCP ${hoverSummary.mcp}`
    : hasData
      ? `上下文 ${ring.percent}% · ${fmt(cost?.contextTokens)}/${fmt(cost?.contextWindow)}（最近一次调用的真实输入规模）`
      : '本会话尚无逐调用用量数据 · 点击查看说明';
  return (
    <div className="ctxring" ref={rootRef}>
      <button
        type="button"
        ref={btnRef}
        className={'ctxring__btn' + (ring.level !== 'none' ? ` is-${ring.level}` : '')}
        aria-haspopup="dialog"
        aria-expanded={open}
        title={hoverText}
        onMouseEnter={() => setHover(true)}
        onMouseLeave={() => setHover(false)}
        onFocus={() => setHover(true)}
        onBlur={() => setHover(false)}
        onClick={() => setOpen((current) => !current)}
      >
        <svg viewBox="0 0 32 32" aria-hidden="true">
          <circle className="rbg" cx="16" cy="16" r={RING_R} />
          <circle
            className="rfg"
            cx="16"
            cy="16"
            r={RING_R}
            strokeDasharray={RING_C.toFixed(1)}
            strokeDashoffset={dashOffset.toFixed(1)}
          />
        </svg>
        <span className="ctxring__pct">{hasData ? `${ring.percent}%` : '—'}</span>
      </button>
      {hover && !open && hoverSummary ? (
        <div className="ctxring__hover" role="status" aria-live="polite">
          {hoverSummary.percent == null ? (
            <span>尚无逐调用用量数据</span>
          ) : (
            <span>
              {hoverSummary.percent}% · {fmt(hoverSummary.tokens ?? undefined)} /{' '}
              {fmt(hoverSummary.maxTokens ?? undefined)}
            </span>
          )}
          <span className="ctxring__hover-meta">
            文件 {hoverSummary.files} · MCP {hoverSummary.mcp}
          </span>
        </div>
      ) : null}
      {open ? (
        <div className="ctxring__pop" role="dialog" aria-label="上下文与用量明细">
          <div className="csec__t">
            <Icon name="layers" /> 上下文占用
            {ring.percent != null ? (
              <span
                className={`pill ${ring.level === 'danger' ? 'pill--danger' : ring.level === 'warn' ? 'pill--warn' : 'pill--accent'}`}
              >
                {ring.percent}%
              </span>
            ) : null}
          </div>
          <div className="usage">
            <div className="usage__row">
              <span className="usage__big">
                {hasData ? `${fmt(cost?.contextTokens)}` : '暂无'}
                {cost?.contextWindow ? <small> / {fmt(cost?.contextWindow)}</small> : null}
              </span>
            </div>
            <div className="meter">
              <i style={{ width: `${Math.round(ratio * 100)}%` }} />
            </div>
            <div className="usage__note">最近一次调用的真实输入 ÷ 窗口上限 · 非累计、非估算</div>
            {ring.level === 'danger' || ring.level === 'warn' ? (
              <div className={'usage__hint' + (ring.level === 'danger' ? ' is-danger' : '')}>
                <Icon name="alert" />
                <span>
                  {ring.level === 'danger'
                    ? '接近上下文上限：可切换到更大窗口模型，或新建会话继续。'
                    : '上下文占用较高：想继续就派生出新会话，任务不受影响。'}
                </span>
              </div>
            ) : null}
          </div>

          <div className="csec__t">
            <Icon name="coins" /> 计费 token · 累计
          </div>
          {billing ? (
            <div className="usage">
              <div className="billrow">
                <span>未缓存输入</span>
                <span className="mono">{fmt(billing.freshInput)}</span>
              </div>
              <div className="billrow">
                <span>缓存写入</span>
                <span className="mono">{fmt(billing.cacheWrite)}</span>
              </div>
              <div className="billrow">
                <span>输出</span>
                <span className="mono">{fmt(billing.output)}</span>
              </div>
              <div className="usage__note">
                缓存读取 <span className="mono">{fmt(billing.cacheRead)}</span> · 命中率{' '}
                <span className="mono">
                  {billing.cacheReadShare != null
                    ? `${Math.round(billing.cacheReadShare * 100)}%`
                    : '—'}
                </span>{' '}
                · ≈0.1× 计费（累计账单量，不代表窗口占用）
              </div>
            </div>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}
