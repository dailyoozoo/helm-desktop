import { describe, expect, it } from 'vitest';
import type { TurnStage } from '@helm/protocol';
import type { SessionState } from '../engine/useSession';
import { activityPresentationParts, thinkingOpenAfterItemUpdate } from './activityViewModel';

/** 与 ActivityRow 的展示组合一致：label + 可选用时后缀 */
function presentationText(state: SessionState, now: number): string | null {
  const parts = activityPresentationParts(state, now);
  if (!parts) return null;
  return `${parts.label}${parts.elapsedText ?? ''}`;
}

function activityState(
  stage: TurnStage | null,
  activity: Partial<NonNullable<SessionState['turnActivity']>> = {},
): SessionState {
  return {
    handleId: 'handle-1',
    historyId: 'history-1',
    sessionId: 'cli-1',
    engine: 'claude-code',
    model: 'claude-sonnet-4.6',
    cwd: 'D:\\work\\demo',
    status: stage ? 'working' : 'idle',
    items: [],
    openAssistantId: null,
    openThinkingId: null,
    cost: { inputTokens: 0, outputTokens: 0, costUsd: 0 },
    startedAt: 1,
    turnActivity: stage ? { stage, since: 1_000, ...activity } : null,
    turnStartedAt: stage ? 1_000 : null,
    disabledMcp: [],
  };
}

describe('activityPresentationParts', () => {
  it.each([
    ['preparing', '正在准备本轮…'],
    ['preparing_runtime', '正在准备 Runtime…'],
    ['restoring_session', '正在恢复会话…'],
    ['waiting_model', '已连接引擎，等待模型响应…'],
    ['reasoning', '正在分析…'],
    ['responding', '正在生成回复…'],
    ['finalizing', '正在整理结果…'],
    ['waiting_approval', '等待审批…'],
    ['stalled', '长时间没有新活动，可继续等待或停止'],
  ] satisfies Array<[TurnStage, string]>)('maps %s to truthful Chinese text', (stage, expected) => {
    expect(presentationText(activityState(stage), 1_000)).toBe(expected);
  });

  it('uses the selected engine name while starting it', () => {
    expect(presentationText(activityState('starting_engine'), 1_000)).toBe('正在启动 Claude Code…');
    expect(presentationText({ ...activityState('starting_engine'), engine: 'codex' }, 1_000)).toBe(
      '正在启动 Codex…',
    );
  });

  it.each([
    ['Read', 'src/App.tsx', '正在读取 src/App.tsx…'],
    ['Edit', 'src/App.tsx', '正在编辑 src/App.tsx…'],
    ['Bash', undefined, '正在运行命令…'],
    ['Grep', 'turnActivity', '正在搜索 turnActivity…'],
    ['Glob', 'src/**/*.tsx', '正在查找 src/**/*.tsx…'],
    ['Write', 'notes.md', '正在写入 notes.md…'],
    ['CustomTool', undefined, '正在运行 CustomTool…'],
  ])('presents tool %s truthfully', (toolName, target, expected) => {
    expect(
      presentationText(
        activityState('using_tool', { toolName, ...(target ? { target } : {}) }),
        1_000,
      ),
    ).toBe(expected);
  });

  it.each([
    'curl https://example.com -H "Authorization: Bearer secret-token"',
    'deploy --password super-secret',
  ])('never renders a Bash command supplied as an activity target: %s', (target) => {
    const text = presentationText(activityState('using_tool', { toolName: 'Bash', target }), 1_000);
    expect(text).toBe('正在运行命令…');
    expect(text).not.toContain(target);
  });

  it('shows retry attempt when the engine reports one', () => {
    expect(presentationText(activityState('retrying', { retryAttempt: 3 }), 1_000)).toBe(
      '连接异常，正在重试（3）…',
    );
  });

  it('adds elapsed seconds only after eight seconds and never invents percentages', () => {
    const state = activityState('waiting_model');
    expect(presentationText(state, 8_999)).toBe('已连接引擎，等待模型响应…');
    const elapsed = presentationText(state, 9_000);
    expect(elapsed).toContain('8 秒');
    expect(elapsed).not.toContain('%');
  });

  it('returns one activity presentation during thinking/tool work and none without activity', () => {
    const thinking = activityState('reasoning');
    thinking.openThinkingId = 'thinking-1';
    thinking.items = [{ kind: 'thinking', id: 'thinking-1', text: '真实分析', done: false }];
    expect(presentationText(thinking, 1_000)).toBe('正在分析…');

    const tool = activityState('using_tool', { toolName: 'Read', target: 'README.md' });
    tool.items = [{ kind: 'tool', id: 'tool-1', name: 'Read', input: {}, status: 'pending' }];
    expect(presentationText(tool, 1_000)).toBe('正在读取 README.md…');
    expect(presentationText(activityState(null), 1_000)).toBeNull();
  });
});

describe('thinkingOpenAfterItemUpdate', () => {
  it('collapses exactly when thinking transitions from active to done', () => {
    expect(thinkingOpenAfterItemUpdate(true, false, true)).toBe(false);
  });

  it('respects manual toggles after the completion transition', () => {
    expect(thinkingOpenAfterItemUpdate(true, true, true)).toBe(true);
    expect(thinkingOpenAfterItemUpdate(false, true, true)).toBe(false);
  });

  it('does not force an active thinking body open on every render', () => {
    expect(thinkingOpenAfterItemUpdate(false, false, false)).toBe(false);
  });
});
