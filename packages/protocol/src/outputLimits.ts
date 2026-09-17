import type { Diff } from './events';

export const MAX_TOOL_OUTPUT_BYTES = 65_536;
export const OUTPUT_TRUNCATED = '\n[ledger_output_truncated]';
const DIFF_OMITTED = '\n[tool_diff_omitted] 差异过大，请打开文件查看完整内容';
const DIFF_OMITTED_BYTES = new TextEncoder().encode(DIFF_OMITTED).length;

function jsonStringBytes(text: string, limit: number): number {
  let bytes = 2;
  for (const character of text) {
    const point = character.codePointAt(0)!;
    if (
      point === 0x22 ||
      point === 0x5c ||
      point === 0x08 ||
      point === 0x09 ||
      point === 0x0a ||
      point === 0x0c ||
      point === 0x0d
    ) {
      bytes += 2;
    } else if (point < 0x20 || (point >= 0xd800 && point <= 0xdfff)) {
      bytes += 6;
    } else {
      bytes += point <= 0x7f ? 1 : point <= 0x7ff ? 2 : point <= 0xffff ? 3 : 4;
    }
    if (bytes > limit) break;
  }
  return bytes;
}

function boundedJsonBytes(value: unknown, limit: number): number {
  if (typeof value === 'string') return jsonStringBytes(value, limit);
  if (typeof value === 'number') return Number.isFinite(value) ? String(value).length : 4;
  if (typeof value === 'boolean') return value ? 4 : 5;
  if (value == null) return 4;
  if (typeof value !== 'object') return limit + 1;

  let bytes = 2;
  let count = 0;
  if (Array.isArray(value)) {
    for (const item of value) {
      bytes += (count++ ? 1 : 0) + boundedJsonBytes(item, limit - bytes);
      if (bytes > limit) break;
    }
  } else {
    for (const key in value) {
      if (!Object.prototype.hasOwnProperty.call(value, key)) continue;
      const item = (value as Record<string, unknown>)[key];
      if (item === undefined) continue;
      bytes += (count++ ? 1 : 0) + jsonStringBytes(key, limit - bytes) + 1;
      if (bytes > limit) break;
      bytes += boundedJsonBytes(item, limit - bytes);
      if (bytes > limit) break;
    }
  }
  return bytes;
}

function utf8Prefix(text: string, limit: number): string {
  let size = 0;
  let end = 0;
  for (const character of text) {
    const point = character.codePointAt(0)!;
    const bytes = point <= 0x7f ? 1 : point <= 0x7ff ? 2 : point <= 0xffff ? 3 : 4;
    if (size + bytes > limit) break;
    size += bytes;
    end += character.length;
  }
  return text.slice(0, end);
}

export function boundedToolOutput(text: string, limit = MAX_TOOL_OUTPUT_BYTES): string {
  const prefix = utf8Prefix(text, limit);
  if (prefix.length === text.length) return text;
  return (
    utf8Prefix(text, Math.max(0, limit - OUTPUT_TRUNCATED.length)) +
    (limit >= OUTPUT_TRUNCATED.length ? OUTPUT_TRUNCATED : '')
  );
}

export function appendToolOutput(previous: string, chunk: string): string {
  const bounded = boundedToolOutput(previous);
  if (bounded.endsWith(OUTPUT_TRUNCATED)) return bounded;
  return boundedToolOutput(bounded + boundedToolOutput(chunk));
}

export function boundedToolResult(output?: string, diff?: Diff): { output?: string; diff?: Diff } {
  const diffBytes = diff ? boundedJsonBytes(diff, MAX_TOOL_OUTPUT_BYTES) : 0;
  const omitted = diffBytes > MAX_TOOL_OUTPUT_BYTES - OUTPUT_TRUNCATED.length;
  const notice = omitted ? DIFF_OMITTED : '';
  const available =
    MAX_TOOL_OUTPUT_BYTES - (omitted ? 0 : diffBytes) - (omitted ? DIFF_OMITTED_BYTES : 0);
  const bounded = output == null ? undefined : boundedToolOutput(output, available);
  return { output: omitted ? (bounded ?? '') + notice : bounded, diff: omitted ? undefined : diff };
}
