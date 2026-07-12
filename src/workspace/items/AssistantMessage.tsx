import { memo, useState } from 'react';
import { Icon } from '../../shell/icons';
import { Markdown, copyText } from '../../lib/markdown';

export const AssistantMessage = memo(function AssistantMessage({
  text,
  className,
  streaming,
  interrupted,
}: {
  text: string;
  className?: string;
  /** 正在流式输出：正文尾部显示闪烁光标（变更-09） */
  streaming?: boolean;
  /** 本条消息被用户中断：标注「已停止」而不是静默留半截（变更-09） */
  interrupted?: boolean;
}) {
  const [copied, setCopied] = useState(false);
  return (
    <div className={className ? `item ws-msg ${className}` : 'item ws-msg'}>
      <div className="item__gut">
        <div className="ava-bot">
          <Icon name="bot" />
        </div>
      </div>
      <div className="item__main">
        <div className="role">
          Helm{' '}
          <span className="pill pill--ghost mono" style={{ height: 18 }}>
            智能体
          </span>
          {interrupted ? <span className="pill pill--warn ws-stopped-pill">已停止</span> : null}
          {/* 消息复制（变更-10）：流式结束后可复制原文 */}
          {!streaming && text ? (
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
          ) : null}
        </div>
        <div className={streaming ? 'prose is-streaming' : 'prose'}>
          {streaming ? <span className="ws-stream-text">{text}</span> : <Markdown text={text} />}
          {streaming ? <span className="ws-caret" aria-hidden="true" /> : null}
        </div>
      </div>
    </div>
  );
});
