import type { EngineId, RuntimeCapabilitySnapshot } from '@helm/protocol';

export type SessionStatus = 'active' | 'idle' | 'done' | 'waiting_approval';

export interface SessionFolder {
  id: string;
  name: string;
  sortOrder: number;
  collapsed: boolean;
  locked: boolean;
  createdAt: number;
  /** 自动项目 Folder 绑定的 canonical 工作目录；人工 Folder 与默认 Folder 为 null。 */
  cwd?: string | null;
}

export interface SessionSummary {
  id: string;
  cliSessionId: string | null;
  title: string;
  engine: EngineId;
  model: string;
  cwd: string;
  status: SessionStatus;
  messageCount: number;
  inputTokens: number;
  outputTokens: number;
  costUsd: number;
  createdAt: number;
  updatedAt: number;
  /** fast model 生成的一句话摘要（P3-5）；null = 尚未生成 */
  summary?: string | null;
  /** 置顶（变更-12）：侧栏排最前 */
  pinned?: boolean;
  runtimeCapabilities?: RuntimeCapabilitySnapshot | null;
  safePermissionProfile?: 'standard' | 'auto';
  folderId?: string;
  cachedInputTokens?: number;
  cacheWriteInputTokens?: number;
  lastContextTokens?: number | null;
  lastContextWindow?: number | null;
  /** 下一 Turn 的用户模型偏好；当前 Binding 不含该模型时后端会回落主模型。 */
  preferredModel?: string | null;
  /** 下一 Turn 的推理强度偏好；null 表示跟随当前 Binding。 */
  preferredReasoningEffort?: import('@helm/protocol').ReasoningEffort | null;
  /** 已归档（变更-34/35 · 切片7 · F1）：可逆标记，区别于删除；归档会话保留历史与用量。 */
  archived?: boolean;
  /** 会话卡「当前在做什么」（切片7 · F2）：最新未完成工具名；undefined = 无进行中工具。 */
  currentTool?: string | null;
  /** 「当前在做什么」的目标摘要（如正在写的文件路径），与 currentTool 配套。 */
  currentTarget?: string | null;
  /** 跨轮累计变更规模（切片7 · F2）：+N -M 由真实 diff 行数聚合。 */
  changeAdditions?: number;
  changeDeletions?: number;
  /** 存在未决审批（切片7 · F1 等审批筛选）。 */
  pendingApproval?: boolean;
  /** 最近一轮是否失败（切片7 · F1 失败筛选）。 */
  lastTurnFailed?: boolean;
  /** 分叉来源标题快照（主侧栏「分叉自 X」副行）；null = 非分叉会话。 */
  forkedFrom?: string | null;
  /** 最新轮次真实状态（主侧栏状态徽标）：running/waiting_approval/stalled/succeeded/failed/interrupted；null = 从未跑过 Turn。 */
  lastTurnStatus?: string | null;
}
