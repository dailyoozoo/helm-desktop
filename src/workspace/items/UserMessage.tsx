import { memo, useState } from 'react';
import { Icon } from '../../shell/icons';
import { Markdown, copyText } from '../../lib/markdown';
import type { PermissionProfile, TurnMode } from '../../engine/transport';

const MODE_LABEL: Record<Exclude<TurnMode, 'build'>, string> = {
  plan: '计划',
  ask: '询问',
};

const PROFILE_LABEL: Record<PermissionProfile, string> = {
  standard: '标准',
  auto: '自动执行',
  full_access: '全部放开',
};

export const UserMessage = memo(function UserMessage({
  text,
  mode,
  permissionProfile,
  className,
}: {
  text: string;
  /** 计划/询问轮次显示模式徽标（变更-04 B.2）；构建不传 */
  mode?: TurnMode;
  /** 每轮实际权限档位；实时消息与 schema v17 历史恢复共用。 */
  permissionProfile?: PermissionProfile;
  className?: string;
}) {
  const [copied, setCopied] = useState(false);
  return (
    <div className={className ? `item ws-msg ${className}` : 'item ws-msg'}>
      <div className="item__gut">
        <div className="avatar avatar--sm">我</div>
      </div>
      <div className="item__main">
        <div className="role">
          你
          {mode && mode !== 'build' ? (
            <span className="pill user-mode-pill">{MODE_LABEL[mode]}</span>
          ) : null}
          {permissionProfile ? (
            <span
              className={`pill user-permission-pill user-permission-pill--${permissionProfile}`}
              title="本轮权限档位"
            >
              {PROFILE_LABEL[permissionProfile]}
            </span>
          ) : null}
          <button
            type="button"
            className="ws-msg-copy"
            title="复制消息"
            aria-label="复制消息"
            onClick={async () => {
              if (await copyText(text)) {
                setCopied(true);
                window.setTimeout(() => setCopied(false), 1500);
              }
            }}
          >
            <Icon name={copied ? 'checkc' : 'copy'} />
          </button>
        </div>
        <div className="user-text">
          <Markdown text={text} />
        </div>
      </div>
    </div>
  );
});
