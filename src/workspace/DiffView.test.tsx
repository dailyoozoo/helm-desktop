import { describe, expect, it } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import type { ReviewChangeFile } from './changeReviewViewModel';
import { DiffView, type DiffExpandState } from './DiffView';

function fileFixture(): ReviewChangeFile {
  return {
    path: 'src/app.ts',
    status: 'm',
    added: 2,
    removed: 1,
    edits: 1,
    hunks: [
      {
        key: 'src/app.ts@0',
        oldStart: 5,
        newStart: 4,
        skip: 3,
        lines: [
          { kind: 'del', oldNo: 5, newNo: null, text: 'const old = 1' },
          { kind: 'add', oldNo: null, newNo: 4, text: 'const fresh = 1' },
          { kind: 'ctx', oldNo: 6, newNo: 5, text: 'const same = 1' },
        ],
      },
    ],
  };
}

const noop = () => undefined;

describe('DiffView', () => {
  it('统一视图渲染双行号与新增/删除行着色', () => {
    const markup = renderToStaticMarkup(
      <DiffView
        file={fileFixture()}
        mode="unified"
        expanded={{}}
        activeHunkKey={null}
        onToggleSkip={noop}
      />,
    );
    expect(markup).toContain('dvl del');
    expect(markup).toContain('dvl add');
    expect(markup).toContain('data-sig="−"');
    expect(markup).toContain('data-sig="+"');
    expect(markup).toContain('const fresh = 1');
    // 删除行不渲染新行号、新增行不渲染旧行号
    expect(markup).toContain('<span class="n">5</span><span class="n"></span>');
    expect(markup).toContain('<span class="n"></span><span class="n">4</span>');
  });

  it('并排视图渲染左右两列并用空行补齐缺侧', () => {
    const markup = renderToStaticMarkup(
      <DiffView
        file={fileFixture()}
        mode="split"
        expanded={{}}
        activeHunkKey={null}
        onToggleSkip={noop}
      />,
    );
    expect(markup).toContain('dvw is-split');
    expect(markup).toContain('dside');
    // 左列只含删除/上下文符号，右列只含新增/上下文符号
    const [, left, right] = markup.split('<div class="dside">');
    expect(left).toContain('data-sig="−"');
    expect(left).not.toContain('data-sig="+"');
    expect(right).toContain('data-sig="+"');
    expect(right).not.toContain('data-sig="−"');
    // 缺侧空行占位
    expect(markup).toContain('dvl pad');
  });

  it('有折叠行时渲染「折叠 N 行未变更」，展开态显示真实行号区间', () => {
    const collapsed = renderToStaticMarkup(
      <DiffView
        file={fileFixture()}
        mode="unified"
        expanded={{}}
        activeHunkKey={null}
        onToggleSkip={noop}
      />,
    );
    expect(collapsed).toContain('⋯ 折叠 3 行未变更');

    const expandedState: DiffExpandState = { 'src/app.ts@0': true };
    const expanded = renderToStaticMarkup(
      <DiffView
        file={fileFixture()}
        mode="unified"
        expanded={expandedState}
        activeHunkKey={null}
        onToggleSkip={noop}
      />,
    );
    expect(expanded).toContain('第 1–3 行未变更（内容未随变更记录）');
    expect(expanded).toContain('点击收起');
  });

  it('当前导航命中的 hunk 带 is-nav 高亮', () => {
    const markup = renderToStaticMarkup(
      <DiffView
        file={fileFixture()}
        mode="unified"
        expanded={{}}
        activeHunkKey="src/app.ts@0"
        onToggleSkip={noop}
      />,
    );
    expect(markup).toContain('dvw__hunk is-nav');
    expect(markup).toContain('data-hunk="src/app.ts@0"');
  });

  it('无折叠区间（skip=0）时不渲染折叠行', () => {
    const file = { ...fileFixture(), hunks: [{ ...fileFixture().hunks[0], skip: 0 }] };
    const markup = renderToStaticMarkup(
      <DiffView
        file={file}
        mode="unified"
        expanded={{}}
        activeHunkKey={null}
        onToggleSkip={noop}
      />,
    );
    expect(markup).not.toContain('dskip');
  });
});
