import type { EngineId } from '@helm/protocol';
import type { McpServer } from '../extensions/extensionsApi';
import type { TurnMode } from '../engine/transport';
import type { EngineConfig } from '../providers/api';
import type { AppSettings } from './types';
import type { UpdateStatus } from './types';

export interface SessionDefaults {
  engine: EngineId;
  cwd: string;
}

export interface WorkspaceIdentity {
  name: string;
  avatar: string;
}

export interface WorkspaceActivity {
  handleId: string | null;
  sessionId: string | null;
  itemsLength: number;
}

export type WorkspaceApprovalMode = 'manual' | 'direct';

export function sessionDefaultsFromSettings(settings: AppSettings): SessionDefaults {
  return {
    engine: settings.engines.defaultEngine,
    cwd: settings.general.defaultDirectory.trim(),
  };
}

/** 新会话默认模式（变更-04 §0.3）：设置存量值 plan → 计划；auto/ask（旧「写入询问」）→ 构建。
 *  每轮实际生效的是发送框当前选中的模式，这里只决定新会话的初始选中。 */
export function defaultTurnModeFromSettings(settings: AppSettings): TurnMode {
  return settings.engines.claudeCode.permissionMode === 'plan' ? 'plan' : 'build';
}

export function workspaceIdentityFromSettings(settings: AppSettings): WorkspaceIdentity {
  const name = settings.general.workspaceName.trim() || 'Helm 工作区';
  const first = name.charAt(0);
  return {
    name,
    avatar: first.toLocaleUpperCase(),
  };
}

export function approvalModeFromSettings(settings: AppSettings): WorkspaceApprovalMode {
  if (settings.general.confirmBeforeCommand) return 'manual';
  return settings.permissions.runCommands === 'allow' ? 'direct' : 'manual';
}

export function toggleApprovalSettings(settings: AppSettings): AppSettings {
  const direct = approvalModeFromSettings(settings) === 'direct';
  return {
    ...settings,
    general: {
      ...settings.general,
      confirmBeforeCommand: direct,
    },
    permissions: {
      ...settings.permissions,
      runCommands: direct ? 'ask' : 'allow',
    },
  };
}

export function addCommandAllowlistPattern(
  permissions: AppSettings['permissions'],
  pattern: string,
): AppSettings['permissions'] {
  const next = pattern.trim();
  if (!next || permissions.commandAllowlist.includes(next)) return permissions;
  return {
    ...permissions,
    commandAllowlist: [...permissions.commandAllowlist, next],
  };
}

export function removeCommandAllowlistPattern(
  permissions: AppSettings['permissions'],
  pattern: string,
): AppSettings['permissions'] {
  return {
    ...permissions,
    commandAllowlist: permissions.commandAllowlist.filter((item) => item !== pattern),
  };
}

export function shouldReopenLastSession(
  settings: AppSettings,
  activity: WorkspaceActivity,
): boolean {
  return (
    settings.general.reopenLastSession &&
    !activity.handleId &&
    !activity.sessionId &&
    activity.itemsLength === 0
  );
}

export function updateStatusSummary(status: UpdateStatus): string {
  return `当前版本 v${status.currentVersion} · ${status.message}`;
}

export function mcpServerCommand(server: McpServer): string {
  return [server.command, ...server.args].filter(Boolean).join(' ');
}

export function mcpStatusLabel(status: McpServer['status']): string {
  if (status === 'connected') return '已连接';
  if (status === 'error') return '错误';
  return '离线';
}

export function engineConfigWithDetection(
  engine: EngineConfig,
  detection: { path: string; version: string },
): EngineConfig {
  return {
    ...engine,
    bin: detection.path,
    status: 'ready',
    version: detection.version,
  };
}
