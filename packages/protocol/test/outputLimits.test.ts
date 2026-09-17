import { describe, expect, it, vi } from 'vitest';
import type { Diff } from '../src/events';
import {
  appendToolOutput,
  boundedToolOutput,
  boundedToolResult,
  MAX_TOOL_OUTPUT_BYTES,
  OUTPUT_TRUNCATED,
} from '../src/outputLimits';

const encoder = new TextEncoder();

function diffWithText(text: string): Diff {
  return {
    path: 'src/文件.ts',
    hunks: [{ oldStart: 1, newStart: 1, lines: [{ kind: 'add', text }] }],
  };
}

function resultBytes(result: ReturnType<typeof boundedToolResult>): number {
  return (
    encoder.encode(result.output ?? '').length +
    (result.diff ? encoder.encode(JSON.stringify(result.diff)).length : 0)
  );
}

describe('tool output UTF-8 budget', () => {
  it('preserves output at the exact boundary and marks only truncated output', () => {
    const exact = 'a'.repeat(MAX_TOOL_OUTPUT_BYTES);
    expect(boundedToolOutput(exact)).toBe(exact);
    const truncated = boundedToolOutput(`${exact}b`);
    expect(encoder.encode(truncated)).toHaveLength(MAX_TOOL_OUTPUT_BYTES);
    expect(truncated).toBe(
      'a'.repeat(MAX_TOOL_OUTPUT_BYTES - OUTPUT_TRUNCATED.length) + OUTPUT_TRUNCATED,
    );
  });

  it.each(['汉', '😀', 'é', '汉😀a', '\ud800'])(
    'keeps %s within the byte budget without splitting code points',
    (text) => {
      const source = text.repeat(MAX_TOOL_OUTPUT_BYTES);
      const bounded = boundedToolOutput(source);
      expect(encoder.encode(bounded).length).toBeLessThanOrEqual(MAX_TOOL_OUTPUT_BYTES);
      expect(bounded.endsWith(OUTPUT_TRUNCATED)).toBe(true);
      const prefix = bounded.slice(0, -OUTPUT_TRUNCATED.length);
      expect(source.startsWith(prefix)).toBe(true);
      if (text.includes('😀')) expect(prefix).not.toMatch(/[\ud800-\udbff]$/u);
    },
  );

  it('does not exceed a budget smaller than the truncation marker', () => {
    expect(boundedToolOutput('😀😀', 5)).toBe('');
    expect(boundedToolOutput('text', 0)).toBe('');
    expect(encoder.encode(boundedToolOutput('中'.repeat(30), 26)).length).toBeLessThanOrEqual(26);
  });

  it('bounds a large incoming chunk before combining and stops appending after truncation', () => {
    const bounded = appendToolOutput('prefix\n', '😀'.repeat(500_000));
    expect(bounded.startsWith('prefix\n')).toBe(true);
    expect(bounded.endsWith(OUTPUT_TRUNCATED)).toBe(true);
    expect(encoder.encode(bounded).length).toBeLessThanOrEqual(MAX_TOOL_OUTPUT_BYTES);
    expect(appendToolOutput(bounded, 'late output')).toBe(bounded);
    expect(appendToolOutput('small', ' chunk')).toBe('small chunk');
  });

  it.each(['normal', '汉😀', '\n\r\t\b\f\u0000\u001f"\\', '\ud800\udfff\ud800'])(
    'shares output and serialized diff bytes including escapes: %j',
    (text) => {
      const diff = diffWithText(text.repeat(100));
      const bounded = boundedToolResult('x'.repeat(MAX_TOOL_OUTPUT_BYTES), diff);
      expect(bounded.diff).toBe(diff);
      expect(bounded.output?.endsWith(OUTPUT_TRUNCATED)).toBe(true);
      expect(resultBytes(bounded)).toBe(MAX_TOOL_OUTPUT_BYTES);
    },
  );

  it('matches JSON number encoding and counts all serialized fields', () => {
    const diff = diffWithText('text');
    diff.hunks[0].oldStart = -0;
    diff.hunks[0].newStart = Number.POSITIVE_INFINITY;
    const bounded = boundedToolResult('x'.repeat(MAX_TOOL_OUTPUT_BYTES), {
      ...diff,
      ...{
        extra: { flag: true, disabled: false, value: null, absent: undefined, values: [1, 1e21] },
      },
    });
    expect(resultBytes(bounded)).toBe(MAX_TOOL_OUTPUT_BYTES);
  });

  it('keeps an exact-fit diff, but omits a diff beyond the shared budget', () => {
    const overhead = encoder.encode(JSON.stringify(diffWithText(''))).length;
    const text = 'x'.repeat(MAX_TOOL_OUTPUT_BYTES - OUTPUT_TRUNCATED.length - overhead);
    const exact = diffWithText(text);
    expect(boundedToolResult('output', exact).diff).toBe(exact);
    const omitted = boundedToolResult('output', diffWithText(`${text}x`));
    expect(omitted.diff).toBeUndefined();
    expect(omitted.output).toContain('tool_diff_omitted');
    expect(resultBytes(omitted)).toBeLessThanOrEqual(MAX_TOOL_OUTPUT_BYTES);
  });

  it('omits oversized diffs without stringifying or reading the remaining lines', () => {
    let trailingReads = 0;
    const diff = diffWithText('😀'.repeat(500_000));
    diff.hunks[0].lines.push({
      kind: 'add',
      get text() {
        trailingReads += 1;
        return 'must not be read';
      },
    });
    const stringify = vi.spyOn(JSON, 'stringify');
    let bounded: ReturnType<typeof boundedToolResult>;
    try {
      bounded = boundedToolResult('汉'.repeat(MAX_TOOL_OUTPUT_BYTES), diff);
      expect(stringify).not.toHaveBeenCalled();
    } finally {
      stringify.mockRestore();
    }
    expect(trailingReads).toBe(0);
    expect(bounded.diff).toBeUndefined();
    expect(bounded.output).toContain('ledger_output_truncated');
    expect(bounded.output).toContain('tool_diff_omitted');
    expect(resultBytes(bounded)).toBeLessThanOrEqual(MAX_TOOL_OUTPUT_BYTES);
  });

  it('handles missing output and diff independently', () => {
    expect(boundedToolResult()).toEqual({ output: undefined, diff: undefined });
    const diff = diffWithText('small');
    expect(boundedToolResult(undefined, diff)).toEqual({ output: undefined, diff });
    const omitted = boundedToolResult(undefined, diffWithText('x'.repeat(MAX_TOOL_OUTPUT_BYTES)));
    expect(omitted.diff).toBeUndefined();
    expect(omitted.output).toContain('tool_diff_omitted');
  });
});
