import { lazy, Suspense, useEffect, useRef, useState } from 'react';
import { listen } from '@tauri-apps/api/event';
import { Titlebar } from './shell/Titlebar';
import { Rail, type PageId } from './shell/Rail';
import { ErrorBoundary } from './shell/ErrorBoundary';
import { ToastLayer } from './components/ToastLayer';
import { showToast } from './components/toast';
import { NewTaskPage } from './home/NewTaskPage';
import { LaunchOverlay } from './shell/LaunchOverlay';
import type { NewTaskLaunchConfig } from './home/newTaskViewModel';
import type { EngineId } from '@helm/protocol';
import { applyAppearanceSettings } from './settings/appearance';
import { loadSettings, saveSettings } from './settings/api';
import { CommandPaletteView } from './settings/CommandPaletteView';
import type { CommandPaletteCommand } from './settings/commandPalette';
import { workspaceIdentityFromSettings } from './settings/settingsViewModel';
import {
  type AppShortcutAction,
  type NavigationPrefix,
  reduceAppShortcut,
  matchesAppShortcut,
  shouldIgnoreNavigationShortcut,
} from './settings/shortcuts';
import { DEFAULT_SETTINGS, type AppSettings } from './settings/types';
import { forkTrace } from './diag/forkTrace';
import { LatestSerialSaver, type SaveState } from './settings/latestSerialSaver';
import { mostRecentSessionId, startupLandingFromRecovery } from './settings/settingsViewModel';
import { listSessions, getActiveSession } from './sessions/api';
import {
  SetupWizardModal,
  dismissSetupWizard,
  probeSetupWizardReadiness,
  readSetupWizardDismissed,
  shouldAutoShowSetupWizard,
  type PageIdLike,
} from './settings/SetupWizardModal';

const ProvidersPage = lazy(() =>
  import('./providers/ProvidersPage').then((module) => ({ default: module.ProvidersPage })),
);
const Workspace = lazy(() =>
  import('./workspace/Workspace').then((module) => ({ default: module.Workspace })),
);
const SessionsPage = lazy(() =>
  import('./sessions/SessionsPage').then((module) => ({ default: module.SessionsPage })),
);
const UsagePage = lazy(() =>
  import('./usage/UsagePage').then((module) => ({ default: module.UsagePage })),
);
const ExtensionsPage = lazy(() =>
  import('./extensions/ExtensionsPage').then((module) => ({ default: module.ExtensionsPage })),
);
const SettingsPage = lazy(() =>
  import('./settings/SettingsPage').then((module) => ({ default: module.SettingsPage })),
);

const TITLES: Record<PageId, string> = {
  home: '新任务',
  workspace: '工作区',
  sessions: '全部任务',
  providers: 'AI 配置',
  extensions: '插件',
  usage: '用量',
  settings: '设置',
};

export function App() {
  const [page, setPage] = useState<PageId>('workspace');
  const settingsReturnPageRef = useRef<PageId>('workspace');
  const [settings, setSettings] = useState<AppSettings>(DEFAULT_SETTINGS);
  const [navPrefix, setNavPrefix] = useState<NavigationPrefix>(null);
  const [paletteOpen, setPaletteOpen] = useState(false);
  const [pendingSessionId, setPendingSessionId] = useState<string | null>(null);
  // 启动首落页决策未出结果前不挂载 Workspace：否则决策窗口（并行拉设置/指针/会话列表）
  // 里工作区会先渲染一帧「开始新会话」空态，再跳到恢复的会话（2026-09-04 用户报告的闪现）。
  // 超时兜底：后端异常挂起时按「维持工作区」放行，不让窗口永久空白。
  const [startupLandingPending, setStartupLandingPending] = useState(true);
  const [pendingDraft, setPendingDraft] = useState<{
    id: number;
    text: string;
    attachments?: string[];
    launch?: NewTaskLaunchConfig;
  } | null>(null);
  // 启动过渡态与草稿投递解耦（2026-08-30 用户报告「过渡页没有了、像跳回新任务页」）：
  // Composer 挂载首帧就会消费草稿并回调 onDraftConsumed，pendingDraft 随即为 null。
  // 过渡页若继续绑 pendingDraft 就只闪一帧，且工作区同时回落到与新任务页雷同的
  // 「开始新会话」空态。改由独立 launchingEngine 承载，仅由 LaunchOverlay 依据真实
  // 后端事件（session_started / error / turn_complete）清除，对齐原型「四步走完再进工作区」。
  const [launchingEngine, setLaunchingEngine] = useState<EngineId | null>(null);
  const draftRequestRef = useRef(0);
  const [newSessionRequest, setNewSessionRequest] = useState(0);
  const [toggleContextRequest, setToggleContextRequest] = useState(0);
  const [cycleEngineRequest, setCycleEngineRequest] = useState(0);
  const [settingsSaveState, setSettingsSaveState] = useState<SaveState>('idle');
  const settingsSaverRef = useRef<LatestSerialSaver<AppSettings> | null>(null);
  if (!settingsSaverRef.current) {
    settingsSaverRef.current = new LatestSerialSaver(saveSettings, 400, setSettingsSaveState, () =>
      showToast('设置保存失败，修改可能在重启后丢失，请重试', 'error'),
    );
  }
  // Git 信息（批次 E）：标题栏显示「项目目录名 › 分支名」
  const [gitInfo, setGitInfo] = useState<{ projectName?: string; branchName?: string }>({});
  // 2026-08-27 对齐原型：任务标题入全局标题栏（原型 workspace-titlebar__task），
  // 右栏开关进标题栏 actions——工作区上报当前标题与展开态，离开工作区即复位。
  const [sessionTitle, setSessionTitle] = useState<string | null>(null);
  const [ctxExpanded, setCtxExpanded] = useState(false);
  // 首启安装引导（2026-09-02）：新装用户进入工作台默认弹出与设置页同款安装向导，
  // 可跳过（localStorage 标记 helm:setup-wizard-dismissed，纯 UI 偏好不进 app_settings）。
  // 探测到四项全就绪的老用户保持无感；跳过后仅保留设置→关于的手动入口兜底。
  const [setupWizardOpen, setSetupWizardOpen] = useState(false);

  useEffect(() => {
    let active = true;
    if (readSetupWizardDismissed()) return;
    probeSetupWizardReadiness()
      .then((readiness) => {
        if (!active) return;
        if (shouldAutoShowSetupWizard(readiness)) {
          setSetupWizardOpen(true);
        } else {
          // 首启探测即全就绪：视为引导已完成，之后不再自动弹
          dismissSetupWizard();
        }
      })
      .catch(() => {
        // 探测失败不阻断首屏；用户仍可从设置→关于→「进入安装向导」手动检查
      });
    return () => {
      active = false;
    };
  }, []);

  // 2026-08-28 消除新任务→工作区跳转卡顿：Workspace chunk 约 480KB，首航需 fetch+parse 才挂载。
  // 首屏空闲时预取（requestIdleCallback），用户点发送前 chunk 已缓存，跳转即时。
  useEffect(() => {
    let cancelled = false;
    const run = () => {
      if (cancelled) return;
      import('./workspace/Workspace').catch(() => {});
    };
    if ('requestIdleCallback' in window) {
      const h = window.requestIdleCallback(run, { timeout: 3000 });
      return () => {
        cancelled = true;
        window.cancelIdleCallback(h);
      };
    }
    const t = setTimeout(run, 1500);
    return () => {
      cancelled = true;
      clearTimeout(t);
    };
  }, []);

  useEffect(() => {
    let active = true;
    loadSettings()
      .then((next) => {
        if (active) {
          setSettings(next);
        }
      })
      .catch(() => {
        if (active) {
          setSettings(DEFAULT_SETTINGS);
          showToast('设置加载失败，已回退到默认设置', 'error');
        }
      });
    return () => {
      active = false;
    };
  }, []);

  // 启动首落页（2026-09-03 用户决议）：有上次任务进它的页面，没有任何任务进新任务页——
  // 兑现设置项「没有可恢复任务时直接进入新任务」的文案承诺。只在启动时决策一次；
  // 指针会话存在时留在工作区（既有 reopenLastSession 自动恢复链路接管），指针缺失但
  // 历史有任务时兜底打开最近会话（pendingSessionId 通道），一个任务都没有才去新任务页。
  // StrictMode 双挂载：仅当首跑被 cleanup 取消（未出结果）时回灌重试，与 Workspace
  // autoResumeAttempted 同款防抖。
  const startupLandingAttempted = useRef(false);
  useEffect(() => {
    if (startupLandingAttempted.current) return;
    let active = true;
    let settled = false;
    // 防御性兜底：invoke 全部挂起（后端异常）时按默认「维持工作区」放行渲染
    const guard = window.setTimeout(() => {
      if (!active || settled) return;
      settled = true;
      startupLandingAttempted.current = true;
      setStartupLandingPending(false);
    }, 2500);
    void (async () => {
      // settings 与恢复数据并行拉取；settings 加载失败按默认值决策（恢复开启）
      const [settingsResult, activeResult, sessionsResult] = await Promise.allSettled([
        loadSettings(),
        getActiveSession(),
        listSessions(),
      ]);
      if (!active) return;
      window.clearTimeout(guard);
      settled = true;
      startupLandingAttempted.current = true;
      const effectiveSettings =
        settingsResult.status === 'fulfilled' ? settingsResult.value : DEFAULT_SETTINGS;
      const landing = startupLandingFromRecovery(effectiveSettings, {
        hasActiveSession: activeResult.status === 'fulfilled' && activeResult.value !== null,
        recentSessionId:
          sessionsResult.status === 'fulfilled' ? mostRecentSessionId(sessionsResult.value) : null,
      });
      if (landing.kind === 'home') {
        setPage('home');
      } else if (landing.kind === 'recent') {
        setPendingSessionId(landing.sessionId);
      }
      // kind === 'workspace'：维持初始页（workspace），工作区自动恢复链路接管
      setStartupLandingPending(false);
    })();
    return () => {
      active = false;
      window.clearTimeout(guard);
      // StrictMode 清理后重跑：首次尝试未出结果不能永久吃掉第二次决策
      if (!settled) startupLandingAttempted.current = false;
    };
  }, []);

  useEffect(
    () => () => {
      void settingsSaverRef.current?.flush().catch(() => undefined);
    },
    [],
  );

  useEffect(() => {
    if (page !== 'workspace') {
      setSessionTitle(null);
      setCtxExpanded(false);
    }
  }, [page]);

  // 深层组件（错误卡/发送前置校验）的跨页跳转通道
  useEffect(() => {
    const onNavigate = (event: Event) => {
      const openSessionId = (event as CustomEvent<{ sessionId?: string }>).detail?.sessionId;
      if (event.type === 'helm:open-session' && openSessionId) {
        forkTrace('app_open_session_received', `target=${openSessionId}`);
        setPage('workspace');
        setPendingSessionId(openSessionId);
        return;
      }
      const page = (event as CustomEvent<{ page?: string }>).detail?.page;
      if (page) setPage(page as PageId);
    };
    window.addEventListener('helm:navigate', onNavigate);
    window.addEventListener('helm:open-session', onNavigate);
    return () => {
      window.removeEventListener('helm:navigate', onNavigate);
      window.removeEventListener('helm:open-session', onNavigate);
    };
  }, []);

  // 系统托盘菜单（P3-2）的跨页跳转：Rust 侧 emit("helm-navigate", "<page>")
  useEffect(() => {
    let active = true;
    let unlisten: (() => void) | null = null;
    void listen<string>('helm-navigate', (event) => {
      if (active && event.payload) setPage(event.payload as PageId);
    })
      .then((stop) => {
        if (active) unlisten = stop;
        else stop();
      })
      .catch(() => {
        // 浏览器预览没有 Tauri 事件桥；托盘也不存在，忽略
      });
    return () => {
      active = false;
      unlisten?.();
    };
  }, []);

  useEffect(() => {
    applyAppearanceSettings(settings.appearance);
  }, [settings.appearance]);

  useEffect(() => {
    if (page !== 'settings') settingsReturnPageRef.current = page;
  }, [page]);

  const persistSettings = (updater: (prev: AppSettings) => AppSettings) => {
    setSettings((prev) => {
      const next = updater(prev);
      // 无变化（updater 返回同引用）时跳过排程，避免探测型 effect 触发冗余保存
      if (next === prev) return prev;
      settingsSaverRef.current?.schedule(next);
      return next;
    });
  };

  const updateSettingsFromPage = (next: AppSettings) => {
    setSettings(next);
    settingsSaverRef.current?.schedule(next);
  };

  const runShortcutAction = (action: AppShortcutAction) => {
    if (action === 'open-command-palette') {
      setPaletteOpen(true);
      return;
    }
    if (action === 'new-session') {
      setPendingDraft(null);
      setLaunchingEngine(null);
      setPage('workspace');
      setNewSessionRequest((value) => value + 1);
      return;
    }
    if (action === 'toggle-context') {
      setPage('workspace');
      setToggleContextRequest((value) => value + 1);
      return;
    }
    if (action === 'cycle-engine') {
      setPage('workspace');
      setCycleEngineRequest((value) => value + 1);
    }
  };

  const runPaletteCommand = (command: CommandPaletteCommand) => {
    setPaletteOpen(false);
    if (command.type === 'session' && command.sessionId) {
      forkTrace('app_palette_session', `target=${command.sessionId}`);
      setPage('workspace');
      setPendingSessionId(command.sessionId);
      return;
    }
    if (command.type === 'provider') {
      setPage('providers');
      return;
    }
    if (command.page) setPage(command.page);
    if (command.action) runShortcutAction(command.action);
  };

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      const next = reduceAppShortcut(navPrefix, event, settings.shortcuts);
      const commandPaletteShortcut = matchesAppShortcut(
        event,
        'open-command-palette',
        settings.shortcuts,
      );
      if (shouldIgnoreNavigationShortcut(event.target) && !commandPaletteShortcut) return;
      if (next.action) {
        event.preventDefault();
        runShortcutAction(next.action);
      } else if (next.page) {
        event.preventDefault();
        setPage(next.page);
      } else if (next.prefix || navPrefix) {
        event.preventDefault();
      }
      if (next.action || next.page) {
        setNavPrefix(null);
        return;
      }
      setNavPrefix(next.prefix);
    };

    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, [navPrefix, settings.shortcuts]);

  // G 和弦前缀超时（变更-12）：按下 G 后 1.5 秒内不接后续键则失效，
  // 不再无限期吞掉下一个按键
  useEffect(() => {
    if (!navPrefix) return;
    const timer = window.setTimeout(() => setNavPrefix(null), 1500);
    return () => window.clearTimeout(timer);
  }, [navPrefix]);

  const workspaceIdentity = workspaceIdentityFromSettings(settings);

  return (
    <div className="app">
      <Titlebar
        title={TITLES[page]}
        workspaceName={workspaceIdentity.name}
        projectName={gitInfo.projectName}
        branchName={gitInfo.branchName}
        taskTitle={page === 'workspace' ? (sessionTitle ?? '未命名会话') : undefined}
        onToggleCtx={
          page === 'workspace' ? () => setToggleContextRequest((value) => value + 1) : undefined
        }
        ctxExpanded={ctxExpanded}
        onOpenCommandPalette={() => setPaletteOpen(true)}
        bare={page === 'home'}
        searchMode={page === 'settings' ? 'none' : 'icon'}
        onBack={page === 'settings' ? () => setPage(settingsReturnPageRef.current) : undefined}
        pageTitle={page === 'settings' ? '设置' : undefined}
      />
      <CommandPaletteView
        open={paletteOpen}
        onClose={() => setPaletteOpen(false)}
        onRun={runPaletteCommand}
      />
      <div className={`body${page === 'settings' ? ' body--secondary' : ''}`}>
        {page !== 'settings' ? (
          <Rail
            active={page}
            onSelect={setPage}
            onSetDefaultDirectory={(path) =>
              persistSettings((prev) => ({
                ...prev,
                general: { ...prev.general, defaultDirectory: path },
              }))
            }
          />
        ) : null}
        <ErrorBoundary
          key={page}
          label={TITLES[page]}
          onNavigateHome={page === 'home' ? undefined : () => setPage('home')}
        >
          <Suspense
            fallback={
              <main className="main">
                <div className="page scroll">
                  <div className="page__sub">正在加载{TITLES[page]}…</div>
                </div>
              </main>
            }
          >
            {page === 'home' ? (
              <main className="main">
                <NewTaskPage
                  defaultEngine={settings.engines.defaultEngine}
                  defaultDirectory={settings.general.defaultDirectory}
                  onNavigate={(p) => setPage(p as PageId)}
                  onStartTask={(draft) => {
                    draftRequestRef.current += 1;
                    setPendingDraft({
                      id: draftRequestRef.current,
                      text: draft.text,
                      attachments: draft.attachments,
                      launch: draft.config,
                    });
                    setLaunchingEngine(draft.config.engine);
                    setPage('workspace');
                    setNewSessionRequest((value) => value + 1);
                  }}
                />
              </main>
            ) : null}
            {page === 'workspace' && !startupLandingPending ? (
              <Workspace
                settings={settings}
                onSettingsChange={persistSettings}
                newSessionRequest={newSessionRequest}
                toggleContextRequest={toggleContextRequest}
                cycleEngineRequest={cycleEngineRequest}
                pendingSessionId={pendingSessionId}
                onClearPendingSessionId={() => setPendingSessionId(null)}
                draftRequest={pendingDraft}
                onDraftConsumed={() => setPendingDraft(null)}
                launching={launchingEngine !== null}
                onGitInfoChange={setGitInfo}
                onSessionTitleChange={setSessionTitle}
                onContextExpandedChange={setCtxExpanded}
              />
            ) : null}
            {page === 'providers' ? <ProvidersPage /> : null}
            {page === 'sessions' ? (
              <SessionsPage onOpenWorkspace={() => setPage('workspace')} />
            ) : null}
            {page === 'usage' ? (
              <main className="main">
                <UsagePage />
              </main>
            ) : null}
            {page === 'extensions' ? (
              <main className="main">
                <ExtensionsPage />
              </main>
            ) : null}
            {page === 'settings' ? (
              <main className="main">
                <SettingsPage
                  initialSettings={settings}
                  onSettingsChange={updateSettingsFromPage}
                  externalSaveState={settingsSaveState}
                  onNavigate={setPage}
                  onOpenWorkspace={() => setPage('workspace')}
                />
              </main>
            ) : null}
            {page !== 'home' &&
            page !== 'workspace' &&
            page !== 'providers' &&
            page !== 'sessions' &&
            page !== 'usage' &&
            page !== 'extensions' &&
            page !== 'settings' ? (
              <main className="main">
                <div className="page scroll">
                  <div className="page__head">
                    <div>
                      <div className="page__title">{TITLES[page]}</div>
                      <div className="page__sub">该界面会在后续切片按实施计划接入真实功能。</div>
                    </div>
                  </div>
                </div>
              </main>
            ) : null}
          </Suspense>
        </ErrorBoundary>
      </div>
      {launchingEngine ? (
        <LaunchOverlay engine={launchingEngine} onClear={() => setLaunchingEngine(null)} />
      ) : null}
      {setupWizardOpen ? (
        <SetupWizardModal
          update={persistSettings}
          onNavigate={(p: PageIdLike) => setPage(p as PageId)}
          onClose={() => {
            // 跳过/完成后都落跳过标记，之后启动不再自动弹
            dismissSetupWizard();
            setSetupWizardOpen(false);
          }}
        />
      ) : null}
      <ToastLayer />
    </div>
  );
}
