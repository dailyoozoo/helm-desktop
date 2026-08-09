import { lazy, Suspense, useEffect, useRef, useState } from 'react';
import { listen } from '@tauri-apps/api/event';
import { Titlebar } from './shell/Titlebar';
import { Rail, type PageId } from './shell/Rail';
import { ErrorBoundary } from './shell/ErrorBoundary';
import { ToastLayer } from './components/ToastLayer';
import { showToast } from './components/toast';
import { HomePage } from './home/HomePage';
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
import { LatestSerialSaver, type SaveState } from './settings/latestSerialSaver';

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
  home: '总览',
  workspace: '工作区',
  sessions: '会话历史',
  providers: '服务商与模型',
  extensions: '扩展中心',
  usage: '用量与成本',
  settings: '设置',
};

export function App() {
  const [page, setPage] = useState<PageId>('workspace');
  const [settings, setSettings] = useState<AppSettings>(DEFAULT_SETTINGS);
  const [navPrefix, setNavPrefix] = useState<NavigationPrefix>(null);
  const [paletteOpen, setPaletteOpen] = useState(false);
  const [pendingSessionId, setPendingSessionId] = useState<string | null>(null);
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

  useEffect(
    () => () => {
      void settingsSaverRef.current?.flush().catch(() => undefined);
    },
    [],
  );

  // 深层组件（错误卡/发送前置校验）的跨页跳转通道
  useEffect(() => {
    const onNavigate = (event: Event) => {
      const openSessionId = (event as CustomEvent<{ sessionId?: string }>).detail?.sessionId;
      if (event.type === 'helm:open-session' && openSessionId) {
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

  const persistSettings = (updater: (prev: AppSettings) => AppSettings) => {
    setSettings((prev) => {
      const next = updater(prev);
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
        onOpenCommandPalette={() => setPaletteOpen(true)}
      />
      <CommandPaletteView
        open={paletteOpen}
        onClose={() => setPaletteOpen(false)}
        onRun={runPaletteCommand}
      />
      <div className="body">
        <Rail
          active={page}
          onSelect={setPage}
          onNewSession={() => {
            setPage('workspace');
            setNewSessionRequest((value) => value + 1);
          }}
          workspaceName={workspaceIdentity.name}
          workspaceAvatar={workspaceIdentity.avatar}
        />
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
                <HomePage
                  onNavigate={(p) => setPage(p as PageId)}
                  onNewSession={() => {
                    setPage('workspace');
                    setNewSessionRequest((value) => value + 1);
                  }}
                />
              </main>
            ) : null}
            {page === 'workspace' ? (
              <Workspace
                settings={settings}
                onSettingsChange={persistSettings}
                newSessionRequest={newSessionRequest}
                toggleContextRequest={toggleContextRequest}
                cycleEngineRequest={cycleEngineRequest}
                pendingSessionId={pendingSessionId}
                onClearPendingSessionId={() => setPendingSessionId(null)}
                onGitInfoChange={setGitInfo}
              />
            ) : null}
            {page === 'providers' ? <ProvidersPage /> : null}
            {page === 'sessions' ? (
              <SessionsPage
                onOpenWorkspace={() => setPage('workspace')}
                onNewSession={() => {
                  setPage('workspace');
                  setNewSessionRequest((value) => value + 1);
                }}
              />
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
      <ToastLayer />
    </div>
  );
}
