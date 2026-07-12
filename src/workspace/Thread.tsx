import { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react';
import type { Decision } from '@helm/protocol';
import type { SessionState } from '../engine/useSession';
import { Icon } from '../shell/icons';
import { UserMessage } from './items/UserMessage';
import { AssistantMessage } from './items/AssistantMessage';
import { ToolBlock } from './items/ToolBlock';
import { ErrorItem } from './items/ErrorItem';
import { ActivityRow } from './items/ActivityRow';
import { ApprovalCard } from './items/ApprovalCard';
import { CheckpointItem } from './items/CheckpointItem';
import { PlanItem } from './items/PlanItem';
import { ThinkingItem } from './items/ThinkingItem';
import { DEFAULT_THREAD_WINDOW, expandThreadWindow, threadWindow } from './threadWindow';

const useClientLayoutEffect = typeof window === 'undefined' ? useEffect : useLayoutEffect;

type ApproveFn = (approvalId: string, decision: Decision) => void;
type RestoreCheckpointFn = (checkpointId: string) => void;
type UndoRevertFn = () => void;

/** 距底部小于该值视为「贴底」，继续自动跟随流式输出 */
const AT_BOTTOM_THRESHOLD = 80;

export function Thread({
  state,
  onApprove,
  onRestoreCheckpoint,
  onUndoRevert,
}: {
  state: SessionState;
  onApprove: ApproveFn;
  onRestoreCheckpoint: RestoreCheckpointFn;
  onUndoRevert: UndoRevertFn;
}) {
  const scrollRef = useRef<HTMLDivElement>(null);
  // 滚动锁定（变更-09）：用户上翻阅读时暂停自动滚底，露出「回到底部」浮标；
  // 用 ref 记录避免每次滚动都重渲染，state 只驱动浮标显隐。
  const atBottomRef = useRef(true);
  const [showJump, setShowJump] = useState(false);
  const [visibleCount, setVisibleCount] = useState(DEFAULT_THREAD_WINDOW);
  const rafRef = useRef(0);
  const prependScrollHeightRef = useRef<number | null>(null);

  useClientLayoutEffect(() => {
    const previousHeight = prependScrollHeightRef.current;
    const el = scrollRef.current;
    if (previousHeight == null || !el) return;
    el.scrollTop += el.scrollHeight - previousHeight;
    prependScrollHeightRef.current = null;
  }, [visibleCount]);

  const handleScroll = useCallback(() => {
    const el = scrollRef.current;
    if (!el) return;
    const atBottom = el.scrollHeight - el.scrollTop - el.clientHeight < AT_BOTTOM_THRESHOLD;
    atBottomRef.current = atBottom;
    setShowJump((prev) => (prev === !atBottom ? prev : !atBottom));
  }, []);

  const scrollToBottom = useCallback(() => {
    const el = scrollRef.current;
    if (!el) return;
    el.scrollTop = el.scrollHeight;
    atBottomRef.current = true;
    setShowJump(false);
  }, []);

  useEffect(() => {
    // 贴底时才跟随新内容；rAF 合帧，避免每个 delta 一次强制同步 layout
    if (!atBottomRef.current) return;
    cancelAnimationFrame(rafRef.current);
    rafRef.current = requestAnimationFrame(() => {
      const el = scrollRef.current;
      if (el && atBottomRef.current) el.scrollTop = el.scrollHeight;
    });
    return () => cancelAnimationFrame(rafRef.current);
  }, [state.items, state.status, state.turnActivity]);

  const empty = state.items.length === 0;
  const { hiddenCount, visibleItems } = threadWindow(state.items, visibleCount);
  // 空态文案跟随当前引擎（P2-3：不写死引擎名）
  const engineIntro =
    state.engine === 'codex'
      ? { label: 'Codex', bin: 'codex' }
      : { label: 'Claude Code', bin: 'claude' };

  return (
    <div className="thread__scroll" ref={scrollRef} onScroll={handleScroll}>
      <div className="thread__inner">
        {empty && (
          <div className="thread-empty">
            <div className="ava-bot" style={{ width: 44, height: 44 }}>
              <Icon name="bot" style={{ width: 24, height: 24 }} />
            </div>
            <div className="thread-empty__title">
              发送一条消息，开始与真实的 {engineIntro.label} 对话
            </div>
            <div className="thread-empty__sub">回复将来自本机的 {engineIntro.bin} 进程。</div>
          </div>
        )}

        {hiddenCount > 0 ? (
          <button
            type="button"
            className="thread-load-earlier"
            onClick={() => {
              prependScrollHeightRef.current = scrollRef.current?.scrollHeight ?? null;
              setVisibleCount((count) => expandThreadWindow(count, hiddenCount));
            }}
          >
            加载更早内容（{hiddenCount}）
          </button>
        ) : null}

        {visibleItems.map((it) => {
          const className = 'reverted' in it && it.reverted ? 'item rolled' : undefined;
          switch (it.kind) {
            case 'user':
              return (
                <UserMessage
                  key={`user-${it.id}`}
                  text={it.text}
                  mode={it.mode}
                  className={className}
                />
              );
            case 'assistant':
              return (
                <AssistantMessage
                  key={`assistant-${it.id}`}
                  text={it.text}
                  className={className}
                  streaming={state.openAssistantId === it.id}
                  interrupted={it.interrupted}
                />
              );
            case 'thinking':
              return <ThinkingItem key={`thinking-${it.id}`} item={it} className={className} />;
            case 'tool':
              return <ToolBlock key={`tool-${it.id}`} item={it} className={className} />;
            case 'approval':
              return (
                <ApprovalCard
                  key={`approval-${it.id}`}
                  item={it}
                  onRespond={onApprove}
                  className={className}
                />
              );
            case 'plan':
              return <PlanItem key={`plan-${it.id}`} item={it} className={className} />;
            case 'checkpoint':
              return (
                <CheckpointItem
                  key={`checkpoint-${it.id}`}
                  id={it.id}
                  label={it.label}
                  ts={it.ts}
                  restored={it.restored}
                  onRestore={onRestoreCheckpoint}
                  onUndo={onUndoRevert}
                />
              );
            case 'error':
              return (
                <ErrorItem key={`error-${it.id}`} message={it.message} errorKind={it.errorKind} />
              );
            default:
              return null;
          }
        })}

        {state.turnActivity ? <ActivityRow state={state} /> : null}
      </div>
      {showJump ? (
        <button type="button" className="thread-jump" onClick={scrollToBottom} title="回到底部">
          <Icon name="down" />
          回到底部
        </button>
      ) : null}
    </div>
  );
}
