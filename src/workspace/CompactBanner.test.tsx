import { describe, expect, it } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import { CompactBanner } from './CompactBanner';

const baseProps = {
  percent: 85,
  working: false,
  onFork: () => {},
  onClose: () => {},
};

describe('CompactBanner', () => {
  it('低于 80% 不渲染', () => {
    const html = renderToStaticMarkup(<CompactBanner {...baseProps} percent={72} engine="codex" />);
    expect(html).toBe('');
  });

  // 2026-09-02：同引擎派生已改为无损分支优先（摘要只是回退路径），按钮文案不再
  // 承诺「摘要」——改回「从摘要派生…」会让文案与实际分流重新对不上。
  it('Codex 显示「压缩上下文」与「派生新会话」', () => {
    const html = renderToStaticMarkup(
      <CompactBanner {...baseProps} engine="codex" onCompact={() => {}} />,
    );
    expect(html).toContain('上下文用了 85%');
    expect(html).toContain('压缩上下文');
    expect(html).toContain('派生新会话');
    expect(html).not.toContain('从摘要派生新会话');
  });

  it('Claude 无压缩按钮（-p 无契约），只留派生', () => {
    const html = renderToStaticMarkup(<CompactBanner {...baseProps} engine="claude" />);
    expect(html).toContain('派生新会话');
    expect(html).not.toContain('压缩上下文');
  });

  it('working 时两个动作按钮禁用', () => {
    const html = renderToStaticMarkup(
      <CompactBanner {...baseProps} engine="codex" working onCompact={() => {}} />,
    );
    expect(html).toContain('disabled=""');
  });
});
