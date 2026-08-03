// 引擎适配器契约（见 docs/技术方案.md 第 4.3 节）。
// 每种 CLI 引擎实现一遍。适配器的灵魂是 parseLine：把该 CLI 的原始 stdout 行
// 解析为归一化的 AgentEvent。UI 永远不直接碰 CLI。

import type { AgentEvent, EngineId } from './events';

export type Decision = 'allow' | 'turn' | 'session' | 'project' | 'always' | 'deny';

/** 一个已启动会话的句柄。具体引擎可在此基础上扩展（如挂上子进程与事件流）。 */
export interface SessionHandle {
  sessionId: string;
  engine: EngineId;
}

export interface EngineAdapter {
  readonly id: EngineId;
  start(opts: { model: string; cwd: string; resumeId?: string }): Promise<SessionHandle>;
  send(handle: SessionHandle, text: string, attachments?: string[]): void;
  approve(handle: SessionHandle, requestId: string, decision: Decision): void;
  interrupt(handle: SessionHandle): void;
  /** 把该 CLI 的原始 stdout 行解析为归一化事件。这是适配器的灵魂。 */
  parseLine(raw: string): AgentEvent[];
  stop(handle: SessionHandle): Promise<void>;
}
