import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import { AssistantMessage } from './AssistantMessage';

describe('AssistantMessage', () => {
  it('uses a lightweight plain-text renderer while streaming', () => {
    const html = renderToStaticMarkup(<AssistantMessage text="# 标题" streaming />);

    expect(html).toContain('ws-stream-text');
    expect(html).toContain('ws-caret');
    expect(html).not.toContain('ai-actions');
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

  it('renders the answer action row (copy/like/dislike/fork) after completion', () => {
    const html = renderToStaticMarkup(
      <AssistantMessage text="完成" showActions onFork={() => undefined} />,
    );

    expect(html).toContain('ai-actions');
    expect(html).toContain('复制回答');
    expect(html).toContain('赞');
    expect(html).toContain('踩');
    expect(html).toContain('从此回答派生新任务');
  });

  it('hides the fork button when no fork handler is provided', () => {
    const html = renderToStaticMarkup(<AssistantMessage text="完成" showActions />);

    expect(html).toContain('ai-actions');
    expect(html).not.toContain('从此回答派生新任务');
  });

  it('hides the action row on intermediate (non-final) assistant steps', () => {
    const html = renderToStaticMarkup(<AssistantMessage text="思考中…" />);

    expect(html).not.toContain('ai-actions');
    expect(html).not.toContain('复制回答');
  });
});
