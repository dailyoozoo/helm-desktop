import { describe, expect, it } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import { contextRingState, ContextRing, type ContextRingDetail } from './ContextRing';

describe('contextRingState', () => {
  it('有 tokens/maxTokens 时给出百分比与 normal 级', () => {
    const state = contextRingState(62_000, 200_000);
    expect(state.percent).toBe(31);
    expect(state.ratio).toBeCloseTo(0.31, 1);
    expect(state.level).toBe('none');
  });

  it('80% 以上为 warn', () => {
    expect(contextRingState(170_000, 200_000).level).toBe('warn');
  });

  it('95% 以上为 danger', () => {
    expect(contextRingState(195_000, 200_000).level).toBe('danger');
  });

  it('缺 tokens 或窗口返回 none 且无 percent', () => {
    expect(contextRingState()).toEqual({ level: 'none' });
    expect(contextRingState(100)).toEqual({ level: 'none' });
  });
});

describe('ContextRing', () => {
  const detail: ContextRingDetail = {
    cost: {
      inputTokens: 0,
      outputTokens: 0,
      costUsd: 0,
      contextTokens: 62_000,
      contextWindow: 200_000,
    },
  };

  it('渲染百分比圆环与数据态', () => {
    const markup = renderToStaticMarkup(<ContextRing detail={detail} />);
    expect(markup).toContain('31%');
    expect(markup).toContain('ctxring__btn');
    expect(markup).toContain('最近一次调用的真实输入');
  });

  it('原型对齐：popover 只有「上下文占用 + 计费 token」两节，且 pill 在标题行内', () => {
    const markup = renderToStaticMarkup(<ContextRing detail={detail} defaultOpen />);
    // 第一节标题行内含 pill（原型 #ctxPct 在 csec__t 行右缘）
    const head = markup.slice(markup.indexOf('上下文占用'), markup.indexOf('计费 token'));
    expect(head).toContain('class="pill');
    // 计费节三行：未缓存输入 / 缓存写入 / 输出
    const billing = markup.slice(markup.indexOf('计费 token'));
    expect(billing).toContain('未缓存输入');
    expect(billing).toContain('缓存写入');
    expect(billing).toContain('输出');
    expect(billing).toContain('命中率');
    expect(billing).toContain('≈0.1× 计费');
    // 严格对齐当前原型骨架：不再有 占用归因 / 本次会话 / 会话上下文 / 历史附件 / MCP
    expect(markup).not.toContain('占用归因');
    expect(markup).not.toContain('本次会话');
    expect(markup).not.toContain('会话上下文');
    expect(markup).not.toContain('历史附件');
    expect(markup).not.toContain('MCP 服务器');
  });

  it('无数据时显示占位符', () => {
    const markup = renderToStaticMarkup(
      <ContextRing detail={{ cost: { inputTokens: 0, outputTokens: 0, costUsd: 0 } }} />,
    );
    expect(markup).toContain('—');
    expect(markup).toContain('尚无逐调用用量数据');
  });

  it('完全无 detail 仍渲染常驻占位圆环', () => {
    const markup = renderToStaticMarkup(<ContextRing />);
    expect(markup).toContain('ctxring__btn');
    expect(markup).toContain('—');
  });
});
