import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import { Titlebar } from './Titlebar';

describe('Titlebar', () => {
  it('renders the Windows caption controls with prototype icon registry', () => {
    const html = renderToStaticMarkup(<Titlebar title="工作区" />);

    expect(html).toContain('aria-label="最小化"');
    expect(html).toContain('aria-label="最大化"');
    expect(html).toContain('aria-label="关闭"');
    // 图标真值：prototype/assets/app.js P 表（icons.tsx 渲染，无 lucide 类名）。
    // 最小化 = M5 12h14；最大化 = 四角括号（maximize）；关闭 = X 双对角线。
    expect(html).toContain('d="M5 12h14"');
    expect(html).toContain('d="M9 4H5.5a1.5 1.5 0 0 0-1.5 1.5V9"');
    expect(html).toContain('d="M6 6 18 18M18 6 6 18"');
  });

  // 2026-08-27 对齐原型：工作区页任务标题入标题栏（原型 workspace-titlebar__task），
  // 右栏开关（原型 #ctxToggle）进标题栏 actions，位于搜索入口之前。
  it('renders the workspace task title and context toggle instead of git info', () => {
    const html = renderToStaticMarkup(
      <Titlebar
        title="工作区"
        taskTitle="修复鉴权令牌刷新"
        onToggleCtx={() => undefined}
        ctxExpanded={false}
        searchMode="icon"
      />,
    );

    expect(html).toContain('修复鉴权令牌刷新');
    expect(html).toContain('titlebar__task');
    expect(html).toContain('aria-label="显示或隐藏右侧工作区"');
    expect(html).toContain('aria-expanded="false"');
    expect(html).not.toContain('titlebar__git');
  });

  it('keeps the git info center for legacy box mode without a task title', () => {
    const html = renderToStaticMarkup(
      <Titlebar title="工作区" projectName="helm" branchName="main" searchMode="box" />,
    );

    expect(html).toContain('titlebar__git');
    expect(html).toContain('helm');
    expect(html).not.toContain('titlebar__task');
  });
});
