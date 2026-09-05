import { useCallback, useEffect, useState, type ReactNode } from 'react';
import type { EngineId } from '@helm/protocol';
import { Dialog as ShadcnDialog, DialogContent, DialogTitle } from '@/components/ui/dialog';
import { Icon } from '../shell/icons';
import { EngineBrand } from '../shell/EngineBrand';
import { showResultToast } from '../components/toast';
import {
  detectEngine,
  detectWorkspaceDeps,
  getReadinessReport,
  installCliEngine,
  installGit,
  selectDirectory,
} from './api';
import { engineConfigWithDetection } from './settingsViewModel';
import { getProviderConfig, saveEngineConfig } from '../providers/api';
import type { AppSettings } from './types';

export type PageIdLike = 'providers' | 'extensions';

// ─── 首启自动弹出判定（纯函数，供 App 与测试共用） ──────────────────────────

export interface SetupWizardReadiness {
  claudeInstalled: boolean;
  claudeDetail: string;
  codexInstalled: boolean;
  codexDetail: string;
  gitReady: boolean;
  gitDetail: string;
  hasReadyProvider: boolean;
  cwdOk: boolean;
  cwdPath: string;
}

/** 就绪度探测：与设置页向导同源（readiness report + 工作区依赖探测） */
export async function probeSetupWizardReadiness(): Promise<SetupWizardReadiness> {
  const [report, deps] = await Promise.all([getReadinessReport(), detectWorkspaceDeps()]);
  return {
    claudeInstalled: report.claudeCode.installed,
    claudeDetail: report.claudeCode.version
      ? report.claudeCode.version +
        (report.claudeCode.login.state === 'ok' ? '' : ' · ' + report.claudeCode.login.detail)
      : (report.claudeCode.error ?? '未检测到 claude CLI'),
    codexInstalled: report.codex.installed,
    codexDetail: report.codex.version
      ? report.codex.version +
        (report.codex.login.state === 'ok' ? '' : ' · ' + report.codex.login.detail)
      : (report.codex.error ?? '未检测到 codex CLI'),
    gitReady: deps.git.available,
    gitDetail: deps.git.version ?? '未检测到 Git',
    hasReadyProvider: report.hasReadyProvider,
    cwdOk: report.cwd.configured && report.cwd.exists,
    cwdPath: report.cwd.path,
  };
}

/**
 * 引导引擎选择（2026-09-02 用户约定）：只装 Codex 按 Codex 引导；
 * 只装 Claude Code / 两个都装 / 两个都没装 → 默认 Claude Code。
 */
export function selectGuideEngine(
  readiness: Pick<SetupWizardReadiness, 'claudeInstalled' | 'codexInstalled'>,
): EngineId {
  if (!readiness.claudeInstalled && readiness.codexInstalled) return 'codex';
  return 'claude-code';
}

/** 至少安装一个 Agent CLI 引擎才算 CLI 项就绪（按 selectGuideEngine 选中的引擎展示） */
export function setupWizardAllReady(readiness: SetupWizardReadiness): boolean {
  return (
    (readiness.claudeInstalled || readiness.codexInstalled) &&
    readiness.gitReady &&
    readiness.hasReadyProvider &&
    readiness.cwdOk
  );
}

/** 未全就绪才需要自动弹引导；环境就绪的老用户保持无感 */
export function shouldAutoShowSetupWizard(readiness: SetupWizardReadiness): boolean {
  return !setupWizardAllReady(readiness);
}

// ─── 跳过标记（纯 UI 偏好走 localStorage，同 Rail.tsx 视图偏好约定，不进 app_settings） ───

export const SETUP_WIZARD_DISMISS_KEY = 'helm:setup-wizard-dismissed';

export function readSetupWizardDismissed(): boolean {
  try {
    if (typeof window === 'undefined') return false;
    return window.localStorage.getItem(SETUP_WIZARD_DISMISS_KEY) === '1';
  } catch {
    return false;
  }
}

export function dismissSetupWizard(): void {
  try {
    if (typeof window === 'undefined') return;
    window.localStorage.setItem(SETUP_WIZARD_DISMISS_KEY, '1');
  } catch {
    // localStorage 不可用时静默：下次启动最多多弹一次引导，不影响功能
  }
}

// ─── 安装向导弹窗（设置页「关于」与工作台首启共用，保持视觉与行为一致） ─────────

type WizardState =
  | { kind: 'loading' }
  | { kind: 'error'; message: string }
  | { kind: 'ready'; readiness: SetupWizardReadiness };

export function SetupWizardModal({
  update,
  onNavigate,
  onClose,
}: {
  update: (updater: (prev: AppSettings) => AppSettings) => void;
  onNavigate?: (page: PageIdLike) => void;
  onClose: () => void;
}) {
  const [state, setState] = useState<WizardState>({ kind: 'loading' });
  const [busy, setBusy] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    setState({ kind: 'loading' });
    try {
      const readiness = await probeSetupWizardReadiness();
      setState({ kind: 'ready', readiness });
    } catch (error) {
      setState({
        kind: 'error',
        message: error instanceof Error ? error.message : String(error),
      });
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const readiness = state.kind === 'ready' ? state.readiness : null;
  const allReady = readiness ? setupWizardAllReady(readiness) : false;
  // CLI 行跟随引导引擎：只装了 Codex 就引导装 Codex，其余情况默认 Claude Code
  const guideEngine: EngineId = readiness ? selectGuideEngine(readiness) : 'claude-code';
  const cliInstalled = readiness
    ? guideEngine === 'codex'
      ? readiness.codexInstalled
      : readiness.claudeInstalled
    : false;
  const cliDetail = readiness
    ? guideEngine === 'codex'
      ? readiness.codexDetail
      : readiness.claudeDetail
    : '';

  // 只装了 Codex 的用户：默认引擎同步为 codex，否则工作台仍按 claude-code 起会话
  useEffect(() => {
    if (guideEngine === 'codex') {
      update((prev) =>
        prev.engines.defaultEngine === 'codex'
          ? prev
          : { ...prev, engines: { ...prev.engines, defaultEngine: 'codex' } },
      );
    }
  }, [guideEngine, update]);

  const installCli = async (engine: 'claude-code' | 'codex') => {
    setBusy(engine);
    try {
      const result = await installCliEngine(engine);
      showResultToast(
        (engine === 'claude-code' ? 'Claude Code' : 'Codex') + ' 已安装（' + result.version + '）',
      );
      // 与引擎页一致：安装成功后同步检测结果进引擎配置
      try {
        const detection = await detectEngine(engine);
        const config = await getProviderConfig();
        const engineConfig = config.engines.find((item) => item.id === engine);
        if (engineConfig) {
          await saveEngineConfig(engineConfigWithDetection(engineConfig, detection));
        }
        update((prev) => ({
          ...prev,
          engines: {
            ...prev.engines,
            [engine === 'claude-code' ? 'claudeCode' : 'codex']: {
              ...prev.engines[engine === 'claude-code' ? 'claudeCode' : 'codex'],
              executablePath: detection.path,
              version: detection.version,
              detected: true,
            },
          },
        }));
      } catch (syncError) {
        console.error('Failed to sync engine config', syncError);
      }
      await refresh();
    } catch (error) {
      showResultToast('一键安装失败：' + (error instanceof Error ? error.message : String(error)));
    } finally {
      setBusy(null);
    }
  };

  const installGitTool = async () => {
    setBusy('git');
    try {
      const result = await installGit();
      showResultToast(
        'Git 已安装（' +
          result.version +
          '）' +
          (result.restartRequired ? '；重启 Helm 后新进程才能使用' : ''),
      );
      await refresh();
    } catch (error) {
      showResultToast('安装失败：' + (error instanceof Error ? error.message : String(error)));
    } finally {
      setBusy(null);
    }
  };

  const chooseCwd = async () => {
    try {
      const selected = await selectDirectory();
      if (selected) {
        update((prev) => ({
          ...prev,
          general: { ...prev.general, defaultDirectory: selected },
        }));
        await refresh();
      }
    } catch (error) {
      showResultToast('选择目录失败：' + (error instanceof Error ? error.message : String(error)));
    }
  };

  return (
    <ShadcnDialog
      open
      onOpenChange={(open) => {
        if (!open) onClose();
      }}
    >
      <DialogContent aria-describedby={undefined} className="cm-setup-modal">
        <div className="cm-setup-hero">
          <span className="cm-setup-hero__mark">
            <Icon name="helm" />
          </span>
          <div>
            <DialogTitle>欢迎使用 Helm</DialogTitle>
            <p>
              开始任务前需要确认几项运行依赖。全部就绪后即可发送第一条指令，你也可以跳过稍后再配置。
            </p>
          </div>
        </div>

        {state.kind === 'loading' ? <div className="empty">正在检查运行依赖…</div> : null}
        {state.kind === 'error' ? (
          <div className="settings-inline-error" role="alert">
            <span>{state.message}</span>
            <button
              className="btn btn--subtle btn--sm"
              type="button"
              onClick={() => void refresh()}
            >
              重试
            </button>
          </div>
        ) : null}

        {readiness ? (
          <div className="cm-setup-list">
            <WizardRow
              ready={cliInstalled}
              icon={<EngineBrand engine={guideEngine} size={14} />}
              title={guideEngine === 'codex' ? 'Codex CLI' : 'Claude Code CLI'}
              detail={cliInstalled ? cliDetail : '通过 npm 安装，国内可使用镜像源加速'}
              action={
                cliInstalled ? undefined : (
                  <button
                    className="cm-action cm-setup-row__action"
                    type="button"
                    disabled={busy !== null}
                    onClick={() => void installCli(guideEngine)}
                  >
                    {busy === guideEngine ? '安装中…' : '安装'}
                  </button>
                )
              }
            />
            <WizardRow
              ready={readiness.gitReady}
              icon={<Icon name="gitbranch" />}
              title="Git for Windows"
              detail={readiness.gitDetail}
              action={
                readiness.gitReady ? undefined : (
                  <button
                    className="cm-action cm-setup-row__action"
                    type="button"
                    disabled={busy !== null}
                    onClick={() => void installGitTool()}
                  >
                    {busy === 'git' ? '安装中…' : '安装'}
                  </button>
                )
              }
            />
            <WizardRow
              ready={readiness.hasReadyProvider}
              icon={<Icon name="slidershorizontal" />}
              title="服务商配置"
              detail={
                readiness.hasReadyProvider
                  ? '至少一个服务商已就绪'
                  : '至少配置一个 API Key 或登录订阅账号'
              }
              action={
                readiness.hasReadyProvider ? undefined : (
                  <button
                    className="cm-action cm-setup-row__action"
                    type="button"
                    onClick={() => {
                      onClose();
                      onNavigate?.('providers');
                    }}
                  >
                    去配置
                  </button>
                )
              }
            />
            <WizardRow
              ready={readiness.cwdOk}
              icon={<Icon name="folder" />}
              title="工作目录"
              detail={readiness.cwdOk ? readiness.cwdPath : '选择任务要操作的项目文件夹'}
              action={
                readiness.cwdOk ? undefined : (
                  <button
                    className="cm-action cm-setup-row__action"
                    type="button"
                    onClick={() => void chooseCwd()}
                  >
                    选择
                  </button>
                )
              }
            />
          </div>
        ) : null}

        <div className="cm-setup-foot">
          <span className="cm-setup-foot__note">
            <Icon name="info" /> 安装优先走国内可直连镜像源，不引导科学上网。
          </span>
          <button className="cm-setup-skip" type="button" onClick={onClose}>
            跳过，稍后再说
          </button>
          <button
            className="cm-action cm-action--primary"
            type="button"
            disabled={!allReady}
            onClick={onClose}
          >
            全部就绪后继续
          </button>
        </div>
      </DialogContent>
    </ShadcnDialog>
  );
}

function WizardRow({
  ready,
  icon,
  title,
  detail,
  action,
}: {
  ready: boolean;
  icon: ReactNode;
  title: string;
  detail: string;
  action?: ReactNode;
}) {
  return (
    <div className={'cm-setup-row' + (ready ? ' is-ready' : ' is-missing')}>
      <span className="cm-setup-row__state">
        <Icon name={ready ? 'check' : 'alert'} />
      </span>
      <span className="cm-setup-row__icon">{icon}</span>
      <div className="cm-setup-row__main">
        <b>{title}</b>
        <small>{detail}</small>
      </div>
      {ready ? (
        <span className="cm-setup-row__done">
          <Icon name="check" /> 已就绪
        </span>
      ) : (
        <span className="cm-setup-row__action">{action}</span>
      )}
    </div>
  );
}
