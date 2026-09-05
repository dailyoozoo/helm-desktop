// CLI↔UI 流式协议：UI→后端 的归一化命令（见 docs/技术方案.md 第 4.2 节）。
import type { EngineId, PermissionProfile } from './events';
import type { ReasoningEffort } from './reasoning';

export type TurnMode = 'build' | 'plan' | 'ask';

/** Tauri `create_session` 的参数；字段名保持 JS invoke 使用的 camelCase。 */
export interface CreateSessionArgs {
  engine: EngineId;
  model: string;
  cwd: string;
  reasoningEffort?: ReasoningEffort;
  mode?: TurnMode;
  permissionProfile?: PermissionProfile;
  /** 页内确认卡已确认；`permissionProfile=full_access` 时必须为 true，否则后端 fail-closed。 */
  fullAccessConfirmed?: boolean;
}

/** Tauri `send_message` 的参数。 */
export interface SendMessageArgs {
  handleId: string;
  text: string;
  displayText?: string;
  attachments?: string[];
  mode?: TurnMode;
  model?: string;
  reasoningEffort?: ReasoningEffort;
}

export interface SetSessionTurnPreferenceArgs {
  handleId: string;
  model: string;
  reasoningEffort?: ReasoningEffort;
}

/** Tauri `side_query` 的参数：旁路提问（变更-34 · D3），读上下文不落盘。 */
export interface SideQueryArgs {
  handleId: string;
  text: string;
}

export interface SetSessionPermissionProfileArgs {
  handleId: string;
  profile: PermissionProfile;
  /** 页内确认卡已确认；`profile=full_access` 时必须为 true，否则后端 fail-closed。 */
  fullAccessConfirmed?: boolean;
}

export type AgentCommand =
  | {
      type: 'send_message';
      sessionId: string;
      text: string;
      attachments?: string[];
      mode?: TurnMode;
      permissionProfile?: PermissionProfile;
      reasoningEffort?: ReasoningEffort;
    }
  | {
      type: 'approve';
      sessionId: string;
      requestId: string;
      decision: 'allow' | 'turn' | 'session' | 'project' | 'always' | 'deny';
    }
  | { type: 'interrupt'; sessionId: string }
  | { type: 'restore_checkpoint'; sessionId: string; checkpointId: string }
  | {
      type: 'create_session';
      engine: EngineId;
      model: string;
      cwd: string;
      mode?: TurnMode;
      permissionProfile?: PermissionProfile;
      fullAccessConfirmed?: boolean;
      reasoningEffort?: ReasoningEffort;
    }
  | { type: 'resume_session'; sessionId: string };
