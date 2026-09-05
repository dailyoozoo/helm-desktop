import { describe, expect, it } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import { FailureCard } from './FailureCard';

describe('FailureCard', () => {
  it('轮次运行中（working）默认展开：渲染错误分类标签、报错输出与自愈说明', () => {
    const markup = renderToStaticMarkup(
      <FailureCard
        item={{
          name: 'Bash',
          input: {},
          output: 'psql: error: connection refused\nat 127.0.0.1:5432',
        }}
        title="数据库连接被拒绝"
        onRetry={() => undefined}
        working
      />,
    );
    expect(markup).toContain('数据库连接被拒绝');
    expect(markup).toContain('网络失败');
    expect(markup).toContain('connection refused');
    expect(markup).toContain('重试可能自愈');
    expect(markup).toContain('重试这一步');
    expect(markup).toContain('复制报错');
  });

  it('权限失败显示不可自愈说明；轮次运行中禁用重试', () => {
    const markup = renderToStaticMarkup(
      <FailureCard
        item={{ name: 'Read', input: {}, outcome: 'runtime_denied' }}
        title="调用被拒绝"
        onRetry={() => undefined}
        working
      />,
    );
    expect(markup).toContain('权限失败');
    expect(markup).toContain('重试无效');
    expect(markup).toContain('disabled');
  });

  it('轮次结束后默认收起为轻量单行：标题与分类药丸常驻，输出与操作收进详情', () => {
    const markup = renderToStaticMarkup(
      <FailureCard
        item={{
          name: 'Bash',
          input: {},
          output: 'psql: error: connection refused',
        }}
        title="数据库连接被拒绝"
        onRetry={() => undefined}
      />,
    );
    expect(markup).toContain('aria-expanded="false"');
    expect(markup).toContain('数据库连接被拒绝');
    expect(markup).toContain('网络失败');
    expect(markup).not.toContain('connection refused');
    expect(markup).not.toContain('重试这一步');
    expect(markup).not.toContain('复制报错');
  });

  it('工具自身报错（denial_source=tool）不再误标为权限失败', () => {
    const markup = renderToStaticMarkup(
      <FailureCard
        item={{
          name: 'tavily_search',
          input: {},
          output: 'Country parameter is not supported for fast search_depth',
          outcome: 'tool_failed',
          denialSource: 'tool',
        }}
        title="tavily_search"
        working
      />,
    );
    expect(markup).not.toContain('权限失败');
    expect(markup).toContain('工具失败');
  });

  it('显示已重试次数', () => {
    const markup = renderToStaticMarkup(
      <FailureCard
        item={{ name: 'Bash', input: {}, output: 'x' }}
        title="重试"
        retryCount={2}
        onRetry={() => undefined}
        working
      />,
    );
    expect(markup).toContain('已重试 2 次');
  });

  it('未提供重试回调时不渲染重试按钮', () => {
    const markup = renderToStaticMarkup(
      <FailureCard item={{ name: 'Bash', input: {}, output: 'x' }} title="只读" working />,
    );
    expect(markup).not.toContain('重试这一步');
    expect(markup).toContain('复制报错');
  });
});
