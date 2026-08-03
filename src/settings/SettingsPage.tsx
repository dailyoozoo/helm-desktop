import { useState, useEffect, useRef } from 'react';
import { listen } from '@tauri-apps/api/event';
import { open } from '@tauri-apps/plugin-dialog';
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
  getPermissionRules,
  createPermissionDenyRule,
  removePermissionRule,
  getPermissionAuditSummary,
  exportPermissionAudit,
  clearPermissionAudit,
  checkForUpdate,
  installUpdate,
  type UpdateCheckResult,
  type PermissionAuditSummary,
} from './api';
import { type AppSettings, DEFAULT_SETTINGS, type UpdateStatus } from './types';
import { applyAppearanceSettings } from './appearance';
import { listMcpServers, type McpServer } from '../extensions/extensionsApi';
import {
  engineConfigWithDetection,
  mcpServerCommand,
  mcpStatusLabel,
  pricingFeedUrlsFromDraft,
  updateStatusSummary,
} from './settingsViewModel';
import { shortcutFromKeyboardEvent, shortcutLabel } from './shortcuts';
import type { PageId } from '../shell/Rail';
import {
  getProviderConfig,
  getPricingCatalogStatus,
  importPricingCatalog,
  refreshPricingCatalog,
  saveEngineConfig,
  type PricingCatalogStatus,
} from '../providers/api';
import { LatestSerialSaver, type SaveState } from './latestSerialSaver';
import { describePermissionRule, type PermissionRule } from './permissionRules';

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
  externalSaveState,
}: {
  initialSettings?: AppSettings;
  onSettingsChange?: (settings: AppSettings) => void;
  onNavigate?: (page: PageId) => void;
  externalSaveState?: SaveState;
} = {}) {
  const [tab, setTab] = useState<SettingTab>(() => {
    const requested = sessionStorage.getItem('helm:settings-tab') as SettingTab | null;
    sessionStorage.removeItem('helm:settings-tab');
    return requested ?? 'general';
  });
  const [settings, setSettings] = useState<AppSettings>(() => initialSettings ?? DEFAULT_SETTINGS);
  const [loading, setLoading] = useState(!initialSettings);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [loadAttempt, setLoadAttempt] = useState(0);
  const [localSaveState, setLocalSaveState] = useState<SaveState>('idle');
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

  if (loading) return <div className="setlayout">加载中...</div>;
  if (loadError) {
    return (
      <div className="setlayout">
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
    <div className="setlayout">
      <nav className="snav">
        <div className="snav__t">设置</div>
        <div className={`snav__save is-${externalSaveState ?? localSaveState}`} aria-live="polite">
          {(externalSaveState ?? localSaveState) === 'pending'
            ? '等待保存'
            : (externalSaveState ?? localSaveState) === 'saving'
              ? '保存中…'
              : (externalSaveState ?? localSaveState) === 'saved'
                ? '已保存'
                : (externalSaveState ?? localSaveState) === 'error'
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
              update={updateSettings}
              flushSettings={() => saverRef.current?.flush() ?? Promise.resolve()}
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
          {tab === 'permissions' && <PermissionsTab />}
          {tab === 'mcp' && <McpSummaryTab onNavigate={onNavigate} />}
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
  update,
  flushSettings,
  onNotify,
}: {
  settings: AppSettings['general'];
  update: (updater: (prev: AppSettings) => AppSettings) => void;
  flushSettings: () => Promise<void>;
  onNotify: (message: string) => void;
}) {
  const [updateStatus, setUpdateStatus] = useState<UpdateStatus | null>(null);
  const [pricingStatus, setPricingStatus] = useState<PricingCatalogStatus | null>(null);
  const [pricingBusy, setPricingBusy] = useState(false);
  const [pricingFeedDraft, setPricingFeedDraft] = useState(() =>
    settings.pricingFeedUrls.join('\n'),
  );
  useEffect(() => {
    setPricingFeedDraft(settings.pricingFeedUrls.join('\n'));
  }, [settings.pricingFeedUrls]);

  const commitPricingFeedDraft = () => {
    const pricingFeedUrls = pricingFeedUrlsFromDraft(pricingFeedDraft);
    if (
      pricingFeedUrls.length === settings.pricingFeedUrls.length &&
      pricingFeedUrls.every((url, index) => url === settings.pricingFeedUrls[index])
    ) {
      return;
    }
    update((prev) => ({
      ...prev,
      general: { ...prev.general, pricingFeedUrls },
    }));
  };

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

  useEffect(() => {
    let active = true;
    getPricingCatalogStatus()
      .then((status) => {
        if (active) setPricingStatus(status);
      })
      .catch(() => {
        if (active) setPricingStatus(null);
      });
    return () => {
      active = false;
    };
  }, []);

  const refreshPricing = async () => {
    setPricingBusy(true);
    try {
      // 镜像输入走 400ms 串行保存；立即点击更新时先刷盘，避免后端读到旧地址。
      await flushSettings();
      const status = await refreshPricingCatalog();
      setPricingStatus(status);
      onNotify(`价格目录已更新至 ${status.catalogVersion}`);
    } catch (error) {
      onNotify(`更新价格目录失败：${error instanceof Error ? error.message : String(error)}`);
    } finally {
      setPricingBusy(false);
    }
  };

  const importPricing = async () => {
    try {
      const selected = await open({
        title: '选择签名价格目录',
        multiple: false,
        filters: [{ name: 'Helm 价格目录', extensions: ['json'] }],
      });
      if (typeof selected !== 'string') return;
      setPricingBusy(true);
      const status = await importPricingCatalog(selected);
      setPricingStatus(status);
      onNotify(`离线价格目录已导入：${status.catalogVersion}`);
    } catch (error) {
      onNotify(`导入价格目录失败：${error instanceof Error ? error.message : String(error)}`);
    } finally {
      setPricingBusy(false);
    }
  };

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
            <b>会话自动起标题</b>
            <small>
              首轮结束后通过当前引擎的快速模型生成标题与摘要（会把首轮内容发给当前绑定服务商）。
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
            <b>系统通知</b>
            <small>轮次完成或出错时弹出 Windows 原生通知，即使 Helm 在后台也能收到提醒。</small>
          </div>
          <label className="switch srow2__c">
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
        <div className="srow2">
          <div className="srow2__m">
            <b>匿名使用分析</b>
            <small>
              控制 Helm 与子进程匿名诊断环境；关闭后会为 CLI 写入
              DO_NOT_TRACK=1。绝不包含提示词或代码。
            </small>
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
            <b>设置备份</b>
            <small>导出当前设置，或从本地文件恢复。</small>
          </div>
          <div className="row gap-sm srow2__c">
            <button className="btn btn--subtle btn--sm" onClick={handleExport} type="button">
              <Icon name="upright" /> 导出
            </button>
            <button className="btn btn--subtle btn--sm" onClick={handleImport} type="button">
              <Icon name="down" /> 导入
            </button>
          </div>
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
        <div className="srow2" style={{ marginTop: '18px' }}>
          <div className="srow2__m">
            <b>价格目录自动更新</b>
            <small>
              当前 {pricingStatus?.catalogVersion ?? '内置目录'} ·
              {pricingStatus?.source === 'cache' ? ' 已验证缓存' : ' 安装包内置'}
              {pricingStatus?.stale ? ' · 已超过新鲜度阈值' : ''}
              {pricingStatus?.lastError ? ` · 上次更新失败：${pricingStatus.lastError}` : ''}
            </small>
          </div>
          <label className="switch srow2__c">
            <input
              type="checkbox"
              checked={settings.pricingAutoUpdate}
              onChange={(event) =>
                update((prev) => ({
                  ...prev,
                  general: { ...prev.general, pricingAutoUpdate: event.target.checked },
                }))
              }
            />
            <i />
          </label>
        </div>
        <div className="field" style={{ marginTop: '12px' }}>
          <label>价格目录镜像（每行一个，国内主镜像在前）</label>
          <textarea
            className="input mono"
            rows={3}
            value={pricingFeedDraft}
            onChange={(event) => setPricingFeedDraft(event.target.value)}
            onBlur={commitPricingFeedDraft}
            placeholder="https://国内镜像.example.com/helm/pricing-catalog.json"
          />
          <div className="hint">
            留空时会尝试读取更新发布源同目录下的
            pricing-catalog.json；所有镜像不可达时继续使用本地目录。
          </div>
        </div>
        <div className="row gap-sm" style={{ marginTop: '10px' }}>
          <button
            className="btn btn--subtle btn--sm"
            disabled={pricingBusy}
            onClick={() => void refreshPricing()}
            type="button"
          >
            <Icon name="refresh" /> {pricingBusy ? '处理中…' : '立即更新价格'}
          </button>
          <button
            className="btn btn--subtle btn--sm"
            disabled={pricingBusy}
            onClick={() => void importPricing()}
            type="button"
          >
            <Icon name="down" /> 导入离线目录
          </button>
        </div>
        <div className="srow2">
          <div className="srow2__m">
            <b>预算遇到缺价模型</b>
            <small>严格模式会在预算启用时阻止缺价模型发送，避免成本被静默低估。</small>
          </div>
          <div className="seg srow2__c">
            <button
              className={settings.pricingUnknownPolicy === 'warn' ? 'is-active' : ''}
              onClick={() =>
                update((prev) => ({
                  ...prev,
                  general: { ...prev.general, pricingUnknownPolicy: 'warn' },
                }))
              }
            >
              提醒
            </button>
            <button
              className={settings.pricingUnknownPolicy === 'block' ? 'is-active' : ''}
              onClick={() =>
                update((prev) => ({
                  ...prev,
                  general: { ...prev.general, pricingUnknownPolicy: 'block' },
                }))
              }
            >
              严格
            </button>
          </div>
        </div>
        <div className="field" style={{ marginTop: '12px' }}>
          <label>价格目录新鲜度阈值（天）</label>
          <input
            className="input mono"
            type="number"
            min="1"
            max="3650"
            value={settings.pricingMaxAgeDays}
            onChange={(event) =>
              update((prev) => ({
                ...prev,
                general: {
                  ...prev.general,
                  pricingMaxAgeDays: Math.max(1, Number(event.target.value || '30')),
                },
              }))
            }
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
                ? `已检测 · ${settings.claudeCode.executablePath} · ${settings.claudeCode.version}`
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
              <Icon name="refresh" /> {detecting === 'claude-code' ? '检测中…' : '重新检测'}
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
                ? `已检测 · ${settings.codex.executablePath} · ${settings.codex.version}`
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
              <Icon name="refresh" /> {detecting === 'codex' ? '检测中…' : '重新检测'}
            </button>
          </div>
        </div>
      </div>
    </section>
  );
}

function PermissionsTab() {
  const [denyEngine, setDenyEngine] = useState<'all' | 'claude-code' | 'codex'>('all');
  const [denyCapability, setDenyCapability] = useState<
    | 'file_read'
    | 'directory_list'
    | 'file_write'
    | 'process_exec'
    | 'network_request'
    | 'mcp_invoke'
  >('process_exec');
  const [denyTarget, setDenyTarget] = useState('');
  const [denyProjectRoot, setDenyProjectRoot] = useState('');
  const [creatingDeny, setCreatingDeny] = useState(false);
  // SQLite Permission Ledger 是持久权限的事实真值；旧字符串清单只在后端迁移兼容。
  const [permissionRules, setPermissionRules] = useState<PermissionRule[]>([]);
  const [auditSummary, setAuditSummary] = useState<PermissionAuditSummary | null>(null);
  const [permissionLoadError, setPermissionLoadError] = useState<string | null>(null);
  const [auditLoadError, setAuditLoadError] = useState<string | null>(null);
  const [includeAuditResources, setIncludeAuditResources] = useState(false);
  const [confirmClearAudit, setConfirmClearAudit] = useState(false);

  useEffect(() => {
    let active = true;
    getPermissionRules()
      .then((rules) => {
        if (active) setPermissionRules(rules);
      })
      .catch((error: unknown) => {
        setPermissionLoadError(error instanceof Error ? error.message : '读取持久权限失败');
      });
    getPermissionAuditSummary()
      .then((summary) => {
        if (active) setAuditSummary(summary);
      })
      .catch((error: unknown) => {
        setAuditLoadError(error instanceof Error ? error.message : '读取权限审计失败');
      });
    return () => {
      active = false;
    };
  }, []);

  const revokePermissionRule = async (rule: PermissionRule) => {
    try {
      const result = await removePermissionRule(rule.id);
      setPermissionRules(result.rules);
      showResultToast(
        result.revocationTooLateCount > 0
          ? `已撤销规则；${result.revocationTooLateCount} 个操作已开始，无法追回，已写入审计`
          : `已撤销「${describePermissionRule(rule).title}」`,
      );
    } catch (error) {
      showResultToast(`撤销失败：${error instanceof Error ? error.message : String(error)}`);
    }
  };

  const createDenyRule = async () => {
    setCreatingDeny(true);
    try {
      const target = denyTarget.trim();
      const operation =
        denyCapability === 'process_exec' || denyCapability === 'mcp_invoke' ? target : '';
      const resourcePattern = operation ? '' : target;
      setPermissionRules(
        await createPermissionDenyRule({
          engine: denyEngine === 'all' ? null : denyEngine,
          capability: denyCapability,
          operation: operation || null,
          resourcePattern: resourcePattern || null,
          projectRoot: denyProjectRoot.trim() || null,
        }),
      );
      setDenyTarget('');
      showResultToast('显式拒绝规则已创建');
    } catch (error) {
      showResultToast(
        `创建拒绝规则失败：${error instanceof Error ? error.message : String(error)}`,
      );
    } finally {
      setCreatingDeny(false);
    }
  };

  const exportAudit = async () => {
    try {
      const path = await exportPermissionAudit(includeAuditResources);
      if (path) showResultToast(`权限审计已导出：${path}`);
    } catch (error) {
      showResultToast(`导出失败：${error instanceof Error ? error.message : String(error)}`);
    }
  };

  const clearAudit = async () => {
    if (!confirmClearAudit) {
      setConfirmClearAudit(true);
      return;
    }
    try {
      setAuditSummary(await clearPermissionAudit());
      setConfirmClearAudit(false);
      showResultToast('已清除可安全删除的权限审计；正在执行的记录会保留');
    } catch (error) {
      showResultToast(`清除失败：${error instanceof Error ? error.message : String(error)}`);
    }
  };

  return (
    <section>
      <div className="sgroup">
        <div className="sgroup__t" style={{ fontSize: '15px' }}>
          显式拒绝
        </div>
        <div className="sgroup__d">
          进入 Helm 权限裁决的动作不能用允许规则覆盖拒绝规则。Runtime
          自行执行且不发审批的动作不会进入这里，全部放开时尤其需要谨慎。
        </div>
        <div className="row gap-sm" style={{ marginTop: 12, flexWrap: 'wrap' }}>
          <select
            className="input"
            value={denyEngine}
            onChange={(event) => setDenyEngine(event.target.value as typeof denyEngine)}
          >
            <option value="all">所有引擎</option>
            <option value="claude-code">Claude Code</option>
            <option value="codex">Codex</option>
          </select>
          <select
            className="input"
            value={denyCapability}
            onChange={(event) => setDenyCapability(event.target.value as typeof denyCapability)}
          >
            <option value="file_read">读取文件</option>
            <option value="directory_list">浏览目录</option>
            <option value="file_write">修改文件</option>
            <option value="process_exec">执行命令</option>
            <option value="network_request">访问网络</option>
            <option value="mcp_invoke">调用 MCP</option>
          </select>
          <input
            className="input mono"
            value={denyTarget}
            onChange={(event) => setDenyTarget(event.target.value)}
            placeholder="命令可执行词、MCP 工具、路径或 URL；留空拒绝整类能力"
          />
        </div>
        <div className="row gap-sm" style={{ marginTop: 8 }}>
          <input
            className="input mono"
            value={denyProjectRoot}
            onChange={(event) => setDenyProjectRoot(event.target.value)}
            placeholder="项目目录；留空对所有项目生效"
          />
          <button
            className="btn btn--subtle btn--sm"
            type="button"
            onClick={async () => {
              const selected = await selectDirectory();
              if (selected) setDenyProjectRoot(selected);
            }}
          >
            <Icon name="folder" /> 选择项目
          </button>
          <button
            className="btn btn--danger btn--sm"
            type="button"
            disabled={creatingDeny}
            onClick={() => void createDenyRule()}
          >
            <Icon name="plus" /> {creatingDeny ? '创建中…' : '创建拒绝规则'}
          </button>
        </div>
      </div>
      <div className="sgroup">
        <div className="sgroup__t" style={{ fontSize: '15px' }}>
          持久权限规则
        </div>
        <div className="sgroup__d">
          Helm 按引擎、具体能力、项目路径和作用域记录永久授权；可在这里随时撤销。
        </div>
        {permissionLoadError ? (
          <div className="settings-inline-error" role="alert">
            <span>{permissionLoadError}</span>
            <button
              className="btn btn--subtle btn--sm"
              type="button"
              onClick={() => {
                setPermissionLoadError(null);
                void getPermissionRules()
                  .then(setPermissionRules)
                  .catch((error: unknown) =>
                    setPermissionLoadError(
                      error instanceof Error ? error.message : '读取持久权限失败',
                    ),
                  );
              }}
            >
              重试
            </button>
          </div>
        ) : null}
        <div className="allowlist">
          {permissionRules.map((rule) => {
            const description = describePermissionRule(rule);
            return (
              <div className="allowlist__row" key={rule.id}>
                <div>
                  <code>{description.title}</code>
                  <div className="sgroup__d" style={{ marginTop: 2 }}>
                    {description.meta}
                  </div>
                </div>
                <button
                  className="btn-icon sm"
                  title="撤销权限规则"
                  onClick={() => void revokePermissionRule(rule)}
                  type="button"
                >
                  <Icon name="x" />
                </button>
              </div>
            );
          })}
          {!permissionLoadError && permissionRules.length === 0 ? (
            <div className="allowlist__empty">
              暂无持久规则——审批卡上选择「永久允许」后会出现在这里
            </div>
          ) : null}
        </div>
      </div>
      <div className="sgroup">
        <div className="sgroup__t" style={{ fontSize: '15px' }}>
          权限审计
        </div>
        <div className="sgroup__d">
          默认保留 {auditSummary?.retentionDays ?? 90}{' '}
          天；导出默认只包含资源摘要，不包含路径、URL、Prompt 或文件内容。
        </div>
        {auditLoadError ? (
          <div className="settings-inline-error" role="alert">
            <span>{auditLoadError}</span>
            <button
              className="btn btn--subtle btn--sm"
              type="button"
              onClick={() => {
                setAuditLoadError(null);
                void getPermissionAuditSummary()
                  .then(setAuditSummary)
                  .catch((error: unknown) =>
                    setAuditLoadError(error instanceof Error ? error.message : '读取权限审计失败'),
                  );
              }}
            >
              重试
            </button>
          </div>
        ) : null}
        <div className="srow2">
          <div className="srow2__m">
            <b>{auditSummary ? `${auditSummary.recordCount} 条审计记录` : '正在读取审计记录…'}</b>
            <small>撤销已启动规则会标记为“撤销过晚”，不会伪装成已追回。</small>
          </div>
          <div className="row gap-sm srow2__c">
            <label className="check-row" title="仅在确有排障需要时开启">
              <input
                type="checkbox"
                checked={includeAuditResources}
                onChange={(event) => setIncludeAuditResources(event.target.checked)}
              />
              导出路径与 URL 明文
            </label>
            <button
              className="btn btn--subtle btn--sm"
              type="button"
              onClick={() => void exportAudit()}
            >
              导出审计
            </button>
            <button
              className={confirmClearAudit ? 'btn btn--danger btn--sm' : 'btn btn--subtle btn--sm'}
              type="button"
              onClick={() => void clearAudit()}
              onBlur={() => setConfirmClearAudit(false)}
            >
              {confirmClearAudit ? '再次点击确认清除' : '清除审计'}
            </button>
          </div>
        </div>
      </div>
    </section>
  );
}

function McpSummaryTab({ onNavigate }: { onNavigate?: (page: PageId) => void }) {
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
          MCP 服务器现已集中在「扩展中心」统一管理。
          <button
            className="btn btn--sm"
            style={{ marginLeft: '8px' }}
            onClick={() => onNavigate?.('extensions')}
            type="button"
          >
            <Icon name="puzzle" /> 前往扩展中心
          </button>
        </p>
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
  const [recordingKey, setRecordingKey] = useState<keyof AppSettings['shortcuts'] | null>(null);
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
            <button
              className={
                'btn btn--subtle btn--sm mono' + (recordingKey === s.key ? ' is-active' : '')
              }
              type="button"
              aria-label={`录制${s.action}快捷键`}
              onClick={() => setRecordingKey(s.key)}
              onKeyDown={(event) => {
                if (recordingKey !== s.key) return;
                event.preventDefault();
                event.stopPropagation();
                if (event.key === 'Escape') {
                  setRecordingKey(null);
                  return;
                }
                const value = shortcutFromKeyboardEvent(event);
                if (!value) return;
                updateShortcut(s.key, value);
                setRecordingKey(null);
              }}
            >
              {recordingKey === s.key ? '请按快捷键…' : '修改'}
            </button>
          </div>
        ))}
        <div className="kbdrow">
          <span>发送消息</span>
          <span className="row gap-sm">
            <span className="keys">
              <span className="kbd">Enter</span>
            </span>
            <span className="faint" style={{ fontSize: 11 }}>
              固定
            </span>
          </span>
        </div>
        <div className="kbdrow">
          <span>插入换行</span>
          <span className="row gap-sm">
            <span className="keys">
              <span className="kbd">Shift</span>
              <span className="kbd">Enter</span>
            </span>
            <span className="faint" style={{ fontSize: 11 }}>
              固定
            </span>
          </span>
        </div>
        <button className="btn btn--subtle btn--sm" type="button" onClick={resetShortcuts}>
          <Icon name="refresh" /> 恢复默认
        </button>
      </div>
    </section>
  );
}
