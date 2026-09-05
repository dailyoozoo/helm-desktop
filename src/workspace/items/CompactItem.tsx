import { memo } from 'react';
import { Icon } from '../../shell/icons';

export interface CompactItemData {
  id: string;
  status: 'submitted' | 'running' | 'succeeded' | 'failed';
  ts: number;
  summary?: string;
  error?: string;
}

const STATUS_LABEL: Record<CompactItemData['status'], string> = {
  submitted: '已提交压缩',
  running: '正在压缩上下文…',
  succeeded: '上下文已压缩',
  failed: '压缩失败',
};

const STATUS_ICON: Record<CompactItemData['status'], 'compress' | 'refresh' | 'flag'> = {
  submitted: 'compress',
  running: 'compress',
  succeeded: 'flag',
  failed: 'flag',
};

/**
 * 上下文压缩标记（对齐原型 .compact 分隔线形态，ws.js L163-166）：
 * 一行安静提示「上下文已压缩」+ mono 压缩摘要；无时间、无展开明细、无着色。
 */
export const CompactItem = memo(function CompactItem({ item }: { item: CompactItemData }) {
  const { id, status, summary, error } = item;
  const isRunning = status === 'submitted' || status === 'running';

  return (
    <div className="compact" data-thread-item-id={id} data-kind="compact">
      <div className="compact__hint" title={summary ?? error}>
        <Icon name={STATUS_ICON[status]} className={isRunning ? 'is-spin' : undefined} />
        <span>{STATUS_LABEL[status]}</span>
        {summary ? <span className="mono">{summary}</span> : null}
      </div>
    </div>
  );
});
