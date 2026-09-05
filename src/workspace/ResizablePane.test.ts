import { describe, expect, it } from 'vitest';
import { clampPaneWidth } from './ResizablePane';

describe('clampPaneWidth（变更-34 · E1 右栏宽度）', () => {
  it('小于最小宽度时钳制到 360px', () => {
    expect(clampPaneWidth(1000, 1200)).toBe(360);
  });

  it('超过 60vw 上限时钳制到 60vw', () => {
    expect(clampPaneWidth(100, 1200)).toBe(720);
  });

  it('合理区间内返回实际宽度（取整）', () => {
    expect(clampPaneWidth(500, 1200)).toBe(700);
    expect(clampPaneWidth(501.4, 1200)).toBe(699);
  });

  it('支持自定义最小宽度', () => {
    expect(clampPaneWidth(1000, 1200, 320)).toBe(320);
  });
});
