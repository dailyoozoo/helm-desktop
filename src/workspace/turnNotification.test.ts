import { describe, expect, it } from 'vitest';
import { isGenuineTurnEnd, type TurnEndTransition } from './turnNotification';

function t(status: 'idle' | 'working', historyId: string | null): TurnEndTransition {
  return { status, historyId };
}

describe('isGenuineTurnEnd', () => {
  it('同一会话轮次真实结束（working → idle）判定为结束', () => {
    expect(isGenuineTurnEnd(t('working', 'h-1'), t('idle', 'h-1'))).toBe(true);
  });

  it('切到另一会话不判定为结束（旧会话轮次仍在后台跑）', () => {
    expect(isGenuineTurnEnd(t('working', 'h-1'), t('idle', 'h-2'))).toBe(false);
  });

  it('切到后台会话（新线程仍 working）不判定为结束', () => {
    expect(isGenuineTurnEnd(t('working', 'h-1'), t('working', 'h-2'))).toBe(false);
  });

  it('新建/重置会话（historyId 置空）不判定为结束', () => {
    expect(isGenuineTurnEnd(t('working', 'h-1'), t('idle', null))).toBe(false);
  });

  it('同线程 working → working 不判定为结束', () => {
    expect(isGenuineTurnEnd(t('working', 'h-1'), t('working', 'h-1'))).toBe(false);
  });

  it('idle → idle / idle → working 不判定为结束', () => {
    expect(isGenuineTurnEnd(t('idle', 'h-1'), t('idle', 'h-1'))).toBe(false);
    expect(isGenuineTurnEnd(t('idle', 'h-1'), t('working', 'h-1'))).toBe(false);
  });

  it('切走再切回后轮次结束仍判定为结束（抓住的是真实结束而非切回本身）', () => {
    // 切回后台会话时 resume_handle 带 working=true，状态升回 working；
    // 随后 turn_complete 才会置 idle，此时 historyId 未变 → 应通知。
    expect(isGenuineTurnEnd(t('working', 'h-1'), t('idle', 'h-1'))).toBe(true);
  });
});
