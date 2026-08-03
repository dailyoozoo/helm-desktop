// 设置数据模型
export interface AppSettings {
  // 通用
  general: {
    workspaceName: string;
    defaultDirectory: string;
    reopenLastSession: boolean;
    anonymousAnalytics: boolean;
    autoUpdateChannel: 'stable' | 'beta';
    updateFeedUrl: string;
    /** 后台刷新签名价格目录；失败时继续使用缓存或安装包内置目录。 */
    pricingAutoUpdate: boolean;
    /** 国内主/备用镜像，填写完整 pricing-catalog.json URL。 */
    pricingFeedUrls: string[];
    pricingUnknownPolicy: 'warn' | 'block';
    pricingMaxAgeDays: number;
    /** 首启引导是否已完成/跳过（完成或显式跳过都置 true） */
    onboardingCompleted?: boolean;
    /** 首轮后用 fast model 自动起标题与摘要（外发到用户绑定的服务商，可关） */
    autoTitleSessions: boolean;
    /** 点关闭按钮时最小化到托盘而不是退出（变更-12）：后台会话继续运行 */
    closeToTray?: boolean;
    /** 轮次完成/出错时弹出系统通知 */
    notifications?: { enabled: boolean };
    /** 旧设置迁移输入；保存时清理，产品不再展示独立辅助模型。 */
    assistantModelId?: string;
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
    };
  };
  // 权限
  permissions: Record<string, never>;
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
    anonymousAnalytics: false,
    autoUpdateChannel: 'stable',
    updateFeedUrl: '',
    pricingAutoUpdate: true,
    pricingFeedUrls: [],
    pricingUnknownPolicy: 'warn',
    pricingMaxAgeDays: 30,
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
    },
  },
  permissions: {},
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
    extensions: 'E',
    usage: 'U',
    settings: ',',
  },
};
