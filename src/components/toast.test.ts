import { afterEach, describe, expect, it, vi } from 'vitest';
import {
  dismissToast,
  resetToastsForTest,
  showToast,
  subscribeToasts,
  type ToastEntry,
} from './toast';

describe('全局通知层数据通道', () => {
  afterEach(() => {
    resetToastsForTest();
    vi.useRealTimers();
  });

  it('showToast 推送给订阅者，错误默认停留更久', () => {
    let latest: ToastEntry[] = [];
    subscribeToasts((toasts) => {
      latest = toasts;
    });

    showToast('已保存', 'success');
    showToast('保存失败', 'error');

    expect(latest).toHaveLength(2);
    expect(latest[0]).toMatchObject({ message: '已保存', kind: 'success' });
    expect(latest[1].kind).toBe('error');
    expect(latest[1].duration).toBeGreaterThan(latest[0].duration);
  });

  it('相同文案与级别的提示会去重，避免批量失败刷屏', () => {
    let latest: ToastEntry[] = [];
    subscribeToasts((toasts) => {
      latest = toasts;
    });

    showToast('读取技能列表失败', 'error');
    showToast('读取技能列表失败', 'error');

    expect(latest).toHaveLength(1);
  });

  it('dismissToast 按 id 移除', () => {
    let latest: ToastEntry[] = [];
    subscribeToasts((toasts) => {
      latest = toasts;
    });

    const id = showToast('稍后消失');
    expect(latest).toHaveLength(1);
    dismissToast(id);
    expect(latest).toHaveLength(0);
  });

  it('订阅时立即收到当前快照，退订后不再收到', () => {
    showToast('已有提示');
    const received: ToastEntry[][] = [];
    const unsubscribe = subscribeToasts((toasts) => received.push(toasts));

    expect(received[0]).toHaveLength(1);
    unsubscribe();
    showToast('退订后的提示');
    expect(received).toHaveLength(1);
  });
});
