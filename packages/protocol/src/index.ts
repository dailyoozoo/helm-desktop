// @helm/protocol —— CLI↔UI 流式协议的单一真值。
// 前端、后端、契约测试都从这里 import，不允许各写一份。

export type {
  AgentEvent,
  AgentEventEnvelope,
  AgentEventType,
  EngineId,
  TurnStage,
  PlanStep,
  Diff,
  DiffHunk,
  DiffLine,
} from './events';
export type { AgentCommand } from './commands';
export type { EngineAdapter, SessionHandle, Decision } from './adapter';
export type {
  AppConfig,
  AuthMethod,
  Binding,
  Engine,
  EquivalentEnvVar,
  Model,
  Protocol,
  Provider,
  ProviderTest,
} from './config';
export { isAgentEvent, assertAgentEvent } from './validate';
