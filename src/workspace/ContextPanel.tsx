import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
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
import { openPathInSystem, readFilePreview, readFilePreviewBytes } from '../engine/transport';
import { Markdown } from '../lib/markdown';
import mammoth from 'mammoth';
import * as XLSX from 'xlsx';
import { searchWorkspaceFiles } from './workspaceApi';
import {
  CONTEXT_PANEL_DEFAULT_TAB,
  CONTEXT_PANEL_FIXED_TABS,
  CONTEXT_PANEL_FIXED_TAB_LABELS,
  DYN_TAB_LABELS,
  contextPanelData,
  closeDynTab as contextPanelDynTabsClose,
  fileTabId,
  fileTabLabel,
  fileTabPath,
  isContextPanelFixedTab,
  isFilePaneTabId,
  openDynTab as contextPanelDynTabsOpen,
  workspaceFileRows,
  type ArtifactPaneTab,
  type ContextPanelFixedTab,
  type CtxDynTabId,
} from './contextPanelViewModel';
import { changeReviewFiles } from './changeReviewViewModel';

type Tab = ContextPanelFixedTab | CtxDynTabId;

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

/** 变更-33：文件/附件预览。切片「文件动态 tab」起：每个文件 tab 自加载、自渲染。 */
type FileTabData =
  | { type: 'text'; content: string }
  | { type: 'markdown'; content: string }
  | {
      type: 'image';
      mime?: string | null;
      content?: string | null;
      truncated: boolean;
      size: number;
    }
  | { type: 'sheet'; sheets: { name: string; html: string }[] }
  | { type: 'docx'; html: string }
  | { type: 'binary'; size: number }
  | { type: 'error'; message: string };

/** base64 → Uint8Array（xlsx/docx 解析库需要字节 buffer）。 */
function base64ToUint8Array(base64: string): Uint8Array {
  const raw = atob(base64);
  const buffer = new Uint8Array(raw.length);
  for (let i = 0; i < raw.length; i += 1) buffer[i] = raw.charCodeAt(i);
  return buffer;
}

/** 用系统默认程序打开（失败静默：文件内容已在 tab 内可读，不打断预览）。 */
function openInSystemAbsolute(path: string): void {
  void openPathInSystem(path).catch(() => {});
}

function workbookSheets(path: string, source: string | Uint8Array): FileTabData {
  const isCsvText = path.toLowerCase().endsWith('.csv');
  const workbook = XLSX.read(source, isCsvText ? { type: 'string' } : { type: 'array' });
  return {
    type: 'sheet',
    sheets: workbook.SheetNames.map((name) => ({
      name,
      html: XLSX.utils.sheet_to_html(workbook.Sheets[name]),
    })),
  };
}

function FileTabView({
  path,
  cwd,
}: {
  /** 工作区相对路径（与「全部文件」行一致） */
  path: string;
  cwd?: string;
}) {
  const absolute = joinPath(cwd ?? '', path);
  const [data, setData] = useState<FileTabData | null>(null);
  const [sheetIndex, setSheetIndex] = useState(0);

  useEffect(() => {
    let cancelled = false;
    setData(null);
    setSheetIndex(0);
    (async () => {
      try {
        const lower = path.toLowerCase();
        const preview = await readFilePreview(absolute);
        if (cancelled) return;
        if (preview.kind === 'image') {
          setData({
            type: 'image',
            mime: preview.mime,
            content: preview.content,
            truncated: preview.truncated,
            size: preview.size,
          });
          return;
        }
        if (preview.kind === 'text') {
          const content = preview.content ?? '';
          if (lower.endsWith('.md') || lower.endsWith('.markdown')) {
            setData({ type: 'markdown', content });
          } else if (lower.endsWith('.csv')) {
            setData(workbookSheets(path, content));
          } else {
            setData({ type: 'text', content });
          }
          return;
        }
        // 二进制：xlsx/xls 走 SheetJS、docx 走 mammoth 前端解析；其余维持系统打开引导
        if (lower.endsWith('.xlsx') || lower.endsWith('.xls')) {
          const bytes = await readFilePreviewBytes(absolute);
          if (cancelled) return;
          setData(workbookSheets(path, base64ToUint8Array(bytes.content)));
          return;
        }
        if (lower.endsWith('.docx')) {
          const bytes = await readFilePreviewBytes(absolute);
          if (cancelled) return;
          const u8 = base64ToUint8Array(bytes.content);
          // mammoth 类型要求精确 ArrayBuffer（排除 SharedArrayBuffer）
          const arrayBuffer = u8.buffer.slice(
            u8.byteOffset,
            u8.byteOffset + u8.byteLength,
          ) as ArrayBuffer;
          const result = await mammoth.convertToHtml({ arrayBuffer });
          if (cancelled) return;
          setData({ type: 'docx', html: result.value });
          return;
        }
        setData({ type: 'binary', size: preview.size });
      } catch (error) {
        if (!cancelled) setData({ type: 'error', message: String(error) });
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [absolute, path]);

  const data_ = data;
  // 正文直出（2026-09-10 用户裁决）：预览条整条去掉，tab 本身已带文件名（悬停看全路径）。
  // 仅表格/文档类在右上角留一个悬浮「系统打开」入口，功能不丢。
  const showOpenSystem = data_?.type === 'sheet' || data_?.type === 'docx';
  return (
    <div className="filepreview">
      {showOpenSystem ? (
        <button
          type="button"
          className="btn btn--sm filepreview__open"
          title="用系统默认程序打开"
          aria-label="用系统默认程序打开"
          onClick={() => openInSystemAbsolute(absolute)}
        >
          <Icon name="upright" />
        </button>
      ) : null}
      <div className="filepreview__body">
        {!data_ ? (
          <div className="filepreview__hint">读取中…</div>
        ) : data_.type === 'error' ? (
          <div className="filepreview__err">预览失败：{data_.message}</div>
        ) : data_.type === 'image' ? (
          data_.content && !data_.truncated ? (
            <img
              className="filepreview__img"
              src={`data:${data_.mime ?? 'image/png'};base64,${data_.content}`}
              alt={fileTabLabel(path)}
            />
          ) : (
            <div className="filepreview__hint">
              {data_.truncated
                ? `图片过大（${(data_.size / 1024 / 1024).toFixed(1)} MB），无法内嵌预览。`
                : '无法内嵌预览。'}
              <button
                type="button"
                className="btn btn--sm"
                onClick={() => openInSystemAbsolute(absolute)}
              >
                用系统默认程序打开
              </button>
            </div>
          )
        ) : data_.type === 'markdown' ? (
          <div className="filepreview__md prose">
            <Markdown text={data_.content} />
          </div>
        ) : data_.type === 'sheet' ? (
          <div className="filepreview__sheet">
            {data_.sheets.length > 1 ? (
              <select
                className="filepreview__sheet-select"
                aria-label="选择工作表"
                value={sheetIndex}
                onChange={(event) => setSheetIndex(Number(event.target.value) || 0)}
              >
                {data_.sheets.map((sheet, index) => (
                  <option key={sheet.name} value={index}>
                    {sheet.name}
                  </option>
                ))}
              </select>
            ) : null}
            <div
              className="filepreview__sheet-table"
              // SheetJS sheet_to_html 输出：单元格文本已转义，仅生成 <table> 结构
              dangerouslySetInnerHTML={{
                __html: data_.sheets[sheetIndex]?.html ?? '',
              }}
            />
          </div>
        ) : data_.type === 'docx' ? (
          <div
            className="md filepreview__docx"
            // mammoth 输出：正文文本已转义，仅生成 p/h/table/ul 等结构标签
            dangerouslySetInnerHTML={{ __html: data_.html }}
          />
        ) : data_.type === 'binary' ? (
          <div className="filepreview__hint">
            二进制文件（{(data_.size / 1024).toFixed(1)} KB），无法内嵌预览。
            <button
              type="button"
              className="btn btn--sm"
              onClick={() => openInSystemAbsolute(absolute)}
            >
              用系统默认程序打开
            </button>
          </div>
        ) : (
          <pre className="filepreview__code">{data_.content}</pre>
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
  // 变更-34 · A4：已打开的动态 tab（交付物类 + 文件 tab，保持打开顺序）。
  const [dynTabs, setDynTabs] = useState<CtxDynTabId[]>([]);
  const applyDynTabs = useCallback((next: { open: CtxDynTabId[]; active: CtxDynTabId | null }) => {
    setDynTabs(next.open);
    // 关闭动态 tab 后回退到默认常驻 tab「修改记录」。
    setTab(next.active ?? CONTEXT_PANEL_DEFAULT_TAB);
  }, []);
  const activeDyn = isContextPanelFixedTab(tab) ? null : tab;
  const openDynTab = useCallback(
    (paneId: CtxDynTabId) => {
      applyDynTabs(contextPanelDynTabsOpen({ open: dynTabs, active: activeDyn }, paneId));
    },
    [applyDynTabs, dynTabs, activeDyn],
  );
  /** 对齐原型 ws.js openFilePreview：点文件行按路径开 tab，同一路径复用同一 tab。 */
  const openFileTab = useCallback(
    (path: string) => {
      openDynTab(fileTabId(path));
    },
    [openDynTab],
  );
  const closeDynTab = useCallback(
    (paneId: CtxDynTabId) => {
      applyDynTabs(contextPanelDynTabsClose({ open: dynTabs, active: activeDyn }, paneId));
    },
    [applyDynTabs, dynTabs, activeDyn],
  );
  /** 动态 tab 标题：交付物类用固定文案，文件 tab 用文件名（完整路径放 title）。 */
  const dynTabLabel = useCallback(
    (id: CtxDynTabId) => (isFilePaneTabId(id) ? fileTabLabel(id) : DYN_TAB_LABELS[id]),
    [],
  );
  // openDynTab 依赖 dynTabs/activeDyn，每次 tab 变化都会重建；副作用只应响应
  // openPaneRequest 本身（点「查看全部文件」等入口），否则文件 tab 激活后会被
  // 重放的旧 request 拉回常驻 tab（2026-09-10 用户实测「点文件仍显示目录」根因）。
  const openDynTabRef = useRef(openDynTab);
  openDynTabRef.current = openDynTab;
  useEffect(() => {
    if (!openPaneRequest) return;
    if (isContextPanelFixedTab(openPaneRequest.tab)) setTab(openPaneRequest.tab);
    else openDynTabRef.current(openPaneRequest.tab);
    // 依赖仅 openPaneRequest：request 序号变更才重放一次（openDynTab 走 ref 取最新）。
  }, [openPaneRequest]);
  // Git 状态（批次 E）
  const [gitStatus, setGitStatus] = useState<GitStatus | undefined>();
  const [stagedFiles, setStagedFiles] = useState<StagedFile[] | undefined>();
  // S3「全部文件」：真实 search_workspace_files 结果（空查询返回最浅 30 条）
  const [allFiles, setAllFiles] = useState<string[] | null>(null);
  const [allFilesError, setAllFilesError] = useState<string | null>(null);

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

  // 变更-34 · A5：交付物区最大化/收起。对齐原型 ws.js setDockMax（workspace.css L628-630）：
  // 最大化 = body.ws-ctx-max 让 .ctx fixed 覆盖线程区（保留标题栏与主侧栏），
  // 不再用 --ctx-w 撑宽网格 —— vw 撑宽会挤爆三列网格导致左侧布局错乱、还原按钮被裁掉。
  const [paneMaximized, setPaneMaximized] = useState(false);
  useEffect(() => {
    // 卸载兜底：右栏关闭/会话切换时不留残留类
    return () => document.body.classList.remove('ws-ctx-max');
  }, []);
  const toggleMaximize = useCallback(() => {
    const next = !paneMaximized;
    document.body.classList.toggle('ws-ctx-max', next);
    setPaneMaximized(next);
  }, [paneMaximized]);
  const collapsePane = useCallback(() => {
    if (paneMaximized) toggleMaximize();
    onCollapse?.();
  }, [paneMaximized, toggleMaximize, onCollapse]);

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
              const label = dynTabLabel(id);
              return (
                <button
                  key={id}
                  role="tab"
                  id={`ctx-tab-${id}`}
                  aria-selected={tab === id}
                  aria-controls={`ctx-panel-${id}`}
                  className={'tab tab--dyn' + (tab === id ? ' is-active' : '')}
                  onClick={() => setTab(id)}
                  title={isFilePaneTabId(id) ? fileTabPath(id) : undefined}
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
            <Icon name={paneMaximized ? 'minimize' : 'maximize'} />
          </button>
          <button
            type="button"
            className="ctx-tool is-close"
            title="关闭右侧工作区"
            aria-label="关闭右侧工作区"
            onClick={collapsePane}
          >
            <Icon name="x" />
          </button>
        </span>
      </div>
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
                {allFiles && !allFilesError ? <span className="cnt">{allFiles.length}</span> : null}
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
                      onClick={() => openFileTab(row.path)}
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
                          : item.kind === 'approval'
                            ? 'shield'
                            : 'layers'
                      }
                    />
                    <span className="nm">
                      {item.kind === 'tool'
                        ? `${item.name}${toolTargetShort(item) ? ` · ${toolTargetShort(item)}` : ''}`
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
                    className={state.runtimeCapabilities ? 'pill pill--success' : 'pill pill--warn'}
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
              <button type="button" className="btn btn--sm ctx-fullbtn" onClick={onOpenExtensions}>
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
              <button type="button" className="btn btn--sm ctx-fullbtn" onClick={onOpenExtensions}>
                管理技能
              </button>
            </div>
          </div>
        )}
        {/* 文件动态 tab（对齐原型 openFilePreview）：预览内容占满面板区 */}
        {isFilePaneTabId(tab) ? (
          <div className="ctx__panel" data-panel="file">
            <FileTabView key={tab} path={fileTabPath(tab)} cwd={state.cwd} />
          </div>
        ) : null}
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
    </aside>
  );
}
