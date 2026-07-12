// CLI↔UI 流式协议：后端→UI 的归一化事件。
// 这是前端、后端、契约测试共享的唯一定义处（见 docs/技术方案.md 第 4 节）。
// 任何引擎适配器都只能对外产出这里定义的事件，不得泄漏某个 CLI 的原始格式。

export type EngineId = 'claude-code' | 'codex';

export type TurnStage =
  | 'preparing'
  | 'starting_engine'
  | 'restoring_session'
  | 'waiting_model'
  | 'reasoning'
  | 'using_tool'
  | 'responding'
  | 'finalizing'
  | 'waiting_approval'
  | 'retrying';

export interface PlanStep {
  text: string;
  status: 'pending' | 'active' | 'done';
}

export interface DiffLine {
  kind: 'add' | 'del' | 'ctx';
  text: string;
}

export interface DiffHunk {
  oldStart: number;
  newStart: number;
  lines: DiffLine[];
}

export interface Diff {
  path: string;
  hunks: DiffHunk[];
}

export type AgentEvent =
  | {
      type: 'session_started';
      sessionId: string;
      engine: EngineId;
      model: string;
      cwd: string;
      ts: number;
    }
  | { type: 'message_delta'; sessionId: string; role: 'assistant'; text: string }
  | { type: 'message_complete'; sessionId: string; role: 'assistant' | 'user'; text: string }
  | { type: 'thinking_delta'; sessionId: string; text: string }
  | { type: 'thinking_complete'; sessionId: string; text: string }
  | {
      type: 'turn_stage';
      sessionId: string;
      stage: TurnStage;
      ts: number;
      engineReportedTtftMs?: number;
      retryAttempt?: number;
    }
  | {
      type: 'tool_call';
      sessionId: string;
      id: string;
      name: string;
      input: unknown;
      status: 'pending';
    }
  | { type: 'tool_progress'; sessionId: string; id: string; chunk: string }
  | {
      type: 'tool_result';
      sessionId: string;
      id: string;
      status: 'success' | 'error';
      output?: string;
      diff?: Diff;
    }
  | {
      type: 'approval_request';
      sessionId: string;
      id: string;
      action: string;
      detail: string;
      input?: unknown;
    }
  | { type: 'plan_update'; sessionId: string; steps: PlanStep[] }
  | { type: 'checkpoint'; sessionId: string; id: string; label: string; ts: number }
  | {
      type: 'token_usage';
      sessionId: string;
      inputTokens: number;
      outputTokens: number;
      costUsd: number;
      contextWindow?: number;
    }
  | { type: 'turn_complete'; sessionId: string; stopReason: 'end' | 'interrupted' | 'error' }
  | { type: 'error'; sessionId?: string; message: string; recoverable: boolean; kind?: ErrorKind };

/**
 * 错误分类：前端据此渲染人话文案与修复动作（去安装 / 去配置 / 选目录…）。
 * 缺省视为 'unknown'。
 */
export type ErrorKind =
  | 'not_installed'
  | 'auth_missing'
  | 'version_incompatible'
  | 'cwd_invalid'
  | 'no_binding'
  | 'network'
  | 'process_crash'
  | 'timeout'
  | 'unknown';

export type AgentEventType = AgentEvent['type'];

/**
 * `agent-event` 通道的实际载荷（变更-06）：事件信封。
 * `historyId` 是稳定的线程身份（历史会话 id；新会话即句柄 id），前端一律按它路由——
 * CLI 侧 sessionId 每轮可能变化（Codex 每轮新 id、Claude resume 后可能换发），不能作路由键。
 */
export interface AgentEventEnvelope {
  historyId: string;
  event: AgentEvent;
}
