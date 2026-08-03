import { describe, expect, it } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import type { ThreadItem } from '../../engine/useSession';
import { ToolGroup } from './ToolGroup';

type ToolItem = Extract<ThreadItem, { kind: 'tool' }>;

function tool(id: string, status: ToolItem['status'], output = ''): ToolItem {
  return {
    kind: 'tool',
    id,
    name: 'Read',
    input: { path: `${id}.md` },
    output,
    status,
    turnId: 'turn-1',
  };
}

describe('ToolGroup', () => {
  it('工具组进入终态后无论成功失败都默认折叠', () => {
    const success = renderToStaticMarkup(
      <ToolGroup items={[tool('a', 'success'), tool('b', 'success')]} />,
    );
    const failed = renderToStaticMarkup(
      <ToolGroup items={[tool('a', 'success'), tool('b', 'error')]} />,
    );

    expect(success).toContain('aria-expanded="false"');
    expect(success).not.toContain('tgrp__body');
    expect(failed).toContain('aria-expanded="false"');
    expect(failed).not.toContain('tgrp__body');
  });

  it('定位组内工具时展开组和对应输出', () => {
    const markup = renderToStaticMarkup(
      <ToolGroup
        items={[tool('a', 'success'), tool('b', 'success', '两行\n输出')]}
        locateTarget={{ id: 'b', request: 1 }}
      />,
    );

    expect(markup).toContain('aria-expanded="true"');
    expect(markup).toContain('data-thread-item-id="b"');
    expect(markup).toContain('两行\n输出');
  });
});
