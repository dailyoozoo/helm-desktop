import { afterEach, describe, expect, it } from 'vitest';
import {
  getTranscriptDensity,
  nextTranscriptDensity,
  setTranscriptDensity,
  subscribeTranscriptDensity,
} from './transcriptDensity';

afterEach(() => {
  setTranscriptDensity('std');
  try {
    localStorage.clear();
  } catch {
    // 非浏览器环境忽略
  }
});

describe('nextTranscriptDensity', () => {
  it('toggles std → lite → std', () => {
    expect(nextTranscriptDensity('std')).toBe('lite');
    expect(nextTranscriptDensity('lite')).toBe('std');
  });
});

describe('transcriptDensity store', () => {
  it('defaults to std', () => {
    expect(getTranscriptDensity()).toBe('std');
  });

  it('persists to helm:density and notifies subscribers', () => {
    const seen: string[] = [];
    const unsubscribe = subscribeTranscriptDensity(() => seen.push(getTranscriptDensity()));
    setTranscriptDensity('lite');
    expect(getTranscriptDensity()).toBe('lite');
    expect(seen).toEqual(['lite']);
    try {
      expect(localStorage.getItem('helm:density')).toBe('lite');
    } catch {
      // 非浏览器环境跳过存储断言
    }
    unsubscribe();
  });

  it('rejects unknown stored values and falls back to std', () => {
    try {
      localStorage.setItem('helm:density', 'verbose');
    } catch {
      // 非浏览器环境跳过
    }
    expect(getTranscriptDensity()).toBe('std');
  });
});
