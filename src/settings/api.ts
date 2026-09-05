import { invoke } from '@tauri-apps/api/core';
import type { AppSettings, UpdateStatus } from './types';
import type { PermissionRule } from './permissionRules';

export async function loadSettings(): Promise<AppSettings> {
  return invoke('load_app_settings');
}

export async function saveSettings(settings: AppSettings): Promise<void> {
  return invoke('save_app_settings', { settings });
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

// —— 工作区环境依赖（变更-37：Node/git 探测与一键安装）——

export interface WorkspaceDepStatus {
  available: boolean;
  version: string | null;
}

export interface WorkspaceDeps {
  node: WorkspaceDepStatus;
  npm: WorkspaceDepStatus;
  git: WorkspaceDepStatus;
}

/** 探测 node / npm / git（真实 `--version`） */
export async function detectWorkspaceDeps(): Promise<WorkspaceDeps> {
  return invoke('detect_workspace_deps');
}

export interface ToolInstallResult {
  /** 安装后复检到的可执行文件路径（PATH 或已知安装目录） */
  path: string;
  version: string;
  /** true 表示 PATH 尚未刷新，需要重启 Helm 后才可在新进程中解析 */
  restartRequired: boolean;
}

/** 一键静默安装 Node LTS（国内镜像 + SHA-256SUMS 验签） */
export async function installNode(): Promise<ToolInstallResult> {
  return invoke('install_node');
}

/** 一键静默安装 git（git-for-windows 国内二进制镜像 + 校验） */
export async function installGit(): Promise<ToolInstallResult> {
  return invoke('install_git');
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

// —— 关于页（S8）：平台信息 / 日志目录 / 诊断包 / 历史对话导入 ——

export interface PlatformInfo {
  osName: string;
  /** 仅 Windows 提供真实内核版本号（RtlGetVersion）；其他平台为 null。 */
  osVersion: string | null;
  arch: string;
  appVersion: string;
  tauriVersion: string;
  webviewVersion: string;
}

/** 真实平台/版本事实，用于「关于」页与反馈核对。 */
export async function getPlatformInfo(): Promise<PlatformInfo> {
  return invoke('get_platform_info');
}

export interface LastDiagnosticsExport {
  path: string;
  exportedAt: string;
}

export interface LogDirInfo {
  path: string;
  fileCount: number;
  lastDiagnosticsExport: LastDiagnosticsExport | null;
}

/** 解析并确保日志目录存在；打开目录复用 open_path_in_system。 */
export async function getLogDirInfo(): Promise<LogDirInfo> {
  return invoke('get_log_dir_info');
}

export interface DiagnosticsExportResult {
  path: string;
  bytes: number;
}

/** 导出脱敏诊断包；用户取消返回 null。 */
export async function exportDiagnosticsBundle(): Promise<DiagnosticsExportResult | null> {
  return invoke('export_diagnostics_bundle');
}

export interface ImportableHistoryEntry {
  engine: 'claude-code' | 'codex' | string;
  path: string;
  fileName: string;
  sessionId: string | null;
  cwd: string | null;
  messageCount: number;
  firstMessagePreview: string | null;
  model: string | null;
  sizeBytes: number;
  modifiedAtMs: number;
}

export interface ImportableHistoryScan {
  entries: ImportableHistoryEntry[];
  totalFound: number;
  skippedTooLarge: number;
  skippedUnparsable: number;
}

/** 扫描本机 Claude Code / Codex 记录文件（只读）。 */
export async function listImportableHistories(
  engine: 'claude-code' | 'codex',
): Promise<ImportableHistoryScan> {
  return invoke('list_importable_histories', { engine });
}

export interface HistoryImportResult {
  sessionId: string;
  title: string;
  engine: string;
  cwd: string;
  importedMessages: number;
  skippedLines: number;
}

/**
 * 把一个 JSONL 历史记录文件导入为本地会话。
 * engine 传 'auto' 时按内容形状探测（Codex rollout 优先，其次 Claude Code）。
 */
export async function importHistory(input: {
  sourcePath: string;
  engine: 'claude-code' | 'codex' | 'auto';
  titleOverride?: string;
}): Promise<HistoryImportResult> {
  return invoke('import_history', {
    sourcePath: input.sourcePath,
    engine: input.engine,
    titleOverride: input.titleOverride ?? null,
  });
}
