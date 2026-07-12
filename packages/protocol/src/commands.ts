// CLI↔UI 流式协议：UI→后端 的归一化命令（见 docs/技术方案.md 第 4.2 节）。

export type AgentCommand =
  | { type: 'send_message'; sessionId: string; text: string; attachments?: string[] }
  | { type: 'approve'; sessionId: string; requestId: string; decision: 'allow' | 'deny' | 'always' }
  | { type: 'interrupt'; sessionId: string }
  | { type: 'restore_checkpoint'; sessionId: string; checkpointId: string }
  | { type: 'create_session'; engine: import('./events').EngineId; model: string; cwd: string }
  | { type: 'resume_session'; sessionId: string };
