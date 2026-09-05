import { describe, expect, it } from 'vitest';
import type { ReadinessReport } from '../settings/api';
import {
  buildReadinessItems,
  deriveAgentReadiness,
  deriveDirectoryReadiness,
  deriveProviderReadiness,
  engineDisplayName,
  engineEffortTiers,
  isTaskReady,
  permissionOptions,
  planAgentInstall,
  readyCount,
  REASONING_EFFORT_LABELS,
  turnModeOptions,
} from './newTaskViewModel';

interface FakeEngineOverride {
  installed: boolean;
}

function fakeReport(
  overrides?: {
    claudeCode?: FakeEngineOverride;
    codex?: FakeEngineOverride;
  } & {
    boundEngines?: string[];
    cwdPath?: string;
    cwdConfigured?: boolean;
    cwdExists?: boolean;
  },
): ReadinessReport {
  const engine = (installed: boolean) => ({
    installed,
    path: installed ? 'C:/cli.exe' : null,
    version: installed ? '1.0.0' : null,
    error: installed ? null : 'not found',
    login: { state: 'unknown' as const, detail: '' },
  });
  const path = overrides?.cwdPath ?? '';
  const claude = overrides?.claudeCode ?? { installed: true };
  const codex = overrides?.codex ?? { installed: true };
  return {
    claudeCode: engine(claude.installed),
    codex: engine(codex.installed),
    hasProvider: true,
    hasReadyProvider: true,
    defaultEngine: 'claude-code',
    boundEngines: overrides?.boundEngines ?? ['claude-code'],
    cwd: {
      configured: overrides?.cwdConfigured ?? path.trim().length > 0,
      exists: overrides?.cwdExists ?? true,
      path,
    },
  };
}

const readyDeps = {
  node: { available: true, version: '22.0.0' },
  npm: { available: true, version: '10.0.0' },
  git: { available: true, version: '2.45.0' },
};

describe('deriveAgentReadiness', () => {
  it('CLI 与 Git 都可用时为就绪', () => {
    const { item, deps } = deriveAgentReadiness({
      engine: 'claude-code',
      cliInstalled: true,
      gitAvailable: true,
      installing: false,
    });
    expect(item.state).toBe('ready');
    expect(item.detail).toBe('Agent CLI 与 Git 均已通过检测');
    expect(item.actionLabel).toBeUndefined();
    expect(deps.every((dep) => dep.state === 'ok')).toBe(true);
  });

  it('CLI 与 Git 都缺失时给出组合文案与安装按钮', () => {
    const { item, deps } = deriveAgentReadiness({
      engine: 'codex',
      cliInstalled: false,
      gitAvailable: false,
      installing: false,
    });
    expect(item.state).toBe('missing');
    expect(item.detail).toBe('需要安装 Codex 与 Git');
    expect(item.actionLabel).toBe('安装 Agent 与 Git');
    expect(deps.map((dep) => dep.state)).toEqual(['missing', 'missing']);
  });

  it('只缺 CLI 时提示 Git 已就绪', () => {
    const { item } = deriveAgentReadiness({
      engine: 'claude-code',
      cliInstalled: false,
      gitAvailable: true,
      installing: false,
    });
    expect(item.detail).toBe('Git 已就绪，还需安装 Claude Code');
    expect(item.actionLabel).toBe('下载并安装 Agent');
  });

  it('只缺 Git 时提示安装 Git', () => {
    const { item } = deriveAgentReadiness({
      engine: 'claude-code',
      cliInstalled: true,
      gitAvailable: false,
      installing: false,
    });
    expect(item.detail).toBe('Claude Code 可运行，还需安装 Git');
    expect(item.actionLabel).toBe('安装 Git');
  });

  it('安装中显示复检文案而不是缺失文案', () => {
    const { item } = deriveAgentReadiness({
      engine: 'claude-code',
      cliInstalled: false,
      gitAvailable: false,
      installing: true,
    });
    expect(item.state).toBe('installing');
    expect(item.detail).toBe('正在准备当前缺失项，完成后自动复检');
    expect(item.actionLabel).toBeUndefined();
  });
});

describe('deriveProviderReadiness', () => {
  it('当前引擎有绑定时就绪', () => {
    const item = deriveProviderReadiness({ boundEngines: ['claude-code'] }, 'claude-code');
    expect(item.state).toBe('ready');
    expect(item.detail).toContain('已配置');
  });

  it('当前引擎无绑定时缺失并指向去配置', () => {
    const item = deriveProviderReadiness({ boundEngines: ['claude-code'] }, 'codex');
    expect(item.state).toBe('missing');
    expect(item.actionLabel).toBe('去配置');
  });

  it('报告未加载时按缺失处理（不伪造就绪）', () => {
    const item = deriveProviderReadiness(null, 'claude-code');
    expect(item.state).toBe('missing');
  });
});

describe('deriveDirectoryReadiness', () => {
  it('已配置且存在时显示目录名', () => {
    const item = deriveDirectoryReadiness({ path: 'D:/work/helm', exists: true });
    expect(item.state).toBe('ready');
    expect(item.detail).toBe('当前目录 · helm');
  });

  it('已配置但不存在时要求重新选择', () => {
    const item = deriveDirectoryReadiness({ path: 'D:/gone', exists: false });
    expect(item.state).toBe('missing');
    expect(item.detail).toContain('不存在');
    expect(item.actionLabel).toBe('选择目录');
  });

  it('未配置时提示尚未选择', () => {
    const item = deriveDirectoryReadiness({ path: '  ', exists: false });
    expect(item.state).toBe('missing');
    expect(item.detail).toBe('尚未选择任务要操作的目录');
  });
});

describe('buildReadinessItems / isTaskReady', () => {
  it('全部真实来源就绪时三项齐备', () => {
    const { items } = buildReadinessItems({
      report: fakeReport(),
      deps: readyDeps,
      engine: 'claude-code',
      directory: { path: 'D:/work/helm', exists: true },
      agentInstalling: false,
    });
    expect(items.map((item) => item.key)).toEqual(['agent', 'provider', 'directory']);
    expect(isTaskReady(items)).toBe(true);
    expect(readyCount(items)).toBe(3);
  });

  it('报告缺失时 agent/provider 不假成就绪，不能发送', () => {
    const { items } = buildReadinessItems({
      report: null,
      deps: null,
      engine: 'claude-code',
      directory: { path: '', exists: false },
      agentInstalling: false,
    });
    expect(isTaskReady(items)).toBe(false);
    expect(readyCount(items)).toBe(0);
  });

  it('切换引擎后服务商检查跟随引擎的绑定状态', () => {
    const report = fakeReport({ boundEngines: ['claude-code'] });
    const claudeSide = buildReadinessItems({
      report,
      deps: readyDeps,
      engine: 'claude-code',
      directory: { path: 'D:/w', exists: true },
      agentInstalling: false,
    });
    const codexSide = buildReadinessItems({
      report,
      deps: readyDeps,
      engine: 'codex',
      directory: { path: 'D:/w', exists: true },
      agentInstalling: false,
    });
    expect(claudeSide.items[1].state).toBe('ready');
    expect(codexSide.items[1].state).toBe('missing');
  });

  it('Codex CLI 缺失而 Claude 正常时 agent 行跟随当前引擎', () => {
    const report = fakeReport({ codex: { installed: false } });
    const { items } = buildReadinessItems({
      report,
      deps: readyDeps,
      engine: 'codex',
      directory: { path: 'D:/w', exists: true },
      agentInstalling: false,
    });
    expect(items[0].state).toBe('missing');
    expect(engineDisplayName('codex')).toBe('Codex');
  });
});

describe('planAgentInstall', () => {
  it('CLI 与 Git 都缺：node → cli → git', () => {
    expect(planAgentInstall({ cliInstalled: false, gitAvailable: false })).toEqual([
      'node',
      'cli',
      'git',
    ]);
  });

  it('只缺 Git：node → git（跳过 CLI 安装）', () => {
    expect(planAgentInstall({ cliInstalled: true, gitAvailable: false })).toEqual(['node', 'git']);
  });

  it('全部就绪时仍保留 node 幂等探测步', () => {
    expect(planAgentInstall({ cliInstalled: true, gitAvailable: true })).toEqual(['node']);
  });
});

describe('选项契约', () => {
  it('模式选项含构建/计划/询问；Codex 计划说明标注近似实现', () => {
    const options = turnModeOptions('codex').map((option) => option.value);
    expect(options).toEqual(['build', 'plan', 'ask']);
    const codexPlan = turnModeOptions('codex').find((option) => option.value === 'plan');
    expect(codexPlan?.desc).toContain('近似');
  });

  it('全部放开必须要求确认且标记高风险', () => {
    const fullAccess = permissionOptions().find((option) => option.value === 'full_access');
    expect(fullAccess?.confirm).toBe(true);
    expect(fullAccess?.tone).toBe('danger');
    expect(fullAccess?.hint).toContain('高风险');
  });

  it('推理强度标签覆盖协议全档位且 auto 表示模型默认', () => {
    expect(Object.keys(REASONING_EFFORT_LABELS).sort()).toEqual(
      ['auto', 'high', 'low', 'max', 'medium', 'minimal', 'none', 'xhigh'].sort(),
    );
    expect(REASONING_EFFORT_LABELS.auto).toBe('自动');
  });

  it('推理强度档位跟随 Agent：Claude/Codex 各自展示 CLI 声明过的档位集（第五轮决议）', () => {
    expect(engineEffortTiers('claude-code')).toEqual([
      'auto',
      'low',
      'medium',
      'high',
      'xhigh',
      'max',
    ]);
    expect(engineEffortTiers('codex')).toEqual([
      'auto',
      'minimal',
      'low',
      'medium',
      'high',
      'xhigh',
    ]);
    // 每一档都必须有中文标签，菜单不允许出现空文案项。
    for (const engine of ['claude-code', 'codex'] as const) {
      for (const tier of engineEffortTiers(engine)) {
        expect(REASONING_EFFORT_LABELS[tier]).toBeTruthy();
      }
    }
  });

  // 快捷开始已按 2026-08-23 第二轮用户决议整块移除（原型让位，登记于 docs/已知限制.md），
  // 不再保留 QUICK_STARTERS 数据契约。
});
