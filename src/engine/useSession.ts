// useSession：消费后端的 AgentEvent 流，归约成「线程状态」供 UI 渲染。
// 红线：助手内容只来自真实进程事件（message_delta/complete），这里不造任何假数据。
// 事件路由（变更-06）：后端信封携带 historyId（稳定线程身份），前端一律按它路由；
// CLI 侧 sessionId 每轮可能变化（Codex 每轮新 id），只作展示/关联用，不作路由键。
import { useCallback, useEffect, useReducer, useRef } from 'react';
import type {
  AgentEvent,
  AgentEventEnvelope,
  Decision,
  Diff,
  EngineId,
  ErrorKind,
  PlanStep,
  ReasoningEffort,
  RuntimeCapabilitySnapshot,
  TurnStage,
} from '@helm/protocol';
import { showToast } from '../components/toast';
import {
  closeSession,
  createSession,
  getTurnSnapshot,
  interrupt,
  onAgentEvent,
  respondApproval,
  restoreCheckpoint,
  sendMessage,
  setSessionMcpDisabled,
  setSessionTurnPreference,
  undoRevert,
  type TurnMode,
  type PermissionProfile,
} from './transport';
import type { SessionDetail, SessionForkSummary } from '../sessions/api';
import { consumePendingResume, type ResumePayload } from '../sessions/resumeBridge';
import type { SessionDefaults } from '../settings/settingsViewModel';

export interface TurnActivity {
  stage: TurnStage;
  since: number;
  toolName?: string;
  target?: string;
  retryAttempt?: number;
  engineReportedTtftMs?: number;
}

export type ApprovalUiStatus = 'pending' | 'applying' | 'resolved' | 'failed';

// ---------------------------------------------------------------------------
// 并行会话注册表（P3-3 / 变更-06）：history 会话 id → { 句柄, 是否有轮次在跑 }。
// 切走时若轮次还在跑则保留句柄（进程后台跑完、事件照常写入历史库），重开时复用续聊。
// working 状态由全局事件监听驱动（turn_complete/error 置空闲），侧栏「运行中」标记
// 通过 subscribeLiveSessions 订阅变化，杜绝「只增不删」的永久误报。
// ---------------------------------------------------------------------------

interface LiveSessionEntry {
  handleId: string;
  working: boolean;
  activity: TurnActivity | null;
  /** 有待处理的审批请求（变更-12）：侧栏黄色徽标 */
  pendingApproval?: boolean;
}

const liveSessions = new Map<string, LiveSessionEntry>();
const liveListeners = new Set<() => void>();
let liveWorkingSnapshot: string[] = [];
let liveApprovalSnapshot: string[] = [];

function sameSnapshot(current: string[], next: string[]): boolean {
  return current.length === next.length && current.every((value, index) => value === next[index]);
}

function notifyLiveChanged(): void {
  const nextWorkingSnapshot = [...liveSessions.entries()]
    .filter(([, entry]) => entry.working)
    .map(([id]) => id);
  const nextApprovalSnapshot = [...liveSessions.entries()]
    .filter(([, entry]) => entry.pendingApproval)
    .map(([id]) => id);
  const workingChanged = !sameSnapshot(liveWorkingSnapshot, nextWorkingSnapshot);
  const approvalChanged = !sameSnapshot(liveApprovalSnapshot, nextApprovalSnapshot);
  if (!workingChanged && !approvalChanged) return;
  if (workingChanged) liveWorkingSnapshot = nextWorkingSnapshot;
  if (approvalChanged) liveApprovalSnapshot = nextApprovalSnapshot;
  for (const listener of liveListeners) listener();
}

/** 订阅注册表变化（配合 useSyncExternalStore 驱动侧栏「运行中」标记重渲染） */
export function subscribeLiveSessions(listener: () => void): () => void {
  liveListeners.add(listener);
  return () => {
    liveListeners.delete(listener);
  };
}

/** 当前有轮次在跑的历史会话 id 列表（稳定快照引用，useSyncExternalStore 用） */
export function liveWorkingSessionIds(): string[] {
  return liveWorkingSnapshot;
}

/** 有待处理审批的历史会话 id 列表（侧栏「待审批」徽标，变更-12） */
export function livePendingApprovalSessionIds(): string[] {
  return liveApprovalSnapshot;
}

/** 删除会话后清掉注册表条目（变更-12：后端已回收句柄，前端不再当作存活） */
export function dropLiveSession(historyId: string): void {
  removeLiveSession(historyId);
}

/** 某个历史会话是否还有存活句柄（可直接复用，不必 resume_session 重启） */
export function liveSessionHandle(historySessionId: string): string | null {
  return liveSessions.get(historySessionId)?.handleId ?? null;
}

/** 某个历史会话是否有轮次正在跑 */
export function liveSessionWorking(historySessionId: string): boolean {
  return liveSessions.get(historySessionId)?.working ?? false;
}

export function liveSessionActivity(historySessionId: string): TurnActivity | null {
  return liveSessions.get(historySessionId)?.activity ?? null;
}

export function registerLiveSession(
  historyId: string,
  handleId: string,
  working: boolean,
  activity: TurnActivity | null = null,
): void {
  const existing = liveSessions.get(historyId);
  const sameHandle = existing?.handleId === handleId;
  liveSessions.set(historyId, {
    handleId,
    working,
    activity: sameHandle ? existing.activity : activity,
    pendingApproval: sameHandle ? existing.pendingApproval : false,
  });
  notifyLiveChanged();
}

function setLiveSessionWorking(historyId: string, working: boolean): void {
  const entry = liveSessions.get(historyId);
  if (!entry) return;
  const changed = entry.working !== working || (!working && entry.activity !== null);
  if (!changed) return;
  entry.working = working;
  if (!working) entry.activity = null;
  notifyLiveChanged();
}

function removeLiveSession(historyId: string): void {
  if (liveSessions.delete(historyId)) notifyLiveChanged();
}

/** 仅供测试：清空注册表 */
export function resetLiveSessionsForTests(): void {
  liveSessions.clear();
  notifyLiveChanged();
}

/**
 * 清扫空闲句柄（变更-06）：切换会话时回收「既不是当前会话、也没有轮次在跑」的
 * 后端运行时，防止 SessionStore 随切页/切会话无限累积（可靠性检查 A10）。
 * 运行中的句柄保留（P3-3 并行会话保活）。
 */
function sweepIdleHandles(exceptHistoryId: string | null): void {
  for (const [historyId, entry] of [...liveSessions]) {
    if (historyId === exceptHistoryId || entry.working) continue;
    liveSessions.delete(historyId);
    closeSession(entry.handleId).catch(() => {
      // 后端已回收或非 Tauri 环境，忽略
    });
  }
  notifyLiveChanged();
}

/**
 * 注册表随事件流自愈：轮次终态置空闲；有输出/新轮次视为运行中。
 * 对所有会话生效（含后台并行会话），与具体视图无关。
 */
export function applyEnvelopeToLiveRegistry(envelope: AgentEventEnvelope): void {
  const entry = liveSessions.get(envelope.historyId);
  if (!entry) return;
  const event = envelope.event;
  if (event.type === 'turn_complete' || event.type === 'error') {
    entry.working = false;
    entry.activity = null;
    entry.pendingApproval = false;
    notifyLiveChanged();
    return;
  }
  if (event.type === 'approval_request') {
    entry.working = true;
    entry.activity = {
      stage: 'waiting_approval',
      since: Date.now(),
    };
    entry.pendingApproval = true;
    notifyLiveChanged();
    return;
  }
  if (event.type === 'turn_stage') {
    entry.activity = {
      stage: event.stage,
      since: event.ts,
      ...(event.retryAttempt !== undefined ? { retryAttempt: event.retryAttempt } : {}),
      ...(event.engineReportedTtftMs !== undefined
        ? { engineReportedTtftMs: event.engineReportedTtftMs }
        : {}),
    };
  } else if (event.type === 'thinking_delta' || event.type === 'thinking_complete') {
    const since = entry.activity?.stage === 'reasoning' ? entry.activity.since : Date.now();
    entry.activity = { stage: 'reasoning', since };
  } else if (event.type === 'tool_call') {
    const target = toolActivityTarget(event.name, event.input);
    entry.activity = {
      stage: 'using_tool',
      since: Date.now(),
      toolName: event.name,
      ...(target ? { target } : {}),
    };
  } else if (event.type === 'message_delta' || event.type === 'message_complete') {
    const since = entry.activity?.stage === 'responding' ? entry.activity.since : Date.now();
    entry.activity = { stage: 'responding', since };
  }
  if (
    event.type === 'session_started' ||
    event.type === 'turn_stage' ||
    event.type === 'message_delta' ||
    event.type === 'message_complete' ||
    event.type === 'thinking_delta' ||
    event.type === 'thinking_complete' ||
    event.type === 'tool_call'
  ) {
    const wasWorking = entry.working;
    const wasPending = entry.pendingApproval;
    entry.working = true;
    entry.pendingApproval = false;
    // 有新输出说明审批已被处理（恢复轮开始），徽标撤下
    if (!wasWorking || wasPending || entry.activity) notifyLiveChanged();
  }
}

// 工作区最后打开的会话（变更-06）：切页卸载后回来时复用存活句柄恢复线程，
// 避免「切页回来线程一片空白 + 空闲句柄永久滞留」。
let lastWorkspaceSessionId: string | null = null;

export function lastOpenWorkspaceSession(): string | null {
  return lastWorkspaceSessionId;
}

function rememberLastWorkspaceSession(historyId: string | null): void {
  lastWorkspaceSessionId = historyId;
}

// ---------------------------------------------------------------------------
// 全局事件监听（单例）：不随组件卸载退订，保证后台会话的 working 状态
// 在任何页面都能被 turn_complete 归位；组件级监听只负责往当前线程分发。
// ---------------------------------------------------------------------------

type EnvelopeListener = (envelope: AgentEventEnvelope) => void;
const envelopeListeners = new Set<EnvelopeListener>();
let globalListenerStarted = false;

function dispatchEnvelope(envelope: AgentEventEnvelope): void {
  applyEnvelopeToLiveRegistry(envelope);
  for (const listener of envelopeListeners) listener(envelope);
}

function ensureGlobalAgentListener(): void {
  if (globalListenerStarted) return;
  globalListenerStarted = true;
  onAgentEvent(dispatchEnvelope).catch(() => {
    // 浏览器预览（无 Tauri）下没有事件桥，保持静态空态即可；允许后续重试
    globalListenerStarted = false;
  });
}

/// 放弃一个句柄：轮次进行中 → 挂后台保活（P3-3 并行会话）；空闲 → 通知后端回收。
/// working 判定读注册表（由事件流驱动），不再依赖组件渲染期状态，
/// 修复「重开运行中会话被错置 idle 后、再切走时误杀后台轮次」。
function releaseHandle(handleId: string, historyId: string | null): void {
  if (historyId && liveSessions.get(historyId)?.working) {
    // 保活：注册表已记录句柄与运行状态，事件继续经后端写入历史库
    return;
  }
  if (historyId) removeLiveSession(historyId);
  closeSession(handleId).catch(() => {
    // 非 Tauri 环境（浏览器预览）或后端已回收，忽略
  });
}

type ThreadItemMeta = {
  turnId?: string;
  startedAt?: number;
  endedAt?: number;
  turnStatus?: 'succeeded' | 'failed' | 'interrupted';
};

export type ThreadItem =
  | ({
      kind: 'user';
      id: string;
      text: string;
      attachments?: string[];
      /** 会话模式（变更-04）：计划/询问轮次在消息旁显示模式徽标；构建不标 */
      mode?: TurnMode;
      permissionProfile?: PermissionProfile;
      reverted?: boolean;
    } & ThreadItemMeta)
  | ({
      kind: 'assistant';
      id: string;
      text: string;
      reverted?: boolean;
      interrupted?: boolean;
    } & ThreadItemMeta)
  | ({
      kind: 'thinking';
      id: string;
      text: string;
      done: boolean;
      reverted?: boolean;
    } & ThreadItemMeta)
  | ({
      kind: 'tool';
      id: string;
      name: string;
      input: unknown;
      status: 'pending' | 'success' | 'error';
      output?: string;
      diff?: Diff;
      outcome?: import('@helm/protocol').ToolOutcomeKind;
      started?: boolean;
      hasOutput?: boolean;
      retryable?: boolean;
      denialSource?: import('@helm/protocol').ToolDenialSource;
      nativeDenialCode?: string;
      reverted?: boolean;
    } & ThreadItemMeta)
  | ({
      kind: 'approval';
      id: string;
      action: string;
      detail: string;
      status: ApprovalUiStatus;
      error?: string;
      availableDecisions: Decision[];
      decision?: Decision;
      persistentLabel?: string;
      matcherSummary?: string;
      reverted?: boolean;
    } & ThreadItemMeta)
  | ({ kind: 'plan'; id: string; steps: PlanStep[]; reverted?: boolean } & ThreadItemMeta)
  | ({
      kind: 'checkpoint';
      id: string;
      label: string;
      ts: number;
      restored: boolean;
      restorable: boolean;
      fileCount: number;
      reason?: string;
    } & ThreadItemMeta)
  | ({ kind: 'error'; id: string; message: string; errorKind?: string } & ThreadItemMeta);

export interface SessionState {
  handleId: string | null;
  /** 稳定线程身份：历史会话 id（新会话即句柄 id），事件路由键（变更-06） */
  historyId: string | null;
  sessionId: string | null;
  engine: EngineId;
  model: string;
  /** 最近一次 SessionStarted 事件携带的路由模型，与下一轮偏好分离。 */
  runtimeModel?: string;
  cwd: string;
  status: 'idle' | 'working';
  items: ThreadItem[];
  openAssistantId: string | null;
  openThinkingId: string | null;
  cost: {
    inputTokens: number;
    cachedInputTokens?: number;
    cacheWriteInputTokens?: number;
    outputTokens: number;
    costUsd: number;
    contextTokens?: number;
    contextWindow?: number;
  };
  startedAt: number | null;
  turnActivity: TurnActivity | null;
  turnStartedAt: number | null;
  /** 会话级停用的 MCP 服务器（变更-11）：下一轮生效，新建/切换会话时清空 */
  disabledMcp: string[];
  runtimeCapabilities?: RuntimeCapabilitySnapshot;
  fork?: SessionForkSummary | null;
}

type Action =
  | { type: 'event'; event: AgentEvent; turnId?: string }
  | {
      type: 'send';
      id: string;
      text: string;
      attachments?: string[];
      mode?: TurnMode;
      permissionProfile?: PermissionProfile;
    }
  | { type: 'handle'; handleId: string }
  | { type: 'idle' }
  | { type: 'reset'; defaults?: SessionDefaults }
  | { type: 'apply_defaults'; defaults: SessionDefaults }
  | { type: 'select_engine'; engine: EngineId; model: string }
  | { type: 'select_model'; model: string }
  | { type: 'approval_applying'; approvalId: string }
  | { type: 'approval_resolved'; approvalId: string; decision: Decision }
  | { type: 'approval_failed'; approvalId: string; error: string }
  | { type: 'working' }
  | { type: 'restore_checkpoint'; checkpointId: string }
  | { type: 'undo_revert' }
  | { type: 'set_cwd'; cwd: string }
  | { type: 'set_disabled_mcp'; disabled: string[] }
  | {
      type: 'resume_handle';
      handleId: string;
      historyId: string;
      working: boolean;
      activity?: TurnActivity | null;
      detail?: SessionDetail;
    };

const STORAGE_KEY = 'helm.workspace.currentSession';

interface StoredSelection {
  engine?: EngineId;
  model?: string;
}

function readStoredSelection(): StoredSelection {
  if (typeof localStorage === 'undefined') return {};
  try {
    const parsed = JSON.parse(localStorage.getItem(STORAGE_KEY) ?? '{}') as StoredSelection;
    return parsed && typeof parsed === 'object' ? parsed : {};
  } catch {
    // 豁免提示：localStorage 里是旧版本/损坏数据时回退空选择即可，无需打扰用户
    return {};
  }
}

function writeStoredSelection(engine: EngineId, model: string) {
  if (typeof localStorage === 'undefined') return;
  localStorage.setItem(STORAGE_KEY, JSON.stringify({ engine, model }));
}

function initialState(defaults?: SessionDefaults): SessionState {
  const stored = readStoredSelection();
  const engine = defaults?.engine ?? stored.engine ?? 'claude-code';
  return {
    handleId: null,
    historyId: null,
    sessionId: null,
    engine,
    model: stored.engine === engine ? (stored.model ?? '') : '',
    runtimeModel: undefined,
    cwd: defaults?.cwd ?? '',
    status: 'idle',
    items: [],
    openAssistantId: null,
    openThinkingId: null,
    cost: { inputTokens: 0, outputTokens: 0, costUsd: 0 },
    startedAt: null,
    turnActivity: null,
    turnStartedAt: null,
    disabledMcp: [],
    fork: null,
  };
}

export function resetSessionState(_state: SessionState, defaults: SessionDefaults): SessionState {
  return {
    handleId: null,
    historyId: null,
    sessionId: null,
    engine: defaults.engine,
    model: '',
    runtimeModel: undefined,
    cwd: defaults.cwd,
    status: 'idle',
    items: [],
    openAssistantId: null,
    openThinkingId: null,
    cost: { inputTokens: 0, outputTokens: 0, costUsd: 0 },
    startedAt: null,
    turnActivity: null,
    turnStartedAt: null,
    disabledMcp: [],
    fork: null,
  };
}

function uid(prefix: string): string {
  return `${prefix}-${crypto.randomUUID()}`;
}

function findLastIndex<T>(items: T[], predicate: (item: T) => boolean): number {
  for (let i = items.length - 1; i >= 0; i -= 1) {
    if (predicate(items[i])) return i;
  }
  return -1;
}

/** 把仍在流式中的 thinking 项落定为 done（正文/工具块开始即思考流结束，变更-09 S2） */
function closeOpenThinking(items: ThreadItem[], openThinkingId: string | null): ThreadItem[] {
  if (!openThinkingId) return items;
  return items.map((it) =>
    it.kind === 'thinking' && it.id === openThinkingId && !it.done
      ? { ...it, done: true, ...(it.startedAt ? { endedAt: Date.now() } : {}) }
      : it,
  );
}

function safeActivityTarget(value: unknown): string | undefined {
  if (typeof value !== 'string') return undefined;
  const normalized = Array.from(value, (char) => {
    const code = char.charCodeAt(0);
    return code <= 31 || code === 127 ? ' ' : char;
  })
    .join('')
    .replace(/\s+/g, ' ')
    .trim();
  if (!normalized) return undefined;
  if (/(?:api[_-]?key|access[_-]?token|password|secret)\s*[:=]/i.test(normalized)) {
    return undefined;
  }
  return normalized.length > 100 ? `${normalized.slice(0, 99)}…` : normalized;
}

function toolActivityTarget(name: string, input: unknown): string | undefined {
  if (!input || typeof input !== 'object' || Array.isArray(input)) return undefined;
  const values = input as Record<string, unknown>;
  const keys =
    name === 'Grep'
      ? ['pattern', 'query', 'path']
      : name === 'Glob'
        ? ['pattern', 'path']
        : name === 'Read' || name === 'Edit' || name === 'Write'
          ? ['file_path', 'filePath', 'path']
          : [];
  for (const key of keys) {
    const target = safeActivityTarget(values[key]);
    if (target) return target;
  }
  return undefined;
}

function derivedActivity(
  s: SessionState,
  stage: TurnStage,
  details: Partial<NonNullable<SessionState['turnActivity']>> = {},
  resetSince = false,
): Pick<SessionState, 'turnActivity' | 'turnStartedAt'> {
  const now = Date.now();
  const since = !resetSince && s.turnActivity?.stage === stage ? s.turnActivity.since : now;
  return {
    turnActivity: { stage, since, ...details },
    turnStartedAt: s.turnStartedAt ?? now,
  };
}

/**
 * 事件路由（变更-06）：只消费信封 historyId 与当前线程一致的事件。
 * historyId 跨轮稳定（Codex 每轮换 CLI session id、Claude resume 换发 id 均不受影响），
 * 未绑定线程（historyId 为 null）不消费任何事件——不存在「认领」竞态。
 */
export function shouldConsumeAgentEvent(
  currentHistoryId: string | null,
  envelope: AgentEventEnvelope,
): boolean {
  return currentHistoryId !== null && envelope.historyId === currentHistoryId;
}

export function reduceSessionEvent(s: SessionState, e: AgentEvent, turnId?: string): SessionState {
  switch (e.type) {
    case 'session_started':
      return {
        ...s,
        sessionId: e.sessionId,
        engine: e.engine,
        runtimeModel: e.model || s.runtimeModel,
        cwd: e.cwd,
        runtimeCapabilities: e.capabilities,
      };

    case 'turn_stage':
      return {
        ...s,
        status: 'working',
        turnActivity: {
          stage: e.stage,
          since: e.ts,
          ...(e.retryAttempt !== undefined ? { retryAttempt: e.retryAttempt } : {}),
          ...(e.engineReportedTtftMs !== undefined
            ? { engineReportedTtftMs: e.engineReportedTtftMs }
            : {}),
        },
        turnStartedAt: s.turnStartedAt ?? e.ts,
      };

    case 'message_delta': {
      const activity = derivedActivity(s, 'responding');
      if (s.openAssistantId) {
        return {
          ...s,
          ...activity,
          items: s.items.map((it) =>
            it.kind === 'assistant' && it.id === s.openAssistantId
              ? { ...it, text: it.text + e.text }
              : it,
          ),
        };
      }
      const id = uid('a');
      return {
        ...s,
        ...activity,
        openAssistantId: id,
        openThinkingId: null,
        // thinking → 正文切换：思考流已结束，把打开的 thinking 项落定为 done，
        // 否则它会永远显示「正在思考」且轮末 ThinkingComplete 会重复建项（变更-09 S2）
        items: [
          ...closeOpenThinking(s.items, s.openThinkingId),
          { kind: 'assistant', id, text: e.text, ...(turnId ? { turnId } : {}) },
        ],
      };
    }

    case 'message_complete': {
      if (e.role !== 'assistant') return s;
      const activity = derivedActivity(s, 'responding');
      if (s.openAssistantId) {
        const open = s.openAssistantId;
        return {
          ...s,
          ...activity,
          openAssistantId: null,
          items: s.items.map((it) =>
            it.kind === 'assistant' && it.id === open ? { ...it, text: e.text } : it,
          ),
        };
      }
      // 双发去重（变更-09）：轮末整条 assistant 消息与流式已收完的内容一致时不再追加
      const last = s.items[s.items.length - 1];
      if (last && last.kind === 'assistant' && last.text === e.text) return { ...s, ...activity };
      return {
        ...s,
        ...activity,
        items: [
          ...s.items,
          { kind: 'assistant', id: uid('a'), text: e.text, ...(turnId ? { turnId } : {}) },
        ],
      };
    }

    case 'thinking_delta': {
      const activity =
        s.turnStartedAt !== null || s.status === 'working' ? derivedActivity(s, 'reasoning') : {};
      if (s.openThinkingId) {
        return {
          ...s,
          ...activity,
          items: s.items.map((it) =>
            it.kind === 'thinking' && it.id === s.openThinkingId
              ? { ...it, text: it.text + e.text }
              : it,
          ),
        };
      }
      const id = uid('th');
      return {
        ...s,
        ...activity,
        openAssistantId: null,
        openThinkingId: id,
        items: [
          ...s.items,
          {
            kind: 'thinking',
            id,
            text: e.text,
            done: false,
            ...(turnId ? { turnId, startedAt: Date.now() } : {}),
          },
        ],
      };
    }

    case 'thinking_complete': {
      const activity =
        s.turnStartedAt !== null || s.status === 'working' ? derivedActivity(s, 'reasoning') : {};
      if (s.openThinkingId) {
        const open = s.openThinkingId;
        return {
          ...s,
          ...activity,
          openThinkingId: null,
          items: s.items.map((it) =>
            it.kind === 'thinking' && it.id === open
              ? {
                  ...it,
                  text: e.text,
                  done: true,
                  ...(it.startedAt ? { endedAt: Date.now() } : {}),
                }
              : it,
          ),
        };
      }
      // 流式项已被正文/工具关闭（变更-09 S2）：优先合并回最后一个未落定的 thinking 项；
      // 内容与已有项一致则视为轮末重放，去重跳过；都不是才追加新项
      const openIndex = findLastIndex(s.items, (it) => it.kind === 'thinking' && !it.done);
      if (openIndex >= 0) {
        return {
          ...s,
          ...activity,
          items: s.items.map((it, i) =>
            i === openIndex && it.kind === 'thinking'
              ? {
                  ...it,
                  text: e.text,
                  done: true,
                  ...(it.startedAt ? { endedAt: Date.now() } : {}),
                }
              : it,
          ),
        };
      }
      if (s.items.some((it) => it.kind === 'thinking' && it.text === e.text)) {
        return { ...s, ...activity };
      }
      return {
        ...s,
        ...activity,
        items: [
          ...s.items,
          {
            kind: 'thinking',
            id: uid('th'),
            text: e.text,
            done: true,
            ...(turnId ? { turnId } : {}),
          },
        ],
      };
    }

    case 'tool_call': {
      const target = toolActivityTarget(e.name, e.input);
      return {
        ...s,
        ...derivedActivity(
          s,
          'using_tool',
          {
            toolName: e.name,
            ...(target ? { target } : {}),
          },
          true,
        ),
        openAssistantId: null,
        openThinkingId: null,
        items: [
          ...closeOpenThinking(s.items, s.openThinkingId),
          {
            kind: 'tool',
            id: e.id,
            name: e.name,
            input: e.input,
            status: 'pending',
            ...(turnId ? { turnId, startedAt: Date.now() } : {}),
          },
        ],
      };
    }

    case 'tool_progress':
      return {
        ...s,
        items: s.items.map((it) =>
          it.kind === 'tool' && it.id === e.id
            ? { ...it, output: (it.output ?? '') + e.chunk }
            : it,
        ),
      };

    case 'tool_result':
      return {
        ...s,
        items: s.items.map((it) =>
          it.kind === 'tool' && it.id === e.id
            ? {
                ...it,
                status: e.status,
                output: e.output ?? it.output,
                diff: e.diff ?? it.diff,
                outcome: e.outcome,
                started: e.started,
                hasOutput: e.hasOutput,
                retryable: e.retryable,
                denialSource: e.denialSource,
                nativeDenialCode: e.nativeDenialCode,
                ...(it.startedAt ? { endedAt: Date.now() } : {}),
              }
            : it,
        ),
      };

    case 'token_usage':
      return {
        ...s,
        cost: {
          inputTokens: s.cost.inputTokens + e.inputTokens,
          outputTokens: s.cost.outputTokens + e.outputTokens,
          costUsd: s.cost.costUsd + e.costUsd,
          contextWindow: e.contextWindow ?? s.cost.contextWindow,
          ...(e.cachedInputTokens !== undefined || s.cost.cachedInputTokens !== undefined
            ? { cachedInputTokens: (s.cost.cachedInputTokens ?? 0) + (e.cachedInputTokens ?? 0) }
            : {}),
          ...(e.cacheWriteInputTokens !== undefined || s.cost.cacheWriteInputTokens !== undefined
            ? {
                cacheWriteInputTokens:
                  (s.cost.cacheWriteInputTokens ?? 0) + (e.cacheWriteInputTokens ?? 0),
              }
            : {}),
        },
      };

    case 'context_usage':
      return {
        ...s,
        cost: {
          ...s.cost,
          contextTokens: e.contextTokens,
          contextWindow: e.contextWindow ?? s.cost.contextWindow,
        },
      };

    case 'turn_complete': {
      // 中断标注（变更-09）：半截流式消息补「已停止」标记，不再静默留存
      const interrupted = e.stopReason === 'interrupted';
      const turnStatus: NonNullable<ThreadItemMeta['turnStatus']> =
        e.stopReason === 'end'
          ? 'succeeded'
          : e.stopReason === 'interrupted'
            ? 'interrupted'
            : 'failed';
      const items = s.items.map((it) => {
        const terminal = turnId && it.turnId === turnId ? { turnStatus } : {};
        if (interrupted && it.kind === 'assistant' && it.id === s.openAssistantId) {
          return { ...it, ...terminal, interrupted: true };
        }
        if (it.kind === 'tool' && it.status === 'pending') {
          return {
            ...it,
            ...terminal,
            status: 'error' as const,
            output:
              e.stopReason === 'interrupted'
                ? '[turn_interrupted] 轮次已中断，工具调用未完成'
                : '[tool_result_missing] 轮次已结束，但 Runtime 未返回最终结果',
            ...(it.startedAt ? { endedAt: Date.now() } : {}),
          };
        }
        return Object.keys(terminal).length ? { ...it, ...terminal } : it;
      });
      return {
        ...s,
        status: 'idle',
        openAssistantId: null,
        openThinkingId: null,
        turnActivity: null,
        turnStartedAt: null,
        items: closeOpenThinking(items, s.openThinkingId),
      };
    }

    case 'error':
      return {
        ...s,
        status: 'idle',
        turnActivity: null,
        turnStartedAt: null,
        items: [
          ...s.items,
          {
            kind: 'error',
            id: uid('e'),
            message: e.message,
            errorKind: e.kind,
            ...(turnId ? { turnId } : {}),
          },
        ],
      };

    case 'approval_request': {
      const existing = s.items.some((item) => item.kind === 'approval' && item.id === e.id);
      const items = existing
        ? s.items.map((item) =>
            item.kind === 'approval' && item.id === e.id
              ? {
                  ...item,
                  action: e.action,
                  detail: e.detail,
                  availableDecisions: e.availableDecisions,
                  persistentLabel: e.persistentLabel,
                  matcherSummary: e.matcherSummary,
                }
              : item,
          )
        : [
            ...closeOpenThinking(s.items, s.openThinkingId),
            {
              kind: 'approval' as const,
              id: e.id,
              action: e.action,
              detail: e.detail,
              availableDecisions: e.availableDecisions,
              persistentLabel: e.persistentLabel,
              matcherSummary: e.matcherSummary,
              status: 'pending' as const,
              ...(turnId ? { turnId, startedAt: Date.now() } : {}),
            },
          ];
      return {
        ...s,
        ...derivedActivity(s, 'waiting_approval', {}, true),
        openAssistantId: null,
        openThinkingId: null,
        items,
      };
    }

    case 'plan_update': {
      const id = `plan-${e.sessionId}`;
      const hasPlan = s.items.some((it) => it.kind === 'plan' && it.id === id);
      return {
        ...s,
        openAssistantId: null,
        items: hasPlan
          ? s.items.map((it) =>
              it.kind === 'plan' && it.id === id ? { ...it, steps: e.steps } : it,
            )
          : [...s.items, { kind: 'plan', id, steps: e.steps, ...(turnId ? { turnId } : {}) }],
      };
    }

    case 'checkpoint':
      return {
        ...s,
        items: [
          ...s.items,
          {
            kind: 'checkpoint',
            id: e.id,
            label: e.label,
            ts: e.ts,
            restored: false,
            restorable: e.restorable,
            fileCount: e.fileCount,
            reason: e.reason,
            ...(turnId ? { turnId } : {}),
          },
        ],
      };

    default:
      return s;
  }
}

export function reduceSessionAction(s: SessionState, a: Action): SessionState {
  switch (a.type) {
    case 'event':
      return reduceSessionEvent(s, a.event, a.turnId);
    case 'send': {
      const now = Date.now();
      return {
        ...s,
        status: 'working',
        openAssistantId: null,
        openThinkingId: null,
        startedAt: s.startedAt ?? now,
        turnActivity: { stage: 'preparing', since: now },
        turnStartedAt: now,
        items: [
          // 悬空审批作废（变更-07）：发新消息即放弃待审批的工具调用，旧卡不可再点
          ...s.items.map((it) =>
            it.kind === 'approval' && it.status === 'pending'
              ? { ...it, status: 'resolved' as const }
              : it,
          ),
          {
            kind: 'user',
            id: a.id,
            text: a.text,
            attachments: a.attachments,
            mode: a.mode,
            permissionProfile: a.permissionProfile,
          },
        ],
      };
    }
    case 'handle':
      // 新会话：句柄 id 即历史会话 id（后端 bind_history_session(handle, handle)）
      return { ...s, handleId: a.handleId, historyId: a.handleId };
    case 'idle':
      return {
        ...s,
        status: 'idle',
        openAssistantId: null,
        openThinkingId: null,
        turnActivity: null,
        turnStartedAt: null,
      };
    case 'working':
      return { ...s, status: 'working' };
    case 'reset':
      return resetSessionState(s, a.defaults ?? { engine: s.engine, cwd: s.cwd });
    case 'apply_defaults':
      if (s.handleId || s.sessionId || s.items.length > 0) return s;
      return {
        ...s,
        engine: a.defaults.engine,
        model: s.engine === a.defaults.engine ? s.model : '',
        cwd: a.defaults.cwd,
      };
    case 'select_engine':
      // 切引擎必须是新 Session：旧消息、用量和原生 id 不能残留在新线程。
      return { ...resetSessionState(s, { engine: a.engine, cwd: s.cwd }), model: a.model };
    case 'select_model':
      // 只写下一 Turn 偏好；当前 Session、历史与 Runtime owner 保持不变。
      return { ...s, model: a.model };
    case 'approval_applying':
      return {
        ...s,
        items: s.items.map((it) =>
          it.kind === 'approval' && it.id === a.approvalId
            ? { ...it, status: 'applying', error: undefined }
            : it,
        ),
      };
    case 'approval_resolved':
      return {
        ...s,
        items: s.items.map((it) =>
          it.kind === 'approval' && it.id === a.approvalId
            ? { ...it, status: 'resolved', decision: a.decision, error: undefined }
            : it,
        ),
      };
    case 'approval_failed':
      return {
        ...s,
        items: s.items.map((it) =>
          it.kind === 'approval' && it.id === a.approvalId
            ? { ...it, status: 'failed', error: a.error }
            : it,
        ),
      };
    case 'restore_checkpoint': {
      const idx = s.items.findIndex((it) => it.kind === 'checkpoint' && it.id === a.checkpointId);
      if (idx === -1) return s;
      return {
        ...s,
        // 回溯后旧 CLI 会话已作废（P2-5）：清除绑定，等下一轮新 session_started 重新绑定
        sessionId: null,
        items: s.items.map((it, i) => {
          if (it.kind === 'checkpoint' && it.id === a.checkpointId) {
            return { ...it, restored: true };
          }
          if (i > idx) {
            if (
              it.kind === 'assistant' ||
              it.kind === 'thinking' ||
              it.kind === 'tool' ||
              it.kind === 'approval' ||
              it.kind === 'plan'
            ) {
              return { ...it, reverted: true };
            }
          }
          return it;
        }),
      };
    }
    case 'set_cwd':
      // 仅在会话未开始时切换用户选择的工作目录。
      return { ...s, cwd: a.cwd };
    case 'set_disabled_mcp':
      return { ...s, disabledMcp: a.disabled };
    case 'undo_revert':
      return {
        ...s,
        items: s.items.map((it) => {
          if (it.kind === 'checkpoint') {
            return { ...it, restored: false };
          }
          if ('reverted' in it && it.reverted) {
            const { reverted: _reverted, ...rest } = it;
            return rest as ThreadItem;
          }
          return it;
        }),
      };
    case 'resume_handle':
      return {
        ...s,
        handleId: a.handleId,
        historyId: a.historyId,
        sessionId: a.detail?.cliSessionId ?? null,
        engine: a.detail?.engine ?? s.engine,
        model: a.detail?.preferredModel ?? a.detail?.model ?? s.model,
        runtimeModel: a.detail?.model ?? s.runtimeModel,
        cwd: a.detail?.cwd ?? s.cwd,
        runtimeCapabilities: a.detail?.runtimeCapabilities ?? s.runtimeCapabilities,
        fork: a.detail?.fork ?? null,
        // 复用后台仍在跑的句柄时保持 working（可靠性检查 A2：
        // 强制 idle 会让下一次切走时误判「空闲」而杀掉后台轮次）
        status: a.working ? 'working' : 'idle',
        openAssistantId: null,
        openThinkingId: null,
        turnActivity: a.working ? (a.activity ?? null) : null,
        turnStartedAt: a.working ? (a.activity?.since ?? null) : null,
        items: a.detail ? itemsFromHistory(a.detail) : s.items,
        cost: a.detail
          ? {
              inputTokens: a.detail.inputTokens,
              cachedInputTokens: a.detail.cachedInputTokens ?? 0,
              cacheWriteInputTokens: a.detail.cacheWriteInputTokens ?? 0,
              outputTokens: a.detail.outputTokens,
              costUsd: a.detail.costUsd,
              ...(a.detail.lastContextTokens != null
                ? { contextTokens: a.detail.lastContextTokens }
                : {}),
              ...(a.detail.lastContextWindow != null
                ? { contextWindow: a.detail.lastContextWindow }
                : {}),
            }
          : s.cost,
        startedAt: a.detail?.createdAt ? a.detail.createdAt * 1000 : s.startedAt,
        disabledMcp: [],
      };
    default:
      return s;
  }
}

function errText(err: unknown): string {
  if (err instanceof Error) return err.message;
  if (typeof err === 'string') return err;
  return '无法连接到后端（CLI 进程）。请确认 Helm 在 Tauri 中运行，且 CLI 路径配置正确。';
}

/** Tauri 命令拒绝会直接走 Promise catch，不会经过后端 emit_error；在这里补齐 UI 分类。 */
export function classifyClientErrorKind(message: string): ErrorKind | undefined {
  const lower = message.toLowerCase();
  if (
    lower.includes("there's an issue with the selected model") ||
    (lower.includes('model') && lower.includes('may not exist or you may not have access'))
  ) {
    return 'model_unavailable';
  }
  if (
    lower.includes('[codex_probe_tool_surface_') ||
    lower.includes('codex protected tool surface') ||
    lower.includes('codex 版本不兼容')
  ) {
    return 'version_incompatible';
  }
  if (lower.includes('工作目录不存在') || lower.includes('未设置工作目录')) {
    return 'cwd_invalid';
  }
  if (lower.includes('还没有配置生效绑定')) return 'no_binding';
  if (
    lower.includes('command not found') ||
    lower.includes('is not recognized') ||
    lower.includes('无法启动 codex 进程') ||
    lower.includes('无法启动 claude 进程')
  ) {
    return 'not_installed';
  }
  if (
    lower.includes('unauthorized') ||
    lower.includes('invalid api key') ||
    lower.includes('authentication') ||
    lower.includes('please run /login')
  ) {
    return 'auth_missing';
  }
  if (lower.includes('超时') || lower.includes('timed out')) return 'timeout';
  if (
    lower.includes('econnrefused') ||
    lower.includes('getaddrinfo') ||
    lower.includes('fetch failed') ||
    lower.includes('connection refused') ||
    lower.includes('network')
  ) {
    return 'network';
  }
  return undefined;
}

function errorEvent(err: unknown): Extract<AgentEvent, { type: 'error' }> {
  const message = errText(err);
  return {
    type: 'error',
    message,
    recoverable: false,
    kind: classifyClientErrorKind(message),
  };
}

type ApprovalDispatchAction = Extract<
  Action,
  { type: 'approval_applying' | 'approval_resolved' | 'approval_failed' }
>;

export async function submitApprovalTransaction({
  approvalId,
  decision,
  respond,
  dispatch,
  onResolved,
  onFailed,
}: {
  approvalId: string;
  decision: Decision;
  respond: (approvalId: string, decision: Decision) => Promise<void>;
  dispatch: (action: ApprovalDispatchAction) => void;
  onResolved: (decision: Decision) => void;
  onFailed: (message: string) => void;
}): Promise<void> {
  dispatch({ type: 'approval_applying', approvalId });
  try {
    await respond(approvalId, decision);
    dispatch({ type: 'approval_resolved', approvalId, decision });
    onResolved(decision);
  } catch (error) {
    const message = errText(error);
    dispatch({ type: 'approval_failed', approvalId, error: message });
    onFailed(message);
  }
}

export async function applyMcpDisabledTransaction({
  handle,
  next,
  rollback,
  sync,
  dispatch,
  onFailed,
}: {
  handle: string;
  next: string[];
  rollback: string[];
  sync: (handleId: string, disabled: string[]) => Promise<void>;
  dispatch: (disabled: string[]) => void;
  onFailed: (message: string) => void;
}): Promise<boolean> {
  dispatch(next);
  try {
    await sync(handle, next);
    return true;
  } catch (error) {
    dispatch(rollback);
    onFailed(errText(error));
    return false;
  }
}

function sameStringList(left: string[], right: string[]): boolean {
  return left.length === right.length && left.every((value, index) => value === right[index]);
}

/** 串行下发会话 MCP 目标状态；旧请求失败时不得覆盖更新的用户选择。 */
export class McpDisabledSyncQueue {
  private chain: Promise<void> = Promise.resolve();
  private desired: string[];
  private confirmed: string[];

  constructor(
    private readonly handle: string,
    initial: string[],
    private readonly sync: (handleId: string, disabled: string[]) => Promise<void>,
    private readonly dispatch: (disabled: string[]) => void,
    private readonly onFailed: (message: string) => void,
  ) {
    this.desired = [...initial];
    this.confirmed = [...initial];
  }

  current(): string[] {
    return [...this.desired];
  }

  update(next: string[]): Promise<boolean> {
    const requested = [...next];
    this.desired = requested;
    this.dispatch(requested);

    const result = this.chain.then(async () => {
      try {
        await this.sync(this.handle, requested);
        this.confirmed = requested;
        return true;
      } catch (error) {
        if (sameStringList(this.desired, requested)) {
          this.desired = [...this.confirmed];
          this.dispatch(this.desired);
        }
        this.onFailed(errText(error));
        return false;
      }
    });
    this.chain = result.then(() => undefined);
    return result;
  }
}

export function itemsFromHistory(detail: SessionDetail): ThreadItem[] {
  // 按时间线穿插（变更-10）：文本→工具→文本的原始顺序不再失真。
  // 全部实体的 ts 均为毫秒（v4 迁移统一单位）；同 ts 时按 seq 保持稳定次序，
  // 老库缺 ts（=0）的工具调用沉底兜底（与迁移前行为一致）。
  const entries: Array<{ ts: number; seq: number; item: ThreadItem }> = [];
  const turnsById = new Map((detail.turns ?? []).map((turn) => [turn.id, turn]));
  const lastKnownEventAt = new Map<string, number>();
  const rememberEvent = (turnId: string | null | undefined, ts: number | null | undefined) => {
    if (!turnId || !ts) return;
    lastKnownEventAt.set(turnId, Math.max(lastKnownEventAt.get(turnId) ?? 0, ts));
  };
  detail.messages.forEach((message) => rememberEvent(message.turnId, message.ts));
  detail.toolCalls.forEach((tool) => rememberEvent(tool.turnId, tool.endedAt ?? tool.ts));
  detail.approvals?.forEach((approval) =>
    rememberEvent(approval.turnId, approval.resolvedAt ?? approval.ts),
  );
  detail.checkpoints.forEach((checkpoint) => rememberEvent(checkpoint.turnId, checkpoint.ts));

  const inferLegacyAssistantTurnId = (message: SessionDetail['messages'][number]) => {
    if (message.role !== 'assistant' || message.turnId) return message.turnId ?? undefined;
    const candidates = (detail.turns ?? []).filter(
      (turn) =>
        turn.endedAt != null &&
        turn.startedAt <= message.ts &&
        message.ts <= turn.endedAt &&
        message.ts >= (lastKnownEventAt.get(turn.id) ?? turn.startedAt),
    );
    return candidates.length === 1 ? candidates[0].id : undefined;
  };
  const terminalTurnStatus = (turnId: string | null | undefined) => {
    const status = turnId ? turnsById.get(turnId)?.status : undefined;
    return status === 'succeeded' || status === 'failed' || status === 'interrupted'
      ? status
      : undefined;
  };
  let seq = 0;
  detail.messages.forEach((message, index) => {
    const messageTurnId = inferLegacyAssistantTurnId(message);
    const turn = messageTurnId ? turnsById.get(messageTurnId) : undefined;
    const turnStatus = terminalTurnStatus(messageTurnId);
    entries.push({
      ts: message.ts,
      seq: seq++,
      item:
        message.role === 'user'
          ? {
              kind: 'user',
              id: `history-u-${index}`,
              text: message.text,
              attachments: message.attachments,
              mode: turn?.mode,
              permissionProfile: turn?.permissionProfile,
              ...(messageTurnId ? { turnId: messageTurnId } : {}),
              ...(turnStatus ? { turnStatus } : {}),
            }
          : {
              kind: 'assistant',
              id: `history-a-${index}`,
              text: message.text,
              // 被回溯的回复保留淡化视觉（P2-5），与实时回溯后的线程一致
              ...(message.reverted ? { reverted: true } : {}),
              ...(messageTurnId ? { turnId: messageTurnId } : {}),
              ...(turnStatus ? { turnStatus } : {}),
            },
    });
  });
  for (const tool of detail.toolCalls) {
    const turnStatus = terminalTurnStatus(tool.turnId);
    entries.push({
      ts: tool.ts || Number.MAX_SAFE_INTEGER,
      seq: seq++,
      item: {
        kind: 'tool',
        id: tool.id,
        name: tool.name,
        input: tool.input,
        status: tool.status,
        output: tool.output ?? undefined,
        diff: tool.diff ?? undefined,
        outcome: tool.outcome ?? undefined,
        started: tool.started ?? undefined,
        hasOutput: tool.hasOutput ?? undefined,
        retryable: tool.retryable ?? undefined,
        denialSource: tool.denialSource ?? undefined,
        nativeDenialCode: tool.nativeDenialCode ?? undefined,
        ...(tool.ts ? { startedAt: tool.ts } : {}),
        ...(tool.endedAt ? { endedAt: tool.endedAt } : {}),
        ...(tool.turnId ? { turnId: tool.turnId } : {}),
        ...(turnStatus ? { turnStatus } : {}),
      },
    });
  }
  for (const approval of detail.approvals ?? []) {
    // 审批卡重建（变更-07）：pending 的悬空审批可继续响应；resolved/expired 只读展示
    entries.push({
      ts: approval.ts || Number.MAX_SAFE_INTEGER,
      seq: seq++,
      item: {
        kind: 'approval',
        id: approval.id,
        action: approval.action,
        detail: approval.detail,
        availableDecisions:
          approval.status === 'pending' ||
          approval.status === 'applying' ||
          approval.status === 'failed'
            ? ['allow', 'deny']
            : [],
        decision: approval.decision ?? undefined,
        status:
          approval.status === 'pending' ||
          approval.status === 'applying' ||
          approval.status === 'failed'
            ? approval.status
            : 'resolved',
        error: approval.error ?? undefined,
        persistentLabel: approval.persistentLabel ?? undefined,
        matcherSummary: approval.matcherSummary ?? undefined,
        ...(approval.turnId ? { turnId: approval.turnId } : {}),
      },
    });
  }
  for (const checkpoint of detail.checkpoints) {
    entries.push({
      ts: checkpoint.ts,
      seq: seq++,
      item: {
        kind: 'checkpoint',
        id: checkpoint.id,
        label: checkpoint.label,
        ts: checkpoint.ts,
        restored: false,
        restorable: checkpoint.restorable ?? false,
        fileCount: checkpoint.fileCount ?? 0,
        reason: checkpoint.reason ?? undefined,
        ...(checkpoint.turnId ? { turnId: checkpoint.turnId } : {}),
      },
    });
  }
  return entries.sort((a, b) => a.ts - b.ts || a.seq - b.seq).map((entry) => entry.item);
}

export function useSession(defaults?: SessionDefaults) {
  const [state, dispatch] = useReducer(reduceSessionAction, defaults, initialState);
  const handleRef = useRef<string | null>(null);
  const disabledMcpRef = useRef<string[]>(state.disabledMcp);
  const mcpSyncQueueRef = useRef<{ handle: string; queue: McpDisabledSyncQueue } | null>(null);
  // 当前句柄对应的 history 会话 id：事件路由键 + 并行会话注册表的 key
  const historyIdRef = useRef<string | null>(null);

  useEffect(() => {
    if (!defaults) return;
    dispatch({ type: 'apply_defaults', defaults });
  }, [defaults]);

  useEffect(() => {
    disabledMcpRef.current = state.disabledMcp;
  }, [state.disabledMcp]);

  useEffect(() => {
    // 全局监听维持注册表（不随卸载退订）；组件级监听只把当前线程的事件送进 reducer。
    ensureGlobalAgentListener();
    const listener: EnvelopeListener = (envelope) => {
      if (shouldConsumeAgentEvent(historyIdRef.current, envelope)) {
        dispatch({ type: 'event', event: envelope.event, turnId: envelope.turnId });
      }
    };
    envelopeListeners.add(listener);

    // 监听 window.postMessage（用于开发测试）：载荷缺 historyId 时视为发给当前线程
    const handlePostMessage = (evt: MessageEvent) => {
      if (evt.data?.type === 'agent-event' && evt.data.payload) {
        const envelope: AgentEventEnvelope = {
          historyId:
            typeof evt.data.historyId === 'string'
              ? evt.data.historyId
              : (historyIdRef.current ?? ''),
          event: evt.data.payload as AgentEvent,
        };
        dispatchEnvelope(envelope);
      }
    };
    window.addEventListener('message', handlePostMessage);

    return () => {
      envelopeListeners.delete(listener);
      window.removeEventListener('message', handlePostMessage);
      // 卸载（切页）不关闭句柄：注册表保留句柄与运行状态，回到工作区时按
      // lastOpenWorkspaceSession 复用恢复线程（可靠性检查 C1/A10）。
    };
  }, []);

  useEffect(() => {
    const applyResume = (payload: ResumePayload) => {
      // publishResume 同时写 pendingResume 并派发事件；事件路径消费后必须清掉
      // pendingResume，否则组件下次挂载会重放陈旧快照（可靠性检查 A4）
      consumePendingResume();
      const prevHandle = handleRef.current;
      const prevHistoryId = historyIdRef.current;
      if (prevHandle && prevHandle !== payload.handleId) {
        // 切换前若上一个会话轮次还在跑，releaseHandle 依注册表保活（P3-3）
        releaseHandle(prevHandle, prevHistoryId);
      }
      handleRef.current = payload.handleId;
      historyIdRef.current = payload.session.id;
      // 复用存活句柄时继承其运行状态；新启的 resume 运行时必然空闲
      const existing = liveSessions.get(payload.session.id);
      const working = existing?.handleId === payload.handleId ? existing.working : false;
      const activity = existing?.handleId === payload.handleId ? existing.activity : null;
      registerLiveSession(payload.session.id, payload.handleId, working);
      sweepIdleHandles(payload.session.id);
      rememberLastWorkspaceSession(payload.session.id);
      dispatch({
        type: 'resume_handle',
        handleId: payload.handleId,
        historyId: payload.session.id,
        working,
        activity,
        detail: payload.session,
      });
    };
    const pending = consumePendingResume();
    if (pending) applyResume(pending);
    const onResume = (event: Event) => {
      const detail = (event as CustomEvent<ResumePayload>).detail;
      if (!detail?.handleId) return;
      applyResume(detail);
    };
    window.addEventListener('helm:resume-session', onResume);
    return () => window.removeEventListener('helm:resume-session', onResume);
  }, []);

  const send = useCallback(
    async (
      text: string,
      attachments: string[] = [],
      mode: TurnMode = 'build',
      commandText?: string,
      reasoningEffort: ReasoningEffort = 'auto',
      permissionProfile: import('./transport').PermissionProfile = 'standard',
      folderId?: string,
    ) => {
      const trimmed = text.trim();
      if (!trimmed) return false;
      const mountedPaths = Array.from(
        new Set(attachments.map((path) => path.trim()).filter(Boolean)),
      );
      dispatch({
        type: 'send',
        id: uid('u'),
        text: trimmed,
        attachments: mountedPaths.length ? mountedPaths : undefined,
        // 构建是默认态，不标；计划/询问记录到消息项供徽标渲染（变更-04 B.2）
        mode: mode === 'build' ? undefined : mode,
        permissionProfile,
      });
      try {
        let handle = handleRef.current;
        if (!handle) {
          handle = await createSession({
            engine: state.engine,
            model: state.model,
            cwd: state.cwd,
            reasoningEffort,
            mode,
            permissionProfile,
            folderId,
          });
          handleRef.current = handle;
          // 新会话的 history id 即句柄 id（后端 bind_history_session(handle, handle)）
          historyIdRef.current = handle;
          rememberLastWorkspaceSession(handle);
          dispatch({ type: 'handle', handleId: handle });
          // 开场前就设置过 MCP 开关：句柄建立后统一下发（变更-11）
          if (state.disabledMcp.length) {
            await applyMcpDisabledTransaction({
              handle,
              next: state.disabledMcp,
              rollback: [],
              sync: setSessionMcpDisabled,
              dispatch: (disabled) => {
                disabledMcpRef.current = disabled;
                dispatch({ type: 'set_disabled_mcp', disabled });
              },
              onFailed: (message) =>
                showToast(`MCP 禁用状态下发失败，已恢复全部启用：${message}`, 'error'),
            });
          }
        }
        registerLiveSession(historyIdRef.current ?? handle, handle, true);
        // commandText（变更-08）：斜杠命令展开结果发给 CLI，线程/历史存 text 原文
        await sendMessage(
          handle,
          trimmed,
          mountedPaths,
          mode,
          commandText,
          state.model,
          reasoningEffort,
        );
        return true;
      } catch (err) {
        if (historyIdRef.current) setLiveSessionWorking(historyIdRef.current, false);
        dispatch({
          type: 'event',
          event: errorEvent(err),
        });
        return false;
      }
    },
    [state.cwd, state.disabledMcp, state.engine, state.model],
  );

  const stop = useCallback(async () => {
    const handle = handleRef.current;
    if (!handle) {
      dispatch({ type: 'idle' });
      return;
    }
    try {
      await interrupt(handle);
      // Stop 是控制面请求；只有后端快照确认终态后才改变前端状态。
      // 这样即使 CLI/IPC 迟到，UI 也不会先显示“已停止”而实际仍在执行。
      for (let attempt = 0; attempt < 40; attempt += 1) {
        const snapshot = await getTurnSnapshot(handle).catch(() => null);
        if (
          snapshot &&
          (snapshot.status === 'succeeded' ||
            snapshot.status === 'failed' ||
            snapshot.status === 'interrupted')
        ) {
          dispatch({ type: 'idle' });
          return;
        }
        await new Promise((resolve) => window.setTimeout(resolve, 50));
      }
      showToast('停止请求已发送，正在等待运行时确认终态', 'info');
    } catch (error) {
      showToast(error instanceof Error ? error.message : '发送停止请求失败', 'error');
    }
  }, []);

  const releaseCurrent = useCallback(() => {
    const handle = handleRef.current;
    if (handle) releaseHandle(handle, historyIdRef.current);
    handleRef.current = null;
    historyIdRef.current = null;
    mcpSyncQueueRef.current = null;
    rememberLastWorkspaceSession(null);
  }, []);

  const reset = useCallback(() => {
    releaseCurrent();
    dispatch({ type: 'reset', defaults });
  }, [defaults, releaseCurrent]);

  const selectEngine = useCallback(
    (engine: EngineId, model: string) => {
      releaseCurrent();
      writeStoredSelection(engine, model);
      dispatch({ type: 'select_engine', engine, model });
    },
    [releaseCurrent],
  );

  const selectModel = useCallback(
    async (model: string, reasoningEffort?: ReasoningEffort) => {
      const handle = handleRef.current;
      if (handle) {
        await setSessionTurnPreference(handle, model, reasoningEffort);
      }
      writeStoredSelection(state.engine, model);
      dispatch({ type: 'select_model', model });
    },
    [state.engine],
  );

  /** 会话级 MCP 开关（变更-11）：切换某个服务器的启用状态，下一轮生效。
   *  句柄已存在时立即同步给后端运行时；尚未开场的会话在 send 创建句柄后统一下发。 */
  const toggleMcpServer = useCallback(async (name: string) => {
    const current = disabledMcpRef.current;
    const next = current.includes(name)
      ? current.filter((item) => item !== name)
      : [...current, name];
    const handle = handleRef.current;
    if (handle) {
      let holder = mcpSyncQueueRef.current;
      if (!holder || holder.handle !== handle) {
        holder = {
          handle,
          queue: new McpDisabledSyncQueue(
            handle,
            current,
            setSessionMcpDisabled,
            (disabled) => {
              disabledMcpRef.current = disabled;
              dispatch({ type: 'set_disabled_mcp', disabled });
            },
            (message) => showToast(`同步 MCP 开关失败：${message}`, 'error'),
          ),
        };
        mcpSyncQueueRef.current = holder;
      }
      await holder.queue.update(next);
    } else {
      disabledMcpRef.current = next;
      dispatch({ type: 'set_disabled_mcp', disabled: next });
    }
  }, []);

  /** 为下一个尚未创建的会话指定工作目录。 */
  const setCwd = useCallback((cwd: string) => {
    dispatch({ type: 'set_cwd', cwd });
  }, []);

  const approve = useCallback(async (approvalId: string, decision: Decision) => {
    const handle = handleRef.current;
    if (!handle) return;
    await submitApprovalTransaction({
      approvalId,
      decision,
      respond: (id, nextDecision) => respondApproval(handle, id, nextDecision),
      dispatch,
      onResolved: (resolvedDecision) => {
        if (resolvedDecision === 'deny') return;
        // 后端确认恢复轮已接管后，线程才回到运行中。
        dispatch({ type: 'working' });
        if (historyIdRef.current) setLiveSessionWorking(historyIdRef.current, true);
      },
      onFailed: (message) => showToast(`审批失败：${message}，可重试`, 'error'),
    });
  }, []);

  const restoreCheckpointAction = useCallback(async (checkpointId: string) => {
    try {
      await restoreCheckpoint(checkpointId);
      dispatch({ type: 'restore_checkpoint', checkpointId });
      showToast('已回溯：文件已还原，后续对话将基于截断后的历史重建上下文', 'success');
    } catch (err) {
      dispatch({
        type: 'event',
        event: errorEvent(err),
      });
    }
  }, []);

  const undoRevertAction = useCallback(async () => {
    // 回溯会作废 CLI 会话 id，因此撤销按内部句柄定位会话
    const handle = handleRef.current;
    if (!handle) return;
    try {
      await undoRevert(handle);
      dispatch({ type: 'undo_revert' });
      showToast('已撤销回溯：完整历史将重新进入 Agent 上下文', 'success');
    } catch (err) {
      dispatch({
        type: 'event',
        event: errorEvent(err),
      });
    }
  }, []);

  return {
    state,
    send,
    stop,
    reset,
    approve,
    selectEngine,
    selectModel,
    setCwd,
    toggleMcpServer,
    restoreCheckpoint: restoreCheckpointAction,
    undoRevert: undoRevertAction,
  };
}
