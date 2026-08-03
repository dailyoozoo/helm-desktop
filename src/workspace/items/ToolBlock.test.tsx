import { describe, expect, it } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import type { ThreadItem } from '../../engine/useSession';
import { ToolBlock } from './ToolBlock';

type ToolItem = Extract<ThreadItem, { kind: 'tool' }>;

function render(item: ToolItem) {
  return renderToStaticMarkup(<ToolBlock item={item} />);
}

describe('ToolBlock', () => {
  it('Diff 交付物成功后保留摘要但默认折叠详情', () => {
    const markup = render({
      kind: 'tool',
      id: 'edit-1',
      name: 'Edit',
      input: { file_path: 'src/app.ts' },
      status: 'success',
      diff: { path: 'src/app.ts', hunks: [] },
    });
    expect(markup).toContain('aria-expanded="false"');
    expect(markup).toContain('src/app.ts');
  });

  it('终端交付物成功后保留命令摘要但默认折叠输出', () => {
    const markup = render({
      kind: 'tool',
      id: 'bash-1',
      name: 'Bash',
      input: { command: 'npm test' },
      status: 'success',
      output: 'PASS',
    });
    expect(markup).toContain('aria-expanded="false"');
    expect(markup).toContain('aria-label="复制命令"');
    expect(markup).toContain('npm test');
  });

  it('兼容终端别名和数组命令，并保持复制入口', () => {
    const markup = render({
      kind: 'tool',
      id: 'shell-1',
      name: 'shell',
      input: { command: ['npm', 'run', 'check'] },
      status: 'success',
    });
    expect(markup).toContain('aria-expanded="false"');
    expect(markup).toContain('aria-label="复制命令"');
    expect(markup).toContain('npm run check');
  });

  it('失败工具折叠详情时仍展示首行错误摘要', () => {
    const markup = render({
      kind: 'tool',
      id: 'web-1',
      name: 'WebSearch',
      input: { query: '上海天气' },
      status: 'error',
      output: '[runtime_web_search_unavailable] 当前服务商不支持网络搜索\n完整错误详情',
    });
    expect(markup).toContain('aria-expanded="false"');
    expect(markup).toContain('[runtime_web_search_unavailable] 当前服务商不支持网络搜索');
  });

  it('自动审查拒绝显示未执行而不是工具失败', () => {
    const markup = render({
      kind: 'tool',
      id: 'auto-1',
      name: 'Bash',
      input: { command: 'which wm' },
      status: 'error',
      outcome: 'auto_review_unavailable',
      started: false,
      output: '自动审查暂时不可用，工具尚未执行。Helm 正在切换兼容执行方式。',
    });
    expect(markup).toContain('未执行');
    expect(markup).toContain('pill--warn');
    expect(markup).not.toContain('pill--danger');
  });
});
