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
      turnId="turn-1"
      entries={processEntries}
      completed={completed}
      waitingApproval={false}
      locateTarget={locateTarget}
      summary={{ turnNumber: 1, toolCount: 2 }}
      process={<div>过程</div>}
    >
      <div data-thread-item-id="b">内容</div>
    </TurnProcess>,
  );
}

describe('TurnProcess', () => {
  it('渲染形态 B（对齐 WorkBuddy 截图）：完成后整轮默认折叠为单行摘要，恢复型失败保留摘要', () => {
    // WorkBuddy 折叠态 = 摘要行（›）+ 最终答案；展开后过程条目各自折叠成单行。
    expect(render('success')).toContain('aria-expanded="false"');
    expect(render('success')).toContain('is-collapsed');
    const recovered = render('error');
    expect(recovered).toContain('aria-expanded="false"');
    expect(recovered).toContain('1 次失败后恢复');
  });

  it('Turn 最终失败时保持展开', () => {
    const failed = renderToStaticMarkup(
      <TurnProcess
        id="turn-process-a"
        turnId="turn-1"
        entries={entries('error')}
        completed={false}
        terminalStatus="failed"
        waitingApproval={false}
        summary={{ turnNumber: 1, toolCount: 2 }}
        process={<div>过程</div>}
      >
        <div data-thread-item-id="b">内容</div>
      </TurnProcess>,
    );
    expect(failed).toContain('aria-expanded="true"');
    expect(failed).toContain('执行失败');
    expect(failed).toContain('data-thread-item-id="b"');
    expect(failed).not.toContain('is-live');
  });

  it('定位到内部项目时展开容器', () => {
    const markup = render('success', { id: 'b', request: 1 });
    expect(markup).toContain('aria-expanded="true"');
    expect(markup).toContain('data-thread-item-id="b"');
  });

  it('批次①：一轮只渲染一个头像与 ai-head，过程体收在 .turn-process 内', () => {
    const markup = render('success');
    expect(markup).toContain('item ai-turn');
    expect(markup.match(/ava-bot/g)).toHaveLength(1);
    expect(markup).toContain('ai-head__name');
    expect(markup).toContain('turn-process__body');
  });
});
