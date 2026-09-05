import { afterEach, describe, expect, it, vi } from 'vitest';
import { forkTrace } from './forkTrace';

// 诊断通道契约：fire-and-forget，后端 invoke 失败必须静默（不能放大成用户可见故障）。
const appendRuntimeLogMock = vi.hoisted(() =>
  vi.fn<(line: string) => Promise<void>>(() => Promise.resolve()),
);
vi.mock('../engine/transport', () => ({
  appendRuntimeLog: appendRuntimeLogMock,
}));

afterEach(() => {
  vi.clearAllMocks();
});

describe('forkTrace', () => {
  it('带详情时输出 stage 与 detail', () => {
    forkTrace('rail_lossless', 'session-abc');
    expect(appendRuntimeLogMock).toHaveBeenCalledOnce();
    const line = appendRuntimeLogMock.mock.calls[0][0];
    expect(line).toContain('[helm-frontend-fork]');
    expect(line).toContain('stage=rail_lossless');
    expect(line).toContain('session-abc');
  });

  it('无详情时只输出 stage', () => {
    forkTrace('app_pending_set');
    const line = appendRuntimeLogMock.mock.calls[0][0];
    expect(line).toBe('[helm-frontend-fork] stage=app_pending_set');
  });

  it('appendRuntimeLog 拒绝时静默吞掉，不向外抛', async () => {
    appendRuntimeLogMock.mockReturnValueOnce(Promise.reject(new Error('log channel down')));
    expect(() => forkTrace('workspace_open_enter', 'session-xyz')).not.toThrow();
    // 微任务冲刷后也不产生 unhandled rejection 路径之外的副作用
    await new Promise((resolve) => setTimeout(resolve, 0));
  });
});
