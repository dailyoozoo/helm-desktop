import { useEffect, useRef, useState } from 'react';
import { Icon } from '../shell/icons';

/** 变更-34 · A2：行内审阅意见起草编辑器（挂在 diff 行下方，Ctrl+Enter 存下攒批）。 */
export function ReviewNoteEditor({
  file,
  line,
  onSave,
  onCancel,
}: {
  file: string;
  line: number;
  onSave: (file: string, line: number, text: string) => void;
  onCancel: () => void;
}) {
  const [text, setText] = useState('');
  const taRef = useRef<HTMLTextAreaElement>(null);

  useEffect(() => {
    taRef.current?.focus();
  }, []);

  const commit = () => {
    const value = text.trim();
    if (!value) {
      onCancel();
      return;
    }
    onSave(file, line, value);
  };

  return (
    <div className="rnote is-draft" data-draft-line={String(line)}>
      <div className="rnote__top">
        <span className="who">你</span>
        <span className="mono">第 {line} 行</span>
      </div>
      <textarea
        ref={taRef}
        value={text}
        placeholder="这一行有什么问题？写完 Ctrl+Enter 存下，最后一次性交回给 Helm"
        onChange={(event) => setText(event.target.value)}
        onKeyDown={(event) => {
          if (event.key === 'Enter' && (event.ctrlKey || event.metaKey)) {
            event.preventDefault();
            commit();
          }
          if (event.key === 'Escape') {
            event.preventDefault();
            onCancel();
          }
        }}
      />
      <div className="rnote__acts">
        <button type="button" className="btn btn--primary btn--sm" onClick={commit}>
          记下
        </button>
        <button type="button" className="btn btn--subtle btn--sm" onClick={onCancel}>
          取消
        </button>
      </div>
    </div>
  );
}

/** 变更-34 · A2：攒批条 —— 意见先攒着，N 条一次性交回给 Agent 改。 */
export function ReviewNoteBatch({
  count,
  onClear,
  onSubmit,
  onSelfReview,
  reviewing,
}: {
  count: number;
  onClear: () => void;
  onSubmit: () => void;
  onSelfReview: () => void;
  reviewing: boolean;
}) {
  return (
    <div className={'notebar' + (count > 0 ? ' is-on' : '')}>
      <Icon name="comment" />
      <span className="t">
        <b>{count}</b> 条审阅意见待交回
      </span>
      <span className="sp" />
      <button
        type="button"
        className="btn btn--subtle btn--sm"
        onClick={onSelfReview}
        disabled={reviewing}
      >
        {reviewing ? 'Helm 评审中…' : '让 Helm 自评审'}
      </button>
      <button
        type="button"
        className="btn btn--subtle btn--sm"
        onClick={onClear}
        disabled={count === 0}
      >
        清空
      </button>
      <button
        type="button"
        className="btn btn--primary btn--sm"
        onClick={onSubmit}
        disabled={count === 0}
      >
        <Icon name="send" /> 交回给 Helm 修改
      </button>
    </div>
  );
}
