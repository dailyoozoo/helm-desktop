import { describe, expect, it } from 'vitest';
import { DEFAULT_SETTINGS } from './types';
import {
  defaultTurnModeForEngine,
  defaultTurnModeFromSettings,
  engineConfigWithDetection,
  mostRecentSessionId,
  pricingFeedUrlsFromDraft,
  sessionDefaultsFromSettings,
  shouldReopenLastSession,
  startupLandingFromRecovery,
  updateStatusSummary,
  workspaceIdentityFromSettings,
} from './settingsViewModel';

describe('settings view model', () => {
  it('normalizes pricing feed drafts only when the editor commits the multiline value', () => {
    expect(
      pricingFeedUrlsFromDraft(
        ' https://mirror-a.example/catalog.json\n\nhttps://mirror-b.example/catalog.json  ',
      ),
    ).toEqual(['https://mirror-a.example/catalog.json', 'https://mirror-b.example/catalog.json']);
  });

  it('turns settings into defaults for a new workspace session', () => {
    const defaults = sessionDefaultsFromSettings({
      ...DEFAULT_SETTINGS,
      general: {
        ...DEFAULT_SETTINGS.general,
        defaultDirectory: 'D:\\work\\helm',
      },
      engines: {
        ...DEFAULT_SETTINGS.engines,
        defaultEngine: 'codex',
      },
    });

    expect(defaults).toEqual({
      engine: 'codex',
      cwd: 'D:\\work\\helm',
    });
  });

  it('maps the stored permissionMode and engine to the default turn mode for new sessions', () => {
    // 两个引擎使用同一用户默认值，不再由旧 evidence 强制 Codex 进入计划模式。
    const withMode = (permissionMode: 'auto' | 'ask' | 'plan') => ({
      ...DEFAULT_SETTINGS,
      engines: {
        ...DEFAULT_SETTINGS.engines,
        claudeCode: { ...DEFAULT_SETTINGS.engines.claudeCode, permissionMode },
      },
    });
    expect(defaultTurnModeFromSettings(withMode('plan'))).toBe('plan');
    expect(defaultTurnModeFromSettings(withMode('auto'))).toBe('build');
    expect(defaultTurnModeFromSettings(withMode('ask'))).toBe('build');
    expect(defaultTurnModeForEngine(withMode('auto'), 'codex')).toBe('build');
    expect(
      defaultTurnModeFromSettings({
        ...withMode('auto'),
        engines: { ...withMode('auto').engines, defaultEngine: 'codex' },
      }),
    ).toBe('build');
  });

  it('turns CLI detection results into the engine config used at launch time', () => {
    expect(
      engineConfigWithDetection(
        {
          id: 'codex',
          name: 'Codex',
          bin: 'codex',
          defaultModel: 'gpt-5-codex',
          status: 'missing',
          version: null,
        },
        { path: 'C:\\Users\\demo\\AppData\\Roaming\\npm\\codex.cmd', version: 'codex 1.2.3' },
      ),
    ).toEqual({
      id: 'codex',
      name: 'Codex',
      bin: 'C:\\Users\\demo\\AppData\\Roaming\\npm\\codex.cmd',
      defaultModel: 'gpt-5-codex',
      status: 'ready',
      version: 'codex 1.2.3',
    });
  });

  it('only reopens the last session on an empty workspace when the setting is enabled', () => {
    expect(
      shouldReopenLastSession(DEFAULT_SETTINGS, {
        handleId: null,
        sessionId: null,
        itemsLength: 0,
      }),
    ).toBe(true);
    expect(
      shouldReopenLastSession(
        { ...DEFAULT_SETTINGS, general: { ...DEFAULT_SETTINGS.general, reopenLastSession: false } },
        { handleId: null, sessionId: null, itemsLength: 0 },
      ),
    ).toBe(false);
    expect(
      shouldReopenLastSession(DEFAULT_SETTINGS, {
        handleId: 'handle-1',
        sessionId: null,
        itemsLength: 1,
      }),
    ).toBe(false);
  });

  it('startup landing keeps the workspace when the pointer session exists (auto-resume handles it)', () => {
    expect(
      startupLandingFromRecovery(DEFAULT_SETTINGS, {
        hasActiveSession: true,
        recentSessionId: 'session-1',
      }),
    ).toEqual({ kind: 'workspace' });
  });

  it('startup landing falls back to the most recent session when the pointer is missing', () => {
    // 截图场景：有 35 个历史会话但指针为空 → 兜底打开最近会话，而不是工作区空态
    expect(
      startupLandingFromRecovery(DEFAULT_SETTINGS, {
        hasActiveSession: false,
        recentSessionId: 'session-7',
      }),
    ).toEqual({ kind: 'recent', sessionId: 'session-7' });
  });

  it('startup landing goes to the new task page only when there is nothing to reopen', () => {
    expect(
      startupLandingFromRecovery(DEFAULT_SETTINGS, {
        hasActiveSession: false,
        recentSessionId: null,
      }),
    ).toEqual({ kind: 'home' });
  });

  it('startup landing honors reopenLastSession=false with the legacy workspace empty state', () => {
    const disabled = {
      ...DEFAULT_SETTINGS,
      general: { ...DEFAULT_SETTINGS.general, reopenLastSession: false },
    };
    expect(
      startupLandingFromRecovery(disabled, {
        hasActiveSession: false,
        recentSessionId: 'session-1',
      }),
    ).toEqual({ kind: 'workspace' });
  });

  it('mostRecentSessionId picks the latest non-archived session by updatedAt', () => {
    const sessions = [
      { id: 'old', updatedAt: 100 },
      { id: 'archived-newest', archived: true, updatedAt: 900 },
      { id: 'newest', updatedAt: 500 },
      { id: 'middle', updatedAt: 300 },
    ];
    expect(mostRecentSessionId(sessions)).toBe('newest');
    expect(mostRecentSessionId([{ id: 'only-archived', archived: true, updatedAt: 1 }])).toBeNull();
    expect(mostRecentSessionId([])).toBeNull();
  });

  it('summarizes unavailable update checks without pretending the app is current', () => {
    expect(
      updateStatusSummary({
        currentVersion: '0.1.0',
        channel: 'beta',
        canCheck: false,
        message: '未配置自动更新发布源；当前仅保存更新通道偏好。',
      }),
    ).toBe('当前版本 v0.1.0 · 未配置自动更新发布源；当前仅保存更新通道偏好。');
  });

  it('always returns the fixed Helm workspace identity (settings input removed)', () => {
    expect(workspaceIdentityFromSettings(DEFAULT_SETTINGS)).toEqual({
      name: 'Helm 工作区',
      avatar: 'H',
    });
  });
});
