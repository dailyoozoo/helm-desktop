// AgentEvent 的运行时校验器。
// TS 类型在运行时会被擦除，所以契约测试需要一个真正的运行时守卫，
// 用它断言「适配器从真实 CLI 输出解析出来的事件」逐个符合协议。这是「防壳子」的强制手段。

import type { AgentEvent, EngineId, PlanStep, TurnStage } from './events';

function isRecord(x: unknown): x is Record<string, unknown> {
  return typeof x === 'object' && x !== null;
}

function isEngineId(x: unknown): x is EngineId {
  return x === 'claude-code' || x === 'codex';
}

function isPlanStep(x: unknown): x is PlanStep {
  return (
    isRecord(x) &&
    typeof x.text === 'string' &&
    (x.status === 'pending' || x.status === 'active' || x.status === 'done')
  );
}

function isTurnStage(x: unknown): x is TurnStage {
  return (
    x === 'preparing' ||
    x === 'preparing_runtime' ||
    x === 'starting_engine' ||
    x === 'restoring_session' ||
    x === 'waiting_model' ||
    x === 'reasoning' ||
    x === 'using_tool' ||
    x === 'responding' ||
    x === 'finalizing' ||
    x === 'waiting_approval' ||
    x === 'retrying' ||
    x === 'stalled'
  );
}

function isDecision(x: unknown): boolean {
  return (
    x === 'allow' ||
    x === 'turn' ||
    x === 'session' ||
    x === 'project' ||
    x === 'always' ||
    x === 'deny'
  );
}

/** 运行时判断一个未知值是否为合法的 AgentEvent（含必填字段与类型）。 */
export function isAgentEvent(x: unknown): x is AgentEvent {
  if (!isRecord(x) || typeof x.type !== 'string') return false;

  const str = (k: string): boolean => typeof x[k] === 'string';
  const strNonEmpty = (k: string): boolean =>
    typeof x[k] === 'string' && (x[k] as string).length > 0;
  const num = (k: string): boolean => typeof x[k] === 'number' && Number.isFinite(x[k] as number);

  switch (x.type) {
    case 'session_started':
      return (
        strNonEmpty('sessionId') &&
        isEngineId(x.engine) &&
        str('model') &&
        str('cwd') &&
        num('ts') &&
        (x.capabilities === undefined ||
          (isRecord(x.capabilities) &&
            ['available', 'unavailable', 'unknown'].includes(String(x.capabilities.webSearch)) &&
            ['available', 'unavailable', 'unknown'].includes(String(x.capabilities.webFetch)) &&
            typeof x.capabilities.approvalContractVersion === 'string'))
      );
    case 'message_delta':
      return strNonEmpty('sessionId') && x.role === 'assistant' && str('text');
    case 'message_complete':
      return (
        strNonEmpty('sessionId') && (x.role === 'assistant' || x.role === 'user') && str('text')
      );
    case 'thinking_delta':
    case 'thinking_complete':
      return strNonEmpty('sessionId') && str('text');
    case 'turn_stage':
      return (
        strNonEmpty('sessionId') &&
        isTurnStage(x.stage) &&
        num('ts') &&
        (!('engineReportedTtftMs' in x) ||
          (num('engineReportedTtftMs') && (x.engineReportedTtftMs as number) >= 0)) &&
        (!('retryAttempt' in x) ||
          (Number.isSafeInteger(x.retryAttempt) && (x.retryAttempt as number) >= 1))
      );
    case 'tool_call':
      return (
        strNonEmpty('sessionId') &&
        strNonEmpty('id') &&
        str('name') &&
        'input' in x &&
        x.status === 'pending'
      );
    case 'tool_progress':
      return strNonEmpty('sessionId') && strNonEmpty('id') && str('chunk');
    case 'tool_result':
      return (
        strNonEmpty('sessionId') &&
        strNonEmpty('id') &&
        (x.status === 'success' || x.status === 'error')
      );
    case 'approval_request':
      return (
        strNonEmpty('sessionId') &&
        strNonEmpty('id') &&
        str('action') &&
        str('detail') &&
        Array.isArray(x.availableDecisions) &&
        x.availableDecisions.length >= 2 &&
        new Set(x.availableDecisions).size === x.availableDecisions.length &&
        x.availableDecisions.every(isDecision) &&
        x.availableDecisions.includes('allow') &&
        x.availableDecisions.includes('deny') &&
        (!('persistentLabel' in x) || str('persistentLabel')) &&
        (!('matcherSummary' in x) || str('matcherSummary'))
      );
    case 'plan_update':
      return strNonEmpty('sessionId') && Array.isArray(x.steps) && x.steps.every(isPlanStep);
    case 'checkpoint':
      return strNonEmpty('sessionId') && strNonEmpty('id') && str('label') && num('ts');
    case 'token_usage':
      return (
        strNonEmpty('sessionId') &&
        num('inputTokens') &&
        (!('cachedInputTokens' in x) || num('cachedInputTokens')) &&
        (!('cacheWriteInputTokens' in x) || num('cacheWriteInputTokens')) &&
        num('outputTokens') &&
        num('costUsd') &&
        (!('serviceTier' in x) || strNonEmpty('serviceTier')) &&
        (!('contextWindow' in x) || num('contextWindow'))
      );
    case 'context_usage':
      return (
        str('sessionId') &&
        num('contextTokens') &&
        (!('contextWindow' in x) || num('contextWindow'))
      );
    case 'turn_complete':
      return (
        strNonEmpty('sessionId') &&
        (x.stopReason === 'end' || x.stopReason === 'interrupted' || x.stopReason === 'error')
      );
    case 'error':
      return (
        str('message') &&
        typeof x.recoverable === 'boolean' &&
        (!('kind' in x) || strNonEmpty('kind'))
      );
    default:
      return false;
  }
}

/** 校验失败时抛出带细节的错误，便于测试与调试定位到具体哪条事件不合法。 */
export function assertAgentEvent(x: unknown): asserts x is AgentEvent {
  if (!isAgentEvent(x)) {
    throw new Error(`不符合 AgentEvent 协议: ${JSON.stringify(x)}`);
  }
}
