import { Icon } from '../shell/icons';
import type { EngineId } from '@helm/protocol';
import type { WorkspaceEngineOption } from './workspaceViewModel';

export function ThreadHead({
  title,
  engineOptions,
  activeOption,
  model,
  onSelectEngine,
  onSelectModel,
  onToggleCtx,
}: {
  /** 会话标题（变更-10）：接真实数据，新会话/未命名回落「未命名会话」 */
  title?: string | null;
  engineOptions: WorkspaceEngineOption[];
  activeOption?: WorkspaceEngineOption;
  model: string;
  onSelectEngine: (engine: EngineId) => void;
  onSelectModel: (model: string) => void;
  onToggleCtx: () => void;
}) {
  const providerLabel = activeOption?.provider?.name ?? '未绑定服务商';
  const engineLabel = activeOption?.engine.name ?? '未配置引擎';
  const engineIcon = activeOption?.engine.id === 'codex' ? 'cpu' : 'zap';

  return (
    <header className="thread__head">
      <div className="grow" style={{ minWidth: 0 }}>
        <div className="thread__title truncate">{title?.trim() || '未命名会话'}</div>
        <div className="thread__meta truncate">
          {engineLabel} · {providerLabel}
        </div>
      </div>

      <div className="menu-wrap">
        <button className="btn btn--subtle btn--sm" title="本会话使用的 CLI 引擎">
          <span className="eng-ic">
            <Icon name={engineIcon} />
          </span>
          <span>{engineLabel}</span>
          <Icon name="down" style={{ width: 13, height: 13 }} />
        </button>
        <div className="menu ws-menu">
          <div className="menu__label">本会话使用的引擎</div>
          {engineOptions.map((option) => (
            <button
              key={option.engine.id}
              className={
                'menu__item' + (option.engine.id === activeOption?.engine.id ? ' is-on' : '')
              }
              onClick={() => onSelectEngine(option.engine.id)}
              type="button"
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
        </div>
      </div>

      <div className="menu-wrap">
        <button className="btn btn--subtle btn--sm" title="当前引擎可用模型">
          <Icon name="sparkles" style={{ width: 14, height: 14, color: 'var(--accent-hi)' }} />
          <span className="mono">{model}</span>
          <Icon name="down" style={{ width: 13, height: 13 }} />
        </button>
        <div className="menu ws-menu">
          <div className="menu__label">模型 · {providerLabel}</div>
          {(activeOption?.models ?? []).map((item) => (
            <button
              key={`${item.providerId}:${item.id}`}
              className={'menu__item' + (item.id === model ? ' is-on' : '')}
              onClick={() => onSelectModel(item.id)}
              type="button"
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
        </div>
      </div>

      <button className="btn-icon" onClick={onToggleCtx} title="切换上下文面板">
        <Icon name="panelright" />
      </button>
    </header>
  );
}
