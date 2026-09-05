import { describe, expect, it } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import { SubagentCard } from './SubagentCard';
import type { SubagentEntry } from './taskViewModel';

const entry = (id: string, partial: Partial<SubagentEntry> = {}): SubagentEntry => ({
  id,
  name: 'api-layer',
  task: '改写 fetch 调用点',
  dur: '2m 10s',
  state: 'ok',
  status: 'success',
  ...partial,
});

describe('SubagentCard', () => {
  it('渲染头部汇总与每行名称/任务/耗时/状态', () => {
    const markup = renderToStaticMarkup(
      <SubagentCard
        items={[
          entry('a', { state: 'run', status: 'pending', dur: '' }),
          entry('b', { name: 'ui-layer' }),
        ]}
      />,
    );
    expect(markup).toContain('并行子代理');
    expect(markup).toContain('1 个运行中');
    expect(markup).toContain('api-layer');
    expect(markup).toContain('改写 fetch 调用点');
    expect(markup).toContain('ui-layer');
    expect(markup).toContain('运行中');
    expect(markup).toContain('完成');
  });

  it('有失败且无运行中时汇总显示失败数；有产出时渲染可展开块', () => {
    const markup = renderToStaticMarkup(
      <SubagentCard
        items={[entry('a', { state: 'err', status: 'error', output: '拆解失败：包名冲突' })]}
      />,
    );
    expect(markup).toContain('1 个失败');
    expect(markup).not.toContain('个运行中');
    expect(markup).toContain('拆解失败：包名冲突');
    expect(markup).toContain('class="sarow__out"');
  });

  it('带 onOpenPane 时渲染查看全部按钮', () => {
    const markup = renderToStaticMarkup(
      <SubagentCard items={[entry('a')]} onOpenPane={() => undefined} />,
    );
    expect(markup).toContain('在交付物区查看全部任务');
  });
});
