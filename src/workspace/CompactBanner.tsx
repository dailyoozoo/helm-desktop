import { Icon } from '../shell/icons';

/**
 * 变更-34/35 · B4：上下文压缩提醒（Thread 下方、Composer 上方，≥80% 显示）。
 * 只给真实出路（2026-08-12 更正）：
 * - Codex：app-server 官方提供 `thread/compact/start` RPC（真实 headless 契约），
 *   提供「压缩上下文」按钮（busy 时禁用）；
 * - Claude：`claude -p` 无 `/compact` 注入契约（官方 issue #1131 实证），
 *   不造假按钮，仅注明自动压缩，出路为「派生新会话」。
 * 两种引擎都提供「派生新会话」（2026-09-02 起走同引擎无损分支优先，
 * CLI 不支持时才回退摘要派生——按钮文案不再承诺「摘要」，与实际分流一致）。
 */

export interface CompactBannerData {
  /** 最近一次调用的真实输入规模 ÷ 窗口（与 ContextRing 同源）。 */
  percent?: number;
  engine: string;
  working: boolean;
}

export function CompactBanner({
  percent,
  engine,
  working,
  onCompact,
  onFork,
  onClose,
}: CompactBannerData & {
  onCompact?: () => void;
  onFork: () => void;
  onClose: () => void;
}) {
  if (percent == null || percent < 80) return null;
  const isCodex = engine === 'codex';
  const note = isCodex
    ? 'Codex 接近上限时会自动压缩，也可以现在就压缩一下'
    : '挨着上限时 Claude Code 会自动压缩，一般不用操心';
  return (
    <div className="cbanner is-on" role="status">
      <Icon name="compress" />
      <span>
        <b>上下文用了 {percent}%</b> · 任务照常跑，不影响
      </span>
      <span className="cbanner__note">{note}</span>
      <span className="sp" />
      {isCodex ? (
        <button
          className="btn btn--subtle btn--sm"
          type="button"
          onClick={onCompact}
          disabled={working}
          title={
            working ? '当前轮次跑着呢，先停止再压缩' : '触发 Codex 原生压缩，保留最近轮次与全部变更'
          }
        >
          压缩上下文
        </button>
      ) : null}
      <button
        className="btn btn--subtle btn--sm"
        type="button"
        onClick={onFork}
        disabled={working}
        title={working ? '当前轮次跑着呢，先停止再派生' : '开个同引擎新会话，原会话完整保留'}
      >
        派生新会话
      </button>
      <button className="btn-icon sm" type="button" onClick={onClose} aria-label="不再提醒">
        <Icon name="x" />
      </button>
    </div>
  );
}
