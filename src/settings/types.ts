// 设置数据模型
export interface AppSettings {
  // 通用
  general: {
    workspaceName: string;
    defaultDirectory: string;
    reopenLastSession: boolean;
    confirmBeforeCommand: boolean;
    anonymousAnalytics: boolean;
    autoUpdateChannel: 'stable' | 'beta';
    updateFeedUrl: string;
    /** 首启引导是否已完成/跳过（完成或显式跳过都置 true） */
    onboardingCompleted?: boolean;
    /** 首轮后用 fast model 自动起标题与摘要（外发到用户绑定的服务商，可关） */
    autoTitleSessions: boolean;
    /** 点关闭按钮时最小化到托盘而不是退出（变更-12）：后台会话继续运行 */
    closeToTray?: boolean;
  };
  // 引擎
  engines: {
    defaultEngine: 'claude-code' | 'codex';
    claudeCode: {
      executablePath: string;
      version: string;
      detected: boolean;
      permissionMode: 'auto' | 'ask' | 'plan';
    };
    codex: {
      executablePath: string;
      version: string;
      detected: boolean;
      sandbox: 'readonly' | 'workspace' | 'full';
    };
  };
  // 权限
  permissions: {
    readFiles: 'allow' | 'ask' | 'deny';
    editFiles: 'allow' | 'ask' | 'deny';
    runCommands: 'allow' | 'ask' | 'deny';
    fetchUrls: 'allow' | 'ask' | 'deny';
    mcpTools: 'allow' | 'ask' | 'deny';
    commandAllowlist: string[];
  };
  // 外观
  appearance: {
    theme: 'light' | 'dark' | 'system';
    accentColor: { base: string; hi: string };
    uiDensity: 'compact' | 'comfortable';
    monospaceFont: string;
    reduceMotion: boolean;
  };
  // 快捷键
  shortcuts: {
    commandPalette: string;
    newSession: string;
    toggleContext: string;
    cycleEngine: string;
    navigationPrefix: string;
    home: string;
    workspace: string;
    providers: string;
    sessions: string;
    extensions: string;
    usage: string;
    settings: string;
  };
  // 并行会话的 worktree 隔离（P3-3）
  worktree: {
    enabled: boolean;
    /** 空 = 默认放在仓库旁边的 `<仓库名>-worktrees/` */
    root: string;
    /** worktree 创建后执行的初始化命令；空 = 不执行 */
    setupScript: string;
  };
}

export interface UpdateStatus {
  currentVersion: string;
  channel: 'stable' | 'beta' | string;
  canCheck: boolean;
  message: string;
}

export const DEFAULT_SETTINGS: AppSettings = {
  general: {
    workspaceName: '我的工作区',
    defaultDirectory:
      typeof window !== 'undefined' && navigator.platform.includes('Win')
        ? 'C:\\Users\\Public\\Documents'
        : '~/code',
    reopenLastSession: true,
    confirmBeforeCommand: true,
    anonymousAnalytics: false,
    autoUpdateChannel: 'stable',
    updateFeedUrl: '',
    onboardingCompleted: false,
    autoTitleSessions: true,
  },
  engines: {
    defaultEngine: 'claude-code',
    claudeCode: {
      executablePath: '',
      version: '',
      detected: false,
      permissionMode: 'ask',
    },
    codex: {
      executablePath: '',
      version: '',
      detected: false,
      sandbox: 'workspace',
    },
  },
  permissions: {
    readFiles: 'allow',
    editFiles: 'allow',
    runCommands: 'ask',
    fetchUrls: 'deny',
    mcpTools: 'ask',
    commandAllowlist: ['git status', 'git diff', 'pnpm test *', 'pnpm lint', 'ls *'],
  },
  appearance: {
    theme: 'light',
    accentColor: { base: 'oklch(55% 0.2 264)', hi: 'oklch(49% 0.21 264)' },
    uiDensity: 'comfortable',
    monospaceFont: 'JetBrains Mono',
    reduceMotion: false,
  },
  shortcuts: {
    commandPalette: 'Ctrl+K',
    newSession: 'Ctrl+N',
    toggleContext: 'Ctrl+.',
    cycleEngine: 'Ctrl+E',
    navigationPrefix: 'G',
    home: 'H',
    workspace: 'W',
    providers: 'P',
    sessions: 'S',
    extensions: 'X',
    usage: 'U',
    settings: ',',
  },
  worktree: {
    enabled: true,
    root: '',
    setupScript: '',
  },
};
