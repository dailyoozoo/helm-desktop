import { Fragment, useState, useEffect, useRef, type CSSProperties } from 'react';
import { Icon } from '../shell/icons';
import { showResultToast } from '../components/toast';
import { loadSettings, saveSettings, selectDirectory, getPermissionRules } from './api';
import { type AppSettings, DEFAULT_SETTINGS } from './types';
import { applyAppearanceSettings } from './appearance';
import { shortcutFromKeyboardEvent, shortcutLabel } from './shortcuts';
import type { PageId } from '../shell/Rail';
import { LatestSerialSaver, type SaveState } from './latestSerialSaver';
import { TasksTab } from './TasksTab';
import { AboutTab } from './AboutTab';
import { AuthorizationDrawer } from './AuthorizationDrawer';

/**
 * 设置页信息架构（S8）：对齐原型 settings.html 的五个一级 Tab。
 * 引擎/权限/MCP 等真实能力保留在对应 Tab 的分区与抽屉中，不删除任何后端接线。
 */
type SettingTab = 'general' | 'tasks' | 'theme' | 'keyboard' | 'about';

/** 旧 sessionStorage 值（引擎/权限/MCP/外观）映射到新五 Tab，避免恢复到已合并的入口。 */
const LEGACY_TAB_MAP: Record<string, SettingTab> = {
  general: 'general',
  engines: 'general',
  permissions: 'general',
  mcp: 'general',
  appearance: 'theme',
  theme: 'theme',
  tasks: 'tasks',
  keyboard: 'keyboard',
  about: 'about',
};

/** 把权限规则的 createdAt 时间戳格式化成「M月D日」，对齐原型 settings.html 授权入口的「最近更新于」。 */
function formatRuleDate(timestamp: number): string {
  const date = new Date(timestamp);
  return `${date.getMonth() + 1}月${date.getDate()}日`;
}

/** 颜色主题（原型 settings.html colorThemes）：六套命名强调色，默认海洋蓝（与 tokens.css 默认一致）。 */
const COLOR_THEMES = [
  { name: '海洋蓝', base: 'oklch(52% 0.12 230)', hi: 'oklch(46% 0.13 230)' },
  { name: '翡翠绿', base: 'oklch(52% 0.12 160)', hi: 'oklch(46% 0.13 160)' },
  { name: '琥珀金', base: 'oklch(58% 0.12 70)', hi: 'oklch(52% 0.13 70)' },
  { name: '玫瑰红', base: 'oklch(54% 0.19 25)', hi: 'oklch(48% 0.2 25)' },
  { name: '紫罗兰', base: 'oklch(55% 0.2 300)', hi: 'oklch(49% 0.21 300)' },
  { name: '石墨灰', base: 'oklch(50% 0.03 250)', hi: 'oklch(44% 0.04 250)' },
];

export function SettingsPage({
  initialSettings,
  onSettingsChange,
  onNavigate,
  onOpenWorkspace,
}: {
  initialSettings?: AppSettings;
  onSettingsChange?: (settings: AppSettings) => void;
  onNavigate?: (page: PageId) => void;
  /** 「全部任务」打开任务后跳转工作台。 */
  onOpenWorkspace?: () => void;
  externalSaveState?: SaveState;
} = {}) {
  const [tab, setTab] = useState<SettingTab>(() => {
    const requested = sessionStorage.getItem('helm:settings-tab');
    sessionStorage.removeItem('helm:settings-tab');
    return (requested && LEGACY_TAB_MAP[requested]) || 'general';
  });
  const [settings, setSettings] = useState<AppSettings>(() => initialSettings ?? DEFAULT_SETTINGS);
  const [loading, setLoading] = useState(!initialSettings);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [loadAttempt, setLoadAttempt] = useState(0);
  const [, setLocalSaveState] = useState<SaveState>('idle');
  const mountedRef = useRef(true);
  const saverRef = useRef<LatestSerialSaver<AppSettings> | null>(null);
  if (!saverRef.current) {
    saverRef.current = new LatestSerialSaver(
      saveSettings,
      400,
      (state) => {
        if (mountedRef.current) setLocalSaveState(state);
      },
      () => showResultToast('设置保存失败，修改可能在重启后丢失，请重试'),
    );
  }

  // 全局通知层（P2-2）：保留 notify 名字，避免改动全部调用点
  const notify = (message: string) => {
    showResultToast(message);
  };

  useEffect(() => {
    if (initialSettings) {
      setSettings(initialSettings);
      setLoadError(null);
      setLoading(false);
      return;
    }
    setLoading(true);
    setLoadError(null);
    loadSettings()
      .then(setSettings)
      .catch((error: unknown) => {
        setLoadError(error instanceof Error ? error.message : '无法读取设置');
      })
      .finally(() => setLoading(false));
  }, [initialSettings, loadAttempt]);

  useEffect(
    () => () => {
      mountedRef.current = false;
      void saverRef.current?.flush().catch(() => undefined);
    },
    [],
  );

  const updateSettings = (updater: (prev: AppSettings) => AppSettings) => {
    setSettings((prev) => {
      const next = updater(prev);
      applyAppearanceSettings(next.appearance);
      onSettingsChange?.(next);
      if (!onSettingsChange) saverRef.current?.schedule(next);
      return next;
    });
  };

  if (loading) return <div className="cm-settings-layout">加载中...</div>;
  if (loadError) {
    return (
      <div className="cm-settings-layout">
        <div className="settings-load-state" role="alert">
          <Icon name="alert" />
          <div>
            <b>设置加载失败，当前不会保存任何修改</b>
            <p>{loadError}</p>
          </div>
          <button className="btn btn--primary" onClick={() => setLoadAttempt((value) => value + 1)}>
            <Icon name="refresh" /> 重试
          </button>
        </div>
      </div>
    );
  }

  return (
    <div className="cm-settings-layout">
      <aside className="cm-settings-sidebar" aria-label="设置导航">
        <nav className="cm-settings-tabs" aria-label="设置分类">
          {(
            [
              { key: 'general', icon: 'settings2', label: '通用' },
              { key: 'tasks', icon: 'inbox', label: '全部任务' },
              { key: 'theme', icon: 'palette', label: '主题' },
              { key: 'keyboard', icon: 'keyboard', label: '快捷键' },
              { key: 'about', icon: 'info', label: '关于' },
            ] as const
          ).map((entry) => (
            <button
              key={entry.key}
              type="button"
              className={tab === entry.key ? 'is-active' : ''}
              aria-current={tab === entry.key ? 'page' : undefined}
              onClick={() => setTab(entry.key)}
            >
              <Icon name={entry.icon} /> {entry.label}
            </button>
          ))}
        </nav>
      </aside>

      <div className="cm-settings-main">
        <div className="cm-settings-content">
          {tab === 'general' && (
            <GeneralTab settings={settings.general} update={updateSettings} onNotify={notify} />
          )}
          {tab === 'tasks' && <TasksTab onOpenWorkspace={onOpenWorkspace} />}
          {tab === 'theme' && <ThemeTab settings={settings.appearance} update={updateSettings} />}
          {tab === 'keyboard' && (
            <KeyboardTab settings={settings.shortcuts} update={updateSettings} />
          )}
          {tab === 'about' && (
            <AboutTab settings={settings} update={updateSettings} onNavigate={onNavigate} />
          )}
        </div>
      </div>
    </div>
  );
}

function GeneralTab({
  settings,
  update,
  onNotify,
}: {
  settings: AppSettings['general'];
  update: (updater: (prev: AppSettings) => AppSettings) => void;
  onNotify: (message: string) => void;
}) {
  // 已保存授权抽屉（S8）：承接原「权限」Tab 的真实能力
  const [authDrawerOpen, setAuthDrawerOpen] = useState(false);
  // 授权入口展示真实数量与最近更新日期（对齐原型 settings.html 的 cm-auth-entry）。
  const [authSummary, setAuthSummary] = useState<{ count: number; lastUpdated: number | null }>({
    count: 0,
    lastUpdated: null,
  });
  useEffect(() => {
    let active = true;
    getPermissionRules()
      .then((rules) => {
        if (!active) return;
        const lastUpdated = rules.length
          ? rules.reduce((max, rule) => Math.max(max, rule.createdAt), 0)
          : null;
        setAuthSummary({ count: rules.length, lastUpdated });
      })
      .catch(() => {
        if (active) setAuthSummary({ count: 0, lastUpdated: null });
      });
    return () => {
      active = false;
    };
  }, [authDrawerOpen]);

  const handleBrowse = async () => {
    try {
      const path = await selectDirectory();
      if (path) {
        update((prev) => ({
          ...prev,
          general: { ...prev.general, defaultDirectory: path },
        }));
      }
    } catch (error) {
      console.error('Failed to select directory', error);
      onNotify(`打开目录选择器失败：${error instanceof Error ? error.message : String(error)}`);
    }
  };

  return (
    <section>
      <div className="cm-section">
        <div className="cm-section__head">
          <div>
            <h2>
              <Icon name="zap" /> 新任务
            </h2>
            <p>这些选项只提供默认偏好，任务发送前仍可在 Composer 调整。</p>
          </div>
        </div>
        <div className="cm-detail-card">
          <div className="cm-option-row">
            <div className="cm-option-row__main">
              <b>新任务默认目录</b>
              <small>默认带出上次使用的工作目录。</small>
            </div>
            <button className="cm-action" type="button" onClick={() => void handleBrowse()}>
              <span className="mono">{settings.defaultDirectory || '选择目录…'}</span>
              <Icon name="folderopen" />
            </button>
          </div>
          <div className="cm-option-row">
            <div className="cm-option-row__main">
              <b>自动生成任务标题</b>
              <small>快速模型不可用时使用首条指令摘要，不阻塞主任务。</small>
            </div>
            <label className="cm-switch">
              <input
                type="checkbox"
                checked={settings.autoTitleSessions}
                onChange={(e) =>
                  update((prev) => ({
                    ...prev,
                    general: { ...prev.general, autoTitleSessions: e.target.checked },
                  }))
                }
              />
              <i />
            </label>
          </div>
          <div className="cm-option-row">
            <div className="cm-option-row__main">
              <b>启动时恢复退出前任务</b>
              <small>没有可恢复任务时直接进入新任务。</small>
            </div>
            <label className="cm-switch">
              <input
                type="checkbox"
                checked={settings.reopenLastSession}
                onChange={(e) =>
                  update((prev) => ({
                    ...prev,
                    general: { ...prev.general, reopenLastSession: e.target.checked },
                  }))
                }
              />
              <i />
            </label>
          </div>
        </div>
      </div>

      <div className="cm-section">
        <div className="cm-section__head">
          <div>
            <h2>
              <Icon name="monitor" /> 桌面行为
            </h2>
            <p>后台任务状态保持可见，不静默终止运行中的任务。</p>
          </div>
        </div>
        <div className="cm-detail-card">
          <div className="cm-option-row">
            <div className="cm-option-row__main">
              <b>关闭主窗口时最小化到托盘</b>
              <small>选择退出且仍有任务运行时会再次确认。</small>
            </div>
            <label className="cm-switch">
              <input
                type="checkbox"
                checked={settings.closeToTray ?? false}
                onChange={(e) =>
                  update((prev) => ({
                    ...prev,
                    general: { ...prev.general, closeToTray: e.target.checked },
                  }))
                }
              />
              <i />
            </label>
          </div>
          <div className="cm-option-row">
            <div className="cm-option-row__main">
              <b>系统通知</b>
              <small>任务完成、失败或需要你处理时发送通知。</small>
            </div>
            <label className="cm-switch">
              <input
                type="checkbox"
                checked={settings.notifications?.enabled ?? true}
                onChange={(e) =>
                  update((prev) => ({
                    ...prev,
                    general: {
                      ...prev.general,
                      notifications: { enabled: e.target.checked },
                    },
                  }))
                }
              />
              <i />
            </label>
          </div>
        </div>
      </div>

      <div className="cm-section">
        <div className="cm-section__head">
          <div>
            <h2>
              <Icon name="sparkles" /> 对话体验
            </h2>
            <p>控制最终结果的呈现方式。</p>
          </div>
        </div>
        <div className="cm-detail-card">
          <div className="cm-option-row">
            <div className="cm-option-row__main">
              <b>生成式 UI</b>
              <small>
                允许最终结果使用交互式可视化输出。默认关闭，关闭时不会注入 Widget
                指引；渲染能力将在后续版本接入，当前仅保存偏好。
              </small>
            </div>
            <label className="cm-switch">
              <input
                type="checkbox"
                checked={settings.generativeUi ?? false}
                onChange={(e) =>
                  update((prev) => ({
                    ...prev,
                    general: { ...prev.general, generativeUi: e.target.checked },
                  }))
                }
              />
              <i />
            </label>
          </div>
        </div>
      </div>

      <div className="cm-section">
        <div className="cm-section__head">
          <div>
            <h2>
              <Icon name="shield" /> 授权
            </h2>
            <p>这里只管理审批时已经保存的跨任务授权，不提供危险全局开关。</p>
          </div>
        </div>
        <button className="st-auth-entry" type="button" onClick={() => setAuthDrawerOpen(true)}>
          <span className="st-auth-entry__icon">
            <Icon name="shield" />
          </span>
          <span className="st-auth-entry__main">
            <b>已保存授权</b>
            <small>
              {authSummary.count > 0
                ? `${authSummary.count} 项${authSummary.lastUpdated ? ' · 最近更新于 ' + formatRuleDate(authSummary.lastUpdated) : ''}`
                : '暂无持久规则——审批卡上选择「总是允许」后会出现在这里'}
            </small>
          </span>
          <span className="st-auth-entry__arrow">
            <Icon name="right" />
          </span>
        </button>
      </div>

      {authDrawerOpen ? <AuthorizationDrawer onClose={() => setAuthDrawerOpen(false)} /> : null}
    </section>
  );
}

/**
 * 主题 Tab（S8）：外观模式、强调色与密度等真实偏好，外加跟随实际 token
 * 的即时预览卡——预览只消费 tokens.css 变量，主题/强调色切换时同步变化。
 */
function ThemeTab({
  settings,
  update,
}: {
  settings: AppSettings['appearance'];
  update: (updater: (prev: AppSettings) => AppSettings) => void;
}) {
  useEffect(() => {
    applyAppearanceSettings(settings);

    (
      window as Window & {
        Helm?: { setAccent: (a: string, h: string, p: boolean) => void };
      }
    ).Helm = {
      setAccent: (base, hi, persist) => {
        applyAppearanceSettings({ ...settings, accentColor: { base, hi } });
        if (persist) {
          update((prev) => ({
            ...prev,
            appearance: { ...prev.appearance, accentColor: { base, hi } },
          }));
        }
      },
    };
  }, [settings, update]);

  const [colorMenuOpen, setColorMenuOpen] = useState(false);

  // 原型 cm-color-select 下拉：点击外部关闭。
  useEffect(() => {
    if (!colorMenuOpen) return;
    const close = (event: MouseEvent) => {
      if (!(event.target instanceof Element) || !event.target.closest('.cm-color-select-wrap')) {
        setColorMenuOpen(false);
      }
    };
    document.addEventListener('mousedown', close);
    return () => document.removeEventListener('mousedown', close);
  }, [colorMenuOpen]);

  const handleAccentClick = (color: { base: string; hi: string }) => {
    update((prev) => ({ ...prev, appearance: { ...prev.appearance, accentColor: color } }));
  };

  const selectedColorTheme = COLOR_THEMES.find(
    (c) => c.base === settings.accentColor.base && c.hi === settings.accentColor.hi,
  );
  const selectedAccentName = selectedColorTheme?.name ?? '自定义';
  const themeModeLabel =
    settings.theme === 'light' ? '浅色' : settings.theme === 'dark' ? '深色' : '跟随系统';

  return (
    <section>
      <div className="cm-section">
        <div className="cm-section__head">
          <div>
            <h2>
              <Icon name="slidershorizontal" /> 主题模式
            </h2>
            <p>跟随系统会根据系统外观自动切换浅色或深色。</p>
          </div>
        </div>
        <div className="cm-detail-card">
          <div className="cm-option-row">
            <div className="cm-option-row__main">
              <b>外观模式</b>
              <small>选择浅色、深色或跟随系统。</small>
            </div>
            <div className="cm-theme-mode-grid">
              {(['light', 'dark', 'system'] as const).map((theme) => (
                <button
                  key={theme}
                  className={
                    settings.theme === theme ? 'cm-theme-mode-card is-active' : 'cm-theme-mode-card'
                  }
                  onClick={() =>
                    update((prev) => ({
                      ...prev,
                      appearance: { ...prev.appearance, theme },
                    }))
                  }
                >
                  <Icon name={theme === 'light' ? 'sun' : theme === 'dark' ? 'moon' : 'monitor'} />{' '}
                  {theme === 'light' ? '浅色' : theme === 'dark' ? '深色' : '跟随系统'}
                </button>
              ))}
            </div>
          </div>
        </div>
      </div>

      <div className="cm-section">
        <div className="cm-section__head">
          <div>
            <h2>
              <Icon name="palette" /> 颜色主题
            </h2>
            <p>选择强调色方案，所有按钮、链接和选中态会同步变化。</p>
          </div>
        </div>
        <div className="cm-detail-card">
          <div className="cm-option-row">
            <div className="cm-option-row__main">
              <b>强调色</b>
              <small>点击选择配色方案。</small>
            </div>
            <div className="u-relative cm-color-select-wrap">
              <button
                className="cm-color-select"
                type="button"
                aria-haspopup="listbox"
                aria-expanded={colorMenuOpen}
                onClick={() => setColorMenuOpen((open) => !open)}
              >
                <span
                  className="cm-color-select__dot"
                  style={{ '--swatch': settings.accentColor.base } as CSSProperties}
                />
                <span className="cm-color-select__name">{selectedAccentName}</span>
                <code className="cm-color-select__code">{settings.accentColor.base}</code>
                <span className="cm-color-select__arrow">
                  <Icon name="chevrondown" />
                </span>
              </button>
              {colorMenuOpen ? (
                <div className="cm-color-dropdown is-open" role="listbox" aria-label="颜色主题">
                  {COLOR_THEMES.map((theme) => (
                    <button
                      key={theme.name}
                      className={
                        'cm-color-dropdown__item' +
                        (theme.base === settings.accentColor.base ? ' is-active' : '')
                      }
                      type="button"
                      role="option"
                      aria-selected={theme.base === settings.accentColor.base}
                      onClick={() => {
                        handleAccentClick(theme);
                        setColorMenuOpen(false);
                      }}
                    >
                      <span className="cm-color-dropdown__dot" style={{ background: theme.base }} />
                      <span className="cm-color-dropdown__name">{theme.name}</span>
                      <code className="cm-color-dropdown__code">{theme.base}</code>
                    </button>
                  ))}
                </div>
              ) : null}
            </div>
          </div>
        </div>
      </div>

      <div className="cm-section">
        <div className="cm-section__head">
          <div>
            <h2>
              <Icon name="eye" /> 预览
            </h2>
            <p>实时查看当前主题效果。</p>
          </div>
        </div>
        <div className="cm-detail-card">
          <div className="cm-option-row cm-option-row--preview">
            <div className="cm-option-row__main">
              <b>效果预览</b>
              <small>
                {themeModeLabel} · {selectedAccentName}
              </small>
            </div>
            <ThemePreview />
          </div>
        </div>
      </div>
    </section>
  );
}

/** 主题预览（S8）：纯 token 消费，无独立状态；随 data-theme/--accent 全局变量实时变化。 */
function ThemePreview() {
  return (
    <div className="cm-theme-preview" role="img" aria-label="主题即时预览">
      <div className="cm-theme-preview__side">
        <div className="cm-theme-preview__dot" />
        <div className="cm-theme-preview__nav is-on" />
        <div className="cm-theme-preview__nav" />
        <div className="cm-theme-preview__nav" />
        <div className="cm-theme-preview__nav" />
      </div>
      <div className="cm-theme-preview__main">
        <div className="cm-theme-preview__head">任务工作区</div>
        <div className="cm-theme-preview__bubble">
          已完成登录与授权流程检查。
          <div className="cm-theme-preview__line" />
          <div className="cm-theme-preview__line" style={{ width: '72%' }} />
        </div>
      </div>
    </div>
  );
}

/** 可修改快捷键注册表（严格对齐原型 settings.html shortcutList 的 4 项）。 */
const SHORTCUT_ENTRIES = [
  {
    action: '命令面板',
    key: 'commandPalette',
    icon: 'search',
    desc: '快速查找命令、跳转页面或打开任务',
  },
  {
    action: '新任务',
    key: 'newSession',
    icon: 'plus',
    desc: '从任意页面快速打开新任务编辑器',
  },
  {
    action: '显示 / 隐藏右侧工作区',
    key: 'toggleContext',
    icon: 'panelright',
    desc: '切换交付物区的可见性',
  },
  {
    action: '停止当前任务',
    key: 'stop',
    icon: 'octagon',
    desc: '中断正在运行的 Agent 轮次',
  },
] as const;

type ShortcutEntry = (typeof SHORTCUT_ENTRIES)[number];

function KeyboardTab({
  settings,
  update,
}: {
  settings: AppSettings['shortcuts'];
  update: (updater: (prev: AppSettings) => AppSettings) => void;
}) {
  const [recordingKey, setRecordingKey] = useState<keyof AppSettings['shortcuts'] | null>(null);
  const [blockedConflict, setBlockedConflict] = useState<{
    key: keyof AppSettings['shortcuts'];
    action: string;
  } | null>(null);

  const shortcuts = SHORTCUT_ENTRIES;

  const updateShortcut = (key: keyof AppSettings['shortcuts'], value: string) => {
    update((prev) => ({ ...prev, shortcuts: { ...prev.shortcuts, [key]: value } }));
  };

  const resetShortcuts = () => {
    update((prev) => ({ ...prev, shortcuts: DEFAULT_SETTINGS.shortcuts }));
    setBlockedConflict(null);
  };

  const shortcutValue = (shortcut: ShortcutEntry) => settings[shortcut.key] ?? '';

  /** 冲突口径（变更-12）：前缀行按 g+键 归一，普通行按完整组合归一；同一绑定只允许一个动作。 */
  const bindingId = (shortcut: ShortcutEntry, value: string) =>
    ('prefix' in shortcut && shortcut.prefix ? 'g+' : '') + value.trim().toLowerCase();

  const conflictActionFor = (shortcut: ShortcutEntry, value: string): string | null => {
    const id = bindingId(shortcut, value);
    if (!id) return null;
    for (const other of shortcuts) {
      if (other.key === shortcut.key) continue;
      const otherValue = shortcutValue(other);
      if (otherValue.trim() !== '' && bindingId(other, otherValue) === id) return other.action;
    }
    return null;
  };

  // 录制（决策记录 §8.4.3）：点击键位开始，Esc 取消，Backspace/Delete 清除；
  // 冲突在当前行就地提示并阻止保存，不只依赖被动标记。
  useEffect(() => {
    if (!recordingKey) return;
    const shortcut = shortcuts.find((entry) => entry.key === recordingKey);
    if (!shortcut) return;
    const onKeyDown = (event: KeyboardEvent) => {
      event.preventDefault();
      event.stopPropagation();
      if (event.key === 'Escape') {
        setRecordingKey(null);
        setBlockedConflict(null);
        return;
      }
      if (event.key === 'Backspace' || event.key === 'Delete') {
        update((prev) => ({
          ...prev,
          shortcuts: { ...prev.shortcuts, [shortcut.key]: '' },
        }));
        setRecordingKey(null);
        setBlockedConflict(null);
        return;
      }
      if (['Control', 'Shift', 'Alt', 'Meta'].includes(event.key)) return;
      const value = shortcutFromKeyboardEvent(event);
      if (!value) return;
      const id = ('prefix' in shortcut && shortcut.prefix ? 'g+' : '') + value.trim().toLowerCase();
      const conflictEntry = shortcuts.find((other) => {
        if (other.key === shortcut.key) return false;
        const otherValue = (settings[other.key] ?? '').trim().toLowerCase();
        if (!otherValue) return false;
        return ('prefix' in other && other.prefix ? 'g+' : '') + otherValue === id;
      });
      if (conflictEntry) {
        setBlockedConflict({ key: shortcut.key, action: conflictEntry.action });
        setRecordingKey(null);
        return;
      }
      update((prev) => ({
        ...prev,
        shortcuts: { ...prev.shortcuts, [shortcut.key]: value },
      }));
      setRecordingKey(null);
      setBlockedConflict(null);
    };
    const onMouseDown = (event: MouseEvent) => {
      if (!(event.target instanceof Element)) return;
      if (event.target.closest(`[data-shortcut-key="${shortcut.key}"]`)) return;
      setRecordingKey(null);
      setBlockedConflict(null);
    };
    document.addEventListener('keydown', onKeyDown, true);
    document.addEventListener('mousedown', onMouseDown);
    return () => {
      document.removeEventListener('keydown', onKeyDown, true);
      document.removeEventListener('mousedown', onMouseDown);
    };
  }, [recordingKey, settings, shortcuts, update]);

  return (
    <section>
      <div className="cm-section">
        <div className="cm-section__head cm-shortcut-section-head">
          <div>
            <h2>
              <Icon name="keyboard" /> 可修改快捷键
            </h2>
            <p>点击键位卡片开始录制，Esc 取消；冲突会即时提示。</p>
          </div>
          <button className="cm-action" type="button" onClick={resetShortcuts}>
            <Icon name="refresh" /> 全部恢复默认
          </button>
        </div>
        {shortcuts.map((s) => {
          const value = shortcutValue(s);
          const recording = recordingKey === s.key;
          const blocked = blockedConflict?.key === s.key ? blockedConflict.action : null;
          const conflictText = blocked ?? conflictActionFor(s, value);
          const prefixKey = 'prefix' in s && s.prefix ? settings.navigationPrefix : null;
          const parts = shortcutLabel(value);
          return (
            <div
              key={s.action}
              className={
                'cm-shortcut-card' +
                (recording ? ' is-recording' : '') +
                (conflictText ? ' is-conflict' : '')
              }
              data-shortcut-key={s.key}
            >
              <span className="cm-shortcut-card__icon">
                <Icon name={s.icon} />
              </span>
              <div className="cm-shortcut-card__main">
                <b>{s.action}</b>
                <small>{s.desc}</small>
                {conflictText ? (
                  <span className="cm-shortcut-card__conflict" role="alert">
                    与「{conflictText}」冲突
                  </span>
                ) : null}
              </div>
              <div className="cm-shortcut-card__right">
                <button
                  className="cm-shortcut-keys"
                  type="button"
                  aria-label={`录制${s.action}快捷键`}
                  onClick={() => {
                    setRecordingKey(recording ? null : s.key);
                    setBlockedConflict(null);
                  }}
                >
                  {recording ? (
                    <span className="cm-key cm-key--placeholder">按下组合键…</span>
                  ) : parts.length === 0 ? (
                    <span className="cm-key cm-key--placeholder">未设置</span>
                  ) : (
                    <>
                      {prefixKey ? <span className="cm-key">{prefixKey}</span> : null}
                      {parts.map((part, index) => (
                        <Fragment key={`${s.key}-${part}-${index}`}>
                          {index > 0 ? <span className="cm-key cm-key--plus">+</span> : null}
                          <span className="cm-key">{part}</span>
                        </Fragment>
                      ))}
                    </>
                  )}
                </button>
                {value ? (
                  <button
                    className="cm-shortcut-clear"
                    type="button"
                    title="清除"
                    aria-label={`清除${s.action}快捷键`}
                    onClick={() => {
                      updateShortcut(s.key, '');
                      setBlockedConflict(null);
                    }}
                  >
                    <Icon name="x" />
                  </button>
                ) : null}
              </div>
            </div>
          );
        })}
      </div>

      <div className="cm-section">
        <div className="cm-section__head">
          <div>
            <h2>
              <Icon name="lock" /> 固定交互
            </h2>
            <p>与输入框和选择弹层行为一致，不支持修改。</p>
          </div>
        </div>
        <div className="cm-detail-card cm-shortcut-fixed">
          <div className="cm-option-row">
            <div className="cm-option-row__main">
              <b>发送 / 换行</b>
              <small>
                <span className="cm-shortcut-fixed-keys">
                  <span className="cm-key">Enter</span>
                </span>
                <span className="cm-key cm-key--sep">发送</span>
                <span className="cm-shortcut-fixed-keys">
                  <span className="cm-key">Shift</span>
                  <span className="cm-key cm-key--plus">+</span>
                  <span className="cm-key">Enter</span>
                </span>
                <span className="cm-key cm-key--sep">换行</span>
              </small>
            </div>
            <span className="cm-source-label">固定</span>
          </div>
          <div className="cm-option-row">
            <div className="cm-option-row__main">
              <b>选择弹层项</b>
              <small>
                <span className="cm-shortcut-fixed-keys">
                  <span className="cm-key">↑</span>
                  <span className="cm-key">↓</span>
                </span>
                <span className="cm-key cm-key--sep">选择</span>
                <span className="cm-shortcut-fixed-keys">
                  <span className="cm-key">Enter</span>
                  <span className="cm-key cm-key--sep">/</span>
                  <span className="cm-key">Tab</span>
                </span>
                <span className="cm-key cm-key--sep">确认</span>
                <span className="cm-shortcut-fixed-keys">
                  <span className="cm-key">Esc</span>
                </span>
                <span className="cm-key cm-key--sep">关闭</span>
              </small>
            </div>
            <span className="cm-source-label">固定</span>
          </div>
          <div className="cm-option-row">
            <div className="cm-option-row__main">
              <b>输入触发</b>
              <small>
                <span className="cm-shortcut-fixed-keys">
                  <span className="cm-key">/</span>
                </span>
                <span className="cm-key cm-key--sep">命令与 Skills</span>
                <span className="cm-shortcut-fixed-keys">
                  <span className="cm-key">@</span>
                </span>
                <span className="cm-key cm-key--sep">文件引用</span>
              </small>
            </div>
            <span className="cm-source-label">固定</span>
          </div>
        </div>
      </div>
    </section>
  );
}
