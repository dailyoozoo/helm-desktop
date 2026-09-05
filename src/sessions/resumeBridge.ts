import type { SessionDetail } from './api';

export interface ResumePayload {
  handleId: string;
  session: SessionDetail;
}

/** 历史先行载荷：只带会话快照（渲染线程内容），不带句柄。
 *  CLI 还在后台重建时先把历史画出来，Composer 由 Workspace 恢复闸门挡住。 */
export interface HistoryOnlyPayload {
  session: SessionDetail;
}

let pendingResume: ResumePayload | null = null;
let pendingHistory: HistoryOnlyPayload | null = null;

export function publishResume(payload: ResumePayload) {
  // 句柄到位即代表完整恢复：清掉更早的历史先行快照，避免组件挂载时重放。
  pendingHistory = null;
  pendingResume = payload;
  window.dispatchEvent(new CustomEvent('helm:resume-session', { detail: payload }));
}

export function consumePendingResume(): ResumePayload | null {
  const payload = pendingResume;
  pendingResume = null;
  return payload;
}

export function publishHistoryOnly(payload: HistoryOnlyPayload) {
  pendingHistory = payload;
  window.dispatchEvent(new CustomEvent('helm:resume-history', { detail: payload }));
}

export function consumePendingHistory(): HistoryOnlyPayload | null {
  const payload = pendingHistory;
  pendingHistory = null;
  return payload;
}

/** 运行时重建失败时丢弃先行渲染的线程（仅当仍无句柄且身份未变才回滚）。 */
export function discardHistoryPreview(sessionId: string) {
  pendingHistory = null;
  window.dispatchEvent(new CustomEvent('helm:resume-history-discard', { detail: { sessionId } }));
}
