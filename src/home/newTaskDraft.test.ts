import { describe, expect, it, beforeEach } from 'vitest';
import { stashHomeDraft, takeHomeDraft } from './newTaskViewModel';

/** node 环境无 window：用 Map 桩替代 sessionStorage（行为契约一致）。 */
const store = new Map<string, string>();

const fakeWindow = () => ({
  sessionStorage: {
    getItem: (key: string) => store.get(key) ?? null,
    setItem: (key: string, value: string) => void store.set(key, String(value)),
    removeItem: (key: string) => void store.delete(key),
  },
});

describe('新任务页草稿保护助手（D-13）', () => {
  beforeEach(() => {
    store.clear();
    (globalThis as Record<string, unknown>).window = fakeWindow();
  });

  it('暂存去空白后取出一次即清除', () => {
    stashHomeDraft('  帮我审查改动  ');
    expect(takeHomeDraft()).toBe('帮我审查改动');
    expect(takeHomeDraft()).toBe('');
  });

  it('空白文本不写入存储', () => {
    stashHomeDraft('   ');
    expect(store.size).toBe(0);
    expect(takeHomeDraft()).toBe('');
  });

  it('存储不可用时静默降级不抛错', () => {
    (globalThis as Record<string, unknown>).window = {};
    expect(() => stashHomeDraft('x')).not.toThrow();
    expect(takeHomeDraft()).toBe('');
  });
});
