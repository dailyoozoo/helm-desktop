import type { EngineId, ReasoningEffort } from '@helm/protocol';
import type { PermissionProfile, TurnMode } from '../engine/transport';
import type { ReadinessReport, WorkspaceDeps } from '../settings/api';

/** S2 · 新任务页启动配置：随首条消息一起交给工作区，创建会话时一次性生效。 */
export interface NewTaskLaunchConfig {
  engine: EngineId;
  cwd: string;
  mode: TurnMode;
  permissionProfile: PermissionProfile;
  /** 新任务页已确认全部放开；工作区首次 create_session 必须带上，避免二次弹窗。 */
  fullAccessConfirmed?: boolean;
  model?: string;
  reasoningEffort?: ReasoningEffort;
}

export type ReadinessKey = 'agent' | 'provider' | 'directory';
export type ReadinessState = 'ready' | 'missing' | 'installing';

/** 三项就绪检查中的一行（Agent / 服务商 / 目录）。 */
export interface ReadinessItem {
  key: ReadinessKey;
  state: ReadinessState;
  title: string;
  detail: string;
  /** 未就绪时的行动按钮文案；ready 时省略。 */
  actionLabel?: string;
}

/** Agent 行内的依赖小状态（CLI / Git）。 */
export interface ReadinessDep {
  id: 'cli' | 'git';
  label: string;
  state: 'ok' | 'missing' | 'installing';
}

export function engineDisplayName(engine: EngineId): string {
  return engine === 'codex' ? 'Codex' : 'Claude Code';
}

/** ReadinessReport 的引擎键是 claudeCode/codex，与 EngineId 不同，统一在此映射。 */
export function engineReadiness(
  report: Pick<ReadinessReport, 'claudeCode' | 'codex'>,
  engine: EngineId,
): ReadinessReport['claudeCode'] {
  return engine === 'codex' ? report.codex : report.claudeCode;
}

/**
 * Agent 就绪 = 当前引擎 CLI 已安装 且 Git 可用。
 * detail 文案与原型 index.html 的 readiness 文案对齐。
 */
export function deriveAgentReadiness(input: {
  engine: EngineId;
  cliInstalled: boolean;
  gitAvailable: boolean;
  installing: boolean;
}): { item: ReadinessItem; deps: ReadinessDep[] } {
  const name = engineDisplayName(input.engine);
  const ready = input.cliInstalled && input.gitAvailable;
  const deps: ReadinessDep[] = [
    {
      id: 'cli',
      label: 'Agent CLI',
      state: input.cliInstalled ? 'ok' : input.installing ? 'installing' : 'missing',
    },
    {
      id: 'git',
      label: 'Git for Windows',
      state: input.gitAvailable ? 'ok' : input.installing ? 'installing' : 'missing',
    },
  ];
  if (ready) {
    return {
      item: { key: 'agent', state: 'ready', title: name, detail: 'Agent CLI 与 Git 均已通过检测' },
      deps,
    };
  }
  if (input.installing) {
    return {
      item: {
        key: 'agent',
        state: 'installing',
        title: name,
        detail: '正在准备当前缺失项，完成后自动复检',
      },
      deps,
    };
  }
  const detail =
    !input.cliInstalled && !input.gitAvailable
      ? `需要安装 ${name} 与 Git`
      : !input.cliInstalled
        ? `Git 已就绪，还需安装 ${name}`
        : `${name} 可运行，还需安装 Git`;
  const actionLabel =
    !input.cliInstalled && !input.gitAvailable
      ? '安装 Agent 与 Git'
      : !input.cliInstalled
        ? '下载并安装 Agent'
        : '安装 Git';
  return {
    item: { key: 'agent', state: 'missing', title: name, detail, actionLabel },
    deps,
  };
}

/**
 * 服务商就绪：当前引擎存在生效绑定即视为可用于本任务，
 * 与工作区发送前置校验同一口径（bindings.some(engineId)）。
 */
export function deriveProviderReadiness(
  report: Pick<ReadinessReport, 'boundEngines'> | null,
  engine: EngineId,
): ReadinessItem {
  const bound = Boolean(report && report.boundEngines.includes(engine));
  return bound
    ? {
        key: 'provider',
        state: 'ready',
        title: '服务商配置',
        detail: '已配置当前 Agent 可用的服务商',
      }
    : {
        key: 'provider',
        state: 'missing',
        title: '服务商配置',
        detail: '尚无可用于当前 Agent 的服务商',
        actionLabel: '去配置',
      };
}

/** 工作目录就绪：已选择且真实存在。 */
export function deriveDirectoryReadiness(cwd: { path: string; exists: boolean }): ReadinessItem {
  const configured = cwd.path.trim().length > 0;
  if (configured && cwd.exists) {
    const name = cwd.path.split(/[\\/]/).filter(Boolean).pop() ?? cwd.path;
    return {
      key: 'directory',
      state: 'ready',
      title: '工作目录',
      detail: `当前目录 · ${name}`,
    };
  }
  if (configured && !cwd.exists) {
    return {
      key: 'directory',
      state: 'missing',
      title: '工作目录',
      detail: '目录不存在或已无法访问，请重新选择',
      actionLabel: '选择目录',
    };
  }
  return {
    key: 'directory',
    state: 'missing',
    title: '工作目录',
    detail: '尚未选择任务要操作的目录',
    actionLabel: '选择目录',
  };
}

/** 汇总三项检查；agent 行的 installing 状态由调用方传入。 */
export function buildReadinessItems(args: {
  report: ReadinessReport | null;
  deps: WorkspaceDeps | null;
  engine: EngineId;
  directory: { path: string; exists: boolean };
  agentInstalling: boolean;
}): { items: ReadinessItem[]; agentDeps: ReadinessDep[] } {
  const cliInstalled = args.report ? engineReadiness(args.report, args.engine).installed : false;
  const gitAvailable = args.deps ? args.deps.git.available : false;
  const agent = deriveAgentReadiness({
    engine: args.engine,
    cliInstalled,
    gitAvailable,
    installing: args.agentInstalling,
  });
  const items: ReadinessItem[] = [
    agent.item,
    deriveProviderReadiness(args.report, args.engine),
    deriveDirectoryReadiness(args.directory),
  ];
  return { items, agentDeps: agent.deps };
}

export function readyCount(items: ReadinessItem[]): number {
  return items.filter((item) => item.state === 'ready').length;
}

export function isTaskReady(items: ReadinessItem[]): boolean {
  return items.length > 0 && items.every((item) => item.state === 'ready');
}

/**
 * Agent 一键安装的真实步骤序列：
 * CLI 安装走 npm（需要 Node），Git 独立安装。顺序固定为 node → cli → git。
 */
export type AgentInstallStep = 'node' | 'cli' | 'git';

export function planAgentInstall(args: {
  cliInstalled: boolean;
  gitAvailable: boolean;
}): AgentInstallStep[] {
  const steps: AgentInstallStep[] = [];
  // install_cli_engine 依赖 npm（installer.rs 在 npm 缺失时直接报错引导装 Node），
  // 因此 CLI 缺失时先确保 Node 存在；Node 已可用时该步是廉价的幂等探测。
  steps.push('node');
  if (!args.cliInstalled) steps.push('cli');
  if (!args.gitAvailable) steps.push('git');
  return steps;
}

/** 任务模式选项（构建/计划/询问）；Codex 无原生计划模式，沿用工作区的近似说明。 */
export function turnModeOptions(engine: EngineId): {
  value: TurnMode;
  label: string;
  hint: string;
  desc: string;
}[] {
  return [
    {
      value: 'build',
      label: '构建',
      hint: '可执行',
      desc: '可写文件、可执行命令；Runtime 询问时显示审批。',
    },
    {
      value: 'plan',
      label: '计划',
      hint: '只规划',
      desc:
        engine === 'codex'
          ? 'Codex 无原生计划模式，以只读沙箱 + 计划指令近似。'
          : '先产出实施方案，确认后再执行。',
    },
    {
      value: 'ask',
      label: '询问',
      hint: '只读',
      desc: '只读，不写文件、不执行写目标命令。',
    },
  ];
}

export interface PermissionOption {
  value: PermissionProfile;
  label: string;
  hint: string;
  desc: string;
  tone: 'normal' | 'warning' | 'danger';
  /** 选择前需要显式确认（全部放开）。 */
  confirm?: boolean;
}

/** 权限档位选项；全部放开必须经过确认且仅当前任务生效。 */
export function permissionOptions(): PermissionOption[] {
  return [
    {
      value: 'standard',
      label: '标准',
      hint: '推荐',
      desc: '读取直通，Runtime 询问时再审批。',
      tone: 'normal',
    },
    {
      value: 'auto',
      label: '自动执行',
      hint: '谨慎使用',
      desc: '额外直通安全网络读取，减少打断。',
      tone: 'warning',
    },
    {
      value: 'full_access',
      label: '全部放开',
      hint: '高风险',
      desc: '跳过审批 · 仅本任务，应用重启后失效。',
      tone: 'danger',
      confirm: true,
    },
  ];
}

export const PERMISSION_LABELS: Record<PermissionProfile, string> = {
  standard: '标准',
  auto: '自动执行',
  full_access: '全部放开',
};

export const TURN_MODE_LABELS: Record<TurnMode, string> = {
  build: '构建',
  plan: '计划',
  ask: '询问',
};

export const REASONING_EFFORT_LABELS: Record<ReasoningEffort, string> = {
  auto: '自动',
  none: '关闭',
  minimal: '极低',
  low: '低',
  medium: '中',
  high: '高',
  xhigh: '超高',
  max: '最大',
};

/**
 * 推理强度档位表已上收 src/reasoning.ts（2026-08-27 用户裁决：新任务页与工作区 Composer
 * 同一张表，强度跟随 Agent；真实探测明确支持时以探测为准，unknown 回落引擎档位表）。
 */
export { engineEffortTiers } from '../reasoning';

// 快捷开始数据已随 2026-08-23 第二轮用户决议移除（原型让位，见 docs/已知限制.md）。

/**
 * 新任务页草稿保护（可靠性检查 D-13）：跳去服务商/插件页前暂存输入，
 * 返回本页时恢复一次即清除。sessionStorage 仅存活当前应用会话，
 * 键名与作用域和原型的 localStorage helm:draft 不同（第二步 Q-2 决议）。
 */
const HOME_DRAFT_KEY = 'helm-home:draft';

export function stashHomeDraft(text: string): void {
  try {
    const value = text.trim();
    if (value) window.sessionStorage.setItem(HOME_DRAFT_KEY, value);
  } catch {
    // 存储不可用（隐私模式等）时静默降级：草稿保护是增强，不是硬依赖。
  }
}

/** 取出并清除暂存草稿；无草稿返回空串。 */
export function takeHomeDraft(): string {
  try {
    const value = window.sessionStorage.getItem(HOME_DRAFT_KEY) ?? '';
    if (value) window.sessionStorage.removeItem(HOME_DRAFT_KEY);
    return value;
  } catch {
    return '';
  }
}
