// 首启引导向导（可靠性检查 §4.3）：欢迎 → 引擎检测 → 接入方式 → 工作目录 → 完成。
// 每一步都可跳过，整个向导可关闭——跳过后由工作区的发送前置校验兜底，闭环不依赖向导。
import { useCallback, useEffect, useState } from 'react';
import { showToast } from '../components/toast';
import { useDialogBehavior } from '../components/useDialogBehavior';
import { isTauriRuntime } from '../lib/env';
import {
  getReadinessReport,
  installCliEngine,
  selectDirectory,
  type EngineReadiness,
  type ReadinessReport,
} from '../settings/api';
import type { AppSettings } from '../settings/types';

const INSTALL_COMMANDS: Record<'claude' | 'codex', { label: string; command: string }> = {
  claude: { label: 'Claude Code', command: 'npm install -g @anthropic-ai/claude-code' },
  codex: { label: 'Codex CLI', command: 'npm install -g @openai/codex' },
};

function EngineRow({
  name,
  bin,
  readiness,
  onInstalled,
}: {
  name: string;
  bin: 'claude' | 'codex';
  readiness: EngineReadiness | null;
  /** 一键安装成功后触发重新检测 */
  onInstalled: () => void;
}) {
  const [copied, setCopied] = useState(false);
  const [installing, setInstalling] = useState(false);
  const install = INSTALL_COMMANDS[bin];

  const handleInstall = async () => {
    setInstalling(true);
    try {
      const result = await installCliEngine(bin === 'claude' ? 'claude-code' : 'codex');
      showToast(`${name} 已安装（${result.version}）`, 'success');
      onInstalled();
    } catch (err) {
      showToast(`一键安装失败：${err instanceof Error ? err.message : String(err)}`, 'error');
    } finally {
      setInstalling(false);
    }
  };

  return (
    <div className="ob-engine">
      <div className="ob-engine__head">
        <b>{name}</b>
        {readiness === null ? (
          <span className="ob-pill">检测中…</span>
        ) : readiness.installed ? (
          <span className="ob-pill ob-pill--ok">已安装 · {readiness.version ?? '未知版本'}</span>
        ) : (
          <span className="ob-pill ob-pill--warn">未安装</span>
        )}
        {readiness?.installed ? (
          readiness.login.state === 'ok' ? (
            <span className="ob-pill ob-pill--ok">已登录</span>
          ) : readiness.login.state === 'missing' ? (
            <span className="ob-pill ob-pill--warn">未登录</span>
          ) : null
        ) : null}
      </div>
      {readiness?.installed && readiness.login.state === 'missing' ? (
        <div className="ob-engine__install">
          <div>
            订阅用户可在终端运行 <code>{bin === 'claude' ? 'claude' : 'codex login'}</code>{' '}
            完成登录（API Key 用户可跳过，在下一步填 Key）。
          </div>
        </div>
      ) : null}
      {readiness && !readiness.installed ? (
        <div className="ob-engine__install">
          <div>
            点「一键安装」自动执行下面的命令（需要本机已装 Node.js），或复制到终端手动执行：
          </div>
          <div className="ob-cmd">
            <code>{install.command}</code>
            <button
              type="button"
              className="btn btn--primary btn--sm"
              disabled={installing}
              onClick={() => void handleInstall()}
            >
              {installing ? '安装中，可能需要几分钟…' : '一键安装'}
            </button>
            <button
              type="button"
              className="btn btn--sm"
              disabled={installing}
              onClick={() => {
                void navigator.clipboard
                  ?.writeText(install.command)
                  .then(() => {
                    setCopied(true);
                    window.setTimeout(() => setCopied(false), 1600);
                  })
                  .catch(() => {
                    showToast('复制失败，请手动选中命令复制', 'error');
                  });
              }}
            >
              {copied ? '已复制' : '复制'}
            </button>
          </div>
        </div>
      ) : null}
      {readiness?.installed && readiness.path ? (
        <div className="ob-engine__path mono">{readiness.path}</div>
      ) : null}
    </div>
  );
}

export function OnboardingWizard({
  settings,
  onUpdateSettings,
  onFinish,
  onNavigate,
}: {
  settings: AppSettings;
  onUpdateSettings: (updater: (prev: AppSettings) => AppSettings) => void;
  /** 完成或跳过：持久化 onboardingCompleted 并关闭向导 */
  onFinish: (target?: 'workspace' | 'providers') => void;
  onNavigate?: (page: string) => void;
}) {
  const [step, setStep] = useState(0);
  const [readiness, setReadiness] = useState<ReadinessReport | null>(null);
  const [detecting, setDetecting] = useState(false);
  const dialogRef = useDialogBehavior(() => onFinish());

  const refresh = useCallback(async () => {
    setDetecting(true);
    try {
      setReadiness(await getReadinessReport());
    } catch {
      setReadiness(null);
      // 浏览器预览没有后端；桌面环境下检测失败必须可见，否则向导第 1 步永远空白
      if (isTauriRuntime()) {
        showToast('就绪度检测失败，可点击「重新检测」重试', 'error');
      }
    } finally {
      setDetecting(false);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const dir = settings.general.defaultDirectory.trim();

  const pickDirectory = async () => {
    try {
      const next = await selectDirectory();
      if (!next) return;
      onUpdateSettings((prev) => ({
        ...prev,
        general: { ...prev.general, defaultDirectory: next },
      }));
    } catch {
      // 用户取消或选择器失败，保持现状
    }
  };

  const checklist = readiness
    ? [
        {
          ok: readiness.claudeCode.installed || readiness.codex.installed,
          text: '至少一个 CLI 引擎已安装',
        },
        { ok: readiness.hasReadyProvider, text: '已配置可用的服务商（含密钥）' },
        {
          ok: readiness.boundEngines.includes(readiness.defaultEngine),
          text: '默认引擎已绑定服务商和模型',
        },
        { ok: Boolean(dir) && readiness.cwd.exists, text: '工作目录有效' },
      ]
    : [];

  return (
    <div
      className="ob-overlay"
      onMouseDown={(event) => event.target === event.currentTarget && onFinish()}
    >
      <div
        ref={dialogRef}
        className="ob-panel"
        role="dialog"
        aria-modal="true"
        aria-label="首启引导"
        tabIndex={-1}
      >
        <div className="ob-steps">
          {['欢迎', '引擎', '接入', '目录', '完成'].map((label, index) => (
            <span key={label} className={'ob-step' + (index === step ? ' is-active' : '')}>
              {label}
            </span>
          ))}
        </div>

        {step === 0 ? (
          <div className="ob-body">
            <h2>欢迎使用 Helm</h2>
            <p>
              Helm 统一管理 Claude Code、Codex 等 CLI Agent
              引擎：服务商、会话、审批、用量，一站式搞定。
            </p>
            <p className="ob-privacy">
              隐私说明：API 密钥只存操作系统钥匙串，会话数据只存本地 SQLite，匿名分析默认关闭。
            </p>
            <div className="ob-actions">
              <button type="button" className="btn btn--primary" onClick={() => setStep(1)}>
                开始配置
              </button>
              <button type="button" className="btn" onClick={() => onFinish()}>
                跳过引导
              </button>
            </div>
          </div>
        ) : null}

        {step === 1 ? (
          <div className="ob-body">
            <h2>检测 CLI 引擎</h2>
            <p>Helm 通过本机的 claude / codex 命令行工具工作，至少需要安装一个。</p>
            <EngineRow
              name="Claude Code"
              bin="claude"
              readiness={readiness?.claudeCode ?? null}
              onInstalled={() => void refresh()}
            />
            <EngineRow
              name="Codex CLI"
              bin="codex"
              readiness={readiness?.codex ?? null}
              onInstalled={() => void refresh()}
            />
            <div className="ob-actions">
              <button
                type="button"
                className="btn"
                onClick={() => void refresh()}
                disabled={detecting}
              >
                {detecting ? '检测中…' : '重新检测'}
              </button>
              <button type="button" className="btn btn--primary" onClick={() => setStep(2)}>
                下一步
              </button>
              <button type="button" className="btn" onClick={() => setStep(2)}>
                稍后安装，先跳过
              </button>
            </div>
          </div>
        ) : null}

        {step === 2 ? (
          <div className="ob-body">
            <h2>选择接入方式</h2>
            <div className="ob-choice">
              <b>A. 我有 Claude / ChatGPT 订阅</b>
              <p>
                在终端执行 <code>claude</code>（或 <code>codex login</code>）按提示完成登录，Helm
                会复用 CLI 的登录态。完成后回到上一步「重新检测」。
              </p>
            </div>
            <div className="ob-choice">
              <b>B. 我有 API Key / 中转服务</b>
              <p>到「服务商与模型」页添加服务商、填入密钥、同步模型并绑定引擎。</p>
              <button type="button" className="btn" onClick={() => onNavigate?.('providers')}>
                前往服务商页配置（向导保持打开）
              </button>
            </div>
            <div className="ob-actions">
              <button type="button" className="btn" onClick={() => setStep(1)}>
                上一步
              </button>
              <button type="button" className="btn btn--primary" onClick={() => setStep(3)}>
                下一步
              </button>
            </div>
          </div>
        ) : null}

        {step === 3 ? (
          <div className="ob-body">
            <h2>选择工作目录</h2>
            <p>Agent 将在这个目录里读写文件、执行命令。建议选择你的代码仓库目录。</p>
            <div className="ob-cmd">
              <code>{dir || '（未设置）'}</code>
              <button type="button" className="btn btn--sm" onClick={() => void pickDirectory()}>
                浏览…
              </button>
            </div>
            <div className="ob-actions">
              <button type="button" className="btn" onClick={() => setStep(2)}>
                上一步
              </button>
              <button
                type="button"
                className="btn btn--primary"
                onClick={() => {
                  void refresh();
                  setStep(4);
                }}
              >
                下一步
              </button>
            </div>
          </div>
        ) : null}

        {step === 4 ? (
          <div className="ob-body">
            <h2>就绪检查</h2>
            {checklist.length ? (
              <ul className="ob-checklist">
                {checklist.map((item) => (
                  <li key={item.text} className={item.ok ? 'is-ok' : 'is-warn'}>
                    {item.ok ? '✅' : '⚠️'} {item.text}
                  </li>
                ))}
              </ul>
            ) : (
              <p>无法获取就绪状态（可能在浏览器预览中）。</p>
            )}
            {checklist.some((item) => !item.ok) ? (
              <p className="ob-privacy">
                未就绪的项不影响完成引导：发送消息前 Helm 会再次校验并给出修复入口。
              </p>
            ) : null}
            <div className="ob-actions">
              <button type="button" className="btn" onClick={() => setStep(3)}>
                上一步
              </button>
              <button
                type="button"
                className="btn btn--primary"
                onClick={() => onFinish('workspace')}
              >
                开始第一个会话
              </button>
            </div>
          </div>
        ) : null}
      </div>
    </div>
  );
}
