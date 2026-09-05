/**
 * 轮次结束通知判据（G-15）。
 *
 * 系统通知只在「同一线程内 working → idle」时触发——那才是轮次真实结束。
 * 切会话 / 新建会话会整体换线程并强制置 idle（resume_handle / reset），
 * 旧会话轮次仍在后台跑（P3-3 保活），不得误报「轮次已完成」。
 */
export interface TurnEndTransition {
  status: 'idle' | 'working';
  historyId: string | null;
}

export function isGenuineTurnEnd(prev: TurnEndTransition, next: TurnEndTransition): boolean {
  return prev.status === 'working' && next.status === 'idle' && prev.historyId === next.historyId;
}
