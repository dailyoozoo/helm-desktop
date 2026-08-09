// 前端传输层：把 UI 动作映射到 Tauri 命令 / 事件。
// 命令、事件载荷一律用 @helm/protocol 的类型，前后端共享同一份协议（不各写一份）。
// 注意：JS 侧用 camelCase 的参数名（handleId），Tauri 会自动映射到 Rust 的 snake_case 参数（handle_id）。
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import type {
  AgentEventEnvelope,
  Decision,
  EngineId,
  ReasoningEffort,
  ReasoningEffortCapability,
  PermissionProfile,
  CreateSessionArgs,
  SendMessageArgs,
  SetSessionPermissionProfileArgs,
  SetSessionTurnPreferenceArgs,
  TurnMode,
} from '@helm/protocol';

export type { PermissionProfile, TurnMode } from '@helm/protocol';

export type CreateSessionOpts = CreateSessionArgs;

// 创建会话 → 后端 spawn 真实 CLI 进程，返回会话句柄 id（后续 send/interrupt 用它定位进程）。
export function createSession(opts: CreateSessionOpts): Promise<string> {
  const args = {
    engine: opts.engine,
    model: opts.model,
    cwd: opts.cwd,
    reasoningEffort: opts.reasoningEffort,
    mode: opts.mode,
    permissionProfile: opts.permissionProfile,
  } satisfies CreateSessionArgs;
  return invoke<string>('create_session', args);
}

// 会话模式（变更-04）：轮次级属性，随每条消息下发；send 缺省仍兼容回落构建，
// 新会话创建时由 Workspace 显式传入引擎感知的初始模式。
// 发送一条用户消息（后端写入 CLI 进程 stdin）。
// commandText（变更-08）：命中斜杠命令时真正发给 CLI 的文本（透传原样命令或本地展开模板）；
// 缺省等于 text。线程/历史存 text 原文，CLI 收 commandText。
export function sendMessage(
  handleId: string,
  text: string,
  attachments?: string[],
  mode?: TurnMode,
  commandText?: string,
  model?: string,
  reasoningEffort?: ReasoningEffort,
): Promise<void> {
  const args = {
    handleId,
    text: commandText ?? text,
    displayText: text,
    attachments,
    mode,
    model,
    reasoningEffort,
  } satisfies SendMessageArgs;
  return invoke<void>('send_message', args);
}

export function setSessionTurnPreference(
  handleId: string,
  model: string,
  reasoningEffort?: ReasoningEffort,
): Promise<void> {
  const args = { handleId, model, reasoningEffort } satisfies SetSessionTurnPreferenceArgs;
  return invoke<void>('set_session_turn_preference', args);
}

export function setSessionPermissionProfile(
  handleId: string,
  profile: PermissionProfile,
): Promise<void> {
  const args = { handleId, profile } satisfies SetSessionPermissionProfileArgs;
  return invoke<void>('set_session_permission_profile', args);
}

export function getReasoningEffortCapability(
  engine: EngineId,
  model: string,
  providerId?: string,
): Promise<ReasoningEffortCapability> {
  return invoke<ReasoningEffortCapability>('get_reasoning_effort_capability', {
    engine,
    model,
    providerId,
  });
}

// 中断当前轮次（后端杀掉/打断 CLI 进程，应随后发出 turn_complete{interrupted}）。
export function interrupt(handleId: string): Promise<void> {
  return invoke<void>('interrupt', { handleId });
}

export function getTurnSnapshot(handleId: string): Promise<{
  historySessionId: string;
  turnId: string;
  turnEpoch: number;
  status: 'running' | 'waiting_approval' | 'stalled' | 'succeeded' | 'failed' | 'interrupted';
  terminalReason?: string;
  recoverable: boolean;
  eventSeq: number;
  updatedAt: number;
  mode: TurnMode;
  permissionProfile: PermissionProfile;
  startedAt: number;
} | null> {
  return invoke('get_turn_snapshot', { handleId });
}

// 关闭并回收一个会话句柄（后端终止残留进程并从 SessionStore 移除，防止 runtime 泄漏）。
export function closeSession(handleId: string): Promise<void> {
  return invoke<void>('close_session', { handleId });
}

// 回应一个审批请求：当次允许 / 总是允许(本会话) / 拒绝
export function respondApproval(
  handleId: string,
  approvalId: string,
  decision: Decision,
): Promise<void> {
  return invoke<void>('approval_response', { handleId, approvalId, decision });
}

// 会话级 MCP 开关（变更-11）：设置停用名单，下一轮启动 CLI 时真实生效。
export function setSessionMcpDisabled(handleId: string, disabled: string[]): Promise<void> {
  return invoke<void>('set_session_mcp_disabled', { handleId, disabled });
}

// 订阅后端推来的归一化事件流（Rust 侧 app.emit("agent-event", AgentEventEnvelope)）。
// 信封携带 historyId（稳定线程身份），前端按它路由事件（变更-06）。
export function onAgentEvent(cb: (envelope: AgentEventEnvelope) => void): Promise<UnlistenFn> {
  return listen<AgentEventEnvelope>('agent-event', (evt) => cb(evt.payload));
}

// 回溯到某个检查点
export function restoreCheckpoint(checkpointId: string): Promise<void> {
  return invoke<void>('restore_checkpoint', { checkpointId });
}

// 撤销回溯（按内部会话句柄定位——回溯后 CLI 会话 id 已作废）
export function undoRevert(handleId: string): Promise<void> {
  return invoke<void>('undo_revert', { handleId });
}

// 批次 E：Git 只读查询命令

/** Git 工作区状态摘要 */
export interface GitStatus {
  /** 当前分支名（detached HEAD 时为 commit hash 前 7 位） */
  branch: string;
  /** 已修改文件数 */
  modified: number;
  /** 新增文件数（未跟踪） */
  added: number;
  /** 已删除文件数 */
  deleted: number;
}

/** 暂存区文件条目 */
export interface StagedFile {
  /** 文件路径（相对于 cwd） */
  path: string;
  /** 变更类型：Added / Modified / Deleted / Renamed */
  status: string;
}

/** 获取当前分支名 */
export function getGitBranch(cwd: string): Promise<string> {
  return invoke<string>('get_git_branch', { cwd });
}

/** 获取工作区状态：modified / added / deleted 文件数 */
export function getGitStatus(cwd: string): Promise<GitStatus> {
  return invoke<GitStatus>('get_git_status', { cwd });
}

/** 获取暂存区文件列表 */
export function getGitStaged(cwd: string): Promise<StagedFile[]> {
  return invoke<StagedFile[]>('get_git_staged', { cwd });
}

// 变更-33：文件/附件预览

/** 预览内容类型 */
export type PreviewKind = 'text' | 'image' | 'binary';

/** 文件预览结果 */
export interface FilePreview {
  kind: PreviewKind;
  /** Text 时为文本内容；Image 时为原始图片 Bytes 的 base64 */
  content?: string | null;
  /** 图片 MIME */
  mime?: string | null;
  /** 实际文件字节数 */
  size: number;
  /** 是否因超过预览上限被截断 */
  truncated: boolean;
}

/** 软件内只读预览文件（文本/图片）；二进制返回类型标记。 */
export function readFilePreview(path: string): Promise<FilePreview> {
  return invoke<FilePreview>('read_file_preview', { path });
}

/** 用系统默认程序打开文件/目录（供二进制或需要在外部查看的文件使用）。 */
export function openPathInSystem(path: string): Promise<void> {
  return invoke<void>('open_path_in_system', { path });
}
