import {
  Fragment,
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from 'react';
import type { Decision } from '@helm/protocol';
import type { SessionState } from '../engine/useSession';
import type { SessionTurn } from '../sessions/api';
import { Icon } from '../shell/icons';
import { UserMessage } from './items/UserMessage';
import { AssistantMessage, type AnswerDeliverables } from './items/AssistantMessage';
import { ToolBlock } from './items/ToolBlock';
import { ErrorItem } from './items/ErrorItem';
import { ActivityRow } from './items/ActivityRow';
import { ApprovalCard } from './items/ApprovalCard';
import { CheckpointItem } from './items/CheckpointItem';
import { PlanItem } from './items/PlanItem';
import { ThinkingItem } from './items/ThinkingItem';
import { CompactItem } from './items/CompactItem';
import { DEFAULT_THREAD_WINDOW, expandThreadWindow, threadWindow } from './threadWindow';
import {
  isLiftedFailureEntry,
  layoutThreadItems,
  type ThreadLayoutEntry,
  type ThreadRenderEntry,
} from './threadGroups';
import { ToolGroup } from './items/ToolGroup';
import { TurnProcess } from './items/TurnProcess';
import { SubagentCard } from './items/SubagentCard';
import { collectSubagents } from './items/taskViewModel';
import { collectTurnDeliverables } from './toolTarget';
import { summarizeTurn } from './turnSummary';
import { ThreadTurnRail } from './ThreadTurnRail';

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

/**
 * 批次①对齐原型 renderThread（ws.js L455-532）：线程按用户消息切成轮次，
 * 用户消息之后的条目收进同一轮的 .ai-turn 容器（一个头像 + 折叠胶囊 + 过程体 +
 * 可见结果）。首条用户消息之前的活动（引擎派生标记等）按原型第 0 轮裸渲染。
 */
type ThreadTurnBlock =
  | { kind: 'prelude'; entries: ThreadLayoutEntry[] }
  | { kind: 'turn'; user: ThreadLayoutEntry | null; rest: ThreadLayoutEntry[]; ordinal: number };

function groupIntoTurnBlocks(entries: ThreadLayoutEntry[]): ThreadTurnBlock[] {
  const blocks: ThreadTurnBlock[] = [];
  let prelude: ThreadLayoutEntry[] = [];
  let current: { user: ThreadLayoutEntry | null; rest: ThreadLayoutEntry[] } | null = null;
  let ordinal = 0;
  const flush = () => {
    if (current) blocks.push({ kind: 'turn', user: current.user, rest: current.rest, ordinal });
    else if (prelude.length) blocks.push({ kind: 'prelude', entries: prelude });
    current = null;
    prelude = [];
  };
  for (const entry of entries) {
    if (entry.kind === 'item' && entry.item.kind === 'user') {
      flush();
      ordinal += 1;
      current = { user: entry, rest: [] };
      continue;
    }
    if (current) current.rest.push(entry);
    else prelude.push(entry);
  }
  flush();
  return blocks;
}

const fmtClock = (ts?: number) =>
  ts ? new Date(ts).toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit' }) : '';

export function Thread({
  state,
  onApprove,
  onRestoreCheckpoint,
  onUndoRevert,
  locateTarget,
  onOpenPane,
  onRetryTool,
  turns,
  onForkAnswer,
  onOpenSourceSession,
}: {
  state: SessionState;
  onApprove: ApproveFn;
  onRestoreCheckpoint: RestoreCheckpointFn;
  onUndoRevert: UndoRevertFn;
  locateTarget?: { id: string; request: number } | null;
  /** 变更-34 · A4/C1：请求在右栏打开交付物 tab（修改记录/全部文件/计划/终端/任务）。 */
  onOpenPane?: (tab: 'changes' | 'files' | 'plan' | 'term' | 'tasks') => void;
  /** 变更-34 · C4：把失败工具作为真实用户消息发回 Agent。 */
  onRetryTool?: (toolId: string) => void;
  /** 变更-34/35 · B2：TurnLedger 逐轮真值（模型等），用于轮次摘要头。 */
  turns?: SessionTurn[] | null;
  /** D-3：回答操作排「从此回答派生新任务」（同引擎派生）；参数为被点回答所属轮次 id，
   *  Codex 支持切点分叉（只带这段回答及之前），无 id 或引擎不支持时整段分叉兜底。 */
  onForkAnswer?: (turnId?: string) => void;
  /** 批次①：原型 .swch 派生胶囊「打开源会话」。 */
  onOpenSourceSession?: (sessionId: string) => void;
}) {
  const scrollRef = useRef<HTMLDivElement>(null);
  // 滚动锁定（变更-09）：用户上翻阅读时暂停自动滚底；
  // 「回到底部」由底部中央渐隐浮层「回到最新」承担（D-10b，用户裁决 B 形态）。
  const atBottomRef = useRef(true);
  const [visibleCount, setVisibleCount] = useState(DEFAULT_THREAD_WINDOW);
  const rafRef = useRef(0);
  const prependScrollHeightRef = useRef<number | null>(null);
  // D-10b：距底 >90px 时浮现「回到最新」浮层；新内容在上翻期间到达时脉冲一次
  // （key 变更触发重挂载 → CSS 动画重放）。任何会话长度均可用，不受 >3 轮门禁限制。
  const [jumpLatest, setJumpLatest] = useState(false);
  const [jumpPulse, setJumpPulse] = useState(false);
  const [jumpPulseTick, setJumpPulseTick] = useState(0);

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
    const distance = el.scrollHeight - el.scrollTop - el.clientHeight;
    atBottomRef.current = distance < AT_BOTTOM_THRESHOLD;
    setJumpLatest(distance > 90);
    if (distance <= 90) setJumpPulse(false);
  }, []);

  // 新内容到达且用户上翻时：浮层脉冲一次提示（滚动中显隐由 handleScroll 负责）
  const itemCount = state.items.length;
  useEffect(() => {
    if (itemCount === 0 || atBottomRef.current) return;
    setJumpLatest(true);
    setJumpPulse(true);
    setJumpPulseTick((tick) => tick + 1);
  }, [itemCount]);

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
  const blocks = useMemo(() => groupIntoTurnBlocks(renderEntries), [renderEntries]);
  // 第 N 轮：按完整 item 序列中的用户消息计数派生（与原型 turnHead 语义一致），
  // 不随窗口截断变化；首个用户消息前的活动内容计为第 1 轮。
  const userOrdinalByItemId = useMemo(() => {
    const result = new Map<string, number>();
    let userCount = 0;
    for (const item of state.items) {
      if (item.kind === 'user') {
        userCount += 1;
        result.set(item.id, userCount);
      }
    }
    return result;
  }, [state.items]);
  // 性能：retryCountFor 是 O(n) 扫描，之前每个工具卡渲染各调一次 → O(n·t)。
  // 预构建「工具 id → 同轮同名前序数」Map，整次渲染只扫一遍 items。
  const retryCountByToolId = useMemo(() => {
    const result = new Map<string, number>();
    const seen = new Map<string, { turnId: string; name: string; count: number }>();
    for (const item of state.items) {
      if (item.kind !== 'tool' || !item.turnId) continue;
      const prior = seen.get(item.name);
      const count = prior && prior.turnId === item.turnId ? prior.count : 0;
      result.set(item.id, count);
      seen.set(item.name, { turnId: item.turnId, name: item.name, count: count + 1 });
    }
    return result;
  }, [state.items]);
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
  const fork = state.fork;

  const renderEntry = (
    entry: ThreadRenderEntry,
    deliverables?: AnswerDeliverables,
    showActions?: boolean,
  ) => {
    if (entry.kind === 'tool-group') {
      return (
        <ToolGroup key={`tool-group-${entry.id}`} items={entry.items} locateTarget={locateTarget} />
      );
    }
    if (entry.kind === 'subagent') {
      return (
        <div key={`subagent-${entry.id}`} data-thread-item-id={entry.items[0]?.id}>
          <SubagentCard
            items={collectSubagents(entry.items)}
            onOpenPane={onOpenPane ? () => onOpenPane('tasks') : undefined}
          />
        </div>
      );
    }
    const it = entry.item;
    const className = 'reverted' in it && it.reverted ? 'rolled' : undefined;
    switch (it.kind) {
      case 'user':
        return <UserMessage key={`user-${it.id}`} text={it.text} className={className} />;
      case 'assistant':
        return (
          <div key={`assistant-${it.id}`} data-thread-item-id={it.id}>
            <AssistantMessage
              text={it.text}
              className={className}
              streaming={state.openAssistantId === it.id}
              deliverables={deliverables}
              onFork={showActions && onForkAnswer ? () => onForkAnswer(it.turnId) : undefined}
              showActions={showActions}
            />
          </div>
        );
      case 'thinking':
        return <ThinkingItem key={`thinking-${it.id}`} item={it} className={className} />;
      case 'tool':
        return (
          <div key={`tool-${it.id}`} data-thread-item-id={it.id}>
            <ToolBlock
              item={it}
              className={className}
              locateTarget={locateTarget}
              retryCount={retryCountByToolId.get(it.id) ?? 0}
              onRetry={onRetryTool}
              working={hasPendingTool}
            />
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
      case 'compact':
        return (
          <div key={`compact-${it.id}`} data-thread-item-id={it.id} className={className}>
            <CompactItem
              item={{
                id: it.id,
                status: it.status,
                ts: it.ts,
                summary: it.summary,
                error: it.error,
              }}
            />
          </div>
        );
      default:
        return null;
    }
  };

  /** 模型切换标记行（原型 .swch__line）：相邻两轮真实路由模型不同时显示。 */
  const swchFor = (turnId: string | undefined) => {
    if (!turns || !turnId) return null;
    const index = turns.findIndex((candidate) => candidate.id === turnId);
    if (index <= 0) return null;
    const current = turns[index];
    if (!current.routedModelId) return null;
    for (let prev = index - 1; prev >= 0; prev -= 1) {
      const candidate = turns[prev];
      if (!candidate.routedModelId) continue;
      if (candidate.routedModelId === current.routedModelId) return null;
      return (
        <div className="swch">
          <span className="swch__line">
            <Icon name="sparkles" />
            <span>模型切换</span>
            <span className="mono">
              {candidate.routedModelId} → {current.routedModelId}
            </span>
            {current.startedAt ? <span className="t">{fmtClock(current.startedAt)}</span> : null}
          </span>
        </div>
      );
    }
    return null;
  };

  const renderTurnBlocks = () =>
    blocks.map((block, blockIndex) => {
      if (block.kind === 'prelude') {
        return (
          <Fragment key={`prelude-${blockIndex}`}>
            {block.entries.map((entry) => renderEntry(entry))}
          </Fragment>
        );
      }
      const withTurnId = block.rest.find(
        (entry): entry is Extract<ThreadLayoutEntry, { kind: 'item' }> =>
          entry.kind === 'item' && Boolean(entry.item.turnId),
      );
      const turnId = withTurnId?.item.turnId;
      if (!block.user && block.rest.length === 0) return null;
      // 渲染形态 B（ADR 0019，对齐 WorkBuddy 截图 2026-08-31）：块内条目按真实时序
      // 平铺为 flatEntries，再切「过程条目」与「常驻条目」两段：
      // - 过程条目（折叠时随过程体收起）：思考/工具/工具组/子代理 + 中间 assistant 正文
      //   + 已处理完的审批卡——即 WorkBuddy 折叠态里全部消失的内容；
      // - 常驻条目（children，折叠时仍显示）：最终回答（块内最后一个 assistant）、
      //   待处理审批、失败/压缩/检查点/计划标记。
      // 注意：isProcessEntry 依赖 lastAssistantEntry，故在其后定义。
      const flatEntries = block.rest;
      const ledgerTurn =
        turnId && turns ? (turns.find((candidate) => candidate.id === turnId) ?? null) : null;
      const ordinal = Math.max(
        1,
        block.user && block.user.kind === 'item'
          ? (userOrdinalByItemId.get(block.user.item.id) ?? block.ordinal)
          : block.ordinal,
      );
      const summary = summarizeTurn(flatEntries, ordinal, ledgerTurn);
      const waitingApproval = turnId
        ? state.items.some(
            (item) =>
              item.kind === 'approval' &&
              item.turnId === turnId &&
              (item.status === 'pending' || item.status === 'applying'),
          )
        : false;
      // 交付物行（批次②裁决）：数据取本轮真实工具调用，口径与计数由
      // collectTurnDeliverables 统一裁决（触碰只认真实文件路径，变更只认真实 diff）。
      // 只挂在块内最后一个 assistant 回答下（原型 options.deliverables 挂最终回答）。
      const turnItems = flatEntries.flatMap((entry) =>
        entry.kind === 'item' && entry.item.kind === 'tool' ? [entry.item] : [],
      );
      const turnDeliverables = collectTurnDeliverables(turnItems);
      const lastAssistantEntry = [...block.rest]
        .reverse()
        .find(
          (entry): entry is Extract<ThreadRenderEntry, { kind: 'item' }> =>
            entry.kind === 'item' && entry.item.kind === 'assistant',
        );
      const isProcessEntry = (entry: ThreadRenderEntry): boolean => {
        // 失败工具例外（TurnProcess 渲染契约）：独立失败工具提成 children 常驻可见，
        // 不依赖整轮折叠——收起轮次的 .failc 也要可见（2026-09-04 视觉矩阵断言）。
        if (isLiftedFailureEntry(entry)) return false;
        if (entry.kind === 'tool-group' || entry.kind === 'subagent') return true;
        if (entry.kind !== 'item') return false;
        const it = entry.item;
        if (it.kind === 'thinking' || it.kind === 'tool') return true;
        // 中间 assistant 正文归过程区（展开态按真实时序穿插显示，折叠态收起）；
        // 已处理完的审批卡同理，待处理审批保持常驻等待用户操作。
        if (it.kind === 'assistant') return entry !== lastAssistantEntry;
        if (it.kind === 'approval') return !(it.status === 'pending' || it.status === 'applying');
        return false;
      };
      const processEntries = flatEntries.filter(isProcessEntry);
      const childEntries = flatEntries.filter((entry) => !isProcessEntry(entry));
      const lastAssistant =
        lastAssistantEntry?.kind === 'item' && lastAssistantEntry.item.kind === 'assistant'
          ? lastAssistantEntry.item
          : null;
      // 渲染形态 B（ADR 0019）：completed 基于当前活动轮 + 块内是否还有 pending 工具
      // + 最后 assistant 的 turnStatus，不再依赖整轮过程容器的 completed 字段。
      const completed =
        activeTurnId !== turnId &&
        !block.rest.some(
          (entry) =>
            entry.kind === 'item' &&
            entry.item.kind === 'tool' &&
            entry.item.status === 'pending' &&
            !entry.item.reverted,
        ) &&
        (lastAssistant == null ||
          ((lastAssistant.turnStatus == null || lastAssistant.turnStatus === 'succeeded') &&
            !lastAssistant.interrupted));
      const deliverables: AnswerDeliverables | undefined =
        lastAssistantEntry && completed && onOpenPane && turnDeliverables.fileCount > 0
          ? {
              documents: turnDeliverables.documents,
              fileCount: turnDeliverables.fileCount,
              changeCount: turnDeliverables.changeCount,
              onOpenFiles: () => onOpenPane('files'),
              onOpenChanges: () => onOpenPane('changes'),
            }
          : undefined;
      const userNode =
        block.user && block.user.kind === 'item' && block.user.item.kind === 'user'
          ? renderEntry(block.user)
          : null;
      return (
        <div className="turn" key={`turn-block-${blockIndex}`}>
          {userNode}
          <TurnProcess
            id={`turn-block-${blockIndex}`}
            turnId={turnId}
            entries={flatEntries}
            completed={completed}
            terminalStatus={
              ledgerTurn?.status === 'failed'
                ? 'failed'
                : ledgerTurn?.status === 'interrupted'
                  ? 'interrupted'
                  : undefined
            }
            waitingApproval={waitingApproval}
            locateTarget={locateTarget}
            summary={summary}
            process={
              processEntries.length
                ? processEntries.map((pe) => (
                    <Fragment key={pe.kind === 'item' ? pe.item.id : pe.id}>
                      {renderEntry(pe)}
                    </Fragment>
                  ))
                : undefined
            }
            swch={swchFor(turnId) ?? undefined}
          >
            {childEntries.map((entry) =>
              lastAssistantEntry && entry === lastAssistantEntry && completed
                ? renderEntry(entry, deliverables, true)
                : renderEntry(entry),
            )}
          </TurnProcess>
        </div>
      );
    });

  return (
    /* 原型结构：viewport 是轨道的定位锚点（自身不滚动），scroll 只负责内容滚动；
       轨道必须做 scroll 的兄弟节点，否则 top:50% 锚到内容总高度、滚动时被带走。 */
    <div className="thread__viewport">
      <div className="thread__scroll" ref={scrollRef} onScroll={handleScroll}>
        <div className="thread__inner">
          {empty && (
            <div className="thread-empty">
              <div className="ava-bot" style={{ width: 44, height: 44 }}>
                <Icon name="bot" className="h-6 w-6" style={{ width: 24, height: 24 }} />
              </div>
              <div className="thread-empty__title">
                发送一条消息，开始与真实的 {engineIntro.label} 对话
              </div>
              <div className="thread-empty__sub">回复将来自本机的 {engineIntro.bin} 进程。</div>
            </div>
          )}

          {!empty && fork ? (
            /* 原型 .swch 派生胶囊（批次①裁决：横幅退役，信息以原型形态保留） */
            <div className="swch">
              <button
                type="button"
                className="swch__chip"
                title={
                  (fork.sourceEngine !== state.engine ? '跨引擎摘要派生可能有损；' : '') +
                  '打开源会话'
                }
                onClick={() => onOpenSourceSession?.(fork.sourceSessionId ?? '')}
              >
                <Icon name="gitbranch" />
                <span>派生自</span>
                <span>{fork.sourceEngine === 'codex' ? 'Codex' : 'Claude Code'}</span>
                {fork.sourceEngine !== state.engine ? (
                  <span className="t">细节可能有损</span>
                ) : null}
              </button>
            </div>
          ) : null}

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

          {renderTurnBlocks()}

          {showActivity ? <ActivityRow state={state} /> : null}
        </div>
      </div>
      <ThreadTurnRail scrollRef={scrollRef} turns={turns} items={state.items} />
      {/* D-10b：「回到最新」底部中央渐隐浮层——锚定 viewport 不随内容滚动；
          key 随脉冲 tick 变化触发重挂载，重放浮现+脉冲动画。 */}
      {jumpLatest && !empty ? (
        <button
          key={jumpPulseTick}
          type="button"
          className={'ws-jumplatest' + (jumpPulse ? ' is-pulse' : '')}
          onClick={() => {
            const el = scrollRef.current;
            if (el) el.scrollTo({ top: el.scrollHeight, behavior: 'smooth' });
          }}
        >
          <Icon name="down" />
          回到最新
        </button>
      ) : null}
    </div>
  );
}
