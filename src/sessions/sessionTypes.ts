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
}
