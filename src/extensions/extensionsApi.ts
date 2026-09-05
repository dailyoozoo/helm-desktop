import { invoke } from '@tauri-apps/api/core';

// 仅类型依赖反向存在（viewModel 对本文件是 import type，运行时无环）
import { normalizeWindowsMcpLaunch } from './extensionsViewModel';

export interface Skill {
  id: string;
  name: string;
  description: string;
  scope: 'global' | 'project';
  source: 'builtin' | 'market' | 'custom' | 'plugin';
  enabled: boolean;
  path: string;
  engine: 'claude-code' | 'codex';
  trigger: string;
}

export type McpTransport = 'stdio' | 'sse' | 'http';

export interface McpServer {
  name: string;
  command: string;
  args: string[];
  env: Record<string, string>;
  /** http 远程连接器的自定义请求头（仅 Claude Code 配置承载；Codex TOML 无此字段） */
  headers?: Record<string, string>;
  transport: McpTransport;
  enabled: boolean;
  status: 'connected' | 'disconnected' | 'error';
  /** 最近一次测试连接的持久化状态（变更-05），unix 秒 */
  lastTestedAt?: number | null;
  toolCount?: number | null;
  lastError?: string | null;
}

/** 单条 JSON 导入结果：imported 已写入双引擎 / skipped 不支持已跳过 / failed 可重试 */
export interface McpImportItemResult {
  name: string;
  status: 'imported' | 'skipped' | 'failed';
  message?: string | null;
  /** 凭证类字段名列表（只回传名字，值已进系统钥匙串） */
  credentialKeys: string[];
  /** 规范化后的连接器定义，供失败重试直接调用 saveMcpServer */
  server?: McpServer | null;
}

/** 凭证存放状态：只回传字段名与是否已入钥匙串，绝不回传值 */
export interface McpCredentialStatus {
  key: string;
  stored: boolean;
}

/** 创建技能请求：路径由后端按引擎与作用域推导，前端不接受路径输入 */
export interface CreateSkillRequest {
  engine: Skill['engine'];
  scope: Skill['scope'];
  id: string;
  name: string;
  description: string;
  instructions: string;
}

/** 技能源码文件（预览/源码抽屉）：内容已经后端脱敏 */
export interface SkillSourceFile {
  path: string;
  content: string;
  truncated: boolean;
}

export interface McpTool {
  name: string;
  description?: string;
}

export interface Subagent {
  id: string;
  name: string;
  model: string;
  role: string;
  tools: string;
  auto: boolean;
  prompt: string;
  scope: 'global' | 'project';
}

export type CommandSource = 'extension' | 'engine-user' | 'engine-project' | 'builtin';

export interface SlashCommand {
  id: string;
  trigger: string;
  description: string;
  scope: 'global' | 'project';
  enabled: boolean;
  body: string;
  engine: 'all' | 'claude-code' | 'codex';
  /** 命令来源；同 trigger 冲突时 extension > engine-project > engine-user > builtin */
  source: CommandSource;
  argumentHint?: string | null;
}

export type HookEvent =
  | 'PreToolUse'
  | 'PostToolUse'
  | 'UserPromptSubmit'
  | 'Notification'
  | 'Stop'
  | 'SubagentStop'
  | 'PreCompact'
  | 'SessionStart'
  | 'SessionEnd';

export const HOOK_EVENTS: HookEvent[] = [
  'PreToolUse',
  'PostToolUse',
  'UserPromptSubmit',
  'Notification',
  'Stop',
  'SubagentStop',
  'PreCompact',
  'SessionStart',
  'SessionEnd',
];

export interface Hook {
  id: string;
  event: HookEvent;
  match: string;
  command: string;
  description: string;
  enabled: boolean;
  scope: 'global' | 'project';
}

/** skills.sh 市场搜索结果 */
export interface MarketSkill {
  skillId: string;
  name: string;
  source: string;
  installs: number;
  /** 一句话介绍；后端 parse_market_search_response 透传后生效（2026-08-27 反馈 #4） */
  description?: string;
}

export async function listSkills(
  engine?: 'claude-code' | 'codex',
  projectDir?: string,
): Promise<Skill[]> {
  return invoke('list_skills', { engine: engine ?? null, projectDir: projectDir || null });
}

export async function toggleSkill(
  skillId: string,
  enabled: boolean,
  projectDir?: string,
): Promise<void> {
  return invoke('toggle_skill', { skillId, enabled, projectDir: projectDir || null });
}

export async function listMcpServers(): Promise<McpServer[]> {
  return invoke('list_mcp_servers');
}

export async function testMcpConnection(server: McpServer): Promise<McpTool[]> {
  // Windows 下裸 npx 直接 spawn 会 program not found，检测前统一规范化
  return invoke('test_mcp_connection', { server: normalizeWindowsMcpLaunch(server) });
}

export async function saveMcpServer(server: McpServer): Promise<void> {
  // 写入引擎配置前同样规范化，保证 CLI 侧也能启动（对齐 Claude Code 官方 Windows 指引）
  return invoke('save_mcp_server', { server: normalizeWindowsMcpLaunch(server) });
}

export async function deleteMcpServer(name: string): Promise<void> {
  return invoke('delete_mcp_server', { name });
}

export async function listSubagents(projectDir?: string): Promise<Subagent[]> {
  return invoke('list_subagents', { projectDir: projectDir || null });
}

export async function saveSubagent(subagent: Subagent, projectDir?: string): Promise<void> {
  return invoke('save_subagent', { subagent, projectDir: projectDir || null });
}

export async function deleteSubagent(id: string, projectDir?: string): Promise<void> {
  return invoke('delete_subagent', { id, projectDir: projectDir || null });
}

export async function listSlashCommands(
  engine?: 'claude-code' | 'codex',
  cwd?: string,
): Promise<SlashCommand[]> {
  return invoke('list_slash_commands', { engine: engine ?? null, cwd: cwd || null });
}

export async function saveSlashCommand(command: SlashCommand, projectDir?: string): Promise<void> {
  return invoke('save_slash_command', { command, projectDir: projectDir || null });
}

export async function deleteSlashCommand(id: string, projectDir?: string): Promise<void> {
  return invoke('delete_slash_command', { id, projectDir: projectDir || null });
}

export async function listHooks(projectDir?: string): Promise<Hook[]> {
  return invoke('list_hooks', { projectDir: projectDir || null });
}

export async function saveHook(hook: Hook, projectDir?: string): Promise<void> {
  return invoke('save_hook', { hook, projectDir: projectDir || null });
}

export async function deleteHook(id: string, projectDir?: string): Promise<void> {
  return invoke('delete_hook', { id, projectDir: projectDir || null });
}

export async function marketSearchSkills(query: string): Promise<MarketSkill[]> {
  return invoke('market_search_skills', { query });
}

export async function marketInstallSkill(
  source: string,
  skillId: string,
  scope: 'global' | 'project',
  projectDir?: string,
): Promise<void> {
  return invoke('market_install_skill', {
    source,
    skillId,
    scope,
    projectDir: projectDir || null,
  });
}

// ==================== S7：技能创建 / 源码 / 卸载，连接器启停 / JSON 导入 / 凭证状态 ====================

export async function createSkill(
  request: CreateSkillRequest,
  projectDir?: string,
): Promise<Skill> {
  return invoke('create_skill', { request, projectDir: projectDir || null });
}

export async function readSkillSource(
  skillId: string,
  engine: Skill['engine'],
  projectDir?: string,
): Promise<SkillSourceFile> {
  return invoke('read_skill_source', {
    skillId,
    engine,
    projectDir: projectDir || null,
  });
}

export async function deleteSkill(
  skillId: string,
  engine: Skill['engine'],
  projectDir?: string,
): Promise<void> {
  return invoke('delete_skill', { skillId, engine, projectDir: projectDir || null });
}

export async function setMcpServerEnabled(name: string, enabled: boolean): Promise<void> {
  return invoke('set_mcp_server_enabled', { name, enabled });
}

export async function importMcpServers(json: string): Promise<McpImportItemResult[]> {
  return invoke('import_mcp_servers', { json });
}

export async function listMcpCredentialStatus(name: string): Promise<McpCredentialStatus[]> {
  return invoke('list_mcp_credential_status', { name });
}
