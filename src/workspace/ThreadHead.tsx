import { useEffect, useRef, useState } from 'react';
import { Icon } from '../shell/icons';
import type { EngineId, ReasoningEffort, ReasoningEffortCapability } from '@helm/protocol';
import type { WorkspaceEngineOption } from './workspaceViewModel';
import { reasoningEffortLabel } from '../reasoning';
import type { SessionFolder } from '../sessions/sessionTypes';

type HeadMenuId = 'folder' | 'engine' | 'model';

export function ThreadHead({
  title,
  folders,
  folderId,
  onSelectFolder,
  engineOptions,
  activeOption,
  model,
  runtimeModel,
  onSelectEngine,
  onSelectModel,
  reasoningEffort,
  reasoningCapability,
  reasoningLoading,
  reasoningDisabled,
  onSelectReasoningEffort,
  onToggleSessions,
  sessionsExpanded,
  onToggleCtx,
  contextExpanded,
  cost,
}: {
  /** 会话标题（变更-10）：接真实数据，新会话/未命名回落「未命名会话」 */
  title?: string | null;
  folders: SessionFolder[];
  folderId: string | null;
  onSelectFolder: (folderId: string) => void;
  engineOptions: WorkspaceEngineOption[];
  activeOption?: WorkspaceEngineOption;
  model: string;
  runtimeModel?: string;
  onSelectEngine: (engine: EngineId) => void;
  onSelectModel: (model: string) => void;
  reasoningEffort: ReasoningEffort;
  reasoningCapability: ReasoningEffortCapability | null;
  reasoningLoading: boolean;
  reasoningDisabled: boolean;
  onSelectReasoningEffort: (effort: ReasoningEffort) => void;
  onToggleSessions: () => void;
  sessionsExpanded: boolean;
  onToggleCtx: () => void;
  contextExpanded: boolean;
  cost?: {
    costUsd: number;
    inputTokens: number;
    outputTokens: number;
  };
}) {
  const providerLabel = activeOption?.provider?.name ?? '未绑定服务商';
  const engineLabel = activeOption?.engine.name ?? '未配置引擎';
  const engineIcon = activeOption?.engine.id === 'codex' ? 'cpu' : 'zap';
  const folder = folderId ? folders.find((item) => item.id === folderId) : undefined;
  const folderLabel = folder?.name ?? (folderId ? '默认' : '按工作目录自动归类');

  // 头部菜单点击开合（B1-1）：对齐 prototype data-menu——同刻至多一个打开、选后关闭、
  // 外点/Esc 关闭且 Esc 把焦点归还触发钮；不再用 CSS hover/focus-within 驱动。
  const [openMenu, setOpenMenu] = useState<HeadMenuId | null>(null);
  const folderBtnRef = useRef<HTMLButtonElement | null>(null);
  const engineBtnRef = useRef<HTMLButtonElement | null>(null);
  const modelBtnRef = useRef<HTMLButtonElement | null>(null);

  useEffect(() => {
    if (!openMenu) return;
    const onPointerDown = (event: MouseEvent) => {
      if (!(event.target instanceof Element) || !event.target.closest('.thread__head .menu-wrap')) {
        setOpenMenu(null);
      }
    };
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== 'Escape') return;
      const trigger =
        openMenu === 'folder' ? folderBtnRef : openMenu === 'engine' ? engineBtnRef : modelBtnRef;
      trigger.current?.focus();
      setOpenMenu(null);
    };
    document.addEventListener('mousedown', onPointerDown);
    document.addEventListener('keydown', onKeyDown);
    return () => {
      document.removeEventListener('mousedown', onPointerDown);
      document.removeEventListener('keydown', onKeyDown);
    };
  }, [openMenu]);

  const toggleMenu = (id: HeadMenuId) => setOpenMenu((current) => (current === id ? null : id));

  return (
    <header className="thread__head">
      <button
        className="btn-icon ws-sidebar-toggle"
        onClick={onToggleSessions}
        title="切换会话列表"
        aria-label="切换会话列表"
        aria-expanded={sessionsExpanded}
        type="button"
      >
        <Icon name="history" />
      </button>
      <div className="grow" style={{ minWidth: 0 }}>
        <div className="thread__title truncate">{title?.trim() || '未命名会话'}</div>
        <div className="thread__meta truncate">
          {engineLabel} · {providerLabel}
          {cost?.costUsd != null && cost.costUsd > 0 ? (
            <span className="thread__cost"> · ${cost.costUsd.toFixed(4)}</span>
          ) : null}
        </div>
      </div>

      <div className="menu-wrap ws-folder-chip-wrap">
        <button
          className="btn btn--subtle btn--sm ws-folder-chip"
          type="button"
          ref={folderBtnRef}
          aria-haspopup="menu"
          aria-expanded={openMenu === 'folder'}
          title={folder?.cwd ?? folderLabel}
          onClick={() => toggleMenu('folder')}
        >
          <Icon name="folder" />
          <span>{folderLabel}</span>
          <Icon name="down" style={{ width: 13, height: 13 }} />
        </button>
        <div
          className={'menu ws-menu ws-folder-picker' + (openMenu === 'folder' ? ' open' : '')}
          role="menu"
        >
          <div className="menu__label">移动到文件夹</div>
          {folders.map((item) => (
            <button
              key={item.id}
              className={'menu__item' + (item.id === folderId ? ' is-on' : '')}
              type="button"
              role="menuitem"
              onClick={() => {
                setOpenMenu(null);
                onSelectFolder(item.id);
              }}
            >
              <Icon name="folder" />
              <span className="ws-menu__text">{item.name}</span>
              <span className="check">
                <Icon name="check" />
              </span>
            </button>
          ))}
        </div>
      </div>

      <div className="menu-wrap">
        <button
          className="btn btn--subtle btn--sm"
          type="button"
          title="本会话使用的 CLI 引擎"
          ref={engineBtnRef}
          aria-haspopup="menu"
          aria-expanded={openMenu === 'engine'}
          onClick={() => toggleMenu('engine')}
        >
          <span className="eng-ic">
            <Icon name={engineIcon} />
          </span>
          <span>{engineLabel}</span>
          <Icon name="down" style={{ width: 13, height: 13 }} />
        </button>
        <div className={'menu ws-menu' + (openMenu === 'engine' ? ' open' : '')} role="menu">
          <div className="menu__label">本会话使用的引擎</div>
          {engineOptions.map((option) => (
            <button
              key={option.engine.id}
              className={
                'menu__item' + (option.engine.id === activeOption?.engine.id ? ' is-on' : '')
              }
              onClick={() => {
                setOpenMenu(null);
                onSelectEngine(option.engine.id);
              }}
              type="button"
              role="menuitem"
            >
              <Icon name={option.engine.id === 'codex' ? 'cpu' : 'zap'} />
              <span className="ws-menu__text">
                {option.engine.name}
                <small>
                  {option.provider?.name ?? '未绑定服务商'} ·{' '}
                  {option.binding?.primaryModel ?? '未设置模型'}
                </small>
              </span>
              <span className="check">
                <Icon name="check" />
              </span>
            </button>
          ))}
          <div
            style={{ padding: '2px 10px 8px', fontSize: 11, color: 'var(--fg-4)', lineHeight: 1.5 }}
          >
            切换引擎会以新会话继续，当前会话完整保留在侧栏。
          </div>
        </div>
      </div>

      <div className="menu-wrap">
        <button
          className="btn btn--subtle btn--sm"
          type="button"
          title="当前引擎可用模型"
          ref={modelBtnRef}
          aria-haspopup="menu"
          aria-expanded={openMenu === 'model'}
          onClick={() => toggleMenu('model')}
        >
          <Icon name="sparkles" style={{ width: 14, height: 14, color: 'var(--accent-hi)' }} />
          <span className="mono" title="下一轮模型偏好">
            {model}
          </span>
          {runtimeModel && runtimeModel !== model ? (
            <span className="ws-route-evidence" title="最近一轮路由模型">
              最近 {runtimeModel}
            </span>
          ) : null}
          {reasoningEffort !== 'auto' ? (
            <span className="ws-reasoning-badge">{reasoningEffortLabel(reasoningEffort)}</span>
          ) : null}
          <Icon name="down" style={{ width: 13, height: 13 }} />
        </button>
        <div className={'menu ws-menu' + (openMenu === 'model' ? ' open' : '')} role="menu">
          <div className="menu__label">模型 · {providerLabel}</div>
          {(activeOption?.models ?? []).map((item) => (
            <button
              key={`${item.providerId}:${item.id}`}
              className={'menu__item' + (item.id === model ? ' is-on' : '')}
              onClick={() => {
                setOpenMenu(null);
                onSelectModel(item.id);
              }}
              type="button"
              role="menuitem"
            >
              <Icon name="sparkles" />
              <span className="mono ws-menu__text">{item.id}</span>
              <span className="check">
                <Icon name="check" />
              </span>
            </button>
          ))}
          {(activeOption?.models ?? []).length === 0 && (
            <div className="ws-menu__empty">当前引擎没有可用模型</div>
          )}
          <div className="menu__sep" />
          <div className="menu__label">推理强度</div>
          {reasoningLoading ? (
            <div className="ws-menu__empty">正在读取 CLI 模型能力…</div>
          ) : (
            (reasoningCapability?.options ?? ['auto']).map((effort) => (
              <button
                key={effort}
                className={
                  'menu__item ws-reasoning-option' + (effort === reasoningEffort ? ' is-on' : '')
                }
                onClick={() => {
                  setOpenMenu(null);
                  onSelectReasoningEffort(effort);
                }}
                type="button"
                role="menuitem"
                disabled={reasoningDisabled}
                title={reasoningDisabled ? '当前轮次运行中，完成后可调整' : undefined}
              >
                <Icon name="sparkles" />
                <span className="ws-menu__text">
                  {reasoningEffortLabel(effort)}
                  <small>
                    {effort === 'auto'
                      ? reasoningCapability?.defaultEffort
                        ? `模型默认 · ${reasoningEffortLabel(reasoningCapability.defaultEffort)}`
                        : '使用模型默认值'
                      : effort === reasoningCapability?.defaultEffort
                        ? '模型默认档位'
                        : '下一轮生效'}
                  </small>
                </span>
                <span className="check">
                  <Icon name="check" />
                </span>
              </button>
            ))
          )}
          {!reasoningLoading && reasoningCapability?.support !== 'supported' ? (
            <div className="ws-menu__empty">当前模型未声明可调档位</div>
          ) : null}
        </div>
      </div>

      <button
        className="btn-icon ws-context-toggle"
        onClick={onToggleCtx}
        title="切换上下文面板"
        aria-label="切换上下文面板"
        aria-expanded={contextExpanded}
        type="button"
      >
        <Icon name="panelright" />
      </button>
    </header>
  );
}
