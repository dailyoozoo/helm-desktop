import { invoke } from '@tauri-apps/api/core';
import type { Diff } from '@helm/protocol';
import type { SessionSummary } from './sessionTypes';

export interface SessionMessage {
  role: 'user' | 'assistant';
  text: string;
  ts: number;
  /** 是否被检查点回溯（P2-5）：重开会话时保留淡化视觉 */
  reverted?: boolean;
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
}

export interface SessionCheckpoint {
  id: string;
  label: string;
  ts: number;
}

/** 审批请求持久化记录（变更-07）：pending 的悬空审批在重开会话时重建审批卡 */
export interface SessionApproval {
  id: string;
  action: string;
  detail: string;
  status: 'pending' | 'resolved' | 'expired';
  ts: number;
}

export interface SessionDetail extends SessionSummary {
  messages: SessionMessage[];
  toolCalls: SessionToolCall[];
  checkpoints: SessionCheckpoint[];
  approvals: SessionApproval[];
}

export function listSessions(): Promise<SessionSummary[]> {
  return invoke<SessionSummary[]>('list_sessions');
}

export function getActiveSession(): Promise<SessionDetail | null> {
  return invoke<SessionDetail | null>('get_active_session');
}

export function getSessionHistory(sessionId: string): Promise<SessionDetail> {
  return invoke<SessionDetail>('get_session_history', { sessionId });
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

// —— worktree 隔离（P3-3） ——

export interface WorktreeInfo {
  path: string;
  branch: string;
  /** setup 脚本输出尾部；失败时以 [setup 脚本失败] 开头 */
  setupOutput: string;
}

export function createSessionWorktree(baseCwd: string, name: string): Promise<WorktreeInfo> {
  return invoke<WorktreeInfo>('create_session_worktree', { baseCwd, name });
}

export function removeSessionWorktree(baseCwd: string, worktreePath: string): Promise<void> {
  return invoke<void>('remove_session_worktree', { baseCwd, worktreePath });
}
