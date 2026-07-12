import { describe, expect, it } from 'vitest';
import { DEFAULT_SETTINGS } from './types';
import {
  addCommandAllowlistPattern,
  approvalModeFromSettings,
  defaultTurnModeFromSettings,
  engineConfigWithDetection,
  removeCommandAllowlistPattern,
  sessionDefaultsFromSettings,
  shouldReopenLastSession,
  toggleApprovalSettings,
  updateStatusSummary,
  workspaceIdentityFromSettings,
} from './settingsViewModel';

describe('settings view model', () => {
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

  it('maps the stored permissionMode to the default turn mode for new sessions', () => {
    // 变更-04 §0.3：plan → 计划；auto 与旧「写入询问」ask 都回落构建
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

  it('derives the workspace approval toggle from persisted command policy', () => {
    expect(approvalModeFromSettings(DEFAULT_SETTINGS)).toBe('manual');
    expect(
      approvalModeFromSettings({
        ...DEFAULT_SETTINGS,
        general: { ...DEFAULT_SETTINGS.general, confirmBeforeCommand: false },
        permissions: { ...DEFAULT_SETTINGS.permissions, runCommands: 'allow' },
      }),
    ).toBe('direct');
  });

  it('toggles the approval shortcut into the settings consumed by the CLI policy', () => {
    expect(toggleApprovalSettings(DEFAULT_SETTINGS).general.confirmBeforeCommand).toBe(false);
    expect(toggleApprovalSettings(DEFAULT_SETTINGS).permissions.runCommands).toBe('allow');

    const direct = {
      ...DEFAULT_SETTINGS,
      general: { ...DEFAULT_SETTINGS.general, confirmBeforeCommand: false },
      permissions: { ...DEFAULT_SETTINGS.permissions, runCommands: 'allow' as const },
    };
    expect(toggleApprovalSettings(direct).general.confirmBeforeCommand).toBe(true);
    expect(toggleApprovalSettings(direct).permissions.runCommands).toBe('ask');
  });

  it('keeps command allowlist edits reversible and duplicate-free', () => {
    const permissions = {
      ...DEFAULT_SETTINGS.permissions,
      commandAllowlist: ['git status', 'pnpm test *'],
    };

    expect(addCommandAllowlistPattern(permissions, ' git status ')).toBe(permissions);
    expect(addCommandAllowlistPattern(permissions, 'cargo test').commandAllowlist).toEqual([
      'git status',
      'pnpm test *',
      'cargo test',
    ]);
    expect(removeCommandAllowlistPattern(permissions, 'git status').commandAllowlist).toEqual([
      'pnpm test *',
    ]);
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

  it('turns the workspace name setting into shell identity text', () => {
    expect(
      workspaceIdentityFromSettings({
        ...DEFAULT_SETTINGS,
        general: { ...DEFAULT_SETTINGS.general, workspaceName: 'Helm 产品组' },
      }),
    ).toEqual({
      name: 'Helm 产品组',
      avatar: 'H',
    });

    expect(
      workspaceIdentityFromSettings({
        ...DEFAULT_SETTINGS,
        general: { ...DEFAULT_SETTINGS.general, workspaceName: '  ' },
      }).name,
    ).toBe('Helm 工作区');
  });
});
