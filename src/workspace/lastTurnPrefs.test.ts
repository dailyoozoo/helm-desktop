import { describe, expect, it } from 'vitest';
import { lastTurnPrefs, normalizeRestoredMode, normalizeRestoredProfile } from './lastTurnPrefs';
import type { SessionTurn } from '../sessions/api';

function turn(overrides: Partial<SessionTurn> & { epoch: number }): SessionTurn {
  return {
    id: `turn-${overrides.epoch}`,
    mode: 'build',
    permissionProfile: 'standard',
    status: 'succeeded',
    startedAt: 1000,
    ...overrides,
  } as SessionTurn;
}

describe('lastTurnPrefs（模式+权限组合持久化，9/4 用户规格）', () => {
  it('无轮次（未开场/旧数据缺 turns）返回 null，调用方回落原默认', () => {
    expect(lastTurnPrefs(null)).toBeNull();
    expect(lastTurnPrefs(undefined)).toBeNull();
    expect(lastTurnPrefs([])).toBeNull();
  });

  it('单轮会话取该轮的组合', () => {
    const prefs = lastTurnPrefs([turn({ epoch: 1, mode: 'build', permissionProfile: 'auto' })]);
    expect(prefs).toEqual({ mode: 'build', permissionProfile: 'auto' });
  });

  it('对话内切换过取最后一轮（恢复对话按最近一次的组合）', () => {
    const prefs = lastTurnPrefs([
      turn({ epoch: 1, mode: 'build', permissionProfile: 'auto' }),
      turn({ epoch: 2, mode: 'plan', permissionProfile: 'standard' }),
      turn({ epoch: 3, mode: 'ask', permissionProfile: 'auto' }),
    ]);
    expect(prefs).toEqual({ mode: 'ask', permissionProfile: 'auto' });
  });

  it('按 epoch 取最大而非数组末位（容忍乱序）', () => {
    const prefs = lastTurnPrefs([
      turn({ epoch: 7, mode: 'plan', permissionProfile: 'standard' }),
      turn({ epoch: 3, mode: 'ask', permissionProfile: 'auto' }),
    ]);
    expect(prefs).toEqual({ mode: 'plan', permissionProfile: 'standard' });
  });

  it('full_access 不自动恢复：降档 auto（lease 只对本会话生效）', () => {
    const prefs = lastTurnPrefs([turn({ epoch: 1, permissionProfile: 'full_access' })]);
    expect(prefs?.permissionProfile).toBe('auto');
  });

  it('未知模式/权限保守回落 build/standard', () => {
    expect(normalizeRestoredMode('legacy')).toBe('build');
    expect(normalizeRestoredProfile('mystery')).toBe('standard');
    expect(normalizeRestoredProfile('standard')).toBe('standard');
  });
});
