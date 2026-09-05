import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import { UserMessage } from './UserMessage';

describe('UserMessage', () => {
  it('renders the plain bubble without any answer action row', () => {
    const html = renderToStaticMarkup(<UserMessage text="今天上海天气怎么样" />);

    expect(html).toContain('今天上海天气怎么样');
    expect(html).toContain('user-text');
    expect(html).not.toContain('ai-actions');
    expect(html).not.toContain('复制回答');
  });

  it('exposes the weakened copy action below the bubble (批次①裁决落地)', () => {
    const html = renderToStaticMarkup(<UserMessage text="原始消息" />);

    expect(html).toContain('user-meta');
    expect(html).toContain('aria-label="复制消息"');
    expect(html).toContain('title="复制消息"');
    expect(html).not.toContain('复制回答');
  });
});
