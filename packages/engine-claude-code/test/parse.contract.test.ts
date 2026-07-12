// 契约测试（防壳子的强制手段，不可跳过 / 不可 mock）。
//
// fixture 是用真实 `claude --output-format stream-json --include-partial-messages` 录下的真实
// JSONL（见 test/fixtures/*.jsonl）。测试断言：parseClaudeLine 在真实输出上产出的每一个事件
// 都符合 @helm/protocol 的 AgentEvent 协议，且一整段真实输出能被还原成合理的事件序列
// （session_started → 文本流 → 工具调用/结果 → 用量 → turn_complete）。
//
// 注意：本测试只读已录制的真实样本，不发起网络请求、不 mock 任何回复。要重新录制样本，
// 跑 npm run probe 那条真实链路即可。

import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import { isAgentEvent, type AgentEvent } from '@helm/protocol';
import { parseClaudeLine } from '../src/parse';

const here = dirname(fileURLToPath(import.meta.url));

function loadFixtureLines(name: string): string[] {
  const raw = readFileSync(join(here, 'fixtures', name), 'utf8');
  return raw.split('\n').filter((line) => line.trim().length > 0);
}

function parseAll(lines: string[]): AgentEvent[] {
  return lines.flatMap((line) => parseClaudeLine(line));
}

const FIXTURES = ['claude-hello.jsonl', 'claude-stream.jsonl'] as const;

describe.each(FIXTURES)('真实输出契约：%s', (fixture) => {
  const lines = loadFixtureLines(fixture);
  const events = parseAll(lines);

  it('fixture 非空且能解析出事件', () => {
    expect(lines.length).toBeGreaterThan(0);
    expect(events.length).toBeGreaterThan(0);
  });

  it('每个解析出的事件都符合 AgentEvent 协议', () => {
    for (const event of events) {
      expect(isAgentEvent(event), `非法事件: ${JSON.stringify(event)}`).toBe(true);
    }
  });

  it('带 sessionId 的事件其 sessionId 非空', () => {
    for (const event of events) {
      if (event.type !== 'error') {
        expect(event.sessionId.length, `空 sessionId: ${JSON.stringify(event)}`).toBeGreaterThan(0);
      }
    }
  });

  it('首事件是 session_started 且带 claude-code / 模型 / 工作目录', () => {
    const first = events[0];
    expect(first.type).toBe('session_started');
    if (first.type === 'session_started') {
      expect(first.engine).toBe('claude-code');
      expect(first.model.length).toBeGreaterThan(0);
      expect(first.cwd.length).toBeGreaterThan(0);
    }
  });

  it('末事件是 turn_complete', () => {
    const last = events[events.length - 1];
    expect(last.type).toBe('turn_complete');
    if (last.type === 'turn_complete') {
      expect(['end', 'interrupted', 'error']).toContain(last.stopReason);
    }
  });

  it('含 token_usage，且 outputTokens / costUsd 非负', () => {
    const usage = events.find((e) => e.type === 'token_usage');
    expect(usage, '缺少 token_usage 事件').toBeDefined();
    if (usage && usage.type === 'token_usage') {
      expect(usage.outputTokens).toBeGreaterThanOrEqual(0);
      expect(usage.costUsd).toBeGreaterThanOrEqual(0);
      expect(usage.contextWindow).toBeGreaterThan(0);
    }
  });

  it('含流式文本增量 message_delta（打字机效果的来源）', () => {
    expect(events.some((e) => e.type === 'message_delta')).toBe(true);
  });

  it('含定稿助手文本 message_complete', () => {
    expect(events.some((e) => e.type === 'message_complete' && e.role === 'assistant')).toBe(true);
  });
});

describe('真实输出契约：含工具调用的回合 (claude-stream.jsonl)', () => {
  const events = parseAll(loadFixtureLines('claude-stream.jsonl'));

  it('含 tool_call，且其后存在 id 匹配的 tool_result', () => {
    const call = events.find((e) => e.type === 'tool_call');
    expect(call, '缺少 tool_call 事件').toBeDefined();
    if (call && call.type === 'tool_call') {
      expect(call.name.length).toBeGreaterThan(0);
      const result = events.find((e) => e.type === 'tool_result' && e.id === call.id);
      expect(result, '缺少与 tool_call 对应的 tool_result').toBeDefined();
      if (result && result.type === 'tool_result') {
        expect(['success', 'error']).toContain(result.status);
        expect((result.output ?? '').length).toBeGreaterThan(0);
      }
    }
  });

  it('事件相对顺序合理：session_started → tool_call → tool_result → turn_complete', () => {
    const typeAt = (t: AgentEvent['type']): number => events.findIndex((e) => e.type === t);
    const started = typeAt('session_started');
    const call = typeAt('tool_call');
    const result = typeAt('tool_result');
    const done = typeAt('turn_complete');
    expect(started).toBe(0);
    expect(started).toBeLessThan(call);
    expect(call).toBeLessThan(result);
    expect(result).toBeLessThan(done);
  });
});

describe('Slice 2 协议：diff 与审批', () => {
  it('从 Write/Edit 的 tool_result diff 内容块解析 Diff', () => {
    const events = parseClaudeLine(
      JSON.stringify({
        type: 'user',
        session_id: 's1',
        message: {
          role: 'user',
          content: [
            {
              type: 'tool_result',
              tool_use_id: 'toolu_1',
              content: [
                { type: 'text', text: 'Updated file' },
                {
                  type: 'diff',
                  path: 'demo.txt',
                  old_string: 'one\ntwo\nthree\n',
                  new_string: 'one\nTWO\nthree\n',
                },
              ],
            },
          ],
        },
      }),
    );

    expect(events).toHaveLength(1);
    const event = events[0];
    expect(event.type).toBe('tool_result');
    if (event.type === 'tool_result') {
      expect(event.diff?.path).toBe('demo.txt');
      expect(event.diff?.hunks[0]?.lines.map((line) => line.kind)).toEqual(['del', 'add']);
    }
  });

  it('把 headless hook defer 的 result 映射为 approval_request，且不提前 turn_complete', () => {
    const events = parseClaudeLine(
      JSON.stringify({
        type: 'result',
        subtype: 'success',
        session_id: 's1',
        stop_reason: 'tool_deferred',
        terminal_reason: 'tool_deferred',
        total_cost_usd: 0.01,
        usage: { input_tokens: 10, output_tokens: 20 },
        deferred_tool_use: {
          id: 'toolu_approval',
          name: 'Write',
          input: { file_path: 'demo.txt', content: 'hello' },
        },
      }),
    );

    expect(events.map((event) => event.type)).toEqual(['token_usage', 'approval_request']);
    const approval = events[1];
    expect(approval.type).toBe('approval_request');
    if (approval.type === 'approval_request') {
      expect(approval.id).toBe('toolu_approval');
      expect(approval.action).toBe('Write');
      expect(approval.detail).toContain('demo.txt');
    }
  });

  it('permission_denials 是拒绝结果，不应再次映射为 approval_request', () => {
    const events = parseClaudeLine(
      JSON.stringify({
        type: 'result',
        subtype: 'success',
        session_id: 's1',
        stop_reason: 'end_turn',
        terminal_reason: 'completed',
        total_cost_usd: 0.01,
        usage: { input_tokens: 10, output_tokens: 20 },
        permission_denials: [
          {
            tool_name: 'Edit',
            tool_use_id: 'toolu_denied',
            tool_input: {
              file_path: 'demo.txt',
              old_string: 'before',
              new_string: 'after',
            },
          },
        ],
      }),
    );

    expect(events.map((event) => event.type)).toEqual(['token_usage', 'turn_complete']);
  });
});

describe('解析器健壮性', () => {
  it('空行 / 空白 / 非 JSON / 残缺 JSON 都返回空数组', () => {
    expect(parseClaudeLine('')).toEqual([]);
    expect(parseClaudeLine('   ')).toEqual([]);
    expect(parseClaudeLine('not json at all')).toEqual([]);
    expect(parseClaudeLine('{ broken')).toEqual([]);
  });

  it('未知事件类型返回空数组', () => {
    expect(parseClaudeLine(JSON.stringify({ type: 'mystery', session_id: 'x' }))).toEqual([]);
  });

  it('把思考增量 thinking_delta 映射为 thinking_delta 事件', () => {
    const line = JSON.stringify({
      type: 'stream_event',
      session_id: 's1',
      event: {
        type: 'content_block_delta',
        index: 0,
        delta: { type: 'thinking_delta', thinking: '...' },
      },
    });
    expect(parseClaudeLine(line)).toEqual([
      { type: 'thinking_delta', sessionId: 's1', text: '...' },
    ]);
  });

  it('把定稿 thinking block 映射为 thinking_complete 事件', () => {
    const line = JSON.stringify({
      type: 'assistant',
      session_id: 's1',
      message: {
        content: [{ type: 'thinking', thinking: '先定位文件，再修改。' }],
      },
    });
    expect(parseClaudeLine(line)).toEqual([
      { type: 'thinking_complete', sessionId: 's1', text: '先定位文件，再修改。' },
    ]);
  });

  it('把 Claude status/requesting 映射为 waiting_model 阶段', () => {
    const events = parseClaudeLine(
      JSON.stringify({
        type: 'system',
        subtype: 'status',
        status: 'requesting',
        session_id: 's1',
      }),
    );

    expect(events).toHaveLength(1);
    expect(events[0]).toMatchObject({
      type: 'turn_stage',
      sessionId: 's1',
      stage: 'waiting_model',
    });
    expect(isAgentEvent(events[0])).toBe(true);
  });

  it('把 Claude message_start 映射为 responding，并保留真实 ttft_ms', () => {
    const events = parseClaudeLine(
      JSON.stringify({
        type: 'stream_event',
        session_id: 's1',
        ttft_ms: 321,
        event: { type: 'message_start', message: { role: 'assistant' } },
      }),
    );

    expect(events).toHaveLength(1);
    expect(events[0]).toMatchObject({
      type: 'turn_stage',
      sessionId: 's1',
      stage: 'responding',
      engineReportedTtftMs: 321,
    });
    expect(isAgentEvent(events[0])).toBe(true);
  });

  it('拒绝 turn_stage 的非法可选数值边界', () => {
    const base = {
      type: 'turn_stage',
      sessionId: 's1',
      stage: 'retrying',
      ts: Date.now(),
    } as const;

    expect(isAgentEvent({ ...base, engineReportedTtftMs: -1 })).toBe(false);
    expect(isAgentEvent({ ...base, engineReportedTtftMs: Number.POSITIVE_INFINITY })).toBe(false);
    expect(isAgentEvent({ ...base, retryAttempt: 0 })).toBe(false);
    expect(isAgentEvent({ ...base, retryAttempt: 1.5 })).toBe(false);
    expect(isAgentEvent({ ...base, retryAttempt: Number.MAX_SAFE_INTEGER + 1 })).toBe(false);
    expect(isAgentEvent({ ...base, engineReportedTtftMs: 0 })).toBe(true);
    expect(isAgentEvent({ ...base, retryAttempt: 1 })).toBe(true);
  });
});
