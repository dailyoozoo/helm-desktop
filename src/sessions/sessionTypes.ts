import type { EngineId } from '@helm/protocol';

export type SessionStatus = 'active' | 'idle' | 'done';

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
}
