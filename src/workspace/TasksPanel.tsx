import { memo, useMemo, useState } from 'react';
import { Icon } from '../shell/icons';
import type { ThreadItem } from '../engine/useSession';
import {
  agentStateLabel,
  collectBackgroundCommands,
  collectSubagents,
  type SubagentEntry,
} from './items/taskViewModel';
import { BackgroundTask } from './items/BackgroundTask';

// 变更-34 · E1：任务 tab —— 本会话并行工作的唯一落点（子代理 + 后台命令）。
function SubagentRow({
  entry,
  onLocate,
}: {
  entry: SubagentEntry;
  onLocate?: (id: string) => void;
}) {
  const [open, setOpen] = useState(false);
  return (
    <div>
      <div className="toolrow" data-kind="saentry">
        <span className="toolrow__ic">
          <Icon name="bot" />
        </span>
        <button type="button" className="toolrow__main" onClick={() => setOpen(!open)}>
          <div className="toolrow__meta">
            <b>{entry.name}</b>
            <small>{entry.task || '—'}</small>
          </div>
          <span className="mono" style={{ fontSize: 11 }}>
            {entry.dur}
          </span>
          <span className={'st ' + entry.state}>{agentStateLabel(entry.state)}</span>
        </button>
        {onLocate ? (
          <button
            type="button"
            className="btn-icon sm"
            title="在线程中定位"
            aria-label="在线程中定位"
            onClick={() => onLocate(entry.id)}
          >
            <Icon name="upright" />
          </button>
        ) : null}
      </div>
      {open && entry.output ? <div className="sarow__out">{entry.output}</div> : null}
    </div>
  );
}

export const TasksPanel = memo(function TasksPanel({
  items,
  onStopTask,
  onLocate,
}: {
  items: ThreadItem[];
  onStopTask?: () => void;
  onLocate?: (id: string) => void;
}) {
  const agents = useMemo(() => collectSubagents(items), [items]);
  const backgrounds = useMemo(() => collectBackgroundCommands(items), [items]);

  return (
    <div className="taskpanel">
      <div>
        <div className="csec__t">
          <Icon name="users" /> 子代理{' '}
          <span className="faint" style={{ marginLeft: 'auto' }}>
            {agents.length ? `${agents.length} 个` : ''}
          </span>
        </div>
        <div className="taskpanel__list">
          {agents.length
            ? agents.map((entry) => (
                <SubagentRow key={entry.id} entry={entry} onLocate={onLocate} />
              ))
            : null}
          {!agents.length ? <div className="taskpanel__empty">本会话没有子代理</div> : null}
        </div>
      </div>
      <div>
        <div className="csec__t">
          <Icon name="terminal" /> 后台命令{' '}
          <span className="faint" style={{ marginLeft: 'auto' }}>
            {backgrounds.length ? `${backgrounds.length} 个` : ''}
          </span>
        </div>
        {backgrounds.some((entry) => entry.state === 'run') && onStopTask ? (
          <p className="taskpanel__hint">
            停止将中断当前轮次，同轮其他工作也会一并停止；暂不支持单任务取消。
          </p>
        ) : null}
        <div className="taskpanel__list">
          {backgrounds.length ? (
            backgrounds.map((entry) => (
              <BackgroundTask key={entry.id} entry={entry} onStop={onStopTask} />
            ))
          ) : (
            <div className="taskpanel__empty">没有后台命令</div>
          )}
        </div>
      </div>
    </div>
  );
});
