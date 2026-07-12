// 前端传输层：把 UI 动作映射到 Tauri 命令 / 事件。
// 命令、事件载荷一律用 @helm/protocol 的类型，前后端共享同一份协议（不各写一份）。
// 注意：JS 侧用 camelCase 的参数名（handleId），Tauri 会自动映射到 Rust 的 snake_case 参数（handle_id）。
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import type { AgentEventEnvelope, Decision, EngineId } from '@helm/protocol';

export interface CreateSessionOpts {
  engine: EngineId;
  model: string;
  cwd: string;
}

// 创建会话 → 后端 spawn 真实 CLI 进程，返回会话句柄 id（后续 send/interrupt 用它定位进程）。
export function createSession(opts: CreateSessionOpts): Promise<string> {
  return invoke<string>('create_session', {
    engine: opts.engine,
    model: opts.model,
    cwd: opts.cwd,
  });
}

// 会话模式（变更-04）：轮次级属性，随每条消息下发；缺省回落构建。
export type TurnMode = 'build' | 'plan' | 'ask';

// 发送一条用户消息（后端写入 CLI 进程 stdin）。
// commandText（变更-08）：命中斜杠命令时真正发给 CLI 的文本（透传原样命令或本地展开模板）；
// 缺省等于 text。线程/历史存 text 原文，CLI 收 commandText。
export function sendMessage(
  handleId: string,
  text: string,
  attachments?: string[],
  mode?: TurnMode,
  commandText?: string,
): Promise<void> {
  return invoke<void>('send_message', {
    handleId,
    text: commandText ?? text,
    displayText: text,
    attachments,
    mode,
  });
}

// 中断当前轮次（后端杀掉/打断 CLI 进程，应随后发出 turn_complete{interrupted}）。
export function interrupt(handleId: string): Promise<void> {
  return invoke<void>('interrupt', { handleId });
}

// 关闭并回收一个会话句柄（后端终止残留进程并从 SessionStore 移除，防止 runtime 泄漏）。
export function closeSession(handleId: string): Promise<void> {
  return invoke<void>('close_session', { handleId });
}

// 回应一个审批请求：allow / deny / always
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
