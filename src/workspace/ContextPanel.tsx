import { useCallback, useEffect, useMemo, useState } from 'react';
import { toolTarget } from './toolTarget';
import { Icon } from '../shell/icons';
import type { SessionState } from '../engine/useSession';
import { contextPanelData } from './contextPanelViewModel';
import type { McpServer, Skill } from '../extensions/extensionsApi';
import type { PermissionProfile, RuntimeCapabilityAvailability } from '@helm/protocol';
import { activityLogGroups } from './activityLog';
import { getGitStatus, getGitStaged, type GitStatus, type StagedFile } from '../engine/transport';
import { openPathInSystem, readFilePreview, type FilePreview } from '../engine/transport';
import { open } from '@tauri-apps/plugin-dialog';
import {
  addSessionContext,
  listSessionContexts,
  removeSessionContext,
  type SessionContextRecord,
} from '../sessions/api';

type Tab = 'files' | 'log' | 'context' | 'tools';

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
      <div className="fpv">
        <div className="fpv__head">
          <span className="fpv__title" title={preview.path}>
            {preview.label}
          </span>
          <span style={{ display: 'flex', gap: 6, alignItems: 'center' }}>
            <button
              type="button"
              className="btn-icon"
              onClick={onOpenSystem}
              title="用系统默认程序打开"
            >
              ↗
            </button>
            <button type="button" className="btn-icon" onClick={onClose} title="关闭预览">
              ✕
            </button>
          </span>
        </div>
        <div className="fv__err">预览失败：{preview.error}</div>
      </div>
    );
  }
  const data = preview.data;
  return (
    <div className="fpv">
      <div className="fpv__head">
        <span className="fpv__title" title={preview.path}>
          {preview.label}
        </span>
        <span style={{ display: 'flex', gap: 6, alignItems: 'center' }}>
          {data.kind === 'binary' ? (
            <button type="button" className="btn btn--sm" onClick={onOpenSystem}>
              用系统默认程序打开
            </button>
          ) : null}
          <button type="button" className="btn-icon" onClick={onClose} title="关闭预览">
            ✕
          </button>
        </span>
      </div>
      <div className="fpv__body">
        {busy ? (
          <div className="fpv__hint">读取中…</div>
        ) : data.kind === 'image' ? (
          data.content && !data.truncated ? (
            <img
              className="fpv__img"
              src={`data:${data.mime ?? 'image/png'};base64,${data.content}`}
              alt={preview.label}
            />
          ) : (
            <div className="fpv__hint">
              {data.truncated
                ? `图片过大（${(data.size / 1024 / 1024).toFixed(1)} MB），无法内嵌预览。`
                : '无法内嵌预览。'}
              <button type="button" className="btn btn--sm" onClick={onOpenSystem}>
                用系统默认程序打开
              </button>
            </div>
          )
        ) : data.kind === 'binary' ? (
          <div className="fpv__hint">
            二进制文件（{(data.size / 1024).toFixed(1)} KB），无法内嵌预览。
            <button type="button" className="btn btn--sm" onClick={onOpenSystem}>
              用系统默认程序打开
            </button>
          </div>
        ) : (
          <pre className="fpv__text">{data.content}</pre>
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

/** 从路径取最后一个片段作为展示名。 */
function attachmentLabel(path: string): string {
  return path.split(/[\\/]/).filter(Boolean).pop() ?? path;
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
  onLocateChange,
  openContextRequest = 0,
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
  /** 批次 I：跳转到变更位置（文件路径 + 行号 + 工具 ID） */
  onLocateChange?: (path: string, lineNumber: number, toolId: string) => void;
  openContextRequest?: number;
}) {
  const [tab, setTab] = useState<Tab>('context');
  // 批次 I：变更导航器展开状态
  const [expandedFiles, setExpandedFiles] = useState<Set<string>>(new Set());
  const toggleExpand = (path: string) => {
    setExpandedFiles((prev) => {
      const next = new Set(prev);
      if (next.has(path)) {
        next.delete(path);
      } else {
        next.add(path);
      }
      return next;
    });
  };
  // Git 状态（批次 E）
  const [gitStatus, setGitStatus] = useState<GitStatus | undefined>();
  const [stagedFiles, setStagedFiles] = useState<StagedFile[] | undefined>();
  const [sessionContexts, setSessionContexts] = useState<SessionContextRecord[]>([]);
  const [contextError, setContextError] = useState<string | null>(null);
  const [contextBusy, setContextBusy] = useState(false);
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

  const loadSessionContexts = useCallback(async () => {
    if (!state.historyId) {
      setSessionContexts([]);
      return;
    }
    try {
      setSessionContexts(await listSessionContexts(state.historyId));
      setContextError(null);
    } catch (error) {
      setContextError(String(error));
    }
  }, [state.historyId]);

  useEffect(() => {
    let cancelled = false;
    const load = async () => {
      if (!state.historyId) {
        setSessionContexts([]);
        return;
      }
      try {
        const contexts = await listSessionContexts(state.historyId);
        if (cancelled) return;
        setSessionContexts(contexts);
        setContextError(null);
      } catch (error) {
        if (cancelled) return;
        setContextError(String(error));
      }
    };
    void load();
    return () => {
      cancelled = true;
    };
  }, [state.historyId, state.status]);

  const pickSessionContext = async (directory: boolean) => {
    if (!state.historyId || state.status === 'working') return;
    setContextBusy(true);
    setContextError(null);
    try {
      const selected = await open({ multiple: true, directory });
      const paths = typeof selected === 'string' ? [selected] : (selected ?? []);
      for (const path of paths) await addSessionContext(state.historyId, path);
      await loadSessionContexts();
    } catch (error) {
      setContextError(String(error));
    } finally {
      setContextBusy(false);
    }
  };

  const removeContext = async (contextId: string) => {
    if (!state.historyId || state.status === 'working') return;
    setContextBusy(true);
    setContextError(null);
    try {
      await removeSessionContext(state.historyId, contextId);
      await loadSessionContexts();
    } catch (error) {
      setContextError(String(error));
    } finally {
      setContextBusy(false);
    }
  };

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
  const activityGroups = useMemo(() => activityLogGroups(state.items), [state.items]);
  const fmt = (n: number) => n.toLocaleString('zh-CN');
  const started = state.startedAt ? new Date(state.startedAt) : null;
  const enabledSkills = skills.filter((skill) => skill.enabled);
  const connectedMcp = mcpServers.filter(
    (server) => (server.toolCount ?? 0) > 0 && !server.lastError,
  );
  useEffect(() => {
    if (openContextRequest > 0) setTab('context');
  }, [openContextRequest]);

  return (
    <aside className="ctx">
      <div className="ctx__tabs tabbar">
        <button
          className={'tab' + (tab === 'log' ? ' is-active' : '')}
          onClick={() => setTab('log')}
        >
          活动
        </button>
        <button
          className={'tab' + (tab === 'files' ? ' is-active' : '')}
          onClick={() => setTab('files')}
        >
          文件
        </button>
        <button
          className={'tab' + (tab === 'context' ? ' is-active' : '')}
          onClick={() => setTab('context')}
        >
          上下文
        </button>
        <button
          className={'tab' + (tab === 'tools' ? ' is-active' : '')}
          onClick={() => setTab('tools')}
        >
          工具
        </button>
      </div>
      <div className="ctx__scroll">
        {tab === 'files' && (
          <div style={panelStyle}>
            <div>
              <div className="csec__t">
                <Icon name="folder" /> 工作目录
              </div>
              <div className="kv">
                <span>文件夹</span>
                <span className="mono">{state.cwd || '—'}</span>
              </div>
            </div>

            {/* Git 状态（批次 E） */}
            {gitStatus && (
              <div>
                <div className="csec__t">
                  <Icon name="gitbranch" /> Git 状态
                </div>
                <div className="kv">
                  <span>当前分支</span>
                  <span className="mono">{gitStatus.branch}</span>
                </div>
                <div className="kv">
                  <span>工作区变更</span>
                  <span>
                    {gitStatus.modified > 0 && (
                      <span className="pill pill--warn" style={{ marginRight: 4 }}>
                        修改 {gitStatus.modified}
                      </span>
                    )}
                    {gitStatus.added > 0 && (
                      <span className="pill pill--success" style={{ marginRight: 4 }}>
                        新增 {gitStatus.added}
                      </span>
                    )}
                    {gitStatus.deleted > 0 && (
                      <span className="pill pill--danger" style={{ marginRight: 4 }}>
                        删除 {gitStatus.deleted}
                      </span>
                    )}
                    {gitStatus.modified === 0 &&
                      gitStatus.added === 0 &&
                      gitStatus.deleted === 0 && <span className="faint">干净</span>}
                  </span>
                </div>
              </div>
            )}

            {/* 暂存区文件列表（批次 E） */}
            {stagedFiles && stagedFiles.length > 0 && (
              <div>
                <div className="csec__t">
                  <Icon name="clip" /> 暂存区
                </div>
                <div style={listStyle}>
                  {stagedFiles.map((file) => (
                    <button
                      type="button"
                      className="filerow fpv-row"
                      key={file.path}
                      style={{
                        display: 'flex',
                        alignItems: 'center',
                        background: 'none',
                        border: 'none',
                        padding: '4px 0',
                        textAlign: 'left',
                        cursor: 'pointer',
                      }}
                      onClick={() => void previewFile(file.path, file.path, true)}
                      title={`预览 ${file.path}`}
                    >
                      <span className="st a">{file.status.charAt(0)}</span>
                      <span className="nm">{file.path}</span>
                    </button>
                  ))}
                </div>
              </div>
            )}

            <div>
              <div className="csec__t">
                <Icon name="gitbranch" /> 文件变更
              </div>
              {data.changeFiles.length ? (
                <div style={listStyle}>
                  {data.changeFiles.map((file) => (
                    <div key={file.path}>
                      <div
                        className="filerow"
                        style={{
                          display: 'flex',
                          alignItems: 'center',
                          width: '100%',
                          padding: '4px 0',
                        }}
                        title={`${file.path}（${file.added} 新增，${file.removed} 删除）`}
                      >
                        <button
                          type="button"
                          style={{
                            display: 'flex',
                            alignItems: 'center',
                            background: 'none',
                            border: 'none',
                            padding: 0,
                            cursor: 'pointer',
                          }}
                          onClick={() => toggleExpand(file.path)}
                          aria-label="展开/折叠变更"
                        >
                          <span className="st m">{expandedFiles.has(file.path) ? '▼' : '▶'}</span>
                        </button>
                        <button
                          type="button"
                          className="nm"
                          style={{
                            flex: 1,
                            overflow: 'hidden',
                            textOverflow: 'ellipsis',
                            whiteSpace: 'nowrap',
                            background: 'none',
                            border: 'none',
                            textAlign: 'left',
                            cursor: 'pointer',
                            padding: '4px 0',
                          }}
                          onClick={() => void previewFile(file.path, file.path, true)}
                          title={`预览 ${file.path}`}
                        >
                          {file.path}
                        </button>
                        <span className="pm">
                          <span className="a">+{file.added}</span>
                          <span className="d">-{file.removed}</span>
                        </span>
                      </div>
                      {expandedFiles.has(file.path) && (
                        <div
                          style={{
                            marginLeft: 16,
                            fontSize: 12,
                            fontFamily: 'var(--font-mono)',
                            lineHeight: 1.6,
                          }}
                        >
                          {file.lines.slice(0, 50).map((line, index) => (
                            <button
                              type="button"
                              key={`${file.path}-${line.lineNumber}-${index}`}
                              style={{
                                display: 'flex',
                                width: '100%',
                                background: 'none',
                                border: 'none',
                                padding: '1px 0',
                                cursor: 'pointer',
                                textAlign: 'left',
                                color:
                                  line.kind === 'add'
                                    ? 'var(--green)'
                                    : line.kind === 'del'
                                      ? 'var(--red)'
                                      : 'var(--fg-3)',
                              }}
                              onClick={() =>
                                onLocateChange?.(line.path, line.lineNumber, line.toolId)
                              }
                              title={`跳转到 ${line.path}:${line.lineNumber}`}
                            >
                              <span
                                style={{
                                  width: 40,
                                  textAlign: 'right',
                                  marginRight: 8,
                                  opacity: 0.5,
                                }}
                              >
                                {line.lineNumber}
                              </span>
                              <span style={{ width: 16, textAlign: 'center', marginRight: 4 }}>
                                {line.kind === 'add' ? '+' : line.kind === 'del' ? '-' : ' '}
                              </span>
                              <span
                                style={{
                                  flex: 1,
                                  overflow: 'hidden',
                                  textOverflow: 'ellipsis',
                                  whiteSpace: 'nowrap',
                                }}
                              >
                                {line.text}
                              </span>
                            </button>
                          ))}
                          {file.lines.length > 50 && (
                            <div style={{ color: 'var(--fg-4)', padding: '2px 0' }}>
                              … 共 {file.lines.length} 行变更
                            </div>
                          )}
                        </div>
                      )}
                    </div>
                  ))}
                </div>
              ) : (
                <div style={hintStyle}>暂无文件变更</div>
              )}
            </div>
          </div>
        )}

        {tab === 'context' && (
          <div style={panelStyle}>
            <div>
              <div className="csec__t">
                <Icon name="layers" /> 上下文占用
              </div>
              <div className="usage">
                <div className="usage__row">
                  <span className="usage__big">
                    {data.contextUsage.tokens == null
                      ? '暂无数据'
                      : `${fmt(data.contextUsage.tokens)}${data.contextUsage.maxTokens ? ` / ${fmt(data.contextUsage.maxTokens)}` : ''}`}
                  </span>
                  {data.contextUsage.ratio != null ? (
                    <span
                      className={`pill ${data.contextUsage.level === 'danger' ? 'pill--danger' : data.contextUsage.level === 'warning' ? 'pill--warn' : 'pill--accent'}`}
                    >
                      {Math.round(data.contextUsage.ratio * 100)}%
                    </span>
                  ) : null}
                </div>
                {data.contextUsage.ratio != null ? (
                  <div className="meter">
                    <i style={{ width: `${Math.round(data.contextUsage.ratio * 100)}%` }} />
                  </div>
                ) : null}
                <div className="usage__note">
                  {data.contextUsage.tokens == null
                    ? '当前 Engine 路径未提供逐调用用量；Helm 不使用累计值估算。'
                    : '口径：最近一次模型调用的真实输入规模，含缓存命中；非累计、非估算。'}
                </div>
                {data.contextUsage.level === 'warning' || data.contextUsage.level === 'danger' ? (
                  <div
                    className={`usage__hint${data.contextUsage.level === 'danger' ? ' is-danger' : ''}`}
                  >
                    <Icon name="alert" />
                    <span>
                      {data.contextUsage.level === 'danger'
                        ? '已接近上下文上限，建议新开会话继续。'
                        : '上下文占用较高，建议尽快收束当前任务。'}
                    </span>
                  </div>
                ) : null}
              </div>
            </div>
            <div>
              <div className="csec__t">
                <Icon name="dollar" /> 计费 token（累计）
              </div>
              <div className="usage">
                <div className="billrow">
                  <span>未缓存输入</span>
                  <span className="mono">{fmt(data.billing.freshInput)}</span>
                </div>
                <div className="billrow">
                  <span>缓存写入</span>
                  <span className="mono">{fmt(data.billing.cacheWrite)}</span>
                </div>
                <div className="billrow">
                  <span>缓存读取</span>
                  <span className="mono">{fmt(data.billing.cacheRead)}</span>
                </div>
                <div className="billrow">
                  <span>输出</span>
                  <span className="mono">{fmt(data.billing.output)}</span>
                </div>
                <div className="billrow">
                  <span>缓存读取占比</span>
                  <span className="mono">
                    {data.billing.cacheReadShare == null
                      ? '暂无'
                      : `${Math.round(data.billing.cacheReadShare * 100)}%`}
                  </span>
                </div>
                <div className="usage__note">
                  计费 token 跨轮累计；缓存读取的实际价格按当前定价目录计算。
                </div>
              </div>
            </div>
            <div>
              <div className="csec__t">
                <Icon name="layers" /> 会话上下文
                <span className="ws-context-actions">
                  <button
                    type="button"
                    className="btn-icon"
                    title="添加文件"
                    aria-label="添加文件到会话上下文"
                    disabled={!state.historyId || state.status === 'working' || contextBusy}
                    onClick={() => void pickSessionContext(false)}
                  >
                    <Icon name="file" />
                  </button>
                  <button
                    type="button"
                    className="btn-icon"
                    title="添加目录"
                    aria-label="添加目录到会话上下文"
                    disabled={!state.historyId || state.status === 'working' || contextBusy}
                    onClick={() => void pickSessionContext(true)}
                  >
                    <Icon name="folder" />
                  </button>
                </span>
              </div>
              {sessionContexts.length ? (
                <div style={listStyle}>
                  {sessionContexts.map((context) => (
                    <div
                      className="filerow ws-context-row"
                      key={context.id}
                      title={context.canonicalPath}
                    >
                      <button
                        type="button"
                        style={{
                          display: 'flex',
                          alignItems: 'center',
                          gap: 6,
                          flex: 1,
                          overflow: 'hidden',
                          background: 'none',
                          border: 'none',
                          padding: 0,
                          textAlign: 'left',
                          cursor: context.kind === 'directory' ? 'default' : 'pointer',
                        }}
                        onClick={() => {
                          if (context.kind === 'directory') {
                            void openInSystem(context.canonicalPath, false);
                          } else {
                            void previewFile(context.canonicalPath, context.displayName, false);
                          }
                        }}
                        title={
                          context.kind === 'directory'
                            ? `在资源管理器中打开 ${context.canonicalPath}`
                            : `预览 ${context.canonicalPath}`
                        }
                      >
                        <span className={`st ${context.status === 'ready' ? 'a' : 'd'}`}>
                          {context.kind === 'directory' ? 'D' : 'F'}
                        </span>
                        <span className="nm mono">
                          {context.displayName}
                          {context.status !== 'ready'
                            ? ` · ${context.statusDetail ?? '不可用'}`
                            : ''}
                        </span>
                      </button>
                      <button
                        type="button"
                        className="btn-icon"
                        title="从后续轮次移除"
                        aria-label={`移除会话上下文 ${context.displayName}`}
                        disabled={state.status === 'working' || contextBusy}
                        onClick={() => void removeContext(context.id)}
                      >
                        <Icon name="x" />
                      </button>
                    </div>
                  ))}
                </div>
              ) : (
                <div style={hintStyle}>暂无会话上下文</div>
              )}
              {contextError ? <div className="usage__hint is-danger">{contextError}</div> : null}
              <div className="usage__note">
                增删只影响后续轮次；运行中不可修改。当前版本仅支持工作目录内路径。
              </div>
            </div>
            <div>
              <div className="csec__t">
                <Icon name="clip" /> 历史附件
              </div>
              {data.historicalAttachments.length ? (
                <div style={listStyle}>
                  {data.historicalAttachments.map((path) => (
                    <button
                      type="button"
                      className="filerow"
                      key={path}
                      title="已发送附件，只读历史；点击预览"
                      style={{
                        display: 'flex',
                        alignItems: 'center',
                        background: 'none',
                        border: 'none',
                        padding: '4px 0',
                        textAlign: 'left',
                        cursor: 'pointer',
                      }}
                      onClick={() => void previewFile(path, attachmentLabel(path), false)}
                    >
                      <span className="st m">A</span>
                      <span className="nm mono">{path}</span>
                    </button>
                  ))}
                </div>
              ) : (
                <div style={hintStyle}>暂无历史附件</div>
              )}
              <div className="usage__note">已发送附件属于消息历史，不能在这里移除。</div>
            </div>
            <div>
              <div className="csec__t">
                <Icon name="dollar" /> 本次会话
              </div>
              <div className="kv">
                <span>消息数</span>
                <span className="mono">{data.messageCount}</span>
              </div>
              <div className="kv">
                <span>花费</span>
                <span className="mono">
                  {state.cost.costUsd > 0 ? `$${state.cost.costUsd.toFixed(4)}` : '暂无价格数据'}
                </span>
              </div>
              {started ? (
                <div className="kv">
                  <span>开始时间</span>
                  <span className="mono">
                    {started.toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit' })}
                  </span>
                </div>
              ) : null}
            </div>
          </div>
        )}

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
                <div style={hintStyle}>还没有配置 MCP 服务器</div>
              )}
              <button type="button" className="btn btn--sm ctx-fullbtn" onClick={onOpenExtensions}>
                <Icon name="plus" /> 添加 / 管理 MCP 服务器
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
      </div>
      {preview ? (
        <FilePreviewPanel
          preview={preview}
          busy={previewBusy}
          onClose={() => setPreview(null)}
          onOpenSystem={() => void openInSystem(preview.path, false)}
        />
      ) : null}
    </aside>
  );
}
