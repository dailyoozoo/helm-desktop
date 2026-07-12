// Claude Code (`claude --output-format stream-json --verbose --include-partial-messages`)
// 真实 stdout 行 → 归一化 AgentEvent 的纯函数。
//
// 这是适配器的灵魂，也是契约测试的对象。映射严格依据实拍的真实输出 schema：
//   system/init                                  → session_started
//   stream_event content_block_delta/text_delta  → message_delta（逐字打字机）
//   assistant message.content[].text             → message_complete（定稿文本）
//   assistant message.content[].tool_use         → tool_call
//   user message.content[].tool_result           → tool_result
//   result                                        → token_usage + turn_complete / approval_request
// 工具入参增量(input_json_delta)、各类 message_start/stop 等噪声统一映射为空数组。
// thinking_delta 是工作区过程流的一等事件，会映射为 thinking_delta。
//
// 函数保持无状态：所需的 sessionId 在每一行里都有；tool_use 的完整入参直接取自
// 已定稿的 assistant 消息，无需拼接 input_json_delta。

import type { AgentEvent, Diff, DiffLine, EngineId } from '@helm/protocol';

const ENGINE: EngineId = 'claude-code';

function isRecord(x: unknown): x is Record<string, unknown> {
  return typeof x === 'object' && x !== null;
}

function asString(x: unknown): string {
  return typeof x === 'string' ? x : '';
}

/** 把 tool_result.content 归一化为一段纯文本输出。content 可能是字符串或内容块数组。 */
function stringifyToolContent(content: unknown): string {
  if (typeof content === 'string') return content;
  if (Array.isArray(content)) {
    const parts: string[] = [];
    for (const block of content) {
      if (isRecord(block) && typeof block.text === 'string') parts.push(block.text);
      else parts.push(JSON.stringify(block));
    }
    return parts.join('\n');
  }
  if (content === undefined || content === null) return '';
  return JSON.stringify(content);
}

function splitLines(text: string): string[] {
  if (text.length === 0) return [];
  const lines = text.replace(/\r\n/g, '\n').split('\n');
  if (lines[lines.length - 1] === '') lines.pop();
  return lines;
}

function lcsMatrix(a: string[], b: string[]): number[][] {
  const matrix = Array.from({ length: a.length + 1 }, () => Array<number>(b.length + 1).fill(0));
  for (let i = 1; i <= a.length; i += 1) {
    for (let j = 1; j <= b.length; j += 1) {
      matrix[i][j] =
        a[i - 1] === b[j - 1]
          ? matrix[i - 1][j - 1] + 1
          : Math.max(matrix[i - 1][j], matrix[i][j - 1]);
    }
  }
  return matrix;
}

function computeDiffLines(oldLines: string[], newLines: string[]): DiffLine[] {
  let pre = 0;
  while (pre < oldLines.length && pre < newLines.length && oldLines[pre] === newLines[pre])
    pre += 1;

  let suff = 0;
  while (
    suff < oldLines.length - pre &&
    suff < newLines.length - pre &&
    oldLines[oldLines.length - 1 - suff] === newLines[newLines.length - 1 - suff]
  ) {
    suff += 1;
  }

  const midOld = oldLines.slice(pre, oldLines.length - suff);
  const midNew = newLines.slice(pre, newLines.length - suff);
  const lcs = lcsMatrix(midOld, midNew);
  const lines: DiffLine[] = [];
  let oi = 0;
  let ni = 0;

  while (oi < midOld.length || ni < midNew.length) {
    if (oi < midOld.length && ni < midNew.length && midOld[oi] === midNew[ni]) {
      lines.push({ kind: 'ctx', text: midOld[oi] });
      oi += 1;
      ni += 1;
    } else if (ni >= midNew.length || (oi < midOld.length && lcs[oi + 1][ni] >= lcs[oi][ni + 1])) {
      lines.push({ kind: 'del', text: midOld[oi] });
      oi += 1;
    } else {
      lines.push({ kind: 'add', text: midNew[ni] });
      ni += 1;
    }
  }

  return lines;
}

function extractDiff(content: unknown): Diff | undefined {
  if (!Array.isArray(content)) return undefined;
  let path = '';
  let oldText = '';
  let newText = '';

  for (const block of content) {
    if (!isRecord(block) || block.type !== 'diff') continue;
    path = asString(block.path);
    oldText = asString(block.old_string);
    newText = asString(block.new_string);
  }

  if (oldText.length === 0 && newText.length === 0) return undefined;
  const oldLines = splitLines(oldText);
  const newLines = splitLines(newText);
  const lines = computeDiffLines(oldLines, newLines);
  if (lines.length === 0) return undefined;
  return { path, hunks: [{ oldStart: 1, newStart: 1, lines }] };
}

function parseSystem(obj: Record<string, unknown>, sessionId: string): AgentEvent[] {
  if (obj.subtype === 'status' && obj.status === 'requesting') {
    return [{ type: 'turn_stage', sessionId, stage: 'waiting_model', ts: Date.now() }];
  }
  if (obj.subtype !== 'init') return [];
  return [
    {
      type: 'session_started',
      sessionId,
      engine: ENGINE,
      model: asString(obj.model),
      cwd: asString(obj.cwd),
      ts: Date.now(),
    },
  ];
}

function parseStreamEvent(obj: Record<string, unknown>, sessionId: string): AgentEvent[] {
  const event = obj.event;
  if (!isRecord(event)) return [];
  if (event.type === 'message_start') {
    const stage: AgentEvent = {
      type: 'turn_stage',
      sessionId,
      stage: 'responding',
      ts: Date.now(),
    };
    if (typeof obj.ttft_ms === 'number' && Number.isFinite(obj.ttft_ms)) {
      stage.engineReportedTtftMs = obj.ttft_ms;
    }
    return [stage];
  }
  if (event.type !== 'content_block_delta') return [];
  const delta = event.delta;
  if (!isRecord(delta)) return [];
  if (delta.type === 'text_delta' && typeof delta.text === 'string' && delta.text.length > 0) {
    return [{ type: 'message_delta', sessionId, role: 'assistant', text: delta.text }];
  }
  if (
    delta.type === 'thinking_delta' &&
    typeof delta.thinking === 'string' &&
    delta.thinking.length > 0
  ) {
    return [{ type: 'thinking_delta', sessionId, text: delta.thinking }];
  }
  return [];
}

function parseAssistant(obj: Record<string, unknown>, sessionId: string): AgentEvent[] {
  const message = obj.message;
  if (!isRecord(message) || !Array.isArray(message.content)) return [];
  const out: AgentEvent[] = [];
  for (const block of message.content) {
    if (!isRecord(block)) continue;
    if (block.type === 'text' && typeof block.text === 'string') {
      out.push({ type: 'message_complete', sessionId, role: 'assistant', text: block.text });
    } else if (block.type === 'thinking' && typeof block.thinking === 'string') {
      out.push({ type: 'thinking_complete', sessionId, text: block.thinking });
    } else if (
      block.type === 'tool_use' &&
      typeof block.id === 'string' &&
      typeof block.name === 'string'
    ) {
      out.push({
        type: 'tool_call',
        sessionId,
        id: block.id,
        name: block.name,
        input: 'input' in block ? block.input : {},
        status: 'pending',
      });
    }
  }
  return out;
}

function parseUser(obj: Record<string, unknown>, sessionId: string): AgentEvent[] {
  const message = obj.message;
  if (!isRecord(message) || !Array.isArray(message.content)) return [];
  const out: AgentEvent[] = [];
  for (const block of message.content) {
    if (!isRecord(block)) continue;
    if (block.type === 'tool_result' && typeof block.tool_use_id === 'string') {
      out.push({
        type: 'tool_result',
        sessionId,
        id: block.tool_use_id,
        status: block.is_error === true ? 'error' : 'success',
        output: stringifyToolContent(block.content),
        diff: extractDiff(block.content),
      });
    }
  }
  return out;
}

function mapStopReason(obj: Record<string, unknown>): 'end' | 'interrupted' | 'error' {
  if (obj.subtype === 'success') return 'end';
  if (obj.terminal_reason === 'interrupted' || obj.stop_reason === 'interrupted')
    return 'interrupted';
  return 'error';
}

function parseResult(obj: Record<string, unknown>, sessionId: string): AgentEvent[] {
  const usage = isRecord(obj.usage) ? obj.usage : {};
  const inputTokens = typeof usage.input_tokens === 'number' ? usage.input_tokens : 0;
  const outputTokens = typeof usage.output_tokens === 'number' ? usage.output_tokens : 0;
  const costUsd = typeof obj.total_cost_usd === 'number' ? obj.total_cost_usd : 0;
  const contextWindow = contextWindowFromModelUsage(obj.modelUsage);
  const tokenUsage: AgentEvent = {
    type: 'token_usage',
    sessionId,
    inputTokens,
    outputTokens,
    costUsd,
  };
  if (tokenUsage.type === 'token_usage' && contextWindow) {
    tokenUsage.contextWindow = contextWindow;
  }
  const out: AgentEvent[] = [tokenUsage];

  const deferred = obj.deferred_tool_use;
  if (isRecord(deferred) && typeof deferred.id === 'string' && typeof deferred.name === 'string') {
    out.push({
      type: 'approval_request',
      sessionId,
      id: deferred.id,
      action: deferred.name,
      detail: JSON.stringify('input' in deferred ? deferred.input : {}, null, 2),
    });
    return out;
  }

  out.push({ type: 'turn_complete', sessionId, stopReason: mapStopReason(obj) });
  return out;
}

function contextWindowFromModelUsage(modelUsage: unknown): number | undefined {
  if (!isRecord(modelUsage)) return undefined;
  for (const value of Object.values(modelUsage)) {
    if (isRecord(value) && typeof value.contextWindow === 'number' && value.contextWindow > 0) {
      return value.contextWindow;
    }
  }
  return undefined;
}

/** 解析一行 Claude Code 的 stream-json 输出，产出 0..n 个归一化事件。 */
export function parseClaudeLine(raw: string): AgentEvent[] {
  const line = raw.trim();
  if (line.length === 0) return [];

  let obj: unknown;
  try {
    obj = JSON.parse(line);
  } catch {
    return [];
  }
  if (!isRecord(obj)) return [];

  // 子代理隔离（变更-09）：parent_tool_use_id 非空的行来自并行子代理（Task），
  // 与主线程共用同一 session_id，直接丢弃，防止子代理输出串进主回复。
  if (obj.parent_tool_use_id !== undefined && obj.parent_tool_use_id !== null) return [];

  const sessionId = asString(obj.session_id);

  switch (obj.type) {
    case 'system':
      return parseSystem(obj, sessionId);
    case 'stream_event':
      return parseStreamEvent(obj, sessionId);
    case 'assistant':
      return parseAssistant(obj, sessionId);
    case 'user':
      return parseUser(obj, sessionId);
    case 'result':
      return parseResult(obj, sessionId);
    default:
      return [];
  }
}
