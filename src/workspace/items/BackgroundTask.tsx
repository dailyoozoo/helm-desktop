import { memo } from 'react';
import { Icon } from '../../shell/icons';
import { agentStateLabel, type BackgroundEntry } from './taskViewModel';

// 变更-34 · C2：后台命令行。显示命令、状态与真实停止入口（中断当前轮次）。
export const BackgroundTask = memo(function BackgroundTask({
  entry,
  onStop,
}: {
  entry: BackgroundEntry;
  /** 停止后台命令 = 中断当前轮次（Helm 以真实 TurnProcess 为停止单位）。 */
  onStop?: () => void;
}) {
  return (
    <div className="toolrow" data-kind="bgtask">
      <span className="toolrow__ic">
        <Icon name="terminal" />
      </span>
      <div className="toolrow__meta">
        <b className="mono" title={entry.command}>
          {entry.command}
        </b>
        {entry.dur ? <small>{entry.dur}</small> : null}
      </div>
      <span className={'st ' + entry.state}>{agentStateLabel(entry.state)}</span>
      {entry.state === 'run' && onStop ? (
        <button
          type="button"
          className="btn btn--subtle btn--sm bg-stop"
          onClick={onStop}
          title="中断当前轮次以停止该命令"
        >
          停止本轮
        </button>
      ) : null}
    </div>
  );
});
