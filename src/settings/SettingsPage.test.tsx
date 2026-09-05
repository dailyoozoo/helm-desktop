import { beforeAll, describe, expect, it, vi } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import { DEFAULT_SETTINGS } from './types';
import { SettingsPage } from './SettingsPage';

describe('SettingsPage 二级页面导航', () => {
  beforeAll(() => {
    vi.stubGlobal('sessionStorage', {
      getItem: () => null,
      removeItem: () => undefined,
    });
  });

  it('只渲染设置分类导航，不再内嵌返回入口', () => {
    const markup = renderToStaticMarkup(<SettingsPage initialSettings={DEFAULT_SETTINGS} />);

    expect(markup).toContain('aria-label="设置导航"');
    // 返回入口已上移到标题栏（Titlebar onBack），设置页本身不再渲染返回
    expect(markup).not.toContain('class="cm-settings-back"');
    expect(markup).not.toContain('class="rail"');
  });

  it('S8：一级导航对齐原型的五个 Tab，不再出现旧的引擎/权限/MCP/外观入口', () => {
    const markup = renderToStaticMarkup(<SettingsPage initialSettings={DEFAULT_SETTINGS} />);

    for (const label of ['通用', '全部任务', '主题', '快捷键', '关于']) {
      expect(markup).toContain(label);
    }
    // 旧的一级 Tab 名称不应再作为导航项出现（引擎/权限/MCP 能力移入通用分区与抽屉）
    expect(markup).not.toContain('> 引擎</button>');
    expect(markup).not.toContain('MCP 服务器</button>');
    expect(markup).not.toContain('> 外观</button>');
  });

  it('S8：默认通用 Tab 包含授权入口与新任务分区（真实设置渲染）', () => {
    const markup = renderToStaticMarkup(<SettingsPage initialSettings={DEFAULT_SETTINGS} />);

    expect(markup).toContain('新任务');
    expect(markup).toContain('已保存授权');
    expect(markup).toContain('对话体验');
  });
});
