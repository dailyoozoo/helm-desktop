import { useMemo, useState, type CSSProperties } from 'react';
import { Icon } from '../shell/icons';
import type { SessionState } from '../engine/useSession';
import { contextPanelData } from './contextPanelViewModel';
import type { McpServer, Skill } from '../extensions/extensionsApi';
import type { AppSettings } from '../settings/types';

type Tab = 'files' | 'context' | 'tools';

const panelStyle: CSSProperties = { display: 'flex', flexDirection: 'column', gap: 20 };
const hintStyle: CSSProperties = { color: 'var(--fg-4)', fontSize: 12.5, lineHeight: 1.6 };
const listStyle: CSSProperties = { display: 'flex', flexDirection: 'column', gap: 2 };

function statusText(status: 'pending' | 'success' | 'error') {
  if (status === 'pending') return '运行中';
  if (status === 'success') return '成功';
  return '失败';
}

/** 权限值 → 药丸（原型工具 tab：自动/询问/关闭） */
function permissionPill(value: string) {
  if (value === 'allow') return { className: 'pill pill--success', label: '自动' };
  if (value === 'deny') return { className: 'pill', label: '关闭' };
  return { className: 'pill pill--warn', label: '询问' };
}

export function ContextPanel({
  state,
  settings,
  mcpServers = [],
  skills = [],
  mcpLoadError,
  skillsLoadError,
  onRetryExtensions,
  onToggleMcp,
  onOpenExtensions,
  onOpenSettings,
}: {
  state: SessionState;
  settings?: AppSettings;
  mcpServers?: McpServer[];
  skills?: Skill[];
  mcpLoadError?: string | null;
  skillsLoadError?: string | null;
  onRetryExtensions?: () => void;
  onToggleMcp?: (name: string) => void;
  onOpenExtensions?: () => void;
  onOpenSettings?: () => void;
}) {
  const [tab, setTab] = useState<Tab>('context');
  // 派生数据 memo（变更-09）：流式期间 Workspace 每帧重渲染，items/cost 未变时不重算
  const data = useMemo(() => contextPanelData(state.items, state.cost), [state.items, state.cost]);
  const fmt = (n: number) => n.toLocaleString('zh-CN');
  const started = state.startedAt ? new Date(state.startedAt) : null;
  const contextPercent = Math.round(data.contextWindow.usedRatio * 100);
  const enabledSkills = skills.filter((skill) => skill.enabled);
  const connectedMcp = mcpServers.filter(
    (server) => (server.toolCount ?? 0) > 0 && !server.lastError,
  );
  const permissions = settings?.permissions;

  return (
    <aside className="ctx">
      <div className="ctx__tabs tabbar">
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
            <div>
              <div className="csec__t">
                <Icon name="clip" /> 已挂载上下文
              </div>
              {data.mountedPaths.length ? (
                <div style={listStyle}>
                  {data.mountedPaths.map((path) => (
                    <div className="filerow" key={path}>
                      <span className="st m">C</span>
                      <span className="nm mono">{path}</span>
                    </div>
                  ))}
                </div>
              ) : (
                <div style={hintStyle}>暂无挂载文件/目录</div>
              )}
            </div>
            <div>
              <div className="csec__t">
                <Icon name="gitbranch" /> 文件变更
              </div>
              {data.changedFiles.length ? (
                <div style={listStyle}>
                  {data.changedFiles.map((file) => (
                    <div
                      className="filerow"
                      key={file.path}
                      title={file.edits > 1 ? `${file.edits} 次编辑的累计行数` : undefined}
                    >
                      <span className="st m">M</span>
                      <span className="nm">{file.path}</span>
                      <span className="pm">
                        <span className="a">+{file.added}</span>
                        <span className="d">-{file.removed}</span>
                        {file.edits > 1 ? <span className="faint">（累计）</span> : null}
                      </span>
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
                <Icon name="dollar" /> 本次会话
              </div>
              <div className="kv">
                <span>输入 token</span>
                <span className="mono">{fmt(state.cost.inputTokens)}</span>
              </div>
              <div className="kv">
                <span>输出 token</span>
                <span className="mono">{fmt(state.cost.outputTokens)}</span>
              </div>
              <div className="kv">
                <span>预估花费</span>
                <span className="mono">
                  {state.cost.costUsd > 0
                    ? `$${state.cost.costUsd.toFixed(4)}`
                    : state.cost.inputTokens + state.cost.outputTokens > 0
                      ? '无价格数据'
                      : '$0.0000'}
                </span>
              </div>
              {started && (
                <div className="kv">
                  <span>开始时间</span>
                  <span className="mono">
                    {started.toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit' })}
                  </span>
                </div>
              )}
            </div>
            <div>
              <div className="csec__t">
                <Icon name="layers" /> Token 用量（累计）
              </div>
              <div className="usage">
                <div className="usage__row">
                  <span>累计 token</span>
                  <span className="usage__big">{fmt(data.contextWindow.usedTokens)}</span>
                </div>
                <div
                  className="ctxmeter"
                  aria-label="累计 token 相对上下文窗口上限的比例（非当前窗口占用）"
                  title="这是本会话输入+输出的累计值与窗口上限之比，不是当前上下文窗口的实时占用"
                >
                  <span style={{ width: `${contextPercent}%` }} />
                </div>
                <div className="kv">
                  <span>上下文窗口上限</span>
                  <span className="mono">
                    {data.contextWindow.maxTokens ? fmt(data.contextWindow.maxTokens) : '未知'}
                  </span>
                </div>
                <div className="kv">
                  <span>挂载路径</span>
                  <span className="mono">{fmt(data.contextWindow.mountedPathCount)}</span>
                </div>
              </div>
              <div style={hintStyle}>
                CLI 只报告逐轮 token 用量：这里是累计值，不代表当前窗口实时占用； 文件级明细 CLI
                未提供，Helm 不伪造估算。
              </div>
            </div>
            <div>
              <div className="csec__t">
                <Icon name="cpu" /> 本轮工具调用
              </div>
              {data.tools.length ? (
                <div>
                  {data.tools.map((tool) => (
                    <div className="toolrow" key={tool.id}>
                      <span className="toolrow__ic">
                        <Icon name={tool.name === 'Bash' ? 'terminal' : 'zap'} />
                      </span>
                      <span className="toolrow__meta">
                        <b>{tool.name}</b>
                        <small>{statusText(tool.status)}</small>
                      </span>
                    </div>
                  ))}
                </div>
              ) : (
                <div style={hintStyle}>暂无工具调用</div>
              )}
            </div>
          </div>
        )}

        {tab === 'tools' && (
          <div style={panelStyle}>
            <div>
              <div className="csec__t">
                <Icon name="shield" /> 工具权限
              </div>
              {permissions ? (
                <div>
                  {[
                    { icon: 'file' as const, name: '读取文件', value: permissions.readFiles },
                    { icon: 'edit' as const, name: '编辑文件', value: permissions.editFiles },
                    {
                      icon: 'terminal' as const,
                      name: '运行命令',
                      value: permissions.runCommands,
                    },
                    { icon: 'upright' as const, name: '网页抓取', value: permissions.fetchUrls },
                    { icon: 'plug' as const, name: 'MCP 工具', value: permissions.mcpTools },
                  ].map((row) => {
                    const pill = permissionPill(row.value);
                    return (
                      <button
                        type="button"
                        className="toolrow ctx-linkrow"
                        key={row.name}
                        title="到设置 · 权限中修改"
                        onClick={onOpenSettings}
                      >
                        <span className="toolrow__ic">
                          <Icon name={row.icon} />
                        </span>
                        <span className="toolrow__meta">
                          <b>{row.name}</b>
                        </span>
                        <span className={pill.className}>{pill.label}</span>
                      </button>
                    );
                  })}
                </div>
              ) : (
                <div style={hintStyle}>设置加载中…</div>
              )}
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
                          onClick={() => onToggleMcp?.(server.name)}
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
    </aside>
  );
}
