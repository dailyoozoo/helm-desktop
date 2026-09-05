import { describe, expect, it } from 'vitest';
import { parseAnsi } from './parseAnsi';

const ESC = '\x1b';

describe('parseAnsi', () => {
  it('无转义序列时原样返回单个 token', () => {
    expect(parseAnsi('Count ----- 1')).toEqual([{ text: 'Count ----- 1' }]);
  });

  it('空输入返回空数组', () => {
    expect(parseAnsi('')).toEqual([]);
  });

  it('把 SGR 转义切成带颜色的 token，转义码本身不出现在文本里', () => {
    const tokens = parseAnsi(`${ESC}[32;1mCount${ESC}[0m 1`);
    expect(tokens).toHaveLength(2);
    expect(tokens[0]).toMatchObject({ text: 'Count', bold: true });
    expect(tokens[0].color).toBeTruthy();
    // 重置后不再带样式
    expect(tokens[1]).toEqual({ text: ' 1' });
    expect(tokens.every((token) => !token.text.includes('\x1b'))).toBe(true);
    expect(tokens.every((token) => !token.text.includes('[32'))).toBe(true);
  });

  it('支持背景色与斜体/下划线，并由 0 全部重置', () => {
    const tokens = parseAnsi(`${ESC}[41;4mX${ESC}[0mY`);
    expect(tokens[0]).toMatchObject({ text: 'X', underline: true });
    expect(tokens[0].bg).toBeTruthy();
    expect(tokens[1]).toEqual({ text: 'Y' });
  });

  it('丢弃非 SGR 的 CSI 序列（清屏/光标移动），不留在文本里', () => {
    const tokens = parseAnsi(`${ESC}[2Ja${ESC}[Hb`);
    expect(tokens.map((t) => t.text).join('')).toBe('ab');
    expect(tokens.every((token) => !token.text.includes('\x1b'))).toBe(true);
  });

  it('39/49 只恢复默认前景/背景，不影响粗体', () => {
    const tokens = parseAnsi(`${ESC}[1;31mA${ESC}[39mB`);
    expect(tokens[0]).toMatchObject({ text: 'A', bold: true });
    expect(tokens[0].color).toBeTruthy();
    expect(tokens[1]).toMatchObject({ text: 'B', bold: true });
    expect(tokens[1].color).toBeUndefined();
  });

  it('多段连续切换颜色时逐段保留', () => {
    const tokens = parseAnsi(`${ESC}[31mR${ESC}[32mG${ESC}[34mB${ESC}[0m`);
    expect(tokens.map((t) => t.text)).toEqual(['R', 'G', 'B']);
    expect(tokens[0].color).not.toBe(tokens[1].color);
  });
});
