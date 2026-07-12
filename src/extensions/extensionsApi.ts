import { invoke } from '@tauri-apps/api/core';

export interface Skill {
  id: string;
  name: string;
  description: string;
  scope: 'global' | 'project';
  source: 'builtin' | 'market' | 'custom';
  enabled: boolean;
  path: string;
  engine: 'claude-code' | 'codex';
  trigger: string;
}

export interface McpServer {
  name: string;
  command: string;
  args: string[];
  env: Record<string, string>;
  transport: 'stdio' | 'sse' | 'http';
  enabled: boolean;
  status: 'connected' | 'disconnected' | 'error';
  /** 最近一次测试连接的持久化状态（变更-05），unix 秒 */
  lastTestedAt?: number | null;
  toolCount?: number | null;
  lastError?: string | null;
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
  return invoke('test_mcp_connection', { server });
}

export async function saveMcpServer(server: McpServer): Promise<void> {
  return invoke('save_mcp_server', { server });
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
