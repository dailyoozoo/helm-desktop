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

  it('渲染形态 B：终端完成态默认折叠为轻量单行「运行命令」，不渲染深色终端卡', () => {
    const markup = render({
      kind: 'tool',
      id: 'bash-1',
      name: 'Bash',
      input: { command: 'npm test' },
      status: 'success',
      output: 'PASS',
    });
    // 轻量行：动作词 + 命令摘要 + 状态药丸；输出与深色终端卡都不渲染
    expect(markup).toContain('运行命令');
    expect(markup).toContain('npm test');
    expect(markup).toContain('exit 0');
    expect(markup).toContain('aria-expanded="false"');
    expect(markup).not.toContain('PASS');
    expect(markup).not.toContain('class="term ');
    expect(markup).not.toContain('aria-label="复制命令"');
  });

  it('终端卡运行中（pending）默认收起为轻量行并显示「运行中」药丸，点开才展开', () => {
    const markup = render({
      kind: 'tool',
      id: 'bash-2',
      name: 'Bash',
      input: { command: 'npm run build' },
      status: 'pending',
      output: 'compiling...',
    });
    // 默认收起：渲染轻量行（.tool.is-lite），不渲染完整 .term 卡
    expect(markup).toContain('tool is-lite');
    expect(markup).toContain('aria-expanded="false"');
    expect(markup).toContain('aria-label="展开命令与输出"');
    expect(markup).not.toContain('class="term ');
    expect(markup).not.toContain('aria-label="复制命令"');
    // 收起时提示运行中（含完整命令摘要与药丸）
    expect(markup).toContain('运行命令');
    expect(markup).toContain('npm run build');
    expect(markup).toContain('运行中');
  });

  it('兼容终端别名和数组命令（轻量行摘要显示完整命令）', () => {
    const markup = render({
      kind: 'tool',
      id: 'shell-1',
      name: 'shell',
      input: { command: ['npm', 'run', 'check'] },
      status: 'success',
    });
    expect(markup).toContain('运行命令');
    expect(markup).toContain('npm run check');
    expect(markup).not.toContain('class="term ');
  });

  it('非拒绝的工具错误提成顶层 .failc 失败卡（非 .tool 内嵌），标题与分类常驻、详情可展开', () => {
    const markup = render({
      kind: 'tool',
      id: 'web-1',
      name: 'WebSearch',
      input: { query: '上海天气' },
      status: 'error',
      output: '[runtime_web_search_unavailable] 当前服务商不支持网络搜索\n完整错误详情',
    });
    // 9/4 折叠化：失败卡默认收起为轻量单行（标题+分类药丸），详情点开展开
    expect(markup).toContain('aria-expanded="false"');
    expect(markup).toContain('failc');
    expect(markup).toContain('联网搜索');
    expect(markup).toContain('网络失败');
    expect(markup).not.toContain('runtime_web_search_unavailable');
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

  it('终端卡无「在交付物区查看」跳转按钮（原型无该入口）', () => {
    const markup = render({
      kind: 'tool',
      id: 'bash-1',
      name: 'Bash',
      input: { command: 'npm test' },
      status: 'success',
    });
    expect(markup).not.toContain('在交付物区查看全部命令输出');
  });
});
