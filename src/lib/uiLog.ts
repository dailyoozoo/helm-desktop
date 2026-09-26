import { invoke } from '@tauri-apps/api/core';

/**
 * 把关键 UI 时序（toast 弹出、安装向导步骤、就绪度探测结果）写进 helm.log，
 * 前缀 [helm-ui]。用途：用户报「先报错后成功」这类顺序问题时，日志即可还原
 * 完整顺序，不依赖截图。失败（如浏览器调试模式无 Tauri 桥）静默，不影响 UI。
 */
export function logUi(event: string, detail?: unknown): void {
  const line = detail === undefined ? event : `${event} ${safeJson(detail)}`;
  // eslint-disable-next-line no-console
  console.info(`[helm-ui] ${line}`);
  void invoke('log_frontend_event', { message: line }).catch(() => {
    // 无 Tauri 桥时放弃落盘，仅保留 console
  });
}

function safeJson(value: unknown): string {
  try {
    return JSON.stringify(value) ?? 'undefined';
  } catch {
    return String(value);
  }
}
