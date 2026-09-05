import { afterEach, describe, expect, it, vi } from 'vitest';

// 阻断真实 api.ts 顶层加载（Tauri invoke），仅探测映射逻辑需要这两个返回值
vi.mock('./api', () => ({
  detectEngine: vi.fn(),
  detectWorkspaceDeps: vi.fn(),
  getReadinessReport: vi.fn(),
  installCliEngine: vi.fn(),
  installGit: vi.fn(),
  selectDirectory: vi.fn(),
}));
vi.mock('../providers/api', () => ({
  getProviderConfig: vi.fn(),
  saveEngineConfig: vi.fn(),
}));

import {
  SETUP_WIZARD_DISMISS_KEY,
  dismissSetupWizard,
  probeSetupWizardReadiness,
  readSetupWizardDismissed,
  selectGuideEngine,
  setupWizardAllReady,
  shouldAutoShowSetupWizard,
  type SetupWizardReadiness,
} from './SetupWizardModal';
import { detectWorkspaceDeps, getReadinessReport } from './api';

const allReady: SetupWizardReadiness = {
  claudeInstalled: true,
  claudeDetail: '1.0.0',
  codexInstalled: false,
  codexDetail: '',
  gitReady: true,
  gitDetail: 'git version 2.45',
  hasReadyProvider: true,
  cwdOk: true,
  cwdPath: 'D:\\proj',
};

describe('SetupWizardModal · 首启弹出判定（纯函数）', () => {
  it('setupWizardAllReady：CLI（任装其一）+ Git + 服务商 + 目录全就绪才算完成', () => {
    expect(setupWizardAllReady(allReady)).toBe(true);
    expect(setupWizardAllReady({ ...allReady, gitReady: false })).toBe(false);
    expect(setupWizardAllReady({ ...allReady, hasReadyProvider: false })).toBe(false);
    expect(setupWizardAllReady({ ...allReady, cwdOk: false })).toBe(false);
    // 两个 CLI 都没装 → 未就绪
    expect(
      setupWizardAllReady({ ...allReady, claudeInstalled: false, codexInstalled: false }),
    ).toBe(false);
    // 只装 Codex 也算 CLI 项就绪
    expect(setupWizardAllReady({ ...allReady, claudeInstalled: false, codexInstalled: true })).toBe(
      true,
    );
  });

  it('shouldAutoShowSetupWizard：未全就绪才弹，就绪则无感', () => {
    expect(shouldAutoShowSetupWizard(allReady)).toBe(false);
    expect(shouldAutoShowSetupWizard({ ...allReady, gitReady: false })).toBe(true);
  });
});

describe('SetupWizardModal · 引导引擎选择（selectGuideEngine）', () => {
  it('只装 Codex → 按 codex 引导', () => {
    expect(selectGuideEngine({ claudeInstalled: false, codexInstalled: true })).toBe('codex');
  });

  it('只装 / 两个都装 / 两个都没装 → 默认 claude-code 引导', () => {
    expect(selectGuideEngine({ claudeInstalled: true, codexInstalled: false })).toBe('claude-code');
    expect(selectGuideEngine({ claudeInstalled: true, codexInstalled: true })).toBe('claude-code');
    expect(selectGuideEngine({ claudeInstalled: false, codexInstalled: false })).toBe(
      'claude-code',
    );
  });
});

describe('SetupWizardModal · probeSetupWizardReadiness 映射', () => {
  it('四项取自 readiness report 与工作区依赖探测（含 codex 双引擎）', async () => {
    vi.mocked(getReadinessReport).mockResolvedValue({
      claudeCode: { installed: true, version: '1.2.3', login: { state: 'ok', detail: '' } },
      codex: { installed: false, version: null, login: { state: 'unknown', detail: '未登录' } },
      hasReadyProvider: true,
      cwd: { configured: true, exists: true, path: 'D:\\proj' },
    } as never);
    vi.mocked(detectWorkspaceDeps).mockResolvedValue({
      node: { available: true, version: 'v22' },
      npm: { available: true, version: '10' },
      git: { available: true, version: 'git version 2.45' },
    } as never);

    const readiness = await probeSetupWizardReadiness();
    expect(readiness.claudeInstalled).toBe(true);
    expect(readiness.claudeDetail).toBe('1.2.3');
    expect(readiness.codexInstalled).toBe(false);
    expect(readiness.codexDetail).toBe('未检测到 codex CLI');
    expect(readiness.gitReady).toBe(true);
    expect(readiness.gitDetail).toBe('git version 2.45');
    expect(readiness.hasReadyProvider).toBe(true);
    expect(readiness.cwdOk).toBe(true);
    expect(readiness.cwdPath).toBe('D:\\proj');
  });

  it('codex 已装时版本与登录态拼接进 detail，可驱动 codex 引导', async () => {
    vi.mocked(getReadinessReport).mockResolvedValue({
      claudeCode: { installed: false, version: null, login: { state: 'unknown', detail: '' } },
      codex: { installed: true, version: '0.49.0', login: { state: 'missing', detail: '未登录' } },
      hasReadyProvider: true,
      cwd: { configured: true, exists: true, path: 'D:\\proj' },
    } as never);
    vi.mocked(detectWorkspaceDeps).mockResolvedValue({
      node: { available: true, version: 'v22' },
      npm: { available: true, version: '10' },
      git: { available: true, version: 'git version 2.45' },
    } as never);

    const readiness = await probeSetupWizardReadiness();
    expect(readiness.codexDetail).toBe('0.49.0 · 未登录');
    expect(selectGuideEngine(readiness)).toBe('codex');
    expect(setupWizardAllReady(readiness)).toBe(true);
  });

  it('CLI 未安装时显示错误/兜底文案；目录已配置但不存在不算就绪', async () => {
    vi.mocked(getReadinessReport).mockResolvedValue({
      claudeCode: {
        installed: false,
        version: '',
        login: { state: 'ok', detail: '' },
        error: '未检测到 claude CLI',
      },
      codex: { installed: false, version: '', login: { state: 'unknown', detail: '' } },
      hasReadyProvider: false,
      cwd: { configured: true, exists: false, path: 'D:\\gone' },
    } as never);
    vi.mocked(detectWorkspaceDeps).mockResolvedValue({
      node: { available: true, version: 'v22' },
      npm: { available: true, version: '10' },
      git: { available: false },
    } as never);

    const readiness = await probeSetupWizardReadiness();
    expect(readiness.claudeDetail).toBe('未检测到 claude CLI');
    expect(readiness.gitReady).toBe(false);
    expect(readiness.cwdOk).toBe(false);
    expect(setupWizardAllReady(readiness)).toBe(false);
  });
});

describe('SetupWizardModal · 跳过标记（localStorage）', () => {
  afterEach(() => {
    delete (globalThis as unknown as { window?: unknown }).window;
  });

  it('无 window（node 环境/异常）时读取返回 false、写入静默', () => {
    expect(readSetupWizardDismissed()).toBe(false);
    expect(() => dismissSetupWizard()).not.toThrow();
  });

  it('dismiss 后读取为 true；key 固定便于跨版本识别', () => {
    const store = new Map<string, string>();
    (globalThis as unknown as { window: unknown }).window = {
      localStorage: {
        getItem: (key: string) => store.get(key) ?? null,
        setItem: (key: string, value: string) => void store.set(key, value),
      },
    };
    expect(readSetupWizardDismissed()).toBe(false);
    dismissSetupWizard();
    expect(store.get(SETUP_WIZARD_DISMISS_KEY)).toBe('1');
    expect(readSetupWizardDismissed()).toBe(true);
  });
});
