import { memo, useState } from 'react';
import { Icon } from '../../shell/icons';
import { Markdown, copyText } from '../../lib/markdown';
import type { TurnMode } from '../../engine/transport';

const MODE_LABEL: Record<Exclude<TurnMode, 'build'>, string> = {
  plan: '计划',
  ask: '询问',
};

export const UserMessage = memo(function UserMessage({
  text,
  mode,
  className,
}: {
  text: string;
  /** 计划/询问轮次显示模式徽标（变更-04 B.2）；构建不传 */
  mode?: TurnMode;
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
