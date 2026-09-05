import { memo, useState } from 'react';
import { Icon } from '../../shell/icons';
import { Markdown, copyText } from '../../lib/markdown';

/** 交付物行（批次②用户裁决补上 · 原型 ws.js L35-44 options.deliverables）。 */
export interface AnswerDeliverables {
  /** 本轮产出（写入）的文件，最多展示 4 个 */
  documents: string[];
  /** 本轮触碰过的文件总数（读 + 写，含未变更） */
  fileCount: number;
  /** 本轮产生变更（写入/修改）的文件数 */
  changeCount: number;
  onOpenFiles: () => void;
  onOpenChanges: () => void;
}

/**
 * 助手回答（批次①对齐原型 ws.js ai 条目）：正文直铺背景不套气泡、无头像无角色行——
 * 头像与「Helm」名字由所在轮次的 .ai-turn/.ai-head 统一承担（原型 L50-51）。
 * 回答操作排（复制/赞/踩/派生，D-3）保留；交付物行（批次②）位于正文与操作排之间。
 */
export const AssistantMessage = memo(function AssistantMessage({
  text,
  className,
  streaming,
  deliverables,
  onFork,
  showActions,
}: {
  text: string;
  className?: string;
  /** 正在流式输出：正文尾部显示闪烁光标（变更-09） */
  streaming?: boolean;
  /** 交付物入口（原型 .deliverables）：产出文档链接 + 查看全部文件/修改记录 */
  deliverables?: AnswerDeliverables;
  /** D-3（可靠性检查-工作区对话页-差异清单）：从此回答派生新任务（同引擎摘要派生） */
  onFork?: () => void;
  /** 仅本轮最终且已完成的回答展示操作排与分支（原型：操作附在结果后，不挂每一步思考） */
  showActions?: boolean;
}) {
  const [copied, setCopied] = useState(false);
  // D-3：赞/踩互斥的本地反馈态（对齐原型 ws.js L624-627：仅视觉互斥，不上报后端）
  const [feedback, setFeedback] = useState<'like' | 'dislike' | null>(null);
  return (
    <div className={className ? `ws-msg ${className}` : 'ws-msg'} data-kind="assistant">
      <div className={streaming ? 'prose is-streaming ai-answer' : 'prose ai-answer'}>
        {streaming ? <span className="ws-stream-text">{text}</span> : <Markdown text={text} />}
        {streaming ? <span className="ws-caret" aria-hidden="true" /> : null}
      </div>
      {/* 交付物行（原型 .deliverables，位于正文与回答操作之间） */}
      {!streaming && text && deliverables ? (
        <div className="deliverables">
          {deliverables.documents.slice(0, 4).map((path) => (
            <button
              key={path}
              type="button"
              className="btn btn--subtle btn--sm file-link"
              title={`在右栏查看 ${path}`}
              onClick={deliverables.onOpenFiles}
            >
              <Icon name="file" />
              {path}
            </button>
          ))}
          {deliverables.fileCount > 0 ? (
            <button
              type="button"
              className="btn btn--subtle btn--sm"
              onClick={deliverables.onOpenFiles}
            >
              <Icon name="folderopen" />
              查看全部文件 <span className="deliverables__count">{deliverables.fileCount}</span>
            </button>
          ) : null}
          {deliverables.changeCount > 0 ? (
            <button
              type="button"
              className="btn btn--subtle btn--sm"
              onClick={deliverables.onOpenChanges}
            >
              <Icon name="split" />
              查看修改记录 <span className="deliverables__count">{deliverables.changeCount}</span>
            </button>
          ) : null}
        </div>
      ) : null}
      {/* D-3：回答操作排（对齐原型 answerActions：复制/赞/踩/派生）；仅最终回答展示 */}
      {!streaming && text && showActions ? (
        <div className="ai-actions" aria-label="回答操作">
          <button
            type="button"
            className="ai-action"
            title="复制回答"
            aria-label="复制回答"
            onClick={async () => {
              if (await copyText(text)) {
                setCopied(true);
                window.setTimeout(() => setCopied(false), 1500);
              }
            }}
          >
            <Icon name={copied ? 'checkc' : 'copy'} />
          </button>
          <button
            type="button"
            className={'ai-action' + (feedback === 'like' ? ' is-on' : '')}
            title="赞"
            aria-label="赞"
            aria-pressed={feedback === 'like'}
            onClick={() => setFeedback((current) => (current === 'like' ? null : 'like'))}
          >
            <Icon name="thumbsup" />
          </button>
          <button
            type="button"
            className={'ai-action' + (feedback === 'dislike' ? ' is-on' : '')}
            title="踩"
            aria-label="踩"
            aria-pressed={feedback === 'dislike'}
            onClick={() => setFeedback((current) => (current === 'dislike' ? null : 'dislike'))}
          >
            <Icon name="thumbsdown" />
          </button>
          {onFork ? (
            <button
              type="button"
              className="ai-action"
              title="从此回答派生新任务"
              aria-label="从此回答派生新任务"
              onClick={onFork}
            >
              <Icon name="gitbranch" />
            </button>
          ) : null}
        </div>
      ) : null}
    </div>
  );
});
