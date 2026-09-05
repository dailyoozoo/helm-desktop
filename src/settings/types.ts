// 设置数据模型
export interface AppSettings {
  // 通用
  general: {
    defaultDirectory: string;
    reopenLastSession: boolean;
    autoUpdateChannel: 'stable' | 'beta';
    updateFeedUrl: string;
    /** 后台刷新签名价格目录；失败时继续使用缓存或安装包内置目录。 */
    pricingAutoUpdate: boolean;
    /** 国内主/备用镜像，填写完整 pricing-catalog.json URL。 */
    pricingFeedUrls: string[];
    pricingUnknownPolicy: 'warn' | 'block';
    pricingMaxAgeDays: number;
    /** 首轮后用 fast model 自动起标题与摘要（外发到用户绑定的服务商，可关） */
    autoTitleSessions: boolean;
    /** 生成式 UI 总开关（默认关闭）：开启才允许最终结果使用交互式可视化输出；渲染能力后续接入。 */
    generativeUi?: boolean;
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
  };
  // 快捷键
  shortcuts: {
    commandPalette: string;
    newSession: string;
    toggleContext: string;
    stop: string;
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
    defaultDirectory:
      typeof window !== 'undefined' && navigator.platform.includes('Win')
        ? 'C:\\Users\\Public\\Documents'
        : '~/code',
    reopenLastSession: true,
    autoUpdateChannel: 'stable',
    updateFeedUrl: '',
    pricingAutoUpdate: true,
    pricingFeedUrls: [],
    pricingUnknownPolicy: 'warn',
    pricingMaxAgeDays: 30,
    autoTitleSessions: true,
    generativeUi: false,
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
    accentColor: { base: 'oklch(52% 0.12 230)', hi: 'oklch(46% 0.13 230)' },
  },
  shortcuts: {
    commandPalette: 'Ctrl+K',
    newSession: 'Ctrl+N',
    toggleContext: 'Ctrl+.',
    stop: '',
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
