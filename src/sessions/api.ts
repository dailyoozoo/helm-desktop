import { invoke } from '@tauri-apps/api/core';
import type { Diff } from '@helm/protocol';
import type { SessionFolder, SessionSummary } from './sessionTypes';

export interface SessionMessage {
  role: 'user' | 'assistant';
  text: string;
  ts: number;
  /** 是否被检查点回溯（P2-5）：重开会话时保留淡化视觉 */
  reverted?: boolean;
  /** schema v17：与逐 Turn 权限审计稳定关联。 */
  turnId?: string | null;
  attachments?: string[];
}

export interface SessionContextRecord {
  id: string;
  kind: 'file' | 'directory';
  sourcePath: string;
  canonicalPath: string;
  displayName: string;
  status: 'ready' | 'missing' | 'blocked';
  statusDetail?: string | null;
  createdAt: number;
  updatedAt: number;
}

export interface SessionToolCall {
  id: string;
  name: string;
  status: 'pending' | 'success' | 'error';
  input: unknown;
  output?: string | null;
  diff?: Diff | null;
  /** 毫秒时间戳（变更-10）：历史恢复按时间线穿插排序用 */
  ts: number;
  endedAt?: number | null;
  turnId?: string | null;
  outcome?: import('@helm/protocol').ToolOutcomeKind | null;
  started?: boolean | null;
  hasOutput?: boolean | null;
  retryable?: boolean | null;
  denialSource?: import('@helm/protocol').ToolDenialSource | null;
  nativeDenialCode?: string | null;
}

export interface SessionCheckpoint {
  id: string;
  label: string;
  ts: number;
  turnId?: string | null;
  restorable?: boolean;
  fileCount?: number;
  reason?: string | null;
}

/** 审批请求持久化记录（变更-07）：pending 的悬空审批在重开会话时重建审批卡 */
export interface SessionApproval {
  id: string;
  action: string;
  detail: string;
  status: 'pending' | 'applying' | 'resolved' | 'failed' | 'expired';
  ts: number;
  decision?: 'allow' | 'turn' | 'session' | 'project' | 'always' | 'deny' | null;
  ruleId?: string | null;
  error?: string | null;
  resolvedAt?: number | null;
  persistentLabel?: string | null;
  matcherSummary?: string | null;
  turnId?: string | null;
}

export interface SessionTurn {
  id: string;
  epoch: number;
  mode: 'build' | 'plan' | 'ask';
  permissionProfile: 'standard' | 'auto' | 'full_access';
  status: 'running' | 'waiting_approval' | 'stalled' | 'succeeded' | 'failed' | 'interrupted';
  startedAt: number;
  endedAt?: number | null;
  terminalReason?: string | null;
  providerDisplayName?: string | null;
  requestedModelId?: string | null;
  routedModelId?: string | null;
  requestedReasoningEffort?: string | null;
  routedReasoningEffort?: string | null;
  resolutionSource?: string | null;
}

export interface SessionDetail extends SessionSummary {
  messages: SessionMessage[];
  toolCalls: SessionToolCall[];
  checkpoints: SessionCheckpoint[];
  approvals: SessionApproval[];
  /** schema v17：逐轮权限审计；旧导入数据可为空。 */
  turns?: SessionTurn[];
  sessionContext?: SessionContextRecord[];
  fork?: SessionForkSummary | null;
}

export interface SessionForkSummary {
  id: string;
  handoffId: string;
  sourceSessionId?: string | null;
  sourceTitleSnapshot: string;
  sourceEngine: string;
  targetEngine: string;
  boundaryTurnId: string;
  boundaryTurnEpoch: number;
  createdAt: number;
}

export interface BackgroundOperation {
  id: string;
  kind: string;
  sourceSessionId?: string | null;
  inputDigest: string;
  input?: unknown;
  idempotencyKey: string;
  status: 'committed' | 'running' | 'succeeded' | 'failed' | 'cancelled' | 'delivery_unknown';
  result?: unknown;
  errorCode?: string | null;
  createdAt: number;
  startedAt?: number | null;
  cancelRequestedAt?: number | null;
  endedAt?: number | null;
}

export function startSessionFork(
  sourceSessionId: string,
  targetEngine: string,
  boundaryTurnId?: string,
): Promise<BackgroundOperation> {
  return invoke<BackgroundOperation>('start_session_fork', {
    sourceSessionId,
    targetEngine,
    boundaryTurnId: boundaryTurnId ?? null,
  });
}

/** 同引擎无损分支结果（十次反馈）：lossless 即时返回新会话；summary 走既有摘要派生轮询。 */
export type BranchForkOutcome =
  | { mode: 'lossless'; sessionId: string }
  | { mode: 'summary'; operation: BackgroundOperation };

export function startSessionBranch(
  sourceSessionId: string,
  sourceTurnId?: string,
): Promise<BranchForkOutcome> {
  return invoke<BranchForkOutcome>('start_session_branch', {
    sourceSessionId,
    sourceTurnId: sourceTurnId ?? null,
  });
}

export function listSessions(): Promise<SessionSummary[]> {
  return invoke<SessionSummary[]>('list_sessions');
}

export function listFolders(): Promise<SessionFolder[]> {
  return invoke<SessionFolder[]>('list_folders');
}

export function setFolderCollapsed(folderId: string, collapsed: boolean): Promise<void> {
  return invoke<void>('set_folder_collapsed', { folderId, collapsed });
}

export function getActiveSession(): Promise<SessionDetail | null> {
  return invoke<SessionDetail | null>('get_active_session');
}

export function getSessionHistory(sessionId: string): Promise<SessionDetail> {
  return invoke<SessionDetail>('get_session_history', { sessionId });
}

export function getBackgroundOperation(operationId: string): Promise<BackgroundOperation | null> {
  return invoke<BackgroundOperation | null>('get_background_operation', { operationId });
}

export function cancelBackgroundOperation(operationId: string): Promise<boolean> {
  return invoke<boolean>('cancel_background_operation', { operationId });
}

export function retryBackgroundOperation(operationId: string): Promise<void> {
  return invoke<void>('retry_background_operation', { operationId });
}

export function listSessionContexts(sessionId: string): Promise<SessionContextRecord[]> {
  return invoke<SessionContextRecord[]>('list_session_contexts', { sessionId });
}

export function addSessionContext(
  sessionId: string,
  sourcePath: string,
): Promise<SessionContextRecord> {
  return invoke<SessionContextRecord>('add_session_context', { sessionId, sourcePath });
}

export function removeSessionContext(sessionId: string, contextId: string): Promise<void> {
  return invoke<void>('remove_session_context', { sessionId, contextId });
}

export function resumeSession(sessionId: string): Promise<string> {
  return invoke<string>('resume_session', { sessionId });
}

// —— 会话管理（变更-12） ——

export function deleteSession(sessionId: string): Promise<void> {
  return invoke<void>('delete_session', { sessionId });
}

export function renameSession(sessionId: string, title: string): Promise<void> {
  return invoke<void>('rename_session', { sessionId, title });
}

export function setSessionPinned(sessionId: string, pinned: boolean): Promise<void> {
  return invoke<void>('set_session_pinned', { sessionId, pinned });
}

export function setSessionArchived(sessionId: string, archived: boolean): Promise<void> {
  return invoke<void>('set_session_archived', { sessionId, archived });
}
