import { invoke } from '@tauri-apps/api/core';
import type { AppSettings, UpdateStatus } from './types';

export async function loadSettings(): Promise<AppSettings> {
  return invoke('load_app_settings');
}

export async function saveSettings(settings: AppSettings): Promise<void> {
  return invoke('save_app_settings', { settings });
}

export async function exportSettings(): Promise<string | null> {
  return invoke('export_app_settings');
}

export async function importSettings(): Promise<AppSettings | null> {
  return invoke('import_app_settings');
}

export async function getUpdateStatus(): Promise<UpdateStatus> {
  return invoke('get_update_status');
}

// —— 真实更新链路（P2-1）——

export interface UpdateCheckResult {
  currentVersion: string;
  available: boolean;
  version: string | null;
  notes: string | null;
}

export async function checkForUpdate(): Promise<UpdateCheckResult> {
  return invoke('check_for_update');
}

export async function installUpdate(): Promise<void> {
  return invoke('install_update');
}

export async function detectEngine(
  engine: 'claude-code' | 'codex',
): Promise<{ path: string; version: string }> {
  return invoke('detect_cli_engine', { engine });
}

// —— 一键安装 CLI（P3-4）——

export interface CliInstallResult {
  path: string;
  version: string;
  /** npm 输出尾部（诊断用） */
  output: string;
}

export async function installCliEngine(engine: 'claude-code' | 'codex'): Promise<CliInstallResult> {
  return invoke('install_cli_engine', { engine });
}

export async function selectDirectory(): Promise<string | null> {
  return invoke('select_directory');
}

// —— 跨会话「始终允许」清单（P2-4） ——

export async function getAlwaysAllowTools(): Promise<string[]> {
  return invoke('get_always_allow_tools');
}

export async function removeAlwaysAllowTool(tool: string): Promise<string[]> {
  return invoke('remove_always_allow_tool', { tool });
}

// —— 冷启动就绪度（首启向导与发送前置校验共用） ——

export interface EngineReadiness {
  installed: boolean;
  path: string | null;
  version: string | null;
  error: string | null;
  /** CLI 自身登录态（订阅登录一等公民，P3-1） */
  login: { state: 'ok' | 'missing' | 'unknown'; detail: string };
}

export interface ReadinessReport {
  claudeCode: EngineReadiness;
  codex: EngineReadiness;
  hasProvider: boolean;
  hasReadyProvider: boolean;
  defaultEngine: string;
  boundEngines: string[];
  cwd: { configured: boolean; exists: boolean; path: string };
}

export async function getReadinessReport(): Promise<ReadinessReport> {
  return invoke('get_readiness_report');
}
