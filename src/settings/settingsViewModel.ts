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

export function pricingFeedUrlsFromDraft(draft: string): string[] {
  return draft
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter(Boolean);
}

export function sessionDefaultsFromSettings(settings: AppSettings): SessionDefaults {
  return {
    engine: settings.engines.defaultEngine,
    cwd: settings.general.defaultDirectory.trim(),
  };
}

/**
 * 两个引擎使用同一新会话默认模式。每轮实际生效的仍是 Composer 当前选择。
 */
export function defaultTurnModeForEngine(settings: AppSettings, _engine: EngineId): TurnMode {
  return settings.engines.claudeCode.permissionMode === 'plan' ? 'plan' : 'build';
}

export function defaultTurnModeFromSettings(settings: AppSettings): TurnMode {
  return defaultTurnModeForEngine(settings, settings.engines.defaultEngine);
}

export function workspaceIdentityFromSettings(settings: AppSettings): WorkspaceIdentity {
  const name = settings.general.workspaceName.trim() || 'Helm 工作区';
  const first = name.charAt(0);
  return {
    name,
    avatar: first.toLocaleUpperCase(),
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
