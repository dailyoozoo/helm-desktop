/** 是否运行在 Tauri 桌面环境。浏览器预览（vite dev/preview 直开）时为 false，此时所有 invoke 都不可用。 */
export function isTauriRuntime(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
}
