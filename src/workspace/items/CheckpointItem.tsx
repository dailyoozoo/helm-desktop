import { memo } from 'react';
import { Icon } from '../../shell/icons';

interface CheckpointItemProps {
  id: string;
  label: string;
  ts: number;
  restored: boolean;
  restorable: boolean;
  fileCount: number;
  reason?: string;
  onRestore: (id: string) => void;
  onUndo: () => void;
}

export const CheckpointItem = memo(function CheckpointItem({
  id,
  label,
  ts,
  restored,
  restorable,
  fileCount,
  reason,
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
        {!restored && restorable && fileCount > 0 ? (
          <button className="ckpt__btn" type="button" onClick={() => onRestore(id)}>
            <Icon name="history" />
            恢复
          </button>
        ) : restored ? (
          <button className="ckpt__btn" type="button" onClick={onUndo}>
            <Icon name="refresh" />
            撤销
          </button>
        ) : (
          <span className="ckpt__state" title={reason ?? '缺少有效文件快照'}>
            不可恢复
          </span>
        )}
      </div>
    </div>
  );
});
