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

export function workspaceIdentityFromSettings(_settings: AppSettings): WorkspaceIdentity {
  // 工作区名称输入框已按原型移除，标题栏固定显示「Helm 工作区」。
  const name = 'Helm 工作区';
  return {
    name,
    avatar: name.charAt(0),
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

/**
 * 启动首落页决策（2026-09-03 用户决议）：首次启动时「有上次任务就进它的页面，
 * 没有任何任务就进新任务页」——兑现设置项「没有可恢复任务时直接进入新任务」的文案承诺。
 * - 关闭「启动时恢复退出前任务」：维持旧行为，落在工作区空态；
 * - 指针会话存在：留在工作区，由工作区既有自动恢复链路接管（App 不重复恢复）；
 * - 指针缺失但历史里有任务（删除指针/旧库升级等）：兜底打开最近一个未归档会话；
 * - 一个任务都没有：去新任务页。
 */
export type StartupLanding =
  | { kind: 'workspace' }
  | { kind: 'recent'; sessionId: string }
  | { kind: 'home' };

export interface StartupRecoveryInput {
  /** active_session_id 指针指向的会话存在 */
  hasActiveSession: boolean;
  /** 指针缺失时从历史挑出的「上次任务」；没有任何未归档会话为 null */
  recentSessionId: string | null;
}

export function startupLandingFromRecovery(
  settings: AppSettings,
  recovery: StartupRecoveryInput,
): StartupLanding {
  if (!settings.general.reopenLastSession) return { kind: 'workspace' };
  if (recovery.hasActiveSession) return { kind: 'workspace' };
  if (recovery.recentSessionId) return { kind: 'recent', sessionId: recovery.recentSessionId };
  return { kind: 'home' };
}

/** 「上次任务」挑选：排除已归档，按 updatedAt（秒）取最新；与侧栏置顶排序解耦。 */
export function mostRecentSessionId(
  sessions: ReadonlyArray<{ id: string; archived?: boolean; updatedAt: number }>,
): string | null {
  let best: { id: string; updatedAt: number } | null = null;
  for (const session of sessions) {
    if (session.archived) continue;
    if (!best || session.updatedAt > best.updatedAt) best = session;
  }
  return best?.id ?? null;
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
