// @helm/protocol —— CLI↔UI 流式协议的单一真值。
// 前端、后端、契约测试都从这里 import，不允许各写一份。

export type {
  AgentEvent,
  AgentEventEnvelope,
  AgentEventType,
  ErrorKind,
  EventServiceTier,
  EngineId,
  PermissionProfile,
  RuntimeCapabilityAvailability,
  RuntimeCapabilitySnapshot,
  TurnStage,
  PlanStep,
  Diff,
  DiffHunk,
  DiffLine,
  ToolOutcomeKind,
  ToolDenialSource,
} from './events';
export type {
  AgentCommand,
  CreateSessionArgs,
  SendMessageArgs,
  SetSessionPermissionProfileArgs,
  SetSessionTurnPreferenceArgs,
  TurnMode,
} from './commands';
export type { EngineAdapter, SessionHandle, Decision } from './adapter';
export type {
  CapabilityEvidence,
  CapabilityIdentity,
  CapabilitySet,
  CapabilitySupport,
  EngineCapabilitySnapshot,
} from './capabilities';
export type {
  ReasoningEffort,
  ReasoningEffortCapability,
  ReasoningEffortSupport,
} from './reasoning';
export type {
  AppConfig,
  AuthMethod,
  Binding,
  Engine,
  EquivalentEnvVar,
  FailureCategory,
  Model,
  Protocol,
  Provider,
  ProviderKind,
  ServiceTier,
  PricingBand,
  PricingTier,
  ModelPriceOverride,
  ProviderPricingMode,
  ProviderPricingPreference,
  PricingCatalogStatus,
  ProviderTest,
} from './config';
export type {
  RuntimeOwnerRef,
  RuntimeGeneration,
  NativeSessionRef,
  TurnAttempt,
  TurnAttemptDeliveryState,
  TurnRecoveryInput,
} from './runtime';
export { isAgentEvent, assertAgentEvent } from './validate';
