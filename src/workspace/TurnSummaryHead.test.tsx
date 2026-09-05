import { describe, expect, it } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import { TurnSummaryHead } from './TurnSummaryHead';
import type { TurnSummaryMeta } from './turnSummary';

const full: TurnSummaryMeta = {
  turnNumber: 3,
  model: 'claude-sonnet-4.6',
  durationSec: 65,
  thinkingSec: 5,
  toolCount: 4,
  added: 12,
  removed: 3,
};

function render(summary: TurnSummaryMeta, open = true) {
  return renderToStaticMarkup(
    <TurnSummaryHead summary={summary} open={open} onToggle={() => {}} status="已完成" />,
  );
}

describe('TurnSummaryHead', () => {
  it('renders thinking seconds, tool count and duration（原型 turnLite 格式）', () => {
    const markup = render(full);
    expect(markup).toContain('思考了 5 秒');
    expect(markup).toContain('1分5秒');
    expect(markup).toContain('工具 4');
    expect(markup).toContain('已完成');
  });

  it('原型格式：胶囊正文不带轮次编号、模型与 ±diff（aria 保留轮次）', () => {
    const markup = render(full);
    expect(markup).toContain('aria-label="第 3 轮');
    expect(markup).not.toContain('>第 3 轮<');
    expect(markup).not.toContain('claude-sonnet-4.6');
    expect(markup).not.toContain('+12');
    expect(markup).not.toContain('−3');
  });

  it('omits missing fields instead of placeholders', () => {
    const markup = render({ turnNumber: 1, toolCount: 0 }, false);
    expect(markup).not.toContain('思考了');
    expect(markup).not.toContain('工具');
    expect(markup).not.toContain('秒');
  });

  it('reflects collapse state in aria-expanded and label', () => {
    const open = render(full, true);
    const closed = render(full, false);
    expect(open).toContain('aria-expanded="true"');
    expect(open).toContain('点击折叠整轮');
    expect(closed).toContain('aria-expanded="false"');
    expect(closed).toContain('点击展开整轮');
  });

  it('renders trailing failure-recovery note when provided', () => {
    const markup = renderToStaticMarkup(
      <TurnSummaryHead
        summary={full}
        open={false}
        onToggle={() => {}}
        status="已完成"
        trailing="1 次失败后恢复"
      />,
    );
    expect(markup).toContain('1 次失败后恢复');
  });

  it('批次①：胶囊形态（turn__lite）带折叠箭头，运行态带呼吸点', () => {
    const idle = render(full, false);
    expect(idle).toContain('turn__lite');
    expect(idle).toContain('chev');
    const live = renderToStaticMarkup(
      <TurnSummaryHead summary={{ turnNumber: 2 }} open onToggle={() => {}} status="进行中" live />,
    );
    expect(live).toContain('live-dot');
    expect(live).toContain('is-live');
  });
});
