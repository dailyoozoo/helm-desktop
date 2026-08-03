import { Icon, type IconName } from './icons';
import { useSyncExternalStore } from 'react';
import { livePendingApprovalSessionIds, subscribeLiveSessions } from '../engine/useSession';

// 左侧导航栏（七屏入口）。Slice 1 只实现工作区，其余入口先占位（后续切片接入路由）。
const RAIL: { id: string; ic: IconName; tip: string }[] = [
  { id: 'home', ic: 'home', tip: '总览' },
  { id: 'workspace', ic: 'chat', tip: '工作区' },
  { id: 'sessions', ic: 'history', tip: '会话历史' },
  { id: 'providers', ic: 'server', tip: '服务商与模型' },
  { id: 'extensions', ic: 'puzzle', tip: '扩展中心' },
  { id: 'usage', ic: 'chart', tip: '用量与成本' },
];

export type PageId =
  | 'home'
  | 'workspace'
  | 'sessions'
  | 'providers'
  | 'extensions'
  | 'usage'
  | 'settings';

export function Rail({
  active,
  onSelect,
  workspaceName = 'Helm 工作区',
  workspaceAvatar = 'H',
  onNewSession,
}: {
  active: PageId;
  onSelect: (page: PageId) => void;
  workspaceName?: string;
  workspaceAvatar?: string;
  onNewSession: () => void;
}) {
  const approvalCount = useSyncExternalStore(
    subscribeLiveSessions,
    () => livePendingApprovalSessionIds().length,
    () => 0,
  );
  return (
    <nav className="rail">
      <div className="rail__logo">
        <Icon name="rocket" style={{ color: '#fff' }} />
      </div>
      {RAIL.map((i) => (
        <button
          key={i.id}
          className={'rail-btn' + (i.id === active ? ' is-active' : '')}
          data-tip={i.tip}
          aria-label={i.tip}
          type="button"
          onClick={() => onSelect(i.id as PageId)}
        >
          <Icon name={i.ic} />
          {i.id === 'workspace' && approvalCount > 0 ? (
            <span className="rbadge" aria-label={`${approvalCount} 个会话等待审批`}>
              {approvalCount > 9 ? '9+' : approvalCount}
            </span>
          ) : null}
        </button>
      ))}
      <div className="rail__sp" />
      <button
        className="rail-btn"
        data-tip="新建会话"
        aria-label="新建会话"
        type="button"
        onClick={onNewSession}
      >
        <Icon name="plus" />
      </button>
      <button
        className={'rail-btn' + (active === 'settings' ? ' is-active' : '')}
        data-tip="设置"
        aria-label="设置"
        type="button"
        onClick={() => onSelect('settings')}
      >
        <Icon name="settings" />
      </button>
      <div className="avatar rail__avatar" title={workspaceName}>
        {workspaceAvatar}
      </div>
    </nav>
  );
}
