import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import { AssistantMessage } from './AssistantMessage';

describe('AssistantMessage', () => {
  it('uses a lightweight plain-text renderer while streaming', () => {
    const html = renderToStaticMarkup(<AssistantMessage text="# 标题" streaming />);

    expect(html).toContain('ws-stream-text');
    expect(html).toContain('ws-caret');
    expect(html).not.toContain('ws-msg-copy');
    expect(html).not.toContain('<h1>标题</h1>');
  });

  it('renders full Markdown after streaming completes', () => {
    const html = renderToStaticMarkup(<AssistantMessage text="# 标题" />);

    expect(html).toContain('<h1>标题</h1>');
  });

  it('renders fenced code after streaming completes', () => {
    const html = renderToStaticMarkup(<AssistantMessage text={'```ts\nconst value = 1;\n```'} />);

    expect(html).toContain('md-code');
    expect(html).toContain('language-ts');
    expect(html).toContain('value =');
  });
});
