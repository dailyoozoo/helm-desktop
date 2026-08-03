import { invoke } from '@tauri-apps/api/core';
import type { AppSettings, UpdateStatus } from './types';
import type { PermissionRule } from './permissionRules';

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

// —— 结构化权限规则（Permission Kernel Phase 1） ——

export async function getPermissionRules(): Promise<PermissionRule[]> {
  return invoke('get_permission_rules');
}

export interface CreateDenyRuleInput {
  engine: 'claude-code' | 'codex' | null;
  capability:
    | 'file_read'
    | 'directory_list'
    | 'file_write'
    | 'process_exec'
    | 'network_request'
    | 'mcp_invoke';
  operation: string | null;
  resourcePattern: string | null;
  projectRoot: string | null;
}

export async function createPermissionDenyRule(
  input: CreateDenyRuleInput,
): Promise<PermissionRule[]> {
  return invoke('create_permission_deny_rule', { input });
}

export interface PermissionRuleRemovalResult {
  rules: PermissionRule[];
  revocationTooLateCount: number;
}

export async function removePermissionRule(ruleId: string): Promise<PermissionRuleRemovalResult> {
  return invoke('remove_permission_rule', { ruleId });
}

export interface PermissionAuditSummary {
  recordCount: number;
  oldestAt: number | null;
  newestAt: number | null;
  retentionDays: number;
}

export async function getPermissionAuditSummary(): Promise<PermissionAuditSummary> {
  return invoke('get_permission_audit_summary');
}

export async function exportPermissionAudit(includeResources: boolean): Promise<string | null> {
  return invoke('export_permission_audit', { includeResources });
}

export async function clearPermissionAudit(): Promise<PermissionAuditSummary> {
  return invoke('clear_permission_audit');
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
