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
const DIFF_LCS_CELL_LIMIT = 262_144;
const DIFF_LOOKAHEAD_LINES = 64;

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

function nextMatchingLine(lines: string[], start: number, text: string): number {
  const end = Math.min(lines.length, start + DIFF_LOOKAHEAD_LINES + 1);
  for (let index = start + 1; index < end; index += 1) {
    if (lines[index] === text) return index - start;
  }
  return -1;
}

function computeDiffLines(oldLines: string[], newLines: string[]) {
  let prefix = 0;
  while (
    prefix < oldLines.length &&
    prefix < newLines.length &&
    oldLines[prefix] === newLines[prefix]
  ) {
    prefix += 1;
  }
  let suffix = 0;
  while (
    suffix < oldLines.length - prefix &&
    suffix < newLines.length - prefix &&
    oldLines[oldLines.length - 1 - suffix] === newLines[newLines.length - 1 - suffix]
  ) {
    suffix += 1;
  }
  const old = oldLines.slice(prefix, oldLines.length - suffix);
  const next = newLines.slice(prefix, newLines.length - suffix);
  const width = next.length + 1;
  const cells = (old.length + 1) * width;
  const matrix = cells <= DIFF_LCS_CELL_LIMIT ? new Uint32Array(cells) : null;
  if (matrix) {
    for (let oldIndex = old.length - 1; oldIndex >= 0; oldIndex -= 1) {
      for (let newIndex = next.length - 1; newIndex >= 0; newIndex -= 1) {
        matrix[oldIndex * width + newIndex] =
          old[oldIndex] === next[newIndex]
            ? matrix[(oldIndex + 1) * width + newIndex + 1] + 1
            : Math.max(
                matrix[(oldIndex + 1) * width + newIndex],
                matrix[oldIndex * width + newIndex + 1],
              );
      }
    }
  }
  const lines: DiffLine[] = [];
  let oldIndex = 0;
  let newIndex = 0;
  while (oldIndex < old.length || newIndex < next.length) {
    if (oldIndex < old.length && newIndex < next.length && old[oldIndex] === next[newIndex]) {
      lines.push({ kind: 'ctx', text: old[oldIndex] });
      oldIndex += 1;
      newIndex += 1;
      continue;
    }
    let deleteFirst = newIndex === next.length;
    if (oldIndex < old.length && newIndex < next.length) {
      if (matrix) {
        deleteFirst =
          matrix[(oldIndex + 1) * width + newIndex] >= matrix[oldIndex * width + newIndex + 1];
      } else {
        const nextOld = nextMatchingLine(old, oldIndex, next[newIndex]);
        const nextNew = nextMatchingLine(next, newIndex, old[oldIndex]);
        if (nextOld < 0 && nextNew < 0) {
          lines.push({ kind: 'del', text: old[oldIndex] }, { kind: 'add', text: next[newIndex] });
          oldIndex += 1;
          newIndex += 1;
          continue;
        }
        deleteFirst = nextOld >= 0 && (nextNew < 0 || nextOld <= nextNew);
      }
    }
    if (deleteFirst) {
      lines.push({ kind: 'del', text: old[oldIndex] });
      oldIndex += 1;
    } else {
      lines.push({ kind: 'add', text: next[newIndex] });
      newIndex += 1;
    }
  }
  return { lines, start: prefix + 1 };
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
  const { lines, start } = computeDiffLines(oldLines, newLines);
  if (lines.length === 0) return undefined;
  return { path, hunks: [{ oldStart: start, newStart: start, lines }] };
}

function parseSystem(obj: Record<string, unknown>, sessionId: string): AgentEvent[] {
  if (obj.subtype === 'status' && obj.status === 'requesting') {
    return [{ type: 'turn_stage', sessionId, stage: 'waiting_model', ts: Date.now() }];
  }
  if (obj.subtype !== 'init') return [];
  const tools = Array.isArray(obj.tools)
    ? obj.tools.filter((tool): tool is string => typeof tool === 'string')
    : [];
  return [
    {
      type: 'session_started',
      sessionId,
      engine: ENGINE,
      model: asString(obj.model),
      cwd: asString(obj.cwd),
      ts: Date.now(),
      capabilities: {
        webSearch:
          tools.length === 0
            ? 'unknown'
            : tools.includes('WebSearch')
              ? 'available'
              : 'unavailable',
        webFetch:
          tools.length === 0 ? 'unknown' : tools.includes('WebFetch') ? 'available' : 'unavailable',
        approvalContractVersion: 'claude-hook-bridge-v1',
      },
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
      if (isClaudeModelUnavailableMessage(block.text)) {
        out.push({
          type: 'error',
          sessionId,
          message: block.text,
          recoverable: false,
          kind: 'model_unavailable',
        });
      } else {
        out.push({ type: 'message_complete', sessionId, role: 'assistant', text: block.text });
      }
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
  const usage = isRecord(message.usage) ? message.usage : undefined;
  if (usage) {
    const fresh = typeof usage.input_tokens === 'number' ? usage.input_tokens : 0;
    const cached =
      typeof usage.cache_read_input_tokens === 'number' ? usage.cache_read_input_tokens : 0;
    const cacheWrite =
      typeof usage.cache_creation_input_tokens === 'number' ? usage.cache_creation_input_tokens : 0;
    const contextTokens = fresh + cached + cacheWrite;
    if (contextTokens > 0) {
      out.push({ type: 'context_usage', sessionId, contextTokens });
    }
  }
  return out;
}

function isClaudeModelUnavailableMessage(text: string): boolean {
  const lower = text.toLowerCase();
  return (
    lower.includes("there's an issue with the selected model") &&
    lower.includes('may not exist or you may not have access')
  );
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
  const uncachedInputTokens = typeof usage.input_tokens === 'number' ? usage.input_tokens : 0;
  const cachedInputTokens =
    typeof usage.cache_read_input_tokens === 'number' ? usage.cache_read_input_tokens : 0;
  const cacheWriteInputTokens =
    typeof usage.cache_creation_input_tokens === 'number' ? usage.cache_creation_input_tokens : 0;
  const inputTokens = uncachedInputTokens + cachedInputTokens + cacheWriteInputTokens;
  const outputTokens = typeof usage.output_tokens === 'number' ? usage.output_tokens : 0;
  const costUsd = typeof obj.total_cost_usd === 'number' ? obj.total_cost_usd : 0;
  const contextWindow = contextWindowFromModelUsage(obj.modelUsage);
  const tokenUsage: AgentEvent = {
    type: 'token_usage',
    sessionId,
    inputTokens,
    ...(cachedInputTokens > 0 ? { cachedInputTokens } : {}),
    ...(cacheWriteInputTokens > 0 ? { cacheWriteInputTokens } : {}),
    outputTokens,
    costUsd,
    ...(typeof usage.service_tier === 'string' &&
    ['standard', 'batch', 'flex', 'priority'].includes(usage.service_tier)
      ? { serviceTier: usage.service_tier }
      : {}),
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
      availableDecisions: ['allow', 'deny'],
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
