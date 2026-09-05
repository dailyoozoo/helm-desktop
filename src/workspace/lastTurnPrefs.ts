import type { SessionTurn } from '../sessions/api';
import type { PermissionProfile, TurnMode } from '../engine/transport';

/**
 * 恢复会话时的「模式 + 权限组合」持久化（2026-09-04 用户规格）：
 * 打开/恢复对话默认沿用该会话最后一轮使用的模式与权限组合；
 * 对话内切换过就取最新（最后一轮即最新切换结果）。
 *
 * 权限红线（后端同源约束）：`safe_permission_profile` 存储列刻意只接受
 * standard/auto（sessions.rs set_safe_permission_profile fail-closed），
 * full_access 是高风险 lease、重启即失效。恢复历史会话时即使最后一轮
 * 是 full_access，也只降档到 auto，绝不自动恢复「全部放开」；
 * 确认态（fullAccessConfirmed）由调用方重置为 false。
 */
export interface LastTurnPrefs {
  mode: TurnMode;
  permissionProfile: Exclude<PermissionProfile, 'full_access'>;
}

const TURN_MODES: readonly TurnMode[] = ['build', 'plan', 'ask'];

export function normalizeRestoredMode(value: unknown): TurnMode {
  return TURN_MODES.includes(value as TurnMode) ? (value as TurnMode) : 'build';
}

/**
 * 恢复档位映射：standard/auto 原样保留；full_access lease 不跨会话恢复，
 * 降一档到 auto（用户日常组合即自动执行）；未知值保守回落 standard。
 */
export function normalizeRestoredProfile(value: unknown): 'standard' | 'auto' {
  if (value === 'standard') return 'standard';
  if (value === 'auto' || value === 'full_access') return 'auto';
  return 'standard';
}

/** 取逐轮账本最后一轮（按 epoch，turns 本身已按 started_at 升序）的组合；无轮次返回 null。 */
export function lastTurnPrefs(turns: SessionTurn[] | null | undefined): LastTurnPrefs | null {
  if (!turns || turns.length === 0) return null;
  let last = turns[0];
  for (const turn of turns) {
    if (turn.epoch >= last.epoch) last = turn;
  }
  return {
    mode: normalizeRestoredMode(last.mode),
    permissionProfile: normalizeRestoredProfile(last.permissionProfile),
  };
}
