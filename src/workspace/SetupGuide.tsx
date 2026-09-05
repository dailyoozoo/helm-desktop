import { useCallback, useEffect, useMemo, useState } from 'react';
import type { EngineId } from '@helm/protocol';
import { showToast } from '../components/toast';
import { Icon, type IconName } from '../shell/icons';
import { EngineBrand } from '../shell/EngineBrand';
import {
  detectEngine,
  detectWorkspaceDeps,
  installCliEngine,
  installGit,
  installNode,
  type ToolInstallResult,
  type WorkspaceDepStatus,
} from '../settings/api';

export type SetupDepId = 'node' | 'git' | 'cli';
export type SetupDepStatus = 'ok' | 'missing' | 'installing' | 'error';

export interface SetupGuideDeps {
  node: SetupDepStatus;
  npm: SetupDepStatus;
  git: SetupDepStatus;
  cli: SetupDepStatus;
}

export interface SetupGuideRow {
  id: SetupDepId;
  name: string;
  desc: string;
  icon: IconName;
  /** 引擎品牌标识（U-07）：存在时渲染优先于语义 icon */
  brand?: EngineId;
  status: SetupDepStatus;
  /** PATH 未刷新，需要重启 Helm 才可在新进程中解析 */
  restartRequired?: boolean;
}

export type { ToolInstallResult, WorkspaceDepStatus };
import type { WorkspaceDeps as BackendWorkspaceDeps } from '../settings/api';
export type WorkspaceDeps = BackendWorkspaceDeps;

/** 共享依赖（Node/npm/git）探测结果在 Helm 会话内缓存；CLI 探测按引擎实时执行 */
let sharedDepsCache: BackendWorkspaceDeps | null = null;

export async function probeSharedDepsCached(): Promise<BackendWorkspaceDeps> {
  if (sharedDepsCache) return sharedDepsCache;
  sharedDepsCache = await detectWorkspaceDeps();
  return sharedDepsCache;
}

export function invalidateSharedDepsCache(): void {
  sharedDepsCache = null;
}

/** 按当前引擎渲染三行依赖（Node/Git 共享，CLI 引擎独立）——纯函数，供组件与测试共用 */
export function setupGuideRows(
  deps: SetupGuideDeps,
  engine: EngineId,
  restartRequired: Partial<Record<SetupDepId, boolean>> = {},
): SetupGuideRow[] {
  const row = (
    id: SetupDepId,
    name: string,
    desc: string,
    icon: IconName,
    brand?: EngineId,
  ): SetupGuideRow => ({
    id,
    name,
    desc,
    icon,
    brand,
    status: deps[id],
    restartRequired: restartRequired[id],
  });
  return [
    row('node', 'Node.js 18+', '运行时与 npm', 'terminal'),
    row('git', 'Git', '代码库操作', 'gitbranch'),
    row(
      'cli',
      engine === 'codex' ? 'Codex CLI' : 'Claude Code CLI',
      'Agent 命令行工具',
      engine === 'codex' ? 'cpu' : 'zap',
      engine,
    ),
  ];
}

/** 三项全绿（Node + Git + 当前引擎 CLI）才算就绪，否则放行发送 */
export function setupGuideAllReady(deps: SetupGuideDeps): boolean {
  return deps.node === 'ok' && deps.git === 'ok' && deps.cli === 'ok';
}

function StatusMark({ status }: { status: SetupDepStatus }) {
  if (status === 'ok') {
    return (
      <span className="ws-sg__st is-ok">
        <Icon name="checkc" />
      </span>
    );
  }
  if (status === 'installing') {
    return (
      <span className="ws-sg__st is-installing">
        <i className="ws-sg__spin" />
      </span>
    );
  }
  return (
    <span className="ws-sg__st is-missing">
      <Icon name="xc" />
    </span>
  );
}

export function SetupGuide({
  engine,
  onReady,
  seedDeps,
}: {
  engine: EngineId;
  onReady: () => void;
  /** 测试/受控注入：提供后不做自动探测，直接用给定状态渲染 */
  seedDeps?: SetupGuideDeps;
}) {
  const [deps, setDeps] = useState<SetupGuideDeps | null>(seedDeps ?? null);
  const [busy, setBusy] = useState<SetupDepId | null>(null);
  const [restartRequired, setRestartRequired] = useState<Partial<Record<SetupDepId, boolean>>>({});
  const [loadError, setLoadError] = useState<string | null>(null);

  useEffect(() => {
    if (seedDeps) return;
    let active = true;
    (async () => {
      try {
        const shared = await probeSharedDepsCached();
        if (!active) return;
        setDeps({
          node: shared.node.available ? 'ok' : 'missing',
          npm: shared.npm.available ? 'ok' : 'missing',
          git: shared.git.available ? 'ok' : 'missing',
          cli: 'missing',
        });
        const cliStatus = await detectEngine(engine).then(
          () => 'ok' as const,
          () => 'missing' as const,
        );
        if (active) {
          setDeps((prev) => (prev ? { ...prev, cli: cliStatus } : prev));
        }
      } catch (error) {
        if (active) {
          setLoadError(error instanceof Error ? error.message : '检测环境依赖失败');
        }
      }
    })();
    return () => {
      active = false;
    };
  }, [engine, seedDeps]);

  const handleInstall = useCallback(
    async (id: SetupDepId) => {
      setBusy(id);
      setLoadError(null);
      try {
        if (id === 'node') {
          const result = await installNode();
          setRestartRequired((prev) => ({ ...prev, node: result.restartRequired }));
          setDeps((prev) => (prev ? { ...prev, node: 'ok', npm: 'ok' } : prev));
          if (result.restartRequired) {
            showToast('Node.js 已安装，重启 Helm 后生效', 'info');
          }
        } else if (id === 'git') {
          const result = await installGit();
          setRestartRequired((prev) => ({ ...prev, git: result.restartRequired }));
          setDeps((prev) => (prev ? { ...prev, git: 'ok' } : prev));
          if (result.restartRequired) {
            showToast('Git 已安装，重启 Helm 后生效', 'info');
          }
        } else {
          await installCliEngine(engine);
          setDeps((prev) => (prev ? { ...prev, cli: 'ok' } : prev));
        }
        invalidateSharedDepsCache();
        // 安装成功后复检共享依赖，保持状态与真实环境一致；
        // 已由安装结果标记为 ok 的项不被 PATH 未刷新时的复检结果降级。
        try {
          const shared = await detectWorkspaceDeps();
          setDeps((prev) =>
            prev
              ? {
                  ...prev,
                  node: shared.node.available ? 'ok' : prev.node,
                  npm: shared.npm.available ? 'ok' : prev.npm,
                  git: shared.git.available ? 'ok' : prev.git,
                }
              : prev,
          );
        } catch {
          // 复检失败保持当前状态，引导卡内错误文本已覆盖安装失败场景
        }
      } catch (error) {
        setLoadError(error instanceof Error ? error.message : '安装失败');
      } finally {
        setBusy(null);
      }
    },
    [engine],
  );

  const allReady = useMemo(() => (deps ? setupGuideAllReady(deps) : false), [deps]);

  useEffect(() => {
    if (allReady) onReady();
  }, [allReady, onReady]);

  if (!deps || allReady) return null;

  const rows = setupGuideRows(deps, engine, restartRequired);

  return (
    <div className="ws-sg">
      <div className="ws-sg__head">
        <Icon name="alert" />
        <span className="ws-sg__title">需要先安装以下环境，完成后即可对话</span>
        <small className="ws-sg__engtag">{engine === 'codex' ? 'Codex' : 'Claude Code'}</small>
      </div>

      <div className="ws-sg__list">
        {rows.map((row) => (
          <div className="ws-sg__row" key={row.id}>
            <StatusMark status={row.status} />
            <span className="ws-sg__ic">
              {row.brand ? <EngineBrand engine={row.brand} size={14} /> : <Icon name={row.icon} />}
            </span>
            <span className="ws-sg__meta">
              <b>{row.name}</b>
              <small>{row.desc}</small>
            </span>
            <span className="ws-sg__act">
              {row.status === 'ok' ? (
                <span className="ws-sg__pill">
                  {row.restartRequired ? '已安装 · 重启 Helm 后生效' : '已安装'}
                </span>
              ) : row.status === 'installing' ? (
                <button className="btn btn--sm" type="button" disabled>
                  <i className="ws-sg__spin" /> 安装中…
                </button>
              ) : (
                <button
                  className="btn btn--sm"
                  type="button"
                  disabled={busy !== null}
                  onClick={() => void handleInstall(row.id)}
                >
                  一键安装
                </button>
              )}
            </span>
          </div>
        ))}
      </div>

      <div className="ws-sg__note">
        <Icon name="info" />
        <span>使用国内镜像源（npmmirror）安装 · 无需额外网络配置</span>
      </div>

      {loadError ? <div className="ws-sg__error">{loadError}</div> : null}

      <div className="ws-sg__foot">
        <span className="ws-sg__hint">
          npm 源不可达时自动切换国内镜像（npmmirror）重试；安装完成后即可对话。
        </span>
        <span className={allReady ? 'ws-sg__ready' : 'ws-sg__waiting'}>
          {allReady ? '环境已就绪' : '环境未就绪，暂不能发送消息'}
        </span>
      </div>
    </div>
  );
}
