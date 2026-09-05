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
    turnCostUsd: 0,
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

  it('renders the live thinking block while reasoning is in progress', () => {
    const markup = renderThread(activeState('thinking'));

    expect(markup).toContain('think is-live');
    expect(markup).toContain('正在思考…');
  });

  it('最终答复流式输出期间保持同 Turn 过程容器展开', () => {
    const state = activeState('thinking');
    state.items = [
      { kind: 'user', id: 'user-1', text: '处理', mode: 'build', turnId: 'turn-1' },
      { kind: 'thinking', id: 'thinking-1', text: '分析', done: true, turnId: 'turn-1' },
      { kind: 'assistant', id: 'assistant-1', text: '正在输出', turnId: 'turn-1' },
    ];
    state.openAssistantId = 'assistant-1';
    state.openThinkingId = null;

    const markup = renderThread(state);
    expect(markup).toContain('turn-process');
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
    expect(markup).toContain('turn-process');
    expect(markup).toContain('aria-expanded="true"');
    expect(markup).not.toContain('>已完成</span>');
  });

  it('纯文本轮次结束后显示已完成，不留下空的过程容器', () => {
    const state = activeState('thinking');
    state.status = 'idle';
    state.openAssistantId = null;
    state.openThinkingId = null;
    state.turnActivity = null;
    state.items = [
      { kind: 'user', id: 'user-1', text: '请只回复两个字：OK', mode: 'build', turnId: 'turn-1' },
      { kind: 'assistant', id: 'assistant-1', text: 'OK', turnId: 'turn-1' },
    ];

    const markup = renderThread(state);
    expect(markup).toContain('>已完成</span>');
    expect(markup).not.toContain('>进行中</span>');
    expect(markup).not.toContain('is-live');
    expect(markup).not.toContain('turn-process__body');
  });
});

/**
 * 交付物行的「触碰文件」口径（2026-08-30 用户报告）：查天气这类轮次跑过一条 shell
 * 命令，却冒出「查看全部文件 1」。根因是统计用了 toolTarget——它为了给工具抬头兜底，
 * 会把 Bash 命令行、Grep pattern、URL 当成目标。这里正反两向锁死口径。
 */
function completedTurn(tools: SessionState['items']): SessionState {
  const state = activeState('tool');
  state.status = 'idle';
  state.openAssistantId = null;
  state.openThinkingId = null;
  state.turnActivity = null;
  state.items = [
    { kind: 'user', id: 'user-1', text: '上海天气怎么样', mode: 'build', turnId: 'turn-1' },
    ...tools,
    { kind: 'assistant', id: 'assistant-1', text: '这是回答。', turnId: 'turn-1' },
  ];
  return state;
}

function renderCompleted(state: SessionState): string {
  return renderToStaticMarkup(
    <Thread
      state={state}
      onApprove={() => {}}
      onRestoreCheckpoint={() => {}}
      onUndoRevert={() => {}}
      onOpenPane={() => {}}
    />,
  );
}

describe('Thread 交付物行 · 触碰文件口径', () => {
  it('只跑过 shell 命令的轮次不显示交付物入口', () => {
    const markup = renderCompleted(
      completedTurn([
        {
          kind: 'tool',
          id: 'tool-1',
          name: 'Bash',
          input: { command: 'pwsh.exe -Command \'echo "websearch probe"\'' },
          status: 'success',
          turnId: 'turn-1',
        },
      ]),
    );
    expect(markup).not.toContain('查看全部文件');
    expect(markup).not.toContain('查看修改记录');
    expect(markup).not.toContain('deliverables');
  });

  it('Grep 的搜索模式与抓取 URL 都不算触碰文件', () => {
    const markup = renderCompleted(
      completedTurn([
        {
          kind: 'tool',
          id: 'tool-1',
          name: 'Grep',
          input: { pattern: 'TODO|FIXME' },
          status: 'success',
          turnId: 'turn-1',
        },
        {
          kind: 'tool',
          id: 'tool-2',
          name: 'WebFetch',
          input: { url: 'https://example.com/a.txt' },
          status: 'success',
          turnId: 'turn-1',
        },
      ]),
    );
    expect(markup).not.toContain('查看全部文件');
    expect(markup).not.toContain('deliverables');
  });

  it('真正读过文件的轮次仍然显示「查看全部文件」', () => {
    const markup = renderCompleted(
      completedTurn([
        {
          kind: 'tool',
          id: 'tool-1',
          name: 'Read',
          input: { file_path: 'D:/work/demo/README.md' },
          status: 'success',
          turnId: 'turn-1',
        },
      ]),
    );
    expect(markup).toContain('deliverables');
    expect(markup).toContain('查看全部文件');
  });
});
