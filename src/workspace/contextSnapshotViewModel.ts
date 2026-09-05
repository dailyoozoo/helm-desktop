import type { ThreadItem } from '../engine/useSession';
import type { McpServer } from '../extensions/extensionsApi';
import type { SessionContextRecord } from '../sessions/api';
import type { AttributionEntry } from './attributionViewModel';
import {
  billingSummary,
  contextPanelData,
  type ContextPanelCost,
  type ContextUsageSummary,
} from './contextPanelViewModel';

/**
 * 变更-34/35 · 切片 D · P1-02：上下文快照统一视图模型。
 * 把 ContextRing、ContextPanel「上下文」tab、未来归因接入都改为复用同一份派生，
 * 消除语境口径分歧：
 *  - `historicalAttachments`：已发送消息内联的附件路径，只读历史，不可变。
 *  - `sessionContexts`：SessionContext 独立持久化、只影响后续轮次，可增删。
 *  - `mcpEnabled`：按本会话 `disabledMcp` 过滤后的有效 MCP 数量与服务器清单。
 *  - `attribution`：来自 Runtime 协议的逐来源规模；当前协议无此字段，恒为空数组 →
 *    AttributionView 显示「暂无归因数据」（AGENTS.md 红线，禁止估算）。
 */
export interface ContextSnapshotInput {
  items: ThreadItem[];
  cost?: ContextPanelCost;
  /** MCP 全局配置（来自扩展中心），由 Workspace 注入。 */
  mcpServers?: McpServer[];
  /** 本会话已停用的 MCP 服务器名集合（state.disabledMcp）。 */
  disabledMcp?: string[];
  /** SessionContext 独立持久化记录（来自 list_session_contexts）。 */
  sessionContexts?: SessionContextRecord[];
}

export interface ContextSnapshotViewModel {
  /** 真实上下文占用（最近一次模型调用输入规模 ÷ 窗口）。 */
  usage: ContextUsageSummary;
  /** 计费 token（跨轮累计四账 + 缓存读取占比）。 */
  billing: ReturnType<typeof billingSummary>;
  /** 消息条数（user + assistant）。 */
  messageCount: number;
  /** 历史附件：已发送消息内联路径，不可变。 */
  historicalAttachments: string[];
  /** 会话上下文：独立持久化、可增删，只影响后续轮次。 */
  sessionContexts: SessionContextRecord[];
  /** 当前会话启用的 MCP 服务器（排除 disabledMcp，且无 lastError、toolCount>0 才算「已连接」）。 */
  mcpEnabled: McpServer[];
  /** 当前会话禁用的 MCP 服务器名（用于区分「未连接」与「会话级停用」）。 */
  mcpDisabled: McpServer[];
  /** E2 归因条目（只读 Runtime 真实逐来源；当前协议无字段 → 永远空数组）。 */
  attribution: AttributionEntry[];
}

/**
 * 把多源输入派生成一份统一快照。ContextRing 与 ContextPanel「上下文」tab 都复用此函数，
 * 不再各自从 items 推导历史附件 / MCP 数量。
 */
export function contextSnapshot(input: ContextSnapshotInput): ContextSnapshotViewModel {
  const { items, cost, mcpServers = [], disabledMcp = [], sessionContexts = [] } = input;
  // 复用 contextPanelData 派生历史附件 / 消息数 / 上下文窗口 / 计费 token，
  // 不在两处各写一遍。gitStatus/stagedFiles 与快照无关，这里不传。
  const panel = contextPanelData(items, cost);
  const disabledSet = new Set(disabledMcp);

  const connected = mcpServers.filter((server) => (server.toolCount ?? 0) > 0 && !server.lastError);
  const mcpEnabled = connected.filter((server) => !disabledSet.has(server.name));
  const mcpDisabled = mcpServers.filter((server) => disabledSet.has(server.name));

  return {
    usage: panel.contextUsage,
    billing: panel.billing,
    messageCount: panel.messageCount,
    historicalAttachments: panel.historicalAttachments,
    sessionContexts,
    mcpEnabled,
    mcpDisabled,
    // 真实逐来源归因当前协议无字段 → 恒为空。
    attribution: [],
  };
}

/** 圆环 hover 摘要：仅展示真实可用的字段，无数据时返回 null（不渲染虚构摘要）。 */
export function contextRingHoverSummary(snapshot: ContextSnapshotViewModel): {
  percent: number | null;
  tokens: number | null;
  maxTokens: number | null;
  files: number;
  mcp: number;
} | null {
  const usage = snapshot.usage;
  if (usage.tokens == null || usage.maxTokens == null) {
    return {
      percent: null,
      tokens: null,
      maxTokens: null,
      files: snapshot.historicalAttachments.length + snapshot.sessionContexts.length,
      mcp: snapshot.mcpEnabled.length,
    };
  }
  const ratio = Math.min(1, usage.tokens / usage.maxTokens);
  return {
    percent: Math.round(ratio * 100),
    tokens: usage.tokens,
    maxTokens: usage.maxTokens,
    files: snapshot.historicalAttachments.length + snapshot.sessionContexts.length,
    mcp: snapshot.mcpEnabled.length,
  };
}
