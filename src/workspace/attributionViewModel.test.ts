import { describe, expect, it } from 'vitest';
import { attributionNote, attributionTip, type AttributionEntry } from './attributionViewModel';

const entries: AttributionEntry[] = [
  { label: '附件', value: '45%', ratio: 0.45, isHot: true, tip: '移除大附件可节省空间' },
  { label: '历史', value: '30%', ratio: 0.3 },
];

describe('attributionNote', () => {
  it('有条目时返回「按来源」', () => {
    expect(attributionNote(entries)).toBe('按来源');
  });

  it('无条目返回 null', () => {
    expect(attributionNote([])).toBeNull();
  });
});

describe('attributionTip', () => {
  it('返回 isHot 条目的建议', () => {
    expect(attributionTip(entries)).toBe('移除大附件可节省空间');
  });

  it('无 isHot 返回 null', () => {
    expect(attributionTip([{ label: '附件', value: '45%' }])).toBeNull();
  });

  it('空数组返回 null', () => {
    expect(attributionTip([])).toBeNull();
  });
});
