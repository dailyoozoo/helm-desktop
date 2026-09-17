import { describe, expect, it } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import type { SessionState } from '../engine/useSession';
import type { SessionTurn } from '../sessions/api';
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
  return renderToStaticMarkup(<Thread state={state} onApprove={() => {}} />);
}

function workingRows(markup: string): string[] {
  return [...markup.matchAll(/class="([^"]+)"/g)]
    .map((match) => match[1])
    .filter((className) => className.split(/\s+/).includes('working'));
}

describe('Thread activity rendering', () => {
  it('renders fatal errors without leaving live thinking, tools or approval controls', () => {
    const initial = activeState('thinking');
    initial.items = [
      { kind: 'user', id: 'user', text: 'request', turnId: 'turn' },
      { kind: 'thinking', id: 'thinking-1', text: 'public thought', done: false, turnId: 'turn' },
      { kind: 'tool', id: 'tool', name: 'Bash', input: {}, status: 'pending', turnId: 'turn' },
      {
        kind: 'approval',
        id: 'approval',
        action: 'Bash',
        detail: 'command',
        status: 'pending',
        availableDecisions: ['allow', 'deny'],
        turnId: 'turn',
      },
    ];
    const failed = reduceSessionEvent(
      initial,
      { type: 'error', message: 'runtime failed', recoverable: false },
      'turn',
    );
    const markup = renderThread(failed);
    expect(markup).not.toContain('think is-live');
    expect(markup).not.toContain('等待审批…');
    expect(markup).not.toContain('正在思考…');
    expect(markup).not.toContain('>允许一次<');
    expect(markup).toContain('runtime failed');
  });

  it('shows an explicit truncated-plan notice without inserting artificial steps', () => {
    const state = activeState('thinking');
    state.status = 'idle';
    state.turnActivity = null;
    state.openThinkingId = null;
    state.items = [
      {
        kind: 'plan',
        id: 'plan',
        steps: [{ text: '真实步骤', status: 'active' }],
        truncated: true,
        turnStatus: 'failed',
      },
    ];
    const markup = renderThread(state);
    expect(markup).toContain('计划过长，历史仅保留部分步骤。');
    expect(markup.match(/<li[ >]/g)).toHaveLength(1);
  });

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

function renderCompleted(state: SessionState, turns?: SessionTurn[]): string {
  return renderToStaticMarkup(
    <Thread state={state} onApprove={() => {}} onOpenPane={() => {}} turns={turns} />,
  );
}

describe('Thread ledger lookup', () => {
  it('keeps ledger indexing and rail projection linear instead of searching per visible turn', () => {
    let ledgerReads = 0;
    const turns: SessionTurn[] = Array.from({ length: 2_000 }, (_, index) => ({
      get id() {
        ledgerReads += 1;
        return `turn-${index}`;
      },
      epoch: index + 1,
      mode: 'build',
      permissionProfile: 'standard',
      status: 'succeeded',
      startedAt: index * 1_000,
      endedAt: index * 1_000 + 500,
      routedModelId: `model-${index % 2}`,
    }));
    const state = activeState('tool');
    state.status = 'idle';
    state.turnActivity = null;
    state.items = Array.from({ length: 20 }, (_, visibleIndex) => {
      const index = 1_980 + visibleIndex;
      return [
        { kind: 'user' as const, id: `user-${index}`, text: 'request', turnId: `turn-${index}` },
        {
          kind: 'assistant' as const,
          id: `answer-${index}`,
          text: 'answer',
          turnId: `turn-${index}`,
        },
      ];
    }).flat();
    const markup = renderToStaticMarkup(
      <Thread state={state} turns={turns} onApprove={() => {}} />,
    );
    expect(ledgerReads).toBe(turns.length * 2);
    expect(markup).toContain('模型切换');
    expect(markup).toContain('model-0');
    expect(markup).toContain('model-1');
  });

  it('compares against the last actually reported model across missing model records', () => {
    const turns: SessionTurn[] = ['model-a', undefined, 'model-b'].map((model, index) => ({
      id: `turn-${index}`,
      epoch: index + 1,
      mode: 'build',
      permissionProfile: 'standard',
      status: 'succeeded',
      startedAt: 1_000 + index,
      endedAt: 2_000 + index,
      routedModelId: model,
    }));
    const state = activeState('tool');
    state.status = 'idle';
    state.turnActivity = null;
    state.items = [
      { kind: 'user', id: 'user', text: 'request', turnId: 'turn-2' },
      { kind: 'assistant', id: 'answer', text: 'answer', turnId: 'turn-2' },
    ];
    const markup = renderToStaticMarkup(
      <Thread state={state} turns={turns} onApprove={() => {}} />,
    );
    expect(markup).toContain('model-a → model-b');
  });
});

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

  it('读取失败的工具不产生交付物入口（变更-37）', () => {
    // 回归：Read 二进制 .xls 报错，路径却进了 touched → 「查看全部文件 1」。
    const markup = renderCompleted(
      completedTurn([
        {
          kind: 'tool',
          id: 'tool-1',
          name: 'Read',
          input: { file_path: 'D:/work/demo/配置.xls' },
          status: 'error',
          turnId: 'turn-1',
        },
      ]),
    );
    expect(markup).not.toContain('查看全部文件');
    expect(markup).not.toContain('deliverables');
  });

  it('只读成功的轮次不再显示交付物入口（变更-37：交付物只认产出）', () => {
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
    expect(markup).not.toContain('deliverables');
    expect(markup).not.toContain('查看全部文件');
  });

  it('真正写入文件的轮次显示交付物入口', () => {
    const markup = renderCompleted(
      completedTurn([
        {
          kind: 'tool',
          id: 'tool-1',
          name: 'Write',
          input: { file_path: 'D:/work/demo/out.md' },
          diff: { path: 'D:/work/demo/out.md', hunks: [] },
          status: 'success',
          turnId: 'turn-1',
        },
      ]),
    );
    expect(markup).toContain('deliverables');
    expect(markup).toContain('查看修改记录');
  });
});

/**
 * 变更-37/38：失败工具曾从过程区抽到 children 底部（01e1a55），把
 * 「正文→工具→正文→工具」的真实时序切成「思考全在上、工具全沉底」。
 * 现在失败卡一律就地留在过程区、按真实时序穿插；轮次成败只影响过程区
 * 默认展开态（失败/中断默认展开，成功折叠），不再用 data-sticky 常驻。
 */
describe('Thread 过程区时序 · 失败工具就地渲染', () => {
  const interleaved = (lastStatus?: 'succeeded' | 'failed') => {
    const state = activeState('tool');
    state.status = 'idle';
    state.openAssistantId = null;
    state.openThinkingId = null;
    state.turnActivity = null;
    state.items = [
      { kind: 'user', id: 'user-1', text: '分析表格', mode: 'build', turnId: 'turn-1' },
      { kind: 'assistant', id: 'a-1', text: '我来读取文件。', turnId: 'turn-1' },
      {
        kind: 'tool',
        id: 'tool-1',
        name: 'Read',
        input: { file_path: 'D:/work/demo/配置.xls' },
        status: 'error',
        outcome: 'tool_failed',
        turnId: 'turn-1',
      },
      { kind: 'assistant', id: 'a-2', text: '换个方式试试。', turnId: 'turn-1' },
      {
        kind: 'tool',
        id: 'tool-2',
        name: 'Bash',
        input: { command: 'python --version' },
        status: 'error',
        outcome: 'tool_failed',
        turnId: 'turn-1',
      },
      {
        kind: 'assistant',
        id: 'a-3',
        text: '遇到环境限制。',
        turnId: 'turn-1',
        ...(lastStatus ? { turnStatus: lastStatus } : {}),
      },
    ];
    return state;
  };
  const ledgerTurn = (status: SessionTurn['status']): SessionTurn[] => [
    {
      id: 'turn-1',
      epoch: 1,
      mode: 'build',
      permissionProfile: 'standard',
      status,
      startedAt: 1,
      endedAt: 2,
    },
  ];

  it('失败卡按真实时序穿插在正文之间，不整批沉到底部', () => {
    const markup = renderCompleted(interleaved());
    const firstText = markup.indexOf('我来读取文件');
    const firstFail = markup.indexOf('工具失败');
    const secondText = markup.indexOf('换个方式试试');
    const lastText = markup.indexOf('遇到环境限制');

    expect(firstText).toBeGreaterThan(-1);
    expect(firstFail).toBeGreaterThan(firstText);
    expect(secondText).toBeGreaterThan(firstFail);
    expect(lastText).toBeGreaterThan(secondText);
  });

  it('无论轮次成败，失败卡都不打 data-sticky（随过程区折叠/展开）', () => {
    for (const turns of [
      undefined,
      ledgerTurn('succeeded'),
      ledgerTurn('failed'),
      ledgerTurn('interrupted'),
    ]) {
      const markup = renderCompleted(interleaved('succeeded'), turns);
      expect(markup.match(/data-sticky="1"/g)).toBeNull();
    }
  });

  it('失败卡仍渲染在过程体内（不是轮次 children 尾部）', () => {
    const markup = renderCompleted(interleaved('failed'), ledgerTurn('failed'));
    const bodyStart = markup.indexOf('turn-process__body');
    const firstFail = markup.indexOf('工具失败');
    expect(bodyStart).toBeGreaterThan(-1);
    expect(firstFail).toBeGreaterThan(bodyStart);
  });

  it('失败轮次默认展开过程区，失败交代由「执行失败」胶囊 + 原因行承担', () => {
    const markup = renderCompleted(interleaved('failed'), ledgerTurn('failed'));
    expect(markup).toContain('执行失败');
    expect(markup).not.toContain('is-collapsed');
  });

  it('成功轮次默认折叠过程区', () => {
    const markup = renderCompleted(interleaved('succeeded'), ledgerTurn('succeeded'));
    expect(markup).toContain('is-collapsed');
  });
});
