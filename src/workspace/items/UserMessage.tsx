import { memo, useState } from 'react';
import { Icon } from '../../shell/icons';
import { Markdown, copyText } from '../../lib/markdown';

/**
 * 用户消息（对齐原型 ws.js 用户气泡）：右对齐浅蓝纯气泡，无头像无名字行。
 * 气泡下方 .user-meta 收弱化的复制按钮（批次①裁决「复制按钮保留、弱化收纳」，
 * 2026-09-02 落地）：hover 消息才渐显，复制成功 1.5s 内以 .is-on 态保持可见。
 */
export const UserMessage = memo(function UserMessage({
  text,
  className,
}: {
  text: string;
  className?: string;
}) {
  const [copied, setCopied] = useState(false);
  return (
    <div className={className ? `item ws-msg ${className}` : 'item ws-msg'} data-kind="user">
      <div className="item__main">
        <div className="user-text">
          <Markdown text={text} />
        </div>
        <div className="user-meta">
          <button
            type="button"
            className={'ai-action' + (copied ? ' is-on' : '')}
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
      </div>
    </div>
  );
});
