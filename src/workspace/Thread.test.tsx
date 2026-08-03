import { describe, expect, it } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import type { SessionState } from '../engine/useSession';
import { reduceSessionEvent } from '../engine/useSession';
import { Thread } from './Thread';

function activeState(kind: 'thinking' | 'tool'): SessionState {
  const thinking = kind === 'thinking';
  return {
    handleId: 'handle-1',
    historyId: 'history-1',
    sessionId: 'cli-1',
    engine: 'claude-code',
    model: 'claude-sonnet-4.6',
    cwd: 'D:\\work\\demo',
    status: 'working',
    items: thinking
      ? [{ kind: 'thinking', id: 'thinking-1', text: '读取真实上下文', done: false }]
      : [{ kind: 'tool', id: 'tool-1', name: 'Read', input: {}, status: 'pending' }],
    openAssistantId: null,
    openThinkingId: thinking ? 'thinking-1' : null,
    cost: { inputTokens: 0, outputTokens: 0, costUsd: 0 },
    startedAt: 1,
    turnActivity: thinking
      ? { stage: 'reasoning', since: Date.now() }
      : { stage: 'using_tool', since: Date.now(), toolName: 'Read', target: 'README.md' },
    turnStartedAt: Date.now(),
    disabledMcp: [],
  };
}

function renderThread(state: SessionState): string {
  return renderToStaticMarkup(
    <Thread
      state={state}
      onApprove={() => {}}
      onRestoreCheckpoint={() => {}}
      onUndoRevert={() => {}}
    />,
  );
}

function workingRows(markup: string): string[] {
  return [...markup.matchAll(/class="([^"]+)"/g)]
    .map((match) => match[1])
    .filter((className) => className.split(/\s+/).includes('working'));
}

describe('Thread activity rendering', () => {
  it('hides the raw Codex tool-surface code behind the version-incompatible guidance', () => {
    const state = reduceSessionEvent(activeState('thinking'), {
      type: 'error',
      message: '[codex_probe_tool_surface_unrecognized] codex 0.144.1 probe rejected',
      recoverable: false,
      kind: 'version_incompatible',
    });
    const markup = renderThread(state);

    expect(markup).toContain('CLI 版本不兼容');
    expect(markup).toContain('Codex 可切到计划/询问模式');
    expect(markup).not.toContain('codex_probe_tool_surface_unrecognized');
  });

  it.each(['thinking', 'tool'] as const)(
    'does not duplicate ActivityRow when an entity already describes active %s',
    (kind) => {
      const markup = renderThread(activeState(kind));

      expect(workingRows(markup)).toHaveLength(0);
      expect(markup).not.toContain('Helm 正在思考');
    },
  );

  it('renders waiting approval without a simultaneous active-thinking label', () => {
    const waiting = reduceSessionEvent(activeState('thinking'), {
      type: 'approval_request',
      sessionId: 'cli-1',
      id: 'approval-1',
      action: 'Bash',
      detail: 'npm test',
      availableDecisions: ['allow', 'deny'],
    });
    const markup = renderThread(waiting);

    expect(markup).toContain('等待审批…');
    expect(markup).not.toContain('正在分析');
  });

  it('keeps elapsed time outside the polite live region', () => {
    const state = activeState('tool');
    state.items = [];
    state.turnActivity = {
      stage: 'using_tool',
      since: Date.now() - 9_000,
      toolName: 'Read',
      target: 'README.md',
    };
    const markup = renderThread(state);

    expect(markup).toMatch(
      /role="status"[^>]*aria-live="polite"[^>]*>正在读取 README\.md…<\/span>/,
    );
    expect(markup).toMatch(/aria-live="off"[^>]*>（已用时 9 秒）<\/span>/);
  });

  it('connects the thinking toggle to its body with expanded ARIA state', () => {
    const markup = renderThread(activeState('thinking'));
    const controlId = markup.match(/aria-controls="([^"]+)"/)?.[1];

    expect(markup).toContain('aria-expanded="true"');
    expect(controlId).toBeTruthy();
    expect(markup).toContain(`id="${controlId}"`);
  });

  it('最终答复流式输出期间保持同 Turn 过程容器展开', () => {
    const state = activeState('thinking');
    state.items = [
      { kind: 'thinking', id: 'thinking-1', text: '分析', done: true, turnId: 'turn-1' },
      { kind: 'assistant', id: 'assistant-1', text: '正在输出', turnId: 'turn-1' },
    ];
    state.openAssistantId = 'assistant-1';
    state.openThinkingId = null;

    const markup = renderThread(state);
    expect(markup).toContain('turn-process__box');
    expect(markup).toContain('aria-expanded="true"');
  });

  it('阶段正文已完成但 Turn 仍运行时保持过程容器展开', () => {
    const state = activeState('thinking');
    state.items = [
      { kind: 'user', id: 'user-1', text: '继续', mode: 'build' },
      { kind: 'thinking', id: 'thinking-1', text: '分析', done: true, turnId: 'turn-1' },
      { kind: 'assistant', id: 'assistant-1', text: '阶段结论', turnId: 'turn-1' },
    ];
    state.openAssistantId = null;
    state.openThinkingId = null;

    const markup = renderThread(state);
    expect(markup).toContain('turn-process__box');
    expect(markup).toContain('aria-expanded="true"');
    expect(markup).not.toContain('>已完成</span>');
  });
});
