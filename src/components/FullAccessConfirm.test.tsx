import { describe, expect, it } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import { FullAccessConfirm, FULL_ACCESS_CONFIRM_COPY } from './FullAccessConfirm';

describe('FullAccessConfirm', () => {
  it('渲染原型 wsconfirm 文案与按钮', () => {
    const markup = renderToStaticMarkup(
      <FullAccessConfirm titleId="testFullAccessTitle" onCancel={() => {}} onConfirm={() => {}} />,
    );
    expect(markup).toContain('wsconfirm');
    expect(markup).toContain('开启「全部放开」？');
    expect(markup).toContain(FULL_ACCESS_CONFIRM_COPY);
    expect(markup).toContain('取消');
    expect(markup).toContain('开启全部放开');
    expect(markup).toContain('id="testFullAccessTitle"');
  });
});
