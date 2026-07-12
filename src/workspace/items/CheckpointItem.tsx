import { memo } from 'react';
import { Icon } from '../../shell/icons';

interface CheckpointItemProps {
  id: string;
  label: string;
  ts: number;
  restored: boolean;
  onRestore: (id: string) => void;
  onUndo: () => void;
}

export const CheckpointItem = memo(function CheckpointItem({
  id,
  label,
  ts,
  restored,
  onRestore,
  onUndo,
}: CheckpointItemProps) {
  const time = new Date(ts).toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit' });

  return (
    <div className={`ckpt ${restored ? 'is-restored' : ''}`}>
      <div className="ckpt__chip">
        <Icon name="flag" />
        <span>{label}</span>
        <span className="t">{time}</span>
        {!restored ? (
          <button className="ckpt__btn" type="button" onClick={() => onRestore(id)}>
            <Icon name="history" />
            恢复
          </button>
        ) : (
          <button className="ckpt__btn" type="button" onClick={onUndo}>
            <Icon name="refresh" />
            撤销
          </button>
        )}
      </div>
    </div>
  );
});
