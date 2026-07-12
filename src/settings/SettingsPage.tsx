import { useState, useEffect, useRef } from 'react';
import { listen } from '@tauri-apps/api/event';
import { Icon } from '../shell/icons';
import { showResultToast } from '../components/toast';
import {
  loadSettings,
  saveSettings,
  detectEngine,
  installCliEngine,
  selectDirectory,
  getUpdateStatus,
  exportSettings,
  importSettings,
  getAlwaysAllowTools,
  removeAlwaysAllowTool,
  checkForUpdate,
  installUpdate,
  type UpdateCheckResult,
} from './api';
import { type AppSettings, DEFAULT_SETTINGS, type UpdateStatus } from './types';
import { applyAppearanceSettings } from './appearance';
import { listMcpServers, type McpServer } from '../extensions/extensionsApi';
import {
  addCommandAllowlistPattern,
  engineConfigWithDetection,
  mcpServerCommand,
  mcpStatusLabel,
  removeCommandAllowlistPattern,
  updateStatusSummary,
} from './settingsViewModel';
import { shortcutLabel } from './shortcuts';
import type { PageId } from '../shell/Rail';
import { getProviderConfig, saveEngineConfig } from '../providers/api';
import { LatestSerialSaver, type SaveState } from './latestSerialSaver';

type SettingTab = 'general' | 'engines' | 'permissions' | 'mcp' | 'appearance' | 'keyboard';

const ACCENT_COLORS = [
  { name: '靛蓝', base: 'oklch(55% 0.2 264)', hi: 'oklch(49% 0.21 264)' },
  { name: '紫罗兰', base: 'oklch(53% 0.23 300)', hi: 'oklch(47% 0.24 300)' },
  { name: '蓝色', base: 'oklch(54% 0.15 235)', hi: 'oklch(48% 0.16 235)' },
  { name: '翠绿', base: 'oklch(53% 0.14 162)', hi: 'oklch(47% 0.15 162)' },
  { name: '琥珀', base: 'oklch(64% 0.14 70)', hi: 'oklch(58% 0.15 70)' },
  { name: '玫红', base: 'oklch(56% 0.21 14)', hi: 'oklch(50% 0.22 14)' },
];

export function SettingsPage({
  initialSettings,
  onSettingsChange,
  onNavigate,
}: {
  initialSettings?: AppSettings;
  onSettingsChange?: (settings: AppSettings) => void;
  onNavigate?: (page: PageId) => void;
} = {}) {
  const [tab, setTab] = useState<SettingTab>(() => {
    const requested = sessionStorage.getItem('helm:settings-tab') as SettingTab | null;
    sessionStorage.removeItem('helm:settings-tab');
    return requested ?? 'general';
  });
  const [settings, setSettings] = useState<AppSettings>(initialSettings ?? DEFAULT_SETTINGS);
  const [loading, setLoading] = useState(!initialSettings);
  const [saveState, setSaveState] = useState<SaveState>('idle');
  const mountedRef = useRef(true);
  const saverRef = useRef<LatestSerialSaver<AppSettings> | null>(null);
  if (!saverRef.current) {
    saverRef.current = new LatestSerialSaver(
      saveSettings,
      400,
      (state) => {
        if (mountedRef.current) setSaveState(state);
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
      setLoading(false);
      return;
    }
    loadSettings()
      .then(setSettings)
      .catch(() => {
        setSettings(DEFAULT_SETTINGS);
        showResultToast('设置加载失败，当前显示默认值');
      })
      .finally(() => setLoading(false));
  }, [initialSettings]);

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
      saverRef.current?.schedule(next);
      return next;
    });
  };

  if (loading) return <div className="setlayout">加载中...</div>;

  return (
    <div className="setlayout">
      <nav className="snav">
        <div className="snav__t">设置</div>
        <div className={`snav__save is-${saveState}`} aria-live="polite">
          {saveState === 'pending'
            ? '等待保存'
            : saveState === 'saving'
              ? '保存中…'
              : saveState === 'saved'
                ? '已保存'
                : saveState === 'error'
                  ? '保存失败'
                  : ''}
        </div>
        <button
          type="button"
          className={tab === 'general' ? 'is-active' : ''}
          onClick={() => setTab('general')}
        >
          <Icon name="sliders" /> 通用
        </button>
        <button
          type="button"
          className={tab === 'engines' ? 'is-active' : ''}
          onClick={() => setTab('engines')}
        >
          <Icon name="zap" /> 引擎
        </button>
        <button
          type="button"
          className={tab === 'permissions' ? 'is-active' : ''}
          onClick={() => setTab('permissions')}
        >
          <Icon name="shield" /> 权限
        </button>
        <button
          type="button"
          className={tab === 'mcp' ? 'is-active' : ''}
          onClick={() => setTab('mcp')}
        >
          <Icon name="plug" /> MCP 服务器
        </button>
        <button
          type="button"
          className={tab === 'appearance' ? 'is-active' : ''}
          onClick={() => setTab('appearance')}
        >
          <Icon name="palette" /> 外观
        </button>
        <button
          type="button"
          className={tab === 'keyboard' ? 'is-active' : ''}
          onClick={() => setTab('keyboard')}
        >
          <Icon name="keyboard" /> 快捷键
        </button>
      </nav>

      <div className="setscroll">
        <div className="setbody">
          {tab === 'general' && (
            <GeneralTab
              settings={settings.general}
              worktree={settings.worktree}
              update={updateSettings}
              onNotify={notify}
            />
          )}
          {tab === 'engines' && (
            <EnginesTab
              settings={settings.engines}
              update={updateSettings}
              onNavigate={onNavigate}
            />
          )}
          {tab === 'permissions' && (
            <PermissionsTab settings={settings.permissions} update={updateSettings} />
          )}
          {tab === 'mcp' && (
            <McpTab
              settings={settings.permissions}
              update={updateSettings}
              onNavigate={onNavigate}
            />
          )}
          {tab === 'appearance' && (
            <AppearanceTab settings={settings.appearance} update={updateSettings} />
          )}
          {tab === 'keyboard' && (
            <KeyboardTab settings={settings.shortcuts} update={updateSettings} />
          )}
        </div>
      </div>
    </div>
  );
}

function GeneralTab({
  settings,
  worktree,
  update,
  onNotify,
}: {
  settings: AppSettings['general'];
  worktree: AppSettings['worktree'];
  update: (updater: (prev: AppSettings) => AppSettings) => void;
  onNotify: (message: string) => void;
}) {
  const [updateStatus, setUpdateStatus] = useState<UpdateStatus | null>(null);

  useEffect(() => {
    let active = true;
    getUpdateStatus()
      .then((status) => {
        if (active) setUpdateStatus(status);
      })
      .catch(() => {
        if (!active) return;
        setUpdateStatus({
          currentVersion: '0.0.0',
          channel: settings.autoUpdateChannel,
          canCheck: false,
          message: '浏览器预览环境无法检查更新；当前仅保存更新通道偏好。',
        });
      });
    return () => {
      active = false;
    };
  }, [settings.autoUpdateChannel, settings.updateFeedUrl]);

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

  const handleExport = async () => {
    try {
      await exportSettings();
      onNotify('设置已导出');
    } catch (error) {
      console.error('Failed to export settings', error);
      onNotify(`导出设置失败：${error instanceof Error ? error.message : String(error)}`);
    }
  };

  const handleImport = async () => {
    try {
      const imported = await importSettings();
      if (imported) {
        update(() => imported);
        applyAppearanceSettings(imported.appearance);
        onNotify('设置已导入并应用');
      }
    } catch (error) {
      console.error('Failed to import settings', error);
      onNotify(`导入设置失败：${error instanceof Error ? error.message : String(error)}`);
    }
  };

  return (
    <section>
      <div className="sgroup">
        <div className="sgroup__t">通用</div>
        <div className="sgroup__d">工作区默认设置与启动行为。</div>
        <div className="row gap-sm" style={{ marginBottom: '18px' }}>
          <button className="btn btn--subtle btn--sm" onClick={handleExport} type="button">
            <Icon name="upright" /> 导出设置
          </button>
          <button className="btn btn--subtle btn--sm" onClick={handleImport} type="button">
            <Icon name="down" /> 导入设置
          </button>
        </div>
        <div className="field" style={{ marginBottom: '18px' }}>
          <label>工作区名称</label>
          <input
            className="input"
            value={settings.workspaceName}
            onChange={(e) =>
              update((prev) => ({
                ...prev,
                general: { ...prev.general, workspaceName: e.target.value },
              }))
            }
          />
        </div>
        <div className="field" style={{ marginBottom: '4px' }}>
          <label>默认工作目录</label>
          <div className="row gap-sm">
            <input
              className="input mono"
              value={settings.defaultDirectory}
              onChange={(e) =>
                update((prev) => ({
                  ...prev,
                  general: { ...prev.general, defaultDirectory: e.target.value },
                }))
              }
            />
            <button className="btn btn--subtle" onClick={handleBrowse}>
              <Icon name="folder" /> 浏览
            </button>
          </div>
        </div>
        <div className="srow2">
          <div className="srow2__m">
            <b>启动时重新打开上次会话</b>
            <small>直接回到你上次所在的对话线程。</small>
          </div>
          <label className="switch srow2__c">
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
        <div className="srow2">
          <div className="srow2__m">
            <b>首次使用引导</b>
            <small>重新运行引擎检测、接入方式与工作目录的配置向导。</small>
          </div>
          <button
            className="btn btn--subtle btn--sm srow2__c"
            onClick={() =>
              update((prev) => ({
                ...prev,
                general: { ...prev.general, onboardingCompleted: false },
              }))
            }
          >
            重新运行引导
          </button>
        </div>
        <div className="srow2">
          <div className="srow2__m">
            <b>运行命令前确认</b>
            <small>无论引擎设置如何，Helm 都会在执行任何 shell 命令前暂停等待批准。</small>
          </div>
          <label className="switch srow2__c">
            <input
              type="checkbox"
              checked={settings.confirmBeforeCommand}
              onChange={(e) =>
                update((prev) => ({
                  ...prev,
                  general: { ...prev.general, confirmBeforeCommand: e.target.checked },
                }))
              }
            />
            <i />
          </label>
        </div>
        <div className="srow2">
          <div className="srow2__m">
            <b>匿名使用分析</b>
            <small>控制 Helm 与子进程匿名诊断环境；关闭后会为 CLI 写入 DO_NOT_TRACK=1。</small>
          </div>
          <label className="switch srow2__c">
            <input
              type="checkbox"
              checked={settings.anonymousAnalytics}
              onChange={(e) =>
                update((prev) => ({
                  ...prev,
                  general: { ...prev.general, anonymousAnalytics: e.target.checked },
                }))
              }
            />
            <i />
          </label>
        </div>
        <div className="srow2">
          <div className="srow2__m">
            <b>会话自动起标题</b>
            <small>
              首轮结束后调用绑定服务商的快速模型生成标题与摘要（会把首轮内容发给你自己配置的服务商；订阅登录无
              Key 时自动跳过）。
            </small>
          </div>
          <label className="switch srow2__c">
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
        <div className="srow2">
          <div className="srow2__m">
            <b>关闭时最小化到托盘</b>
            <small>
              点关闭按钮时隐藏到系统托盘而不是退出，后台会话继续运行；从托盘菜单可显示窗口或真正退出。
            </small>
          </div>
          <label className="switch srow2__c">
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
        <div className="srow2">
          <div className="srow2__m">
            <b>自动更新</b>
            <small>
              {updateStatus ? updateStatusSummary(updateStatus) : '正在读取当前版本与更新通道…'}
            </small>
          </div>
          <div className="seg srow2__c">
            <button
              className={settings.autoUpdateChannel === 'stable' ? 'is-active' : ''}
              onClick={() =>
                update((prev) => ({
                  ...prev,
                  general: { ...prev.general, autoUpdateChannel: 'stable' },
                }))
              }
            >
              稳定版
            </button>
            <button
              className={settings.autoUpdateChannel === 'beta' ? 'is-active' : ''}
              onClick={() =>
                update((prev) => ({
                  ...prev,
                  general: { ...prev.general, autoUpdateChannel: 'beta' },
                }))
              }
            >
              测试版
            </button>
          </div>
        </div>
        <div className="field" style={{ marginTop: '12px' }}>
          <label>更新发布源</label>
          <input
            className="input mono"
            value={settings.updateFeedUrl}
            onChange={(e) =>
              update((prev) => ({
                ...prev,
                general: { ...prev.general, updateFeedUrl: e.target.value },
              }))
            }
            placeholder="https://updates.example.com/helm/latest.json"
          />
        </div>
        <UpdateActions feedConfigured={settings.updateFeedUrl.trim().length > 0} />
      </div>
      <div className="sgroup">
        <div className="sgroup__t">并行会话（worktree 隔离）</div>
        <div className="sgroup__d">
          并行会话可运行在独立的 Git worktree
          里，互不踩踏工作区文件。只做本地分支与目录操作，不涉及远端。
        </div>
        <div className="srow2">
          <div className="srow2__m">
            <b>启用 worktree 隔离</b>
            <small>关闭后，工作区不再提供「并行 worktree 会话」入口。</small>
          </div>
          <label className="switch srow2__c">
            <input
              type="checkbox"
              checked={worktree.enabled}
              onChange={(e) =>
                update((prev) => ({
                  ...prev,
                  worktree: { ...prev.worktree, enabled: e.target.checked },
                }))
              }
            />
            <i />
          </label>
        </div>
        <div className="field" style={{ marginTop: '12px' }}>
          <label>worktree 存放位置（留空 = 仓库旁边的「仓库名-worktrees」目录）</label>
          <input
            className="input mono"
            value={worktree.root}
            onChange={(e) =>
              update((prev) => ({
                ...prev,
                worktree: { ...prev.worktree, root: e.target.value },
              }))
            }
            placeholder="D:\worktrees"
          />
        </div>
        <div className="field" style={{ marginTop: '12px' }}>
          <label>创建后初始化命令（可选，在新 worktree 内执行，如安装依赖）</label>
          <input
            className="input mono"
            value={worktree.setupScript}
            onChange={(e) =>
              update((prev) => ({
                ...prev,
                worktree: { ...prev.worktree, setupScript: e.target.value },
              }))
            }
            placeholder="npm install"
          />
        </div>
      </div>
    </section>
  );
}

/** 真实更新链路（P2-1）：检查 → 展示新版本 → 下载安装（进度来自 update-progress 事件） */
function UpdateActions({ feedConfigured }: { feedConfigured: boolean }) {
  const [checking, setChecking] = useState(false);
  const [installing, setInstalling] = useState(false);
  const [available, setAvailable] = useState<UpdateCheckResult | null>(null);
  const [progress, setProgress] = useState<{ downloaded: number; total: number | null } | null>(
    null,
  );

  useEffect(() => {
    if (!installing) return;
    let unlisten: (() => void) | null = null;
    let active = true;
    void listen<{ downloaded?: number; total?: number | null; finished?: boolean }>(
      'update-progress',
      (event) => {
        if (!active) return;
        if (event.payload.finished) {
          setProgress(null);
          return;
        }
        setProgress({
          downloaded: event.payload.downloaded ?? 0,
          total: event.payload.total ?? null,
        });
      },
    ).then((stop) => {
      if (active) unlisten = stop;
      else stop();
    });
    return () => {
      active = false;
      unlisten?.();
    };
  }, [installing]);

  const handleCheck = async () => {
    setChecking(true);
    setAvailable(null);
    try {
      const result = await checkForUpdate();
      if (result.available) {
        setAvailable(result);
        showResultToast(`发现新版本 v${result.version ?? ''}`);
      } else {
        showResultToast(`当前已是最新版本（v${result.currentVersion}）`);
      }
    } catch (error) {
      showResultToast(`检查更新失败：${error instanceof Error ? error.message : String(error)}`);
    } finally {
      setChecking(false);
    }
  };

  const handleInstall = async () => {
    setInstalling(true);
    setProgress(null);
    try {
      await installUpdate();
      // 成功路径应用会自动重启，走不到这里
    } catch (error) {
      showResultToast(`安装更新失败：${error instanceof Error ? error.message : String(error)}`);
      setInstalling(false);
      setProgress(null);
    }
  };

  const progressText =
    progress && progress.total
      ? `下载中 ${Math.min(100, Math.round((progress.downloaded / progress.total) * 100))}%`
      : '下载中…';

  return (
    <div className="row gap-sm" style={{ marginTop: '12px', alignItems: 'center' }}>
      <button
        className="btn btn--subtle btn--sm"
        type="button"
        disabled={!feedConfigured || checking || installing}
        title={feedConfigured ? undefined : '先填写更新发布源'}
        onClick={handleCheck}
      >
        <Icon name="refresh" /> {checking ? '检查中…' : '检查更新'}
      </button>
      {available?.available ? (
        <button
          className="btn btn--primary btn--sm"
          type="button"
          disabled={installing}
          onClick={handleInstall}
        >
          <Icon name="down" /> {installing ? progressText : `下载并安装 v${available.version}`}
        </button>
      ) : null}
      {available?.notes ? (
        <span className="faint" style={{ fontSize: 12 }}>
          {available.notes}
        </span>
      ) : null}
    </div>
  );
}

function EnginesTab({
  settings,
  update,
  onNavigate,
}: {
  settings: AppSettings['engines'];
  update: (updater: (prev: AppSettings) => AppSettings) => void;
  onNavigate?: (page: PageId) => void;
}) {
  const [detecting, setDetecting] = useState<'claude-code' | 'codex' | null>(null);
  const [installingEngine, setInstallingEngine] = useState<'claude-code' | 'codex' | null>(null);
  const [detectErrors, setDetectErrors] = useState<
    Partial<Record<'claude-code' | 'codex', string>>
  >({});

  // 一键安装（P3-4）：真实 npm install -g 官方包，成功后自动复检并同步引擎配置
  const handleInstall = async (engine: 'claude-code' | 'codex') => {
    setInstallingEngine(engine);
    try {
      const result = await installCliEngine(engine);
      showResultToast(
        `${engine === 'claude-code' ? 'Claude Code' : 'Codex'} 已安装（${result.version}）`,
      );
      await handleDetect(engine);
    } catch (error) {
      showResultToast(`一键安装失败：${error instanceof Error ? error.message : String(error)}`);
    } finally {
      setInstallingEngine(null);
    }
  };

  const handleDetect = async (engine: 'claude-code' | 'codex') => {
    setDetecting(engine);
    setDetectErrors((prev) => ({ ...prev, [engine]: undefined }));
    try {
      const result = await detectEngine(engine);
      const configKey = engine === 'claude-code' ? 'claudeCode' : 'codex';
      update((prev) => ({
        ...prev,
        engines: {
          ...prev.engines,
          [configKey]: {
            ...prev.engines[configKey],
            executablePath: result.path,
            version: result.version,
            detected: true,
          },
        },
      }));
      try {
        const config = await getProviderConfig();
        const engineConfig = config.engines.find((item) => item.id === engine);
        if (engineConfig) {
          await saveEngineConfig(engineConfigWithDetection(engineConfig, result));
        }
      } catch (syncError) {
        console.error('Failed to sync engine config', syncError);
        showResultToast('检测成功，但同步引擎配置失败，服务商页的引擎状态可能未更新');
      }
    } catch (error) {
      const bin = engine === 'claude-code' ? 'claude' : 'codex';
      setDetectErrors((prev) => ({
        ...prev,
        [engine]: `未检测到 ${bin} CLI（${String(error)}）。请先安装并确认 ${bin} 在 PATH 中，然后重新检测。`,
      }));
    } finally {
      setDetecting(null);
    }
  };

  return (
    <section>
      <div className="sgroup">
        <div className="sgroup__t">引擎</div>
        <div className="sgroup__d">
          Helm 通过这些 CLI 工作。完整目录请查看{' '}
          <a
            href="#providers"
            style={{ color: 'var(--accent-hi)' }}
            onClick={(event) => {
              event.preventDefault();
              onNavigate?.('providers');
            }}
          >
            服务商与模型
          </a>
          。
        </div>
        <div className="srow2">
          <div className="srow2__m">
            <b>新会话默认引擎</b>
            <small>当你点击"新建会话"而未指定引擎时使用。</small>
          </div>
          <div className="seg srow2__c">
            <button
              className={settings.defaultEngine === 'claude-code' ? 'is-active' : ''}
              onClick={() =>
                update((prev) => ({
                  ...prev,
                  engines: { ...prev.engines, defaultEngine: 'claude-code' },
                }))
              }
            >
              Claude Code
            </button>
            <button
              className={settings.defaultEngine === 'codex' ? 'is-active' : ''}
              onClick={() =>
                update((prev) => ({
                  ...prev,
                  engines: { ...prev.engines, defaultEngine: 'codex' },
                }))
              }
            >
              Codex
            </button>
          </div>
        </div>
        <div className="srow2">
          <div className="srow2__m">
            <b>Claude Code 可执行文件</b>
            <small className="mono">
              {settings.claudeCode.detected
                ? `detected · ${settings.claudeCode.executablePath} · ${settings.claudeCode.version}`
                : '未检测'}
            </small>
            {detectErrors['claude-code'] ? (
              <small style={{ color: 'var(--danger, #e5484d)' }}>
                {detectErrors['claude-code']}
              </small>
            ) : null}
          </div>
          <div className="row gap-sm srow2__c">
            {!settings.claudeCode.detected ? (
              <button
                className="btn btn--primary btn--sm"
                onClick={() => void handleInstall('claude-code')}
                disabled={installingEngine !== null || detecting === 'claude-code'}
              >
                <Icon name="down" /> {installingEngine === 'claude-code' ? '安装中…' : '一键安装'}
              </button>
            ) : null}
            <button
              className="btn btn--subtle btn--sm"
              onClick={() => handleDetect('claude-code')}
              disabled={detecting === 'claude-code' || installingEngine === 'claude-code'}
            >
              <Icon name="refresh" /> {detecting === 'claude-code' ? '检测中...' : '重新检测'}
            </button>
          </div>
        </div>
        <div className="srow2">
          <div className="srow2__m">
            <b>新会话默认模式</b>
            <small>新会话发送框初始选中的模式；对话中可随时按轮切换（构建/计划/询问）。</small>
          </div>
          <div className="seg srow2__c">
            {(['build', 'plan'] as const).map((mode) => {
              // 存量兼容（变更-04 §0.3）：旧值 auto/ask 都视为构建，只有 plan 视为计划
              const active =
                mode === 'plan'
                  ? settings.claudeCode.permissionMode === 'plan'
                  : settings.claudeCode.permissionMode !== 'plan';
              return (
                <button
                  key={mode}
                  className={active ? 'is-active' : ''}
                  onClick={() =>
                    update((prev) => ({
                      ...prev,
                      engines: {
                        ...prev.engines,
                        claudeCode: {
                          ...prev.engines.claudeCode,
                          permissionMode: mode === 'plan' ? 'plan' : 'auto',
                        },
                      },
                    }))
                  }
                >
                  {mode === 'plan' ? '计划' : '构建'}
                </button>
              );
            })}
          </div>
        </div>
        <div className="srow2">
          <div className="srow2__m">
            <b>Codex 可执行文件</b>
            <small className="mono">
              {settings.codex.detected
                ? `detected · ${settings.codex.executablePath} · ${settings.codex.version}`
                : '未检测'}
            </small>
            {detectErrors['codex'] ? (
              <small style={{ color: 'var(--danger, #e5484d)' }}>{detectErrors['codex']}</small>
            ) : null}
          </div>
          <div className="row gap-sm srow2__c">
            {!settings.codex.detected ? (
              <button
                className="btn btn--primary btn--sm"
                onClick={() => void handleInstall('codex')}
                disabled={installingEngine !== null || detecting === 'codex'}
              >
                <Icon name="down" /> {installingEngine === 'codex' ? '安装中…' : '一键安装'}
              </button>
            ) : null}
            <button
              className="btn btn--subtle btn--sm"
              onClick={() => handleDetect('codex')}
              disabled={detecting === 'codex' || installingEngine === 'codex'}
            >
              <Icon name="refresh" /> {detecting === 'codex' ? '检测中...' : '重新检测'}
            </button>
          </div>
        </div>
        <div className="srow2">
          <div className="srow2__m">
            <b>Codex 沙箱</b>
            <small>代理进程的文件系统访问范围。</small>
          </div>
          <div className="seg seg3 srow2__c">
            {(['readonly', 'workspace', 'full'] as const).map((mode) => (
              <button
                key={mode}
                className={settings.codex.sandbox === mode ? 'is-active' : ''}
                onClick={() =>
                  update((prev) => ({
                    ...prev,
                    engines: {
                      ...prev.engines,
                      codex: { ...prev.engines.codex, sandbox: mode },
                    },
                  }))
                }
              >
                {mode === 'readonly' ? '只读' : mode === 'workspace' ? '工作区' : '完全'}
              </button>
            ))}
          </div>
        </div>
      </div>
    </section>
  );
}

function PermissionsTab({
  settings,
  update,
}: {
  settings: AppSettings['permissions'];
  update: (updater: (prev: AppSettings) => AppSettings) => void;
}) {
  const [newPattern, setNewPattern] = useState('');
  // 跨会话「始终允许」清单（P2-4）：由审批链路写入，这里只读与撤销
  const [alwaysAllow, setAlwaysAllow] = useState<string[]>([]);

  useEffect(() => {
    let active = true;
    getAlwaysAllowTools()
      .then((tools) => {
        if (active) setAlwaysAllow(tools);
      })
      .catch(() => {
        // 浏览器预览无后端；桌面端读取失败时保持空列表，撤销操作仍会给出错误提示
      });
    return () => {
      active = false;
    };
  }, []);

  const revokeAlwaysAllow = async (tool: string) => {
    try {
      setAlwaysAllow(await removeAlwaysAllowTool(tool));
      showResultToast(`已撤销「${tool}」的始终允许，新会话将恢复询问`);
    } catch (error) {
      showResultToast(`撤销失败：${error instanceof Error ? error.message : String(error)}`);
    }
  };

  const updatePermission = (
    key: keyof Omit<AppSettings['permissions'], 'commandAllowlist'>,
    value: 'allow' | 'ask' | 'deny',
  ) => {
    update((prev) => ({ ...prev, permissions: { ...prev.permissions, [key]: value } }));
  };

  const addPattern = () => {
    update((prev) => ({
      ...prev,
      permissions: addCommandAllowlistPattern(prev.permissions, newPattern),
    }));
    setNewPattern('');
  };

  const removePattern = (pattern: string) => {
    update((prev) => ({
      ...prev,
      permissions: removeCommandAllowlistPattern(prev.permissions, pattern),
    }));
  };

  const PermissionRow = ({
    title,
    desc,
    value,
    onChange,
  }: {
    title: string;
    desc: string;
    value: 'allow' | 'ask' | 'deny';
    onChange: (v: 'allow' | 'ask' | 'deny') => void;
  }) => (
    <div className="srow2">
      <div className="srow2__m">
        <b>{title}</b>
        <small>{desc}</small>
      </div>
      <div className="seg seg3 srow2__c">
        {(['allow', 'ask', 'deny'] as const).map((mode) => (
          <button
            key={mode}
            className={value === mode ? 'is-active' : ''}
            onClick={() => onChange(mode)}
          >
            {mode === 'allow' ? '允许' : mode === 'ask' ? '询问' : '拒绝'}
          </button>
        ))}
      </div>
    </div>
  );

  return (
    <section>
      <div className="sgroup perm-table">
        <div className="sgroup__t">工具权限</div>
        <div className="sgroup__d">
          设置每个工具的限制方式。<b>询问</b> 会在操作执行前于对话中内联提示你。
        </div>
        <PermissionRow
          title="读取文件"
          desc="查看工作目录内的任意文件。"
          value={settings.readFiles}
          onChange={(v) => updatePermission('readFiles', v)}
        />
        <PermissionRow
          title="编辑文件"
          desc="在工作区中创建、修改或删除文件。"
          value={settings.editFiles}
          onChange={(v) => updatePermission('editFiles', v)}
        />
        <PermissionRow
          title="运行命令"
          desc="通过引擎执行 shell 命令。"
          value={settings.runCommands}
          onChange={(v) => updatePermission('runCommands', v)}
        />
        <PermissionRow
          title="网络抓取"
          desc="允许代理读取外部 URL。"
          value={settings.fetchUrls}
          onChange={(v) => updatePermission('fetchUrls', v)}
        />
        <PermissionRow
          title="MCP 工具"
          desc="已连接的 MCP 服务器所提供的工具。"
          value={settings.mcpTools}
          onChange={(v) => updatePermission('mcpTools', v)}
        />
      </div>
      <div className="sgroup">
        <div className="sgroup__t" style={{ fontSize: '15px' }}>
          命令允许列表
        </div>
        <div className="sgroup__d">即使"运行命令"设为询问，这些命令也会无需提示直接运行。</div>
        <div className="allowlist">
          {settings.commandAllowlist.map((pattern) => (
            <div className="allowlist__row" key={pattern}>
              <code>{pattern}</code>
              <button
                className="btn-icon sm"
                title="移除允许模式"
                onClick={() => removePattern(pattern)}
                type="button"
              >
                <Icon name="x" />
              </button>
            </div>
          ))}
          {settings.commandAllowlist.length === 0 ? (
            <div className="allowlist__empty">暂无允许模式</div>
          ) : null}
        </div>
        <div className="row gap-sm" style={{ marginTop: '12px' }}>
          <input
            className="input mono"
            placeholder="git commit *"
            value={newPattern}
            onChange={(e) => setNewPattern(e.target.value)}
            onKeyDown={(e) => e.key === 'Enter' && addPattern()}
          />
          <button className="btn btn--subtle btn--sm" onClick={addPattern}>
            <Icon name="plus" /> 添加模式
          </button>
        </div>
      </div>
      <div className="sgroup">
        <div className="sgroup__t" style={{ fontSize: '15px' }}>
          始终允许的工具
        </div>
        <div className="sgroup__d">
          在会话中点过「始终允许」的工具会记录在这里，跨会话生效；移除后新会话将恢复询问。
        </div>
        <div className="allowlist">
          {alwaysAllow.map((tool) => (
            <div className="allowlist__row" key={tool}>
              <code>{tool}</code>
              <button
                className="btn-icon sm"
                title="撤销始终允许"
                onClick={() => void revokeAlwaysAllow(tool)}
                type="button"
              >
                <Icon name="x" />
              </button>
            </div>
          ))}
          {alwaysAllow.length === 0 ? (
            <div className="allowlist__empty">暂无记录——审批弹卡上点「始终允许」后会出现在这里</div>
          ) : null}
        </div>
      </div>
    </section>
  );
}

function McpTab({
  settings,
  update,
  onNavigate,
}: {
  settings: AppSettings['permissions'];
  update: (updater: (prev: AppSettings) => AppSettings) => void;
  onNavigate?: (page: PageId) => void;
}) {
  const [servers, setServers] = useState<McpServer[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let active = true;
    listMcpServers()
      .then((next) => {
        if (!active) return;
        setServers(next);
        setError(null);
      })
      .catch((err: unknown) => {
        if (!active) return;
        setServers([]);
        setError(err instanceof Error ? err.message : '无法读取 MCP 服务器');
      })
      .finally(() => {
        if (active) setLoading(false);
      });
    return () => {
      active = false;
    };
  }, []);

  return (
    <section>
      <div className="sgroup">
        <div className="sgroup__t">MCP 服务器</div>
        <div className="sgroup__d">Model Context Protocol 服务器为每个引擎扩展额外工具。</div>
        <p className="muted" style={{ fontSize: '12.5px', marginBottom: '14px' }}>
          MCP 服务器现已集中在「扩展中心」统一管理，这里仅保留全局工具策略。
          <button
            className="btn btn--sm"
            style={{ marginLeft: '8px' }}
            onClick={() => onNavigate?.('extensions')}
            type="button"
          >
            <Icon name="puzzle" /> 前往扩展中心
          </button>
        </p>
        <div className="srow2">
          <div className="srow2__m">
            <b>MCP 工具默认策略</b>
            <small>应用到 Claude Code 的 MCP 工具审批 hook。</small>
          </div>
          <div className="seg seg3 srow2__c">
            {(['allow', 'ask', 'deny'] as const).map((mode) => (
              <button
                key={mode}
                className={settings.mcpTools === mode ? 'is-active' : ''}
                onClick={() =>
                  update((prev) => ({
                    ...prev,
                    permissions: { ...prev.permissions, mcpTools: mode },
                  }))
                }
                type="button"
              >
                {mode === 'allow' ? '允许' : mode === 'ask' ? '询问' : '拒绝'}
              </button>
            ))}
          </div>
        </div>
        {loading ? <div className="empty">加载 MCP 服务器...</div> : null}
        {error ? (
          <div className="empty" style={{ color: 'var(--danger)' }}>
            {error}
          </div>
        ) : null}
        {!loading && !error && servers.length === 0 ? (
          <div className="empty">暂无 MCP 服务器</div>
        ) : null}
        {servers.map((server) => {
          const connected = server.status === 'connected';
          return (
            <div className="srow2" key={server.name}>
              <div className="srow2__m">
                <b>{server.name}</b>
                <small className="mono">
                  {mcpServerCommand(server) || server.transport} · {mcpStatusLabel(server.status)}
                </small>
              </div>
              <div className="row gap-sm srow2__c">
                <span className={connected ? 'pill pill--success' : 'pill'}>
                  <span className={connected ? 'dot dot--on' : 'dot dot--off'}></span>
                  {mcpStatusLabel(server.status)}
                </span>
                <span className="pill">{server.enabled ? '配置启用' : '配置停用'}</span>
              </div>
            </div>
          );
        })}
        <button
          className="btn btn--primary"
          style={{ marginTop: '18px' }}
          onClick={() => onNavigate?.('extensions')}
          type="button"
        >
          <Icon name="plus" /> 添加 MCP 服务器
        </button>
      </div>
    </section>
  );
}

function AppearanceTab({
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

  const handleAccentClick = (color: { base: string; hi: string }) => {
    update((prev) => ({ ...prev, appearance: { ...prev.appearance, accentColor: color } }));
  };

  const selectedIdx = ACCENT_COLORS.findIndex(
    (c) => c.base === settings.accentColor.base && c.hi === settings.accentColor.hi,
  );

  return (
    <section>
      <div className="sgroup">
        <div className="sgroup__t">外观</div>
        <div className="sgroup__d">调整桌面客户端的外观。强调色会立即应用到各处。</div>
        <div className="srow2">
          <div className="srow2__m">
            <b>主题</b>
            <small>浅色专为共享屏幕和演示而调校。跟随系统会遵循你的操作系统设置。</small>
          </div>
          <div className="seg seg3 srow2__c">
            {(['light', 'dark', 'system'] as const).map((theme) => (
              <button
                key={theme}
                className={settings.theme === theme ? 'is-active' : ''}
                onClick={() =>
                  update((prev) => ({
                    ...prev,
                    appearance: { ...prev.appearance, theme },
                  }))
                }
              >
                {theme === 'light' ? '浅色' : theme === 'dark' ? '深色' : '跟随系统'}
              </button>
            ))}
          </div>
        </div>
        <div className="srow2">
          <div className="srow2__m">
            <b>强调色</b>
            <small>用于操作、聚焦边框和激活状态。</small>
          </div>
          <div className="swatches srow2__c">
            {ACCENT_COLORS.map((color, idx) => (
              <button
                key={idx}
                className={`sw ${selectedIdx === idx ? 'is-on' : ''}`}
                style={{ background: color.base }}
                title={color.name}
                onClick={() => handleAccentClick(color)}
              />
            ))}
          </div>
        </div>
        <div className="srow2">
          <div className="srow2__m">
            <b>界面密度</b>
            <small>紧凑模式可在会话侧边栏和表格中容纳更多内容。</small>
          </div>
          <div className="seg srow2__c">
            {(['compact', 'comfortable'] as const).map((density) => (
              <button
                key={density}
                className={settings.uiDensity === density ? 'is-active' : ''}
                onClick={() =>
                  update((prev) => ({
                    ...prev,
                    appearance: { ...prev.appearance, uiDensity: density },
                  }))
                }
              >
                {density === 'compact' ? '紧凑' : '舒适'}
              </button>
            ))}
          </div>
        </div>
        <div className="srow2">
          <div className="srow2__m">
            <b>等宽字体</b>
            <small>用于代码、差异对比、模型 ID 和终端。</small>
          </div>
          <select
            className="select srow2__c"
            style={{ width: '180px' }}
            value={settings.monospaceFont}
            onChange={(e) =>
              update((prev) => ({
                ...prev,
                appearance: { ...prev.appearance, monospaceFont: e.target.value },
              }))
            }
          >
            <option>JetBrains Mono</option>
            <option>SF Mono</option>
            <option>Cascadia Code</option>
            <option>IBM Plex Mono</option>
          </select>
        </div>
        <div className="srow2">
          <div className="srow2__m">
            <b>减弱动效</b>
            <small>尽量减少动画和过渡效果。</small>
          </div>
          <label className="switch srow2__c">
            <input
              type="checkbox"
              checked={settings.reduceMotion}
              onChange={(e) =>
                update((prev) => ({
                  ...prev,
                  appearance: { ...prev.appearance, reduceMotion: e.target.checked },
                }))
              }
            />
            <i />
          </label>
        </div>
      </div>
    </section>
  );
}

function KeyboardTab({
  settings,
  update,
}: {
  settings: AppSettings['shortcuts'];
  update: (updater: (prev: AppSettings) => AppSettings) => void;
}) {
  const shortcuts = [
    { action: '命令面板', key: 'commandPalette' },
    { action: '新建会话', key: 'newSession' },
    { action: '导航前缀', key: 'navigationPrefix' },
    { action: '总览', key: 'home', prefix: true },
    { action: '打开工作区', key: 'workspace', prefix: true },
    { action: '服务商与模型', key: 'providers', prefix: true },
    { action: '会话历史', key: 'sessions', prefix: true },
    { action: '扩展中心', key: 'extensions', prefix: true },
    { action: '用量与成本', key: 'usage', prefix: true },
    { action: '设置', key: 'settings', prefix: true },
    { action: '切换上下文面板', key: 'toggleContext' },
    { action: '切换引擎', key: 'cycleEngine' },
  ] as const;

  const updateShortcut = (key: keyof AppSettings['shortcuts'], value: string) => {
    update((prev) => ({ ...prev, shortcuts: { ...prev.shortcuts, [key]: value } }));
  };

  const resetShortcuts = () => {
    update((prev) => ({ ...prev, shortcuts: DEFAULT_SETTINGS.shortcuts }));
  };

  const displayKeys = (shortcut: (typeof shortcuts)[number]) => {
    const value = settings[shortcut.key];
    return 'prefix' in shortcut && shortcut.prefix
      ? [settings.navigationPrefix, ...shortcutLabel(value)]
      : shortcutLabel(value);
  };

  // 冲突检测（变更-12）：同一按键被多个动作绑定时先到先得，后者永远不触发——必须可见
  const bindingCounts = new Map<string, number>();
  for (const shortcut of shortcuts) {
    const value = (settings[shortcut.key] ?? '').trim().toLowerCase();
    if (!value) continue;
    const full = 'prefix' in shortcut && shortcut.prefix ? `g+${value}` : value;
    bindingCounts.set(full, (bindingCounts.get(full) ?? 0) + 1);
  }
  const isConflicting = (shortcut: (typeof shortcuts)[number]) => {
    const value = (settings[shortcut.key] ?? '').trim().toLowerCase();
    if (!value) return false;
    const full = 'prefix' in shortcut && shortcut.prefix ? `g+${value}` : value;
    return (bindingCounts.get(full) ?? 0) > 1;
  };

  return (
    <section>
      <div className="sgroup">
        <div className="sgroup__t">快捷键</div>
        <div className="sgroup__d">应用中可用的快捷键绑定。</div>
        {shortcuts.map((s) => (
          <div key={s.action} className="kbdrow">
            <span>{s.action}</span>
            <span className="keys">
              {displayKeys(s).map((k, index) => (
                <span key={`${s.action}-${k}-${index}`} className="kbd">
                  {k}
                </span>
              ))}
              {isConflicting(s) ? (
                <span
                  className="pill pill--warn"
                  title="该按键被多个动作绑定，只有排在前面的动作会触发"
                >
                  冲突
                </span>
              ) : null}
            </span>
            <input
              className="input mono"
              style={{ width: 150 }}
              value={settings[s.key]}
              onChange={(e) => updateShortcut(s.key, e.target.value)}
            />
          </div>
        ))}
        <button className="btn btn--subtle btn--sm" type="button" onClick={resetShortcuts}>
          <Icon name="refresh" /> 恢复默认
        </button>
      </div>
    </section>
  );
}
