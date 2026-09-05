import { useCallback, useEffect, useMemo, useState } from 'react';
import { toolTarget } from './toolTarget';
import { Icon } from '../shell/icons';
import { ChangeReview } from './ChangeReview';
import { TasksPanel } from './TasksPanel';
import type { SessionState, ThreadItem } from '../engine/useSession';
import { isTerminalToolName } from './threadGroups';
import type { McpServer, Skill } from '../extensions/extensionsApi';
import type { PermissionProfile, RuntimeCapabilityAvailability } from '@helm/protocol';
import { activityLogGroups } from './activityLog';
import { getGitStatus, getGitStaged, type GitStatus, type StagedFile } from '../engine/transport';
import { openPathInSystem, readFilePreview, type FilePreview } from '../engine/transport';
import { searchWorkspaceFiles } from './workspaceApi';
import {
  CONTEXT_PANEL_DEFAULT_TAB,
  CONTEXT_PANEL_FIXED_TABS,
  CONTEXT_PANEL_FIXED_TAB_LABELS,
  DYN_TAB_LABELS,
  contextPanelData,
  closeDynTab as contextPanelDynTabsClose,
  isContextPanelFixedTab,
  openDynTab as contextPanelDynTabsOpen,
  workspaceFileRows,
  type ArtifactPaneTab,
  type ContextPanelFixedTab,
} from './contextPanelViewModel';
import { changeReviewFiles } from './changeReviewViewModel';

type Tab = ContextPanelFixedTab | ArtifactPaneTab;

/** S3：动态 tab 渲染顺序（changes/files 是常驻 tab；上下文已移入 Composer 圆环 popover）。 */
const DYN_CONTENT_ORDER: ArtifactPaneTab[] = ['plan', 'term', 'tasks'];
/* 批次②用户裁决：tabbar 右上只留「最大化/关闭」（原型 .ctx__tools），撤掉
   「活动日志/工具权限」常驻入口按钮。两个面板（log/tools）代码保留，
   仍可经 openPaneRequest 程序化打开，只是不再有常驻按钮入口。 */

const panelStyle = { display: 'flex', flexDirection: 'column', gap: 20 } as const;

/** 把相对 cwd 的路径拼成绝对路径；已是绝对/盘符路径则原样返回。 */
export function joinPath(cwd: string, relative: string): string {
  const trimmed = relative.trim();
  if (!trimmed) return trimmed;
  if (/^[a-zA-Z]:[\\/]/.test(trimmed) || trimmed.startsWith('\\\\') || trimmed.startsWith('~'))
    return trimmed;
  return `${cwd.replace(/[\\/]$/, '')}/${trimmed.replace(/^[\\/]+/, '')}`;
}

/** 变更-33：文件/附件预览面板（ContextPanel 内嵌）。 */
type PreviewState =
  | { path: string; label: string; data: FilePreview }
  | { path: string; label: string; error: string };

function FilePreviewPanel({
  preview,
  busy,
  onClose,
  onOpenSystem,
}: {
  preview: PreviewState;
  busy: boolean;
  onClose: () => void;
  onOpenSystem: () => void;
}) {
  if ('error' in preview) {
    return (
      <div className="filepreview">
        <div className="filepreview__bar">
          <Icon name="file" />
          <span className="filepreview__path" title={preview.path}>
            {preview.label}
          </span>
          <span className="sp" />
          <button
            type="button"
            className="ctx-tool"
            onClick={onOpenSystem}
            title="用系统默认程序打开"
          >
            <Icon name="upright" />
          </button>
          <button type="button" className="ctx-tool is-close" onClick={onClose} title="关闭预览">
            <Icon name="x" />
          </button>
        </div>
        <div className="filepreview__err">预览失败：{preview.error}</div>
      </div>
    );
  }
  const data = preview.data;
  return (
    <div className="filepreview">
      <div className="filepreview__bar">
        <Icon name="file" />
        <span className="filepreview__path" title={preview.path}>
          {preview.label}
        </span>
        <span className="sp" />
        {data.kind === 'binary' ? (
          <button type="button" className="btn btn--sm" onClick={onOpenSystem}>
            用系统默认程序打开
          </button>
        ) : null}
        <button type="button" className="ctx-tool is-close" onClick={onClose} title="关闭预览">
          <Icon name="x" />
        </button>
      </div>
      <div className="filepreview__body">
        {busy ? (
          <div className="filepreview__hint">读取中…</div>
        ) : data.kind === 'image' ? (
          data.content && !data.truncated ? (
            <img
              className="filepreview__img"
              src={`data:${data.mime ?? 'image/png'};base64,${data.content}`}
              alt={preview.label}
            />
          ) : (
            <div className="filepreview__hint">
              {data.truncated
                ? `图片过大（${(data.size / 1024 / 1024).toFixed(1)} MB），无法内嵌预览。`
                : '无法内嵌预览。'}
              <button type="button" className="btn btn--sm" onClick={onOpenSystem}>
                用系统默认程序打开
              </button>
            </div>
          )
        ) : data.kind === 'binary' ? (
          <div className="filepreview__hint">
            二进制文件（{(data.size / 1024).toFixed(1)} KB），无法内嵌预览。
            <button type="button" className="btn btn--sm" onClick={onOpenSystem}>
              用系统默认程序打开
            </button>
          </div>
        ) : (
          <pre className="filepreview__code">{data.content}</pre>
        )}
      </div>
    </div>
  );
}
const hintStyle = { color: 'var(--fg-4)', fontSize: 12.5, lineHeight: 1.6 } as const;
const listStyle = { display: 'flex', flexDirection: 'column', gap: 2 } as const;

function statusText(status: 'pending' | 'success' | 'error') {
  if (status === 'pending') return '运行中';
  if (status === 'success') return '成功';
  return '失败';
}

/** 活动日志行的目标摘要：共享 toolTarget 提取后按紧凑列表截断到 48 字符。 */
function toolTargetShort(item: Extract<SessionState['items'][number], { kind: 'tool' }>): string {
  const target = toolTarget(item.name, item.input).trim().split(/\r?\n/, 1)[0];
  return target.length > 48 ? `${target.slice(0, 47)}…` : target;
}

function toolLogMeta(item: Extract<SessionState['items'][number], { kind: 'tool' }>): string {
  const duration =
    item.startedAt && item.endedAt
      ? ` · ${Math.max(0, (item.endedAt - item.startedAt) / 1000).toFixed(1)}s`
      : '';
  return `${statusText(item.status)}${duration}`;
}

function capabilityPill(value: RuntimeCapabilityAvailability) {
  if (value === 'available') return { className: 'pill pill--success', label: '可用' };
  if (value === 'unavailable') return { className: 'pill', label: '不可用' };
  return { className: 'pill pill--warn', label: '未知' };
}

type PlanThreadItem = Extract<ThreadItem, { kind: 'plan' }>;

interface TermEntry {
  id: string;
  command: string;
  status: 'pending' | 'success' | 'error';
  output?: string;
  startedAt?: number;
  endedAt?: number;
  turnId?: string;
}

function extractTerminalEntries(items: ThreadItem[]): TermEntry[] {
  const entries: TermEntry[] = [];
  for (const item of items) {
    if (item.kind !== 'tool') continue;
    if (!isTerminalToolName(item.name)) continue;
    if (item.reverted) continue;
    const input = item.input as Record<string, unknown> | undefined;
    const rawCommand = input && typeof input === 'object' ? input.command : '';
    const command = Array.isArray(rawCommand)
      ? rawCommand.map(String).join(' ')
      : String(rawCommand ?? '');
    entries.push({
      id: item.id,
      command: command || item.name,
      status: item.status,
      output: item.output,
      startedAt: item.startedAt,
      endedAt: item.endedAt,
      turnId: item.turnId,
    });
  }
  return entries;
}

function formatTermDuration(start?: number, end?: number): string {
  if (!start || !end) return '';
  return `${Math.max(0, (end - start) / 1000).toFixed(1)}s`;
}

function PlanPanel({
  items,
  onLocateItem,
}: {
  items: ThreadItem[];
  onLocateItem?: (itemId: string) => void;
}) {
  const plans = useMemo(
    () => items.filter((item): item is PlanThreadItem => item.kind === 'plan' && !item.reverted),
    [items],
  );
  if (plans.length === 0) {
    return (
      <div className="ctx__panel" style={{ padding: 16 }}>
        <div className="aempty">
          <div className="aempty__in">
            <Icon name="flag" />
            <h4>本会话还没有计划</h4>
            <p>用「计划」模式让 Agent 先给出方案，确认后再执行。</p>
          </div>
        </div>
      </div>
    );
  }
  return (
    <div
      className="ctx__panel"
      style={{ padding: 16, display: 'flex', flexDirection: 'column', gap: 16 }}
    >
      {plans.map((plan) => {
        const done = plan.steps.filter((s) => s.status === 'done').length;
        const active = plan.steps.some((s) => s.status === 'active');
        return (
          <div
            key={plan.id}
            className="plan"
            style={{
              background: 'var(--surface-2)',
              borderRadius: 'var(--r)',
              padding: 12,
            }}
          >
            <div
              className="plan__t"
              style={{
                display: 'flex',
                alignItems: 'center',
                gap: 6,
                marginBottom: 8,
              }}
            >
              <Icon name="flag" />
              <span>计划</span>
              <span className="faint" style={{ marginLeft: 'auto' }}>
                {done}/{plan.steps.length} 步{active ? ' · 进行中' : ''}
              </span>
              {onLocateItem ? (
                <button
                  type="button"
                  className="btn-icon sm"
                  title="在线程中定位"
                  onClick={() => onLocateItem(plan.id)}
                >
                  <Icon name="left" />
                </button>
              ) : null}
            </div>
            <ul
              style={{
                listStyle: 'none',
                padding: 0,
                margin: 0,
                display: 'flex',
                flexDirection: 'column',
                gap: 4,
              }}
            >
              {plan.steps.map((step, index) => (
                <li
                  key={`${index}-${step.text}`}
                  style={{
                    display: 'flex',
                    alignItems: 'flex-start',
                    gap: 8,
                    fontSize: 13,
                    color:
                      step.status === 'done'
                        ? 'var(--fg-3)'
                        : step.status === 'active'
                          ? 'var(--fg)'
                          : 'var(--fg-2)',
                  }}
                >
                  <span
                    className="box"
                    style={{
                      width: 16,
                      height: 16,
                      flex: 'none',
                      display: 'grid',
                      placeItems: 'center',
                    }}
                  >
                    {step.status === 'done' ? (
                      <Icon name="check" className="h-3 w-3" style={{ width: 12, height: 12 }} />
                    ) : step.status === 'active' ? (
                      <i
                        style={{
                          width: 7,
                          height: 7,
                          borderRadius: '50%',
                          background: 'var(--accent)',
                          display: 'block',
                        }}
                      />
                    ) : (
                      <i
                        style={{
                          width: 7,
                          height: 7,
                          borderRadius: '50%',
                          border: '1px solid var(--border-2)',
                          display: 'block',
                        }}
                      />
                    )}
                  </span>
                  <span>{step.text}</span>
                </li>
              ))}
            </ul>
          </div>
        );
      })}
    </div>
  );
}

function TermPanel({
  items,
  onLocateItem,
}: {
  items: ThreadItem[];
  onLocateItem?: (itemId: string) => void;
}) {
  const entries = useMemo(() => extractTerminalEntries(items), [items]);
  if (entries.length === 0) {
    return (
      <div className="ctx__panel" style={{ padding: 16 }}>
        <div className="aempty">
          <div className="aempty__in">
            <Icon name="terminal" />
            <h4>还没有可回看的终端输出</h4>
            <p>终端命令的完整输出会在这里停留，便于反复回看。</p>
          </div>
        </div>
      </div>
    );
  }
  return (
    <div
      className="ctx__panel"
      style={{ padding: 16, display: 'flex', flexDirection: 'column', gap: 10 }}
    >
      {entries.map((entry) => {
        const dur = formatTermDuration(entry.startedAt, entry.endedAt);
        return (
          <div
            key={entry.id}
            style={{
              border: '1px solid var(--line)',
              borderRadius: 'var(--r-sm)',
              overflow: 'hidden',
              background: 'var(--surface-2)',
            }}
          >
            <div
              style={{
                display: 'flex',
                alignItems: 'center',
                gap: 8,
                padding: '6px 10px',
                borderBottom: '1px solid var(--line)',
                fontSize: 12.5,
              }}
            >
              <Icon
                name="terminal"
                style={{ width: 14, height: 14, flex: 'none', color: 'var(--fg-3)' }}
              />
              <code
                className="mono"
                title={entry.command}
                style={{
                  flex: 1,
                  minWidth: 0,
                  overflow: 'hidden',
                  textOverflow: 'ellipsis',
                  whiteSpace: 'nowrap',
                  fontSize: 12,
                }}
              >
                {entry.command}
              </code>
              <span
                className={`pill ${entry.status === 'success' ? 'pill--success' : entry.status === 'error' ? 'pill--danger' : 'pill--warn'}`}
                style={{ height: 18 }}
              >
                {statusText(entry.status)}
              </span>
              {dur ? <span className="faint mono">{dur}</span> : null}
              {onLocateItem ? (
                <button
                  type="button"
                  className="btn-icon sm"
                  title="在线程中定位"
                  onClick={() => onLocateItem(entry.id)}
                >
                  <Icon name="left" />
                </button>
              ) : null}
            </div>
            {entry.output ? (
              <pre
                className="term__out"
                style={{
                  margin: 0,
                  padding: 10,
                  maxHeight: 240,
                  overflow: 'auto',
                  fontSize: 12,
                  fontFamily: 'var(--font-mono)',
                  whiteSpace: 'pre-wrap',
                  wordBreak: 'break-all',
                }}
              >
                {entry.output}
              </pre>
            ) : (
              <div style={{ padding: 10, color: 'var(--fg-4)', fontSize: 12 }}>
                {entry.status === 'pending' ? '执行中…' : '无输出'}
              </div>
            )}
          </div>
        );
      })}
    </div>
  );
}

export function ContextPanel({
  state,
  permissionProfile,
  mcpServers = [],
  skills = [],
  mcpLoadError,
  skillsLoadError,
  onRetryExtensions,
  onToggleMcp,
  onOpenExtensions,
  onLocateItem,
  onStopTask,
  openPaneRequest,
  onCollapse,
}: {
  state: SessionState;
  permissionProfile: PermissionProfile;
  mcpServers?: McpServer[];
  skills?: Skill[];
  mcpLoadError?: string | null;
  skillsLoadError?: string | null;
  onRetryExtensions?: () => void;
  onToggleMcp?: (name: string) => void | Promise<void>;
  onOpenExtensions?: () => void;
  onLocateItem?: (itemId: string) => void;
  /** 变更-34 · C2：停止后台命令（真实能力为中断当前轮次）。 */
  onStopTask?: () => void;
  /** 变更-34 · A5：收起右栏面板（Workspace 关闭 showCtx）。 */
  onCollapse?: () => void;
  /** 变更-34 · A4：外部请求打开某个交付物动态 tab（request 递增触发）；常驻 tab 直接切换。 */
  openPaneRequest?: { tab: Tab; request: number } | null;
}) {
  // S3：默认激活常驻「修改记录」，标签与原型一致。
  const [tab, setTab] = useState<Tab>(CONTEXT_PANEL_DEFAULT_TAB);
  // 变更-34 · A4：已打开的交付物动态 tab（保持打开顺序）。
  const [dynTabs, setDynTabs] = useState<ArtifactPaneTab[]>([]);
  const applyDynTabs = useCallback(
    (next: { open: ArtifactPaneTab[]; active: ArtifactPaneTab | null }) => {
      setDynTabs(next.open);
      // 关闭动态 tab 后回退到默认常驻 tab「修改记录」。
      setTab(next.active ?? CONTEXT_PANEL_DEFAULT_TAB);
    },
    [],
  );
  const activeDyn = isContextPanelFixedTab(tab) ? null : tab;
  const openDynTab = useCallback(
    (paneId: ArtifactPaneTab) => {
      applyDynTabs(contextPanelDynTabsOpen({ open: dynTabs, active: activeDyn }, paneId));
    },
    [applyDynTabs, dynTabs, activeDyn],
  );
  const closeDynTab = useCallback(
    (paneId: ArtifactPaneTab) => {
      applyDynTabs(contextPanelDynTabsClose({ open: dynTabs, active: activeDyn }, paneId));
    },
    [applyDynTabs, dynTabs, activeDyn],
  );
  useEffect(() => {
    if (!openPaneRequest) return;
    if (isContextPanelFixedTab(openPaneRequest.tab)) setTab(openPaneRequest.tab);
    else openDynTab(openPaneRequest.tab);
  }, [openPaneRequest, openDynTab]);
  // Git 状态（批次 E）
  const [gitStatus, setGitStatus] = useState<GitStatus | undefined>();
  const [stagedFiles, setStagedFiles] = useState<StagedFile[] | undefined>();
  // S3「全部文件」：真实 search_workspace_files 结果（空查询返回最浅 30 条）
  const [allFiles, setAllFiles] = useState<string[] | null>(null);
  const [allFilesError, setAllFilesError] = useState<string | null>(null);
  // 变更-33：文件/附件预览
  const [preview, setPreview] = useState<PreviewState | null>(null);
  const [previewBusy, setPreviewBusy] = useState(false);

  const previewFile = useCallback(
    async (rawPath: string, label: string, needsCwd: boolean) => {
      setPreviewBusy(true);
      try {
        const path = needsCwd && state.cwd ? joinPath(state.cwd, rawPath) : rawPath;
        const data = await readFilePreview(path);
        setPreview({ path, label, data });
      } catch (error) {
        setPreview({ path: rawPath, label, error: String(error) });
      } finally {
        setPreviewBusy(false);
      }
    },
    [state.cwd],
  );

  const openInSystem = useCallback(
    async (rawPath: string, needsCwd: boolean) => {
      const path = needsCwd && state.cwd ? joinPath(state.cwd, rawPath) : rawPath;
      try {
        await openPathInSystem(path);
      } catch (error) {
        // 静默展示：由系统打开失败时不强刷 UI，仅保留该路径信息
        setPreview({ path, label: rawPath, error: String(error) });
      }
    },
    [state.cwd],
  );

  // S3「全部文件」：cwd 变化时经真实 search_workspace_files 列出工作区文件（空查询=最浅 30 条）
  useEffect(() => {
    if (!state.cwd) {
      setAllFiles(null);
      setAllFilesError(null);
      return;
    }
    let cancelled = false;
    searchWorkspaceFiles(state.cwd, '')
      .then((files) => {
        if (cancelled) return;
        setAllFiles(files);
        setAllFilesError(null);
      })
      .catch((error) => {
        if (cancelled) return;
        setAllFiles(null);
        setAllFilesError(String(error));
      });
    return () => {
      cancelled = true;
    };
  }, [state.cwd]);

  // 当 cwd 变化时获取 git 状态
  useEffect(() => {
    if (!state.cwd) {
      setGitStatus(undefined);
      setStagedFiles(undefined);
      return;
    }

    let cancelled = false;

    Promise.all([getGitStatus(state.cwd), getGitStaged(state.cwd)])
      .then(([status, staged]) => {
        if (cancelled) return;
        setGitStatus(status);
        setStagedFiles(staged);
      })
      .catch(() => {
        if (cancelled) return;
        // git 获取失败（可能不是 git 仓库）
        setGitStatus(undefined);
        setStagedFiles(undefined);
      });

    return () => {
      cancelled = true;
    };
  }, [state.cwd]);

  // 派生数据 memo（变更-09）：流式期间 Workspace 每帧重渲染，items/cost 未变时不重算
  const data = useMemo(
    () => contextPanelData(state.items, state.cost, gitStatus, stagedFiles),
    [state.items, state.cost, gitStatus, stagedFiles],
  );
  const fileRows = useMemo(
    () => workspaceFileRows(allFiles ?? [], stagedFiles),
    [allFiles, stagedFiles],
  );
  // 原型 #dtabChgCnt：修改记录 tab 上的变更文件计数（与 ChangeReview 同源派生）
  const changeCount = useMemo(() => changeReviewFiles(state.items).files.length, [state.items]);
  const activityGroups = useMemo(() => activityLogGroups(state.items), [state.items]);
  const enabledSkills = skills.filter((skill) => skill.enabled);
  const connectedMcp = mcpServers.filter(
    (server) => (server.toolCount ?? 0) > 0 && !server.lastError,
  );

  // 变更-34 · A5：交付物区最大化/收起。最大化覆盖 --ctx-w（还原回拖拽记忆值或 CSS 默认 clamp）。
  const [paneMaximized, setPaneMaximized] = useState(false);
  const toggleMaximize = useCallback(() => {
    const root = document.documentElement;
    if (paneMaximized) {
      try {
        const saved = localStorage.getItem('helm:ctxw');
        if (saved) {
          // 切片 A（P1-04）：旧持久化值低于新最小宽度（360px）时丢弃，回落到 CSS clamp 默认值。
          const numeric = parseInt(saved, 10);
          if (saved.endsWith('px') && !Number.isNaN(numeric) && numeric < 360) {
            root.style.removeProperty('--ctx-w');
          } else {
            root.style.setProperty('--ctx-w', saved);
          }
        } else {
          root.style.removeProperty('--ctx-w');
        }
      } catch {
        root.style.removeProperty('--ctx-w');
      }
      setPaneMaximized(false);
    } else {
      root.style.setProperty('--ctx-w', 'min(92vw, 1440px)');
      setPaneMaximized(true);
    }
  }, [paneMaximized]);

  return (
    <aside className="ctx">
      <div className="ctx__tabs tabbar" role="tablist" aria-label="交付物区">
        {CONTEXT_PANEL_FIXED_TABS.map((id) => (
          <button
            key={id}
            role="tab"
            id={`ctx-tab-${id}`}
            aria-selected={tab === id}
            aria-controls={`ctx-panel-${id}`}
            className={'tab' + (tab === id ? ' is-active' : '')}
            onClick={() => setTab(id)}
            onKeyDown={(event) => {
              if (event.key === 'ArrowRight' || event.key === 'ArrowLeft') {
                event.preventDefault();
                const all = [...CONTEXT_PANEL_FIXED_TABS, ...dynTabs] as Tab[];
                const currentIndex = all.indexOf(tab);
                const direction = event.key === 'ArrowRight' ? 1 : -1;
                const nextIndex = (currentIndex + direction + all.length) % all.length;
                setTab(all[nextIndex] ?? CONTEXT_PANEL_DEFAULT_TAB);
              }
            }}
          >
            {CONTEXT_PANEL_FIXED_TAB_LABELS[id]}
            {id === 'changes' && changeCount > 0 ? (
              <span className="tab__n">{changeCount}</span>
            ) : null}
          </button>
        ))}
        {dynTabs.length > 0 && (
          <span className="ctx__dyn">
            {dynTabs.map((id) => {
              const label = DYN_TAB_LABELS[id];
              return (
                <button
                  key={id}
                  role="tab"
                  id={`ctx-tab-${id}`}
                  aria-selected={tab === id}
                  aria-controls={`ctx-panel-${id}`}
                  className={'tab tab--dyn' + (tab === id ? ' is-active' : '')}
                  onClick={() => setTab(id)}
                  onKeyDown={(event) => {
                    if (event.key === 'ArrowRight' || event.key === 'ArrowLeft') {
                      event.preventDefault();
                      const all = [...CONTEXT_PANEL_FIXED_TABS, ...dynTabs] as Tab[];
                      const currentIndex = all.indexOf(tab);
                      const direction = event.key === 'ArrowRight' ? 1 : -1;
                      const nextIndex = (currentIndex + direction + all.length) % all.length;
                      setTab(all[nextIndex] ?? CONTEXT_PANEL_DEFAULT_TAB);
                    }
                    if (event.key === 'Delete' || event.key === 'Backspace') {
                      event.preventDefault();
                      const tabButton = event.currentTarget;
                      closeDynTab(id);
                      // 切片 D · P2-03：焦点回退到上一个仍存在的 tab，
                      // 不要让键盘用户跳出 tablist 顺序（closeDynTab 会切到 changes，
                      // 这里只确保新焦点真的落在该 tab button 上）。
                      window.requestAnimationFrame(() => {
                        const target = document.getElementById('ctx-tab-changes');
                        if (target) {
                          target.focus();
                        } else {
                          tabButton.focus();
                        }
                      });
                    }
                  }}
                >
                  {label}
                  <button
                    type="button"
                    className="tab__x"
                    aria-label={`关闭${label}面板`}
                    title={`关闭${label}面板（Delete）`}
                    onClick={(event) => {
                      event.stopPropagation();
                      const xButton = event.currentTarget;
                      closeDynTab(id);
                      window.requestAnimationFrame(() => {
                        const target = document.getElementById('ctx-tab-changes');
                        if (target) {
                          target.focus();
                        } else {
                          xButton.focus();
                        }
                      });
                    }}
                  >
                    <Icon name="x" />
                  </button>
                </button>
              );
            })}
          </span>
        )}
        {/* 批次②裁决：右上只留 最大化/关闭（原型 .ctx__tools）。 */}
        <span className="ctx__tools">
          <button
            type="button"
            className="ctx-tool"
            title={paneMaximized ? '还原右侧工作区' : '最大化右侧工作区'}
            aria-label={paneMaximized ? '还原右侧工作区' : '最大化右侧工作区'}
            aria-pressed={paneMaximized}
            onClick={toggleMaximize}
          >
            <Icon name={paneMaximized ? 'compress' : 'expand'} />
          </button>
          <button
            type="button"
            className="ctx-tool is-close"
            title="关闭右侧工作区"
            aria-label="关闭右侧工作区"
            onClick={onCollapse}
          >
            <Icon name="x" />
          </button>
        </span>
      </div>
      {preview ? (
        <FilePreviewPanel
          preview={preview}
          busy={previewBusy}
          onClose={() => setPreview(null)}
          onOpenSystem={() => void openInSystem(preview.path, false)}
        />
      ) : (
        <div
          className="ctx__scroll"
          role="tabpanel"
          id={`ctx-panel-${tab}`}
          aria-labelledby={`ctx-tab-${tab}`}
          tabIndex={0}
        >
          {tab === 'changes' && (
            <div className="ctx__panel" data-panel="changes">
              <ChangeReview items={state.items} />
            </div>
          )}
          {tab === 'files' && (
            <div className="ctx-pane ctx-pane--gap">
              {/* 原型 L211-215：工作目录事实（文件夹 / 分支 / 状态） */}
              <div>
                <div className="csec__t">
                  <Icon name="folder" /> 工作目录
                </div>
                <div className="kv">
                  <span>文件夹</span>
                  <span className="mono">{state.cwd || '—'}</span>
                </div>
                {gitStatus ? (
                  <>
                    <div className="kv">
                      <span>分支</span>
                      <span className="mono">{gitStatus.branch}</span>
                    </div>
                    <div className="kv">
                      <span>状态</span>
                      <span>
                        {gitStatus.modified + gitStatus.added + gitStatus.deleted > 0 ? (
                          <span className="pill pill--warn pill--compact">
                            {gitStatus.modified + gitStatus.added + gitStatus.deleted} 项变更
                          </span>
                        ) : (
                          <span className="pill pill--success pill--compact">干净</span>
                        )}
                      </span>
                    </div>
                  </>
                ) : null}
              </div>

              {/* 原型 L216-218：全部文件（真实 search_workspace_files；空查询=最浅 30 条） */}
              <div>
                <div className="csec__t">
                  <Icon name="folderopen" /> 全部文件
                  {allFiles && !allFilesError ? (
                    <span className="cnt">{allFiles.length}</span>
                  ) : null}
                </div>
                {!state.cwd ? (
                  <div style={hintStyle}>未设置工作目录</div>
                ) : allFilesError ? (
                  <div style={hintStyle}>读取失败：{allFilesError}</div>
                ) : fileRows.length ? (
                  <div>
                    {fileRows.map((row) => (
                      <button
                        type="button"
                        className="filerow"
                        key={row.path}
                        title={`预览 ${row.path}`}
                        onClick={() => void previewFile(row.path, row.path, true)}
                      >
                        <span className={`st ${row.badge ? row.badge.toLowerCase() : 'none'}`}>
                          {row.badge ? row.badge.toUpperCase() : <Icon name="dot" />}
                        </span>
                        <span className="nm">
                          {row.dir ? <span className="dir">{row.dir}</span> : null}
                          {row.base}
                        </span>
                        <span className="go">
                          <Icon name="right" />
                        </span>
                      </button>
                    ))}
                    {(allFiles?.length ?? 0) >= 30 ? (
                      <div style={hintStyle}>
                        仅显示最浅 30 条路径；更多文件可用 @ 在输入框精确引用。
                      </div>
                    ) : null}
                  </div>
                ) : (
                  <div style={hintStyle}>{allFiles ? '暂无文件' : '读取中…'}</div>
                )}
              </div>
            </div>
          )}

          {/* S3：右栏不再有「上下文」tab —— 上下文/计费/会话上下文管理只从 Composer 圆环 popover 进入。 */}

          {tab === 'log' && (
            <div style={panelStyle}>
              <div className="csec__t">
                <Icon name="clock" /> 活动日志{' '}
                <span className="faint" style={{ marginLeft: 'auto' }}>
                  {data.tools.length} 个工具
                </span>
              </div>
              {activityGroups.map((group) => (
                <div key={group.id}>
                  <div className="lgt">{group.label}</div>
                  {group.items.map((item) => (
                    <button
                      className={`lgrow${item.kind === 'tool' && item.status === 'error' ? ' is-err' : ''}`}
                      key={item.id}
                      onClick={() => onLocateItem?.(item.id)}
                    >
                      <Icon
                        name={
                          item.kind === 'tool'
                            ? 'zap'
                            : item.kind === 'checkpoint'
                              ? 'checkc'
                              : item.kind === 'approval'
                                ? 'shield'
                                : 'layers'
                        }
                      />
                      <span className="nm">
                        {item.kind === 'tool'
                          ? `${item.name}${toolTargetShort(item) ? ` · ${toolTargetShort(item)}` : ''}`
                          : item.kind === 'checkpoint'
                            ? item.label
                            : item.kind === 'approval'
                              ? item.action
                              : '计划'}
                      </span>
                      <span className="m">{item.kind === 'tool' ? toolLogMeta(item) : ''}</span>
                    </button>
                  ))}
                </div>
              ))}
              {activityGroups.length === 0 ? <div style={hintStyle}>暂无活动</div> : null}
            </div>
          )}

          {tab === 'tools' && (
            <div style={panelStyle}>
              <div>
                <div className="csec__t">
                  <Icon name="shield" /> 工具权限
                </div>
                <div>
                  <div className="toolrow">
                    <span className="toolrow__ic">
                      <Icon name="shield" />
                    </span>
                    <span className="toolrow__meta">
                      <b>当前会话权限</b>
                      <small>在发送框按 Session 切换</small>
                    </span>
                    <span className="pill pill--success">
                      {permissionProfile === 'standard'
                        ? '标准'
                        : permissionProfile === 'auto'
                          ? '自动执行'
                          : '全部放开'}
                    </span>
                  </div>
                  <div className="toolrow">
                    <span className="toolrow__ic">
                      <Icon name="terminal" />
                    </span>
                    <span className="toolrow__meta">
                      <b>Runtime</b>
                      <small>
                        {state.engine === 'claude-code' ? 'Claude Code' : 'Codex'} 原生工具面
                      </small>
                    </span>
                    <span className="pill pill--success">托管</span>
                  </div>
                  {(
                    [
                      ['upright', '网页搜索', state.runtimeCapabilities?.webSearch ?? 'unknown'],
                      ['plug', '网页抓取', state.runtimeCapabilities?.webFetch ?? 'unknown'],
                    ] as const
                  ).map(([icon, name, availability]) => {
                    const pill = capabilityPill(availability);
                    return (
                      <div className="toolrow" key={name}>
                        <span className="toolrow__ic">
                          <Icon name={icon} />
                        </span>
                        <span className="toolrow__meta">
                          <b>{name}</b>
                          <small>来自当前 Runtime 能力握手</small>
                        </span>
                        <span className={pill.className}>{pill.label}</span>
                      </div>
                    );
                  })}
                  <div className="toolrow">
                    <span className="toolrow__ic">
                      <Icon name="check" />
                    </span>
                    <span className="toolrow__meta">
                      <b>Runtime 审批</b>
                      <small>当前代际协商的审批契约</small>
                    </span>
                    <span
                      className={
                        state.runtimeCapabilities ? 'pill pill--success' : 'pill pill--warn'
                      }
                    >
                      {state.runtimeCapabilities?.approvalContractVersion || '未知'}
                    </span>
                  </div>
                </div>
              </div>

              <div>
                <div className="csec__t">
                  <Icon name="plug" /> MCP 服务器
                  <span className="faint ctx-count">已连接 {connectedMcp.length} 个</span>
                </div>
                {mcpLoadError ? (
                  <div className="ctx-load-error" role="alert">
                    <span>MCP 配置读取失败：{mcpLoadError}</span>
                    <button type="button" className="btn btn--sm" onClick={onRetryExtensions}>
                      重试
                    </button>
                  </div>
                ) : mcpServers.length ? (
                  <div>
                    {mcpServers.map((server) => {
                      const disabled = state.disabledMcp.includes(server.name);
                      return (
                        <div className="toolrow" key={server.name}>
                          <span className="toolrow__ic">
                            <Icon name="server" />
                          </span>
                          <span className="toolrow__meta">
                            <b>{server.name}</b>
                            <small>
                              {server.lastError
                                ? '未连接'
                                : server.toolCount != null
                                  ? `${server.toolCount} 个工具`
                                  : '未测试'}
                            </small>
                          </span>
                          <button
                            type="button"
                            role="switch"
                            aria-checked={!disabled}
                            aria-label={`本会话${disabled ? '启用' : '停用'} ${server.name}`}
                            className={'ws-switch' + (disabled ? '' : ' is-on')}
                            title={
                              disabled
                                ? '本会话已停用，下一轮生效'
                                : '本会话启用中；点击停用（下一轮生效）'
                            }
                            onClick={() => void onToggleMcp?.(server.name)}
                          >
                            <span className="ws-switch__knob" />
                          </button>
                        </div>
                      );
                    })}
                    <div style={hintStyle}>开关只影响当前会话，下一轮对话生效。</div>
                  </div>
                ) : (
                  <div style={hintStyle}>还没有配置连接器</div>
                )}
                <button
                  type="button"
                  className="btn btn--sm ctx-fullbtn"
                  onClick={onOpenExtensions}
                >
                  <Icon name="plus" /> 添加 / 管理连接器
                </button>
              </div>

              <div>
                <div className="csec__t">
                  <Icon name="sparkles" /> 技能
                  <span className="faint ctx-count">已启用 {enabledSkills.length} 个</span>
                </div>
                {skillsLoadError ? (
                  <div className="ctx-load-error" role="alert">
                    <span>技能清单读取失败：{skillsLoadError}</span>
                    <button type="button" className="btn btn--sm" onClick={onRetryExtensions}>
                      重试
                    </button>
                  </div>
                ) : enabledSkills.length ? (
                  <div style={listStyle}>
                    {enabledSkills.slice(0, 6).map((skill) => (
                      <div className="filerow" key={skill.id}>
                        <span className="st a">S</span>
                        <span className="nm">{skill.name}</span>
                      </div>
                    ))}
                    {enabledSkills.length > 6 ? (
                      <div style={hintStyle}>… 共 {enabledSkills.length} 个</div>
                    ) : null}
                  </div>
                ) : (
                  <div style={hintStyle}>当前引擎暂无可用技能</div>
                )}
                <button
                  type="button"
                  className="btn btn--sm ctx-fullbtn"
                  onClick={onOpenExtensions}
                >
                  管理技能
                </button>
              </div>
            </div>
          )}
          {DYN_CONTENT_ORDER.map((id) => {
            if (tab !== id) return null;
            if (id === 'tasks') {
              return (
                <div className="ctx__panel" data-panel={id} key={id} style={{ padding: 16 }}>
                  <TasksPanel items={state.items} onStopTask={onStopTask} onLocate={onLocateItem} />
                </div>
              );
            }
            if (id === 'plan') {
              return <PlanPanel key={id} items={state.items} onLocateItem={onLocateItem} />;
            }
            if (id === 'term') {
              return <TermPanel key={id} items={state.items} onLocateItem={onLocateItem} />;
            }
            // preview：当前没有真实 dev server 预览能力，不保留占位 tab。
            return null;
          })}
        </div>
      )}
    </aside>
  );
}
