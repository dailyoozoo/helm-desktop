import type { SessionDetail } from './api';

export interface ResumePayload {
  handleId: string;
  session: SessionDetail;
}

let pendingResume: ResumePayload | null = null;

export function publishResume(payload: ResumePayload) {
  pendingResume = payload;
  window.dispatchEvent(new CustomEvent('helm:resume-session', { detail: payload }));
}

export function consumePendingResume(): ResumePayload | null {
  const payload = pendingResume;
  pendingResume = null;
  return payload;
}
