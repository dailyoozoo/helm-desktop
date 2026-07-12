// 工作区专用的 Tauri 命令封装（变更-12）
import { invoke } from '@tauri-apps/api/core';

/** @文件引用：工作目录下按名称片段搜索文件（相对路径，正斜杠） */
export function searchWorkspaceFiles(cwd: string, query: string): Promise<string[]> {
  return invoke<string[]>('search_workspace_files', { cwd, query });
}

/** 粘贴图片：字节落盘为附件文件，返回绝对路径 */
export function savePastedImage(bytes: Uint8Array, extension: string): Promise<string> {
  return invoke<string>('save_pasted_image', { bytes: Array.from(bytes), extension });
}
