import { describe, expect, it } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import type { ThreadItem } from '../../engine/useSession';
import type { ThreadRenderEntry } from '../threadGroups';
import { TurnProcess } from './TurnProcess';

function entries(status: 'success' | 'error'): ThreadRenderEntry[] {
  const items: Extract<ThreadItem, { kind: 'tool' }>[] = [
    { kind: 'tool', id: 'a', name: 'Read', input: {}, status: 'success', turnId: 'turn-1' },
    { kind: 'tool', id: 'b', name: 'Read', input: {}, status, turnId: 'turn-1' },
  ];
  return [{ kind: 'tool-group', id: 'a', items }];
}

function render(
  status: 'success' | 'error',
  locateTarget?: { id: string; request: number },
  completed = true,
) {
  const processEntries = entries(status);
  return renderToStaticMarkup(
    <TurnProcess
      id="turn-process-a"
      entries={processEntries}
      completed={completed}
      waitingApproval={false}
      locateTarget={locateTarget}
    >
      <div data-thread-item-id="b">内容</div>
    </TurnProcess>,
  );
}

describe('TurnProcess', () => {
  it('成功结束后默认折叠，中途失败恢复后也折叠并保留摘要', () => {
    expect(render('success')).toContain('aria-expanded="false"');
    const recovered = render('error');
    expect(recovered).toContain('aria-expanded="false"');
    expect(recovered).toContain('1 次失败后恢复');
  });

  it('Turn 最终失败时保持展开', () => {
    const failed = render('error', undefined, false);
    expect(failed).toContain('aria-expanded="true"');
    expect(failed).toContain('执行失败');
    expect(failed).toContain('data-thread-item-id="b"');
  });

  it('定位到内部项目时展开容器', () => {
    const markup = render('success', { id: 'b', request: 1 });
    expect(markup).toContain('aria-expanded="true"');
    expect(markup).toContain('data-thread-item-id="b"');
  });
});
