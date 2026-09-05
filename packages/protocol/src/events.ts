// CLI↔UI 流式协议：后端→UI 的归一化事件。
// 这是前端、后端、契约测试共享的唯一定义处（见 docs/技术方案.md 第 4 节）。
// 任何引擎适配器都只能对外产出这里定义的事件，不得泄漏某个 CLI 的原始格式。

import type { Decision } from './adapter';

export type EngineId = 'claude-code' | 'codex';

export type PermissionProfile = 'standard' | 'auto' | 'full_access';
type ExtensibleString<Known extends string> = Known | (string & Record<never, never>);
export type EventServiceTier = ExtensibleString<'standard' | 'batch' | 'flex' | 'priority'>;

export type RuntimeCapabilityAvailability = 'available' | 'unavailable' | 'unknown';

export interface RuntimeCapabilitySnapshot {
  webSearch: RuntimeCapabilityAvailability;
  webFetch: RuntimeCapabilityAvailability;
  approvalContractVersion: string;
  capabilitySnapshotId?: string;
  autoReviewStrategy?: 'unknown' | 'native' | 'compatible' | 'unavailable';
}

export type TurnStage =
  | 'preparing'
  | 'preparing_runtime'
  | 'starting_engine'
  | 'restoring_session'
  | 'waiting_model'
  | 'reasoning'
  | 'using_tool'
  | 'responding'
  | 'finalizing'
  | 'waiting_approval'
  | 'retrying'
  | 'stalled';

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

export type ToolOutcomeKind =
  | 'tool_succeeded'
  | 'auto_review_unavailable'
  | 'auto_review_parse_error'
  | 'auto_review_blocked'
  | 'runtime_denied'
  | 'tool_failed';

export type ToolDenialSource = 'auto_reviewer' | 'runtime' | 'tool';

export type AgentEvent =
  | {
      type: 'session_started';
      sessionId: string;
      engine: EngineId;
      model: string;
      cwd: string;
      ts: number;
      capabilities?: RuntimeCapabilitySnapshot;
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
      /** 结构化终态；缺省仅用于读取旧协议事件。 */
      outcome?: ToolOutcomeKind;
      /** Runtime 是否已开始执行工具。拒绝前失败必须为 false。 */
      started?: boolean;
      hasOutput?: boolean;
      retryable?: boolean;
      denialSource?: ToolDenialSource;
      /** 受控的 Runtime evidence code，不包含工具输入或密钥。 */
      nativeDenialCode?: string;
    }
  | {
      type: 'approval_request';
      sessionId: string;
      id: string;
      action: string;
      detail: string;
      input?: unknown;
      /** 后端对本次请求计算出的唯一合法决定集合。 */
      availableDecisions: Decision[];
      /** 后端根据规范化 matcher 生成，前端不得自行扩大授权范围。 */
      persistentLabel?: string;
      matcherSummary?: string;
    }
  | { type: 'plan_update'; sessionId: string; steps: PlanStep[] }
  | {
      type: 'checkpoint';
      sessionId: string;
      id: string;
      label: string;
      ts: number;
      restorable: boolean;
      fileCount: number;
      reason?: string;
    }
  | {
      type: 'token_usage';
      sessionId: string;
      inputTokens: number;
      cachedInputTokens?: number;
      cacheWriteInputTokens?: number;
      outputTokens: number;
      costUsd: number;
      serviceTier?: EventServiceTier;
      contextWindow?: number;
    }
  | {
      /** 最近一次模型调用的真实输入规模；替换式更新，禁止跨调用累加。 */
      type: 'context_usage';
      sessionId: string;
      contextTokens: number;
      contextWindow?: number;
    }
  | {
      /**
       * Codex 原生上下文压缩生命周期（P0-04）。
       * 由 app-server `contextCompaction` item 的 started/completed 上报。
       * 状态机：submitted（RPC 已提交，等 app-server 确认）→ running → succeeded/failed。
       * summary 为压缩完成后 app-server 返回的真实摘要，缺省不补写虚构内容。
       */
      type: 'context_compaction';
      sessionId: string;
      /** 压缩记录稳定身份（Codex item id；无 item id 时由后端合成）。 */
      id: string;
      status: 'submitted' | 'running' | 'succeeded' | 'failed';
      ts: number;
      /** succeeded 时的真实摘要正文；app-server 未提供则缺省。 */
      summary?: string;
      /** failed 时的真实错误原因。 */
      error?: string;
    }
  | { type: 'turn_complete'; sessionId: string; stopReason: 'end' | 'interrupted' | 'error' }
  | {
      type: 'error';
      sessionId?: string;
      message: string;
      recoverable: boolean;
      kind?: ErrorKind;
      /** kind 为 tool_stalled 时的细分：waiting_approval（有审批等用户）| executing | waiting_result。 */
      stalledKind?: 'waiting_approval' | 'executing' | 'waiting_result' | string;
    };

/**
 * 错误分类：前端据此渲染人话文案与修复动作（去安装 / 去配置 / 选目录…）。
 * 缺省视为 'unknown'。
 */
export type ErrorKind = ExtensibleString<
  | 'not_installed'
  | 'auth_missing'
  | 'version_incompatible'
  | 'cwd_invalid'
  | 'no_binding'
  | 'model_unavailable'
  | 'network'
  | 'process_crash'
  | 'timeout'
  | 'unknown'
>;

export type AgentEventType = AgentEvent['type'];

/**
 * `agent-event` 通道的实际载荷（变更-06）：事件信封。
 * `historyId` 是稳定的线程身份（历史会话 id；新会话即句柄 id），前端一律按它路由——
 * CLI 侧 sessionId 每轮可能变化（Codex 每轮新 id、Claude resume 后可能换发），不能作路由键。
 */
export interface AgentEventEnvelope {
  historyId: string;
  eventSeq?: number;
  turnId?: string;
  turnEpoch?: number;
  attemptNo?: number;
  runtimeGenerationId?: string;
  event: AgentEvent;
}
