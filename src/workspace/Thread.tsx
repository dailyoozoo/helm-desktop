import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
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
import { layoutThreadItems, type ThreadRenderEntry } from './threadGroups';
import { ToolGroup } from './items/ToolGroup';
import { TurnProcess } from './items/TurnProcess';

const useClientLayoutEffect = typeof window === 'undefined' ? useEffect : useLayoutEffect;

type ApproveFn = (approvalId: string, decision: Decision) => void;
type RestoreCheckpointFn = (checkpointId: string) => void;
type UndoRevertFn = () => void;

/** 距底部小于该值视为「贴底」，继续自动跟随流式输出 */
const AT_BOTTOM_THRESHOLD = 80;

export function currentWorkingTurnId(state: SessionState): string | null {
  if (state.status !== 'working') return null;
  let lastUserIndex = -1;
  for (let index = state.items.length - 1; index >= 0; index -= 1) {
    if (state.items[index]?.kind === 'user') {
      lastUserIndex = index;
      break;
    }
  }
  for (let index = state.items.length - 1; index > lastUserIndex; index -= 1) {
    const turnId = state.items[index]?.turnId;
    if (turnId) return turnId;
  }
  return null;
}

export function Thread({
  state,
  onApprove,
  onRestoreCheckpoint,
  onUndoRevert,
  locateTarget,
}: {
  state: SessionState;
  onApprove: ApproveFn;
  onRestoreCheckpoint: RestoreCheckpointFn;
  onUndoRevert: UndoRevertFn;
  locateTarget?: { id: string; request: number } | null;
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
  const { hiddenCount, visibleItems } = useMemo(
    () => threadWindow(state.items, visibleCount),
    [state.items, visibleCount],
  );
  const renderEntries = useMemo(() => layoutThreadItems(visibleItems), [visibleItems]);
  useEffect(() => {
    if (!locateTarget) return;
    const index = state.items.findIndex((item) => item.id === locateTarget.id);
    if (index < 0) return;
    const required = state.items.length - index;
    if (required > visibleCount) {
      setVisibleCount(required);
      return;
    }
    requestAnimationFrame(() =>
      requestAnimationFrame(() => {
        const element = Array.from(
          document.querySelectorAll<HTMLElement>('[data-thread-item-id]'),
        ).find((candidate) => candidate.dataset.threadItemId === locateTarget.id);
        element?.scrollIntoView({ behavior: 'smooth', block: 'center' });
        element?.classList.add('is-flash');
        window.setTimeout(() => element?.classList.remove('is-flash'), 1_400);
      }),
    );
  }, [locateTarget, state.items, visibleCount]);
  // 空态文案跟随当前引擎（P2-3：不写死引擎名）
  const engineIntro =
    state.engine === 'codex'
      ? { label: 'Codex', bin: 'codex' }
      : { label: 'Claude Code', bin: 'claude' };
  const hasPendingTool = state.items.some(
    (item) => item.kind === 'tool' && item.status === 'pending' && !item.reverted,
  );
  const hasOpenThinking = state.items.some(
    (item) => item.kind === 'thinking' && !item.done && !item.reverted,
  );
  const showActivity =
    Boolean(state.turnActivity) &&
    !(state.turnActivity?.stage === 'using_tool' && hasPendingTool) &&
    !(state.turnActivity?.stage === 'reasoning' && hasOpenThinking);
  const activeTurnId = currentWorkingTurnId(state);

  const renderEntry = (entry: ThreadRenderEntry) => {
    if (entry.kind === 'tool-group') {
      return (
        <ToolGroup key={`tool-group-${entry.id}`} items={entry.items} locateTarget={locateTarget} />
      );
    }
    const it = entry.item;
    const className = 'reverted' in it && it.reverted ? 'item rolled' : undefined;
    switch (it.kind) {
      case 'user':
        return (
          <UserMessage
            key={`user-${it.id}`}
            text={it.text}
            mode={it.mode}
            permissionProfile={it.permissionProfile}
            className={className}
          />
        );
      case 'assistant':
        return (
          <div key={`assistant-${it.id}`} data-thread-item-id={it.id}>
            <AssistantMessage
              text={it.text}
              className={className}
              streaming={state.openAssistantId === it.id}
              interrupted={it.interrupted}
            />
          </div>
        );
      case 'thinking':
        return (
          <ThinkingItem
            key={`thinking-${it.id}`}
            item={it}
            className={className}
            locateTarget={locateTarget}
          />
        );
      case 'tool':
        return (
          <div key={`tool-${it.id}`} data-thread-item-id={it.id}>
            <ToolBlock item={it} className={className} locateTarget={locateTarget} />
          </div>
        );
      case 'approval':
        return (
          <div key={`approval-${it.id}`} data-thread-item-id={it.id}>
            <ApprovalCard item={it} onRespond={onApprove} className={className} />
          </div>
        );
      case 'plan':
        return (
          <div key={`plan-${it.id}`} data-thread-item-id={it.id}>
            <PlanItem item={it} className={className} />
          </div>
        );
      case 'checkpoint':
        return (
          <div key={`checkpoint-${it.id}`} data-thread-item-id={it.id}>
            <CheckpointItem
              id={it.id}
              label={it.label}
              ts={it.ts}
              restored={it.restored}
              restorable={it.restorable}
              fileCount={it.fileCount}
              reason={it.reason}
              onRestore={onRestoreCheckpoint}
              onUndo={onUndoRevert}
            />
          </div>
        );
      case 'error':
        return (
          <div key={`error-${it.id}`} data-thread-item-id={it.id}>
            <ErrorItem message={it.message} errorKind={it.errorKind} stalledKind={it.stalledKind} />
          </div>
        );
      default:
        return null;
    }
  };

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

        {renderEntries.map((entry) => {
          if (entry.kind !== 'turn-process') return renderEntry(entry);
          const waitingApproval = state.items.some(
            (item) =>
              item.kind === 'approval' &&
              item.turnId === entry.turnId &&
              (item.status === 'pending' || item.status === 'applying'),
          );
          return (
            <TurnProcess
              key={entry.id}
              id={entry.id}
              entries={entry.entries}
              completed={entry.completed && entry.turnId !== activeTurnId}
              terminalStatus={entry.terminalStatus}
              waitingApproval={waitingApproval}
              locateTarget={locateTarget}
            >
              {entry.entries.map(renderEntry)}
            </TurnProcess>
          );
        })}

        {showActivity ? <ActivityRow state={state} /> : null}
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
