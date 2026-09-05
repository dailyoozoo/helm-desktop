import {
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type ClipboardEvent,
  type KeyboardEvent,
  type CompositionEvent,
  type ReactNode,
} from 'react';
import { createPortal } from 'react-dom';
import { open } from '@tauri-apps/plugin-dialog';
import { Icon } from '../shell/icons';
import { showToast } from '../components/toast';
import { FullAccessConfirm } from '../components/FullAccessConfirm';
import type { Skill, SlashCommand } from '../extensions/extensionsApi';
import type { PermissionProfile, TurnMode } from '../engine/transport';
import type { SessionState } from '../engine/useSession';
import type { ReasoningEffort, ReasoningEffortCapability } from '@helm/protocol';
import { effortOptionsFor, reasoningEffortLabel } from '../reasoning';
import { savePastedImage, searchWorkspaceFiles } from './workspaceApi';
import {
  completeSlashCommand,
  filterSlashCommands,
  helmCommandAction,
  matchSlashCommand,
  resolveEnterAction,
} from './slashCommands';
import { CapCenterModal, type CapCenterKey } from './CapCenterModal';
import { ContextPill, contextPillLabel, type ContextPillItem } from './ContextPill';
import { ContextRing, type ContextRingDetail, type SessionContextEditActions } from './ContextRing';
import type { ContextSnapshotViewModel } from './contextSnapshotViewModel';

const useClientLayoutEffect = typeof window === 'undefined' ? useEffect : useLayoutEffect;

function sourceLabel(source: SlashCommand['source']) {
  if (source === 'extension') return '扩展中心';
  if (source === 'engine-project') return '项目';
  if (source === 'engine-user') return '引擎';
  return '内置';
}

/** 模式对应的输入框提示（变更-04 B.2：切了要有感觉） */
const MODE_PLACEHOLDER: Record<TurnMode, string> = {
  build: '让 Helm 构建或修改点什么…  Enter 发送 · / 唤起命令',
  plan: '描述你想让 Helm 先规划什么…（计划确认后才会执行）',
  ask: '就这个代码库提个问题…（只读，不会改动文件）',
};

interface QueuedMessage {
  text: string;
  attachments: string[];
}

/** 由普通路径数组构造 attachment 药丸（附加文件/目录、粘贴图片） */
export function attachmentPills(paths: string[], cwd?: string): ContextPillItem[] {
  return paths.map((path) => ({
    kind: 'attachment',
    path,
    label: contextPillLabel(path, 'attachment', cwd),
  }));
}

export function settleQueuedMessage(
  queue: QueuedMessage[],
  delivered: QueuedMessage,
  sent: boolean | void,
): { queue: QueuedMessage[]; paused: boolean } {
  if (sent === false) return { queue, paused: true };
  return {
    queue: queue[0] === delivered ? queue.slice(1) : queue.filter((item) => item !== delivered),
    paused: false,
  };
}

const HISTORY_LIMIT = 50;

/** 推理强度档位说明（原型 workspace.js EFFORT_DESC L945-948：Claude Code 与 Codex
 *  两个变体；档位集合来自真实 CLI 探测，未列出的档位回落通用文案）。 */
export function effortDescription(effort: ReasoningEffort, engine?: string): string {
  const codex = engine === 'codex';
  switch (effort) {
    case 'auto':
      return '使用当前模型的默认推理预算。';
    case 'low':
      return '更快返回，适合简单修改与明确指令。';
    case 'medium':
      return '在速度和分析深度之间保持平衡。';
    case 'high':
      return codex ? '增加分析预算，适合复杂编码任务。' : '增加分析预算，适合复杂重构和排障。';
    case 'xhigh':
      return codex
        ? '使用当前模型声明的最高推理档位。'
        : '投入更多预算，适合跨模块改造和疑难问题。';
    case 'max':
      return '使用当前引擎支持的最大推理预算。';
    default:
      return '下一轮生效。';
  }
}

/** 原型 .floatmenu 浮层（实现侧全局类 .home-floatmenu* 与其逐值同源）：fixed 定位、
 *  portal 挂 body、外点/Esc 关闭——对应原型 openMenuFloat 的 React 化。 */
function ComposerFloat({
  anchor,
  onClose,
  children,
}: {
  anchor: { left: number; top: number } | null;
  onClose: () => void;
  children: ReactNode;
}) {
  const ref = useRef<HTMLDivElement | null>(null);
  const [pos, setPos] = useState<{ left: number; top: number } | null>(null);
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el || !anchor) return;
    const width = el.offsetWidth;
    const height = el.offsetHeight;
    const left = Math.min(Math.max(8, anchor.left), window.innerWidth - width - 8);
    setPos({ left, top: Math.max(8, anchor.top - height - 8) });
  }, [anchor]);
  useEffect(() => {
    if (!anchor) return;
    const onPointerDown = (event: MouseEvent) => {
      if (ref.current?.contains(event.target as Node)) return;
      onClose();
    };
    const onKeyDown = (event: globalThis.KeyboardEvent) => {
      if (event.key === 'Escape') onClose();
    };
    document.addEventListener('mousedown', onPointerDown, true);
    document.addEventListener('keydown', onKeyDown, true);
    return () => {
      document.removeEventListener('mousedown', onPointerDown, true);
      document.removeEventListener('keydown', onKeyDown, true);
    };
  }, [anchor, onClose]);
  if (!anchor) return null;
  return createPortal(
    <div
      ref={ref}
      className="home-floatmenu home-floatmenu--fixed"
      role="menu"
      style={{
        position: 'fixed',
        bottom: 'auto',
        visibility: pos ? 'visible' : 'hidden',
        left: pos?.left ?? -9999,
        top: pos?.top ?? -9999,
      }}
    >
      {children}
    </div>,
    document.body,
  );
}

/** 浮层菜单项（原型 floatmenu button：图标 + 主行/副行 + 右侧 hint 角标）。 */
function FloatItem({
  icon,
  label,
  hint,
  desc,
  active,
  warn,
  danger,
  mono,
  disabled,
  onClick,
}: {
  icon: Parameters<typeof Icon>[0]['name'];
  label: string;
  hint?: string;
  desc?: string;
  active?: boolean;
  warn?: boolean;
  danger?: boolean;
  mono?: boolean;
  disabled?: boolean;
  onClick: () => void;
}) {
  const cls =
    'home-floatmenu__item' +
    (active ? ' is-active' : '') +
    (warn ? ' is-warn' : '') +
    (danger ? ' is-danger' : '');
  return (
    <button type="button" role="menuitem" className={cls} disabled={disabled} onClick={onClick}>
      <Icon name={icon} />
      <span className="home-floatmenu__copy">
        <span className={mono ? 'mono' : undefined}>{label}</span>
        {desc ? <small>{desc}</small> : null}
      </span>
      {hint ? <span className="home-floatmenu__hint">{hint}</span> : null}
    </button>
  );
}

export function Composer({
  working,
  holding = false,
  mode,
  engine,
  reasoningEffort = 'auto',
  cwd = '',
  slashCommands = [],
  skills = [],
  onModeChange,
  permissionProfile = 'standard',
  onPermissionProfileChange,
  onCommandAction,
  onSend,
  onStop,
  cost,
  sessionContextEdit,
  contextDetail,
  contextSnapshot,
  draftRequest,
  onDraftConsumed,
  model,
  modelOptions = [],
  modelProviderLabel,
  onSelectModel,
  reasoningCapability,
  reasoningLoading,
  reasoningDisabled,
  onSelectReasoningEffort,
}: {
  working: boolean;
  /** B 方案「引擎恢复中」：历史已先行渲染、句柄尚未到位。此时发送走「排队」而非
   *  直接发送（直接发会因无句柄误开新会话），句柄就绪（holding 转 false）后队列自动
   *   flush。对用户无感——输入框照常可打字可发送，只是消息先进队列。 */
  holding?: boolean;
  /** 会话模式（变更-04）：状态在 Workspace，Composer 受控展示 */
  mode: TurnMode;
  engine?: string;
  reasoningEffort?: ReasoningEffort;
  /** 工作目录（变更-12）：@文件引用在此目录下搜索 */
  cwd?: string;
  slashCommands?: SlashCommand[];
  skills?: Skill[];
  onModeChange: (mode: TurnMode) => void;
  permissionProfile?: PermissionProfile;
  onPermissionProfileChange?: (profile: PermissionProfile) => void;
  onCommandAction: (action: string) => void;
  onSend: (text: string, attachments: string[]) => void | Promise<boolean>;
  onStop: () => void;
  /** 批次③：模型偏好与推理强度迁入底栏（原 ThreadHead 职责，原型 #modelBtn/#effortBtn） */
  model?: string;
  modelOptions?: { id: string; contextWindow?: number }[];
  modelProviderLabel?: string;
  onSelectModel?: (model: string) => void;
  reasoningCapability?: ReasoningEffortCapability | null;
  reasoningLoading?: boolean;
  reasoningDisabled?: boolean;
  onSelectReasoningEffort?: (effort: ReasoningEffort) => void;
  cost?: SessionState['cost'];
  /** S3：会话上下文增删只从圆环 popover 进入（真实 list/add/remove_session_contexts）。 */
  sessionContextEdit?: SessionContextEditActions;
  /** 变更-34/35 · D4/E2：上下文圆环完整明细（占用/归因/计费/会话/文件）。 */
  contextDetail?: ContextRingDetail;
  /** 切片 D · P1-02：上下文圆环的统一快照（含分栏文件 / 有效 MCP / 归因）。 */
  contextSnapshot?: ContextSnapshotViewModel;
  /** 新任务页带入的草稿；只填入输入框与附件药丸，不绕过发送前置检查。 */
  draftRequest?: { id: number; text: string; attachments?: string[] } | null;
  onDraftConsumed?: () => void;
}) {
  const [text, setText] = useState('');
  // 批次③修订：底栏弹层对齐原型——模式/权限/模型/强度用 .floatmenu 浮层（实现侧由
  // 新任务页移植的同款 .home-floatmenu 全局类承载），「+」用 .cm-menu.workspace-cap-menu。
  const [openMenu, setOpenMenu] = useState<'cap' | 'mode' | 'profile' | 'model' | 'effort' | null>(
    null,
  );
  // 「+」能力菜单的居中搜索弹窗（原型 #workspaceCenter：文件与目录 / 命令与技能）
  const [capCenter, setCapCenter] = useState<CapCenterKey | null>(null);
  const [floatAnchor, setFloatAnchor] = useState<{ left: number; top: number } | null>(null);
  const [fullAccessPending, setFullAccessPending] = useState(false);
  const permissionDescription =
    permissionProfile === 'standard'
      ? '危险操作会询问'
      : permissionProfile === 'auto'
        ? '安全操作自动执行'
        : '跳过 Helm 审批';
  const [attachments, setAttachments] = useState<ContextPillItem[]>([]);
  const [highlight, setHighlight] = useState(0);
  const [dismissed, setDismissed] = useState(false);
  // 中文输入法组字中（变更-08）：组字期间的 Enter/↑↓/Esc 必须让给 IME，不触发发送/菜单
  const [composing, setComposing] = useState(false);
  // @文件引用（变更-12）
  const [mentionResults, setMentionResults] = useState<string[]>([]);
  const [mentionHighlight, setMentionHighlight] = useState(0);
  // 排队消息（变更-12）：轮次进行中发送 → 入队，本轮结束自动发出
  const [queue, setQueue] = useState<QueuedMessage[]>([]);
  const [queuePaused, setQueuePaused] = useState(false);
  // 恢复失败回滚要用「最新队列」但不想把 queue 塞进事件 effect 依赖（监听器会
  // 随每条排队消息重挂）——ref 镜像，与 useSession 的 disabledMcpRef 同构。
  const queueRef = useRef(queue);
  useEffect(() => {
    queueRef.current = queue;
  }, [queue]);
  const ref = useRef<HTMLTextAreaElement>(null);
  const onSendRef = useRef(onSend);
  const queueSendingRef = useRef(false);
  const menuRef = useRef<HTMLDivElement>(null);
  const activeItemRef = useRef<HTMLButtonElement>(null);
  // 输入历史（变更-12）：↑↓ 在空输入框时回溯已发送消息
  const historyRef = useRef<string[]>([]);
  const historyPosRef = useRef(-1);
  const draftRef = useRef('');
  const consumedDraftIdRef = useRef<number | null>(null);

  useEffect(() => {
    onSendRef.current = onSend;
  }, [onSend]);

  // B 方案「历史先行」失败回滚：恢复期间排队的消息不吞也不误发——句柄没等到、
  // 线程被丢弃（helm:resume-history-discard）时把文本与附件原样退回输入框，由用户决定重发。
  // 读 queueRef 而非在 setQueue updater 里做副作用——StrictMode 双调用会把退回文本翻倍。
  useEffect(() => {
    const onDiscard = () => {
      const queued = queueRef.current;
      if (queued.length === 0) return;
      const drained = queued.map((message) => message.text).join('\n\n');
      const restoredPaths = queued.flatMap((message) => message.attachments);
      setQueue([]);
      setText((existing) => (existing.trim() ? `${existing}\n\n${drained}` : drained));
      if (restoredPaths.length) {
        setAttachments((items) => {
          const restored = attachmentPills(restoredPaths, cwd).filter(
            (pill) => !items.some((it) => it.path === pill.path),
          );
          return [...items, ...restored];
        });
      }
      setQueuePaused(false);
      window.setTimeout(() => ref.current?.focus(), 0);
    };
    window.addEventListener('helm:resume-history-discard', onDiscard);
    return () => window.removeEventListener('helm:resume-history-discard', onDiscard);
  }, [cwd]);

  // 批次③修订：「+」能力菜单（cm-menu）外点/Esc 关闭；floatmenu 浮层自行处理。
  // 触发钮本身不在菜单内：外点先关、随后的 click 再按当前态翻转，与原型 openMenuFloat 一致。
  useEffect(() => {
    if (openMenu !== 'cap') return;
    const onPointerDown = (event: MouseEvent) => {
      if (!(event.target instanceof Element) || !event.target.closest('.workspace-cap-menu')) {
        setOpenMenu(null);
      }
    };
    const onKeyDown = (event: globalThis.KeyboardEvent) => {
      if (event.key === 'Escape') setOpenMenu(null);
    };
    document.addEventListener('mousedown', onPointerDown);
    document.addEventListener('keydown', onKeyDown);
    return () => {
      document.removeEventListener('mousedown', onPointerDown);
      document.removeEventListener('keydown', onKeyDown);
    };
  }, [openMenu]);

  // 原型 openChoice → Helm.menu：以触发钮 rect 定位的 fixed 浮层，悬于按钮上方 8px、
  // 视口内夹紧；经 portal 挂 body，不受 composer focus-within 等局部 stacking context 影响。
  const closeFloat = () => {
    setOpenMenu(null);
    setFloatAnchor(null);
  };
  const toggleFloat = (menu: 'mode' | 'profile' | 'model' | 'effort', el: HTMLElement) => {
    if (openMenu === menu) {
      closeFloat();
      return;
    }
    const rect = el.getBoundingClientRect();
    setFloatAnchor({ left: rect.left, top: rect.top });
    setOpenMenu(menu);
  };

  // 新任务页「发送」直达：draft 进场即自动发出首条消息（2026-08-28 用户决议，
  // 行为变更：原型 helm:draft 只预填等待确认，用户明确要求点一次发送就跑）。
  const [autoSendDraft, setAutoSendDraft] = useState(false);
  useEffect(() => {
    if (!draftRequest || consumedDraftIdRef.current === draftRequest.id) return;
    consumedDraftIdRef.current = draftRequest.id;
    setText(draftRequest.text);
    draftRef.current = draftRequest.text;
    // S2：新任务页选择的文件/目录随草稿成为首条消息的附件药丸
    if (draftRequest.attachments?.length) {
      setAttachments((current) => {
        const pills = attachmentPills(draftRequest.attachments ?? [], cwd);
        return Array.from(
          new Map([...current, ...pills].map((pill) => [pill.path, pill])).values(),
        );
      });
    }
    onDraftConsumed?.();
    if (draftRequest.text.trim()) setAutoSendDraft(true);
    window.setTimeout(() => ref.current?.focus(), 0);
  }, [draftRequest, onDraftConsumed, cwd]);

  // draft 文本与附件都就位后自动发送一次（工作区挂载初期 working=false，
  // 前置校验失败时 onSend 返回 false，消息留在输入框由用户修复后手动重发）。
  useEffect(() => {
    if (!autoSendDraft || working) return;
    if (!text.trim()) return;
    setAutoSendDraft(false);
    void submitOrQueue();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [autoSendDraft, working, text]);

  // textarea 自增高（上限 190px，与原型一致）。
  useClientLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    el.style.height = 'auto';
    el.style.height = `${Math.min(el.scrollHeight, 190)}px`;
  }, [text]);

  const rememberHistory = (value: string) => {
    const trimmed = value.trim();
    if (!trimmed) return;
    const next = historyRef.current.filter((item) => item !== trimmed);
    next.push(trimmed);
    historyRef.current = next.slice(-HISTORY_LIMIT);
    historyPosRef.current = -1;
  };

  const submitOrQueue = async () => {
    const t = text.trim();
    if (!t) return;
    rememberHistory(t);
    if (working || holding) {
      setQueue((current) => [...current, { text: t, attachments: attachments.map((a) => a.path) }]);
      setText('');
      setAttachments([]);
    } else {
      const sent = await onSendRef.current(
        t,
        attachments.map((a) => a.path),
      );
      if (sent === false) return;
      setText('');
      setAttachments([]);
    }
  };

  // 本轮结束后自动发出队首消息；手动 Stop 后由用户显式恢复，避免意外继续执行。
  // holding（引擎恢复中）期间同样压住队列，句柄到位转 false 后才放行 flush。
  useEffect(() => {
    if (working || holding || queuePaused || queue.length === 0 || queueSendingRef.current) return;
    queueSendingRef.current = true;
    const next = queue[0];
    void (async (): Promise<boolean | void> => onSendRef.current(next.text, next.attachments))()
      .then((sent) => {
        queueSendingRef.current = false;
        setQueue((current) => settleQueuedMessage(current, next, sent).queue);
        if (sent === false) {
          setQueuePaused(true);
          showToast('排队消息发送失败，已保留在队列中；修复后可再次发送', 'error');
        }
      })
      .catch((error: unknown) => {
        queueSendingRef.current = false;
        setQueuePaused(true);
        showToast(
          `排队消息发送失败，已保留在队列中：${error instanceof Error ? error.message : String(error)}`,
          'error',
        );
      });
  }, [working, holding, queue, queuePaused]);

  const handleStop = () => {
    if (queue.length) setQueuePaused(true);
    onStop();
  };

  const sendQueuedMessage = async (index: number) => {
    if (working || queueSendingRef.current) {
      showToast('当前轮次仍在运行，请先停止或等待结束', 'info');
      return;
    }
    const message = queue[index];
    if (!message) return;
    queueSendingRef.current = true;
    setQueue((current) => current.filter((_item, itemIndex) => itemIndex !== index));
    const sent = await onSendRef.current(message.text, message.attachments);
    queueSendingRef.current = false;
    if (sent === false) {
      setQueue((current) => {
        const next = [...current];
        next.splice(Math.min(index, next.length), 0, message);
        return next;
      });
      setQueuePaused(true);
      showToast('排队消息发送失败，已保留在队列中', 'error');
    }
  };

  const removeQueuedMessage = (index: number) => {
    const removed = queue[index];
    if (!removed) return;
    const next = queue.filter((_item, itemIndex) => itemIndex !== index);
    setQueue(next);
    if (next.length === 0) setQueuePaused(false);
    setText((current) => (current.trim() ? `${removed.text}\n\n${current}` : removed.text));
    setAttachments((current) => {
      const restored = attachmentPills(removed.attachments, cwd).filter(
        (pill) => !current.some((item) => item.path === pill.path),
      );
      return [...current, ...restored];
    });
    window.setTimeout(() => ref.current?.focus(), 0);
  };

  // 斜杠菜单只在输入第一个 token（尚未出现空白）时展开；Esc 关闭后继续输入命令字符会重新打开。
  const slashQuery = text.trimStart();
  const typingTrigger =
    engine === 'codex' ? /^(\/|\$)\S*$/.test(slashQuery) : /^\/\S*$/.test(slashQuery);
  const showSlashMenu = typingTrigger && !dismissed;
  const skillCommands: SlashCommand[] = skills
    .filter((skill) => skill.enabled && skill.engine === engine)
    .map((skill) => ({
      id: `__skill_${skill.id}`,
      trigger: skill.trigger,
      description: skill.description,
      scope: skill.scope,
      enabled: true,
      body: '',
      engine: skill.engine,
      source: 'engine-user',
      argumentHint: '技能参数（可选）',
    }));
  const triggerCommands = slashQuery.startsWith('$')
    ? skillCommands
    : [...slashCommands, ...(engine === 'claude-code' ? skillCommands : [])];
  const filteredSlashCommands = showSlashMenu
    ? filterSlashCommands(triggerCommands, slashQuery)
    : [];
  const activeIndex = Math.min(highlight, Math.max(filteredSlashCommands.length - 1, 0));
  // 首 token 已命中命令且在补参数阶段时，显示参数提示行。
  const activeCommand = !typingTrigger ? matchSlashCommand(triggerCommands, slashQuery) : undefined;
  // 首 token 形如命令但没有任何匹配：Enter 不应把未知命令原文发出去（变更-08）
  const unknownCommand =
    slashQuery.startsWith('/') &&
    typingTrigger &&
    slashQuery.length > 1 &&
    filteredSlashCommands.length === 0;

  // @文件引用（变更-12）：最后一个 token 以 @ 开头时弹文件联想（斜杠命令优先）
  const lastToken = text.split(/\s/).pop() ?? '';
  const mentionTyping = !typingTrigger && lastToken.startsWith('@') && Boolean(cwd);
  const mentionQuery = mentionTyping ? lastToken.slice(1) : '';
  const showMentionMenu = !working && mentionTyping && !dismissed;
  const mentionIndex = Math.min(mentionHighlight, Math.max(mentionResults.length - 1, 0));

  useEffect(() => {
    setHighlight(0);
  }, [slashQuery]);

  useEffect(() => {
    setMentionHighlight(0);
  }, [mentionQuery]);

  // @ 联想防抖搜索（150ms）；真实遍历工作目录，深度/数量后端限流
  useEffect(() => {
    if (!showMentionMenu) {
      setMentionResults([]);
      return;
    }
    let active = true;
    const timer = window.setTimeout(() => {
      searchWorkspaceFiles(cwd, mentionQuery)
        .then((files) => {
          if (active) setMentionResults(files);
        })
        .catch(() => {
          if (active) setMentionResults([]);
        });
    }, 150);
    return () => {
      active = false;
      window.clearTimeout(timer);
    };
  }, [showMentionMenu, cwd, mentionQuery]);

  useEffect(() => {
    // Esc 关闭后：文本变化（继续输入/删改命令字符）即重新打开菜单
    if (dismissed) setDismissed(false);
    // eslint-disable-next-line react-hooks/exhaustive-deps -- 只跟随文本变化重置
  }, [text]);

  // 键盘高亮项滚入可视区（菜单可滚动后必需）
  useEffect(() => {
    activeItemRef.current?.scrollIntoView({ block: 'nearest' });
  }, [activeIndex, mentionIndex, showSlashMenu, showMentionMenu]);

  const focusEnd = () => {
    requestAnimationFrame(() => {
      const el = ref.current;
      if (!el) return;
      el.focus();
      el.setSelectionRange(el.value.length, el.value.length);
    });
  };

  const chooseSlashCommand = (command: SlashCommand) => {
    const action = helmCommandAction(command);
    if (action) {
      setText('');
      setDismissed(true);
      onCommandAction(action);
      return;
    }
    setText(completeSlashCommand(command));
    focusEnd();
  };

  const chooseMention = (relativePath: string) => {
    // 变更-34 · D1：@提及选择后生成可移除药丸，路径不再残留进输入文本，
    // 发送时随 prompt 一并交出；绝对路径映射到 cwd。
    // 目录条目带尾斜杠（search_workspace_files 目录约定），映射绝对路径前去掉。
    const absolute = `${cwd.replace(/[\\/]+$/, '')}/${relativePath.replace(/\/+$/, '')}`;
    setText((current) => {
      const idx = current.lastIndexOf(lastToken);
      return idx >= 0 ? current.slice(0, idx) : current;
    });
    setAttachments((current) => {
      const pill: ContextPillItem = {
        kind: 'mention',
        path: absolute,
        label: contextPillLabel(absolute, 'mention', cwd),
      };
      const next = current.filter((item) => item.path !== absolute);
      return [...next, pill];
    });
    setMentionResults([]);
    focusEnd();
  };

  const recallHistory = (direction: 1 | -1): boolean => {
    const history = historyRef.current;
    if (!history.length) return false;
    let pos = historyPosRef.current;
    if (direction === 1) {
      // ↑ 回溯更早
      if (pos === -1) {
        draftRef.current = text;
        pos = history.length - 1;
      } else if (pos > 0) {
        pos -= 1;
      } else {
        return true; // 已到最早，保持
      }
    } else {
      // ↓ 回到更新/草稿
      if (pos === -1) return false;
      if (pos < history.length - 1) {
        pos += 1;
      } else {
        historyPosRef.current = -1;
        setText(draftRef.current);
        focusEnd();
        return true;
      }
    }
    historyPosRef.current = pos;
    setText(history[pos]);
    focusEnd();
    return true;
  };

  const onCompositionStart = (_e: CompositionEvent<HTMLTextAreaElement>) => setComposing(true);
  const onCompositionEnd = (_e: CompositionEvent<HTMLTextAreaElement>) => setComposing(false);

  const onKeyDown = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    // IME 防护（变更-08）：组字期间 Enter=确认候选词、↑↓=选词、Esc=取消组字，
    // 全部让给输入法。isComposing 与 keyCode 229 双保险（WebView2/Chromium）。
    if (composing || e.nativeEvent.isComposing || e.keyCode === 229) return;
    if (showSlashMenu) {
      if (filteredSlashCommands.length) {
        if (e.key === 'ArrowDown') {
          e.preventDefault();
          setHighlight((activeIndex + 1) % filteredSlashCommands.length);
          return;
        }
        if (e.key === 'ArrowUp') {
          e.preventDefault();
          setHighlight(
            (activeIndex - 1 + filteredSlashCommands.length) % filteredSlashCommands.length,
          );
          return;
        }
        if (e.key === 'Tab' || e.key === 'Enter') {
          e.preventDefault();
          chooseSlashCommand(filteredSlashCommands[activeIndex]);
          return;
        }
      } else if (e.key === 'Tab') {
        // 空匹配态不让 Tab 把焦点甩出输入框
        e.preventDefault();
        return;
      }
      if (e.key === 'Escape') {
        e.preventDefault();
        setDismissed(true);
        return;
      }
    }
    if (showMentionMenu && mentionResults.length) {
      if (e.key === 'ArrowDown') {
        e.preventDefault();
        setMentionHighlight((mentionIndex + 1) % mentionResults.length);
        return;
      }
      if (e.key === 'ArrowUp') {
        e.preventDefault();
        setMentionHighlight((mentionIndex - 1 + mentionResults.length) % mentionResults.length);
        return;
      }
      if (e.key === 'Tab' || e.key === 'Enter') {
        e.preventDefault();
        chooseMention(mentionResults[mentionIndex]);
        return;
      }
      if (e.key === 'Escape') {
        e.preventDefault();
        setDismissed(true);
        return;
      }
    }
    // 输入历史（变更-12）：空输入框（或正处于回溯态）时 ↑↓ 翻已发送消息
    if (e.key === 'ArrowUp' && (text === '' || historyPosRef.current !== -1)) {
      if (recallHistory(1)) {
        e.preventDefault();
        return;
      }
    }
    if (e.key === 'ArrowDown' && historyPosRef.current !== -1) {
      if (recallHistory(-1)) {
        e.preventDefault();
        return;
      }
    }
    if (e.key === 'Enter') {
      const action = resolveEnterAction({
        shiftKey: e.shiftKey,
        isComposing: composing || e.nativeEvent.isComposing,
        // holding（引擎恢复中）与 working 一样走排队路径：Enter 不直接发送，
        // 消息入队，句柄就绪后自动 flush——用户无感。
        working: working || holding,
        menuOpen: showSlashMenu,
        hasMenuMatches: filteredSlashCommands.length > 0,
        unknownCommand,
      });
      // 'newline' 交给 textarea 默认行为（不 preventDefault）
      if (action === 'ime' || action === 'newline') return;
      e.preventDefault();
      if (action === 'pick') {
        chooseSlashCommand(filteredSlashCommands[activeIndex]);
      } else if (action === 'send' || action === 'queue') {
        submitOrQueue();
      }
      // 'block'：未知命令，什么都不做，保留输入待修改
    }
  };

  // 图片粘贴（变更-12）：剪贴板图片落成附件文件，随消息注入 CLI prompt
  const onPaste = async (e: ClipboardEvent<HTMLTextAreaElement>) => {
    const images = Array.from(e.clipboardData?.items ?? []).filter((item) =>
      item.type.startsWith('image/'),
    );
    if (!images.length) return;
    e.preventDefault();
    for (const item of images) {
      const file = item.getAsFile();
      if (!file) continue;
      try {
        const bytes = new Uint8Array(await file.arrayBuffer());
        const extension = (item.type.split('/')[1] ?? 'png').toLowerCase();
        const path = await savePastedImage(bytes, extension);
        addAttachments([path]);
      } catch (err) {
        showToast(`图片粘贴失败：${err instanceof Error ? err.message : String(err)}`, 'error');
      }
    }
  };

  const addAttachments = (paths: string[]) => {
    const pills = attachmentPills(paths, cwd);
    setAttachments((current) =>
      Array.from(new Map([...current, ...pills].map((p) => [p.path, p])).values()),
    );
    requestAnimationFrame(() => ref.current?.focus());
  };

  const normalizeSelection = (selection: string | string[] | null) => {
    if (!selection) return [];
    return (Array.isArray(selection) ? selection : [selection])
      .map((path) => path.trim())
      .filter(Boolean);
  };

  const attachFiles = async () => {
    try {
      const selected = await open({ multiple: true, directory: false });
      addAttachments(normalizeSelection(selected));
    } catch {
      // 浏览器预览没有 Tauri dialog；Tauri 环境下取消选择会返回 null。
    }
  };

  const removeAttachment = (path: string) => {
    setAttachments((current) => current.filter((item) => item.path !== path));
  };

  return (
    <div className="composer">
      <div className="composer__inner">
        <div className="composer__box">
          {attachments.length ? (
            <div className="cpills" aria-label="已挂载上下文">
              {attachments.map((item) => (
                <ContextPill
                  key={item.path}
                  item={item}
                  disabled={working}
                  onRemove={removeAttachment}
                />
              ))}
            </div>
          ) : null}
          <textarea
            ref={ref}
            id="composer-input"
            rows={1}
            value={text}
            placeholder={
              working ? '继续输入，Enter 加入队列（本轮结束后自动发送）' : MODE_PLACEHOLDER[mode]
            }
            aria-label={working ? '消息输入框（运行中，Enter 排队）' : '消息输入框'}
            aria-expanded={showSlashMenu || showMentionMenu}
            aria-haspopup="listbox"
            onChange={(e) => setText(e.target.value)}
            onKeyDown={onKeyDown}
            onPaste={(e) => void onPaste(e)}
            onCompositionStart={onCompositionStart}
            onCompositionEnd={onCompositionEnd}
            onBlur={(e) => {
              // 失焦收起菜单；点击菜单项时焦点落在菜单内，不收起（否则 click 丢失）
              if (menuRef.current?.contains(e.relatedTarget as Node)) return;
              setDismissed(true);
            }}
          />
          {showSlashMenu ? (
            <div className="slash open" ref={menuRef} role="listbox" aria-label="斜杠命令">
              {filteredSlashCommands.length ? (
                filteredSlashCommands.map((command, index) => (
                  <button
                    key={command.id}
                    type="button"
                    role="option"
                    aria-selected={index === activeIndex}
                    ref={index === activeIndex ? activeItemRef : undefined}
                    className={'slash__i' + (index === activeIndex ? ' is-active' : '')}
                    onMouseEnter={() => setHighlight(index)}
                    onClick={() => chooseSlashCommand(command)}
                  >
                    <b>{command.trigger}</b>
                    {command.argumentHint ? (
                      <span className="slash__hint">{command.argumentHint}</span>
                    ) : null}
                    <span className="slash__desc">{command.description}</span>
                    <span className="pill">
                      {command.id.startsWith('__skill_')
                        ? '技能'
                        : command.id.startsWith('__helm_')
                          ? 'Helm'
                          : sourceLabel(command.source)}
                    </span>
                  </button>
                ))
              ) : (
                <div className="slash__empty">没有匹配的斜杠命令（Enter 不会发送，Esc 关闭）</div>
              )}
            </div>
          ) : null}
          {showMentionMenu ? (
            <div className="slash open" ref={menuRef} role="listbox" aria-label="文件引用">
              {mentionResults.length ? (
                mentionResults.map((file, index) => (
                  <button
                    key={file}
                    type="button"
                    role="option"
                    aria-selected={index === mentionIndex}
                    ref={index === mentionIndex ? activeItemRef : undefined}
                    className={'slash__i' + (index === mentionIndex ? ' is-active' : '')}
                    onMouseEnter={() => setMentionHighlight(index)}
                    onClick={() => chooseMention(file)}
                  >
                    <Icon
                      name={file.endsWith('/') ? 'folder' : 'file'}
                      className="h-3.5 w-3.5"
                      style={{ width: 13, height: 13, flex: 'none' }}
                    />
                    <span className="slash__desc mono">{file}</span>
                  </button>
                ))
              ) : (
                <div className="slash__empty">
                  {cwd ? '没有匹配的文件（在工作目录下搜索）' : '未设置工作目录，无法引用文件'}
                </div>
              )}
            </div>
          ) : null}
          {activeCommand ? (
            <div className="composer__cmdhint faint" aria-label="命令参数提示">
              <b className="mono">{activeCommand.trigger}</b>
              <span className="mono">
                {activeCommand.argumentHint || activeCommand.description}
              </span>
            </div>
          ) : null}
          {queue.length ? (
            <div className="qrow" aria-label="排队中的消息">
              {queue.map((message, index) => (
                <span
                  className={'qchip' + (queuePaused ? ' is-held' : '')}
                  key={`${index}-${message.text}`}
                >
                  <span className="tag">{queuePaused ? '已暂停' : `排队 #${index + 1}`}</span>
                  <span className="txt" title={message.text}>
                    {message.text.slice(0, 48)}
                    {message.text.length > 48 ? '…' : ''}
                  </span>
                  <button
                    type="button"
                    title="移回输入框"
                    aria-label={`将排队消息 ${index + 1} 移回输入框`}
                    onClick={() => removeQueuedMessage(index)}
                  >
                    <Icon name="x" />
                  </button>
                  {queuePaused ? (
                    <button
                      type="button"
                      title="立即发送这条消息"
                      aria-label={`立即发送排队消息 ${index + 1}`}
                      disabled={working}
                      onClick={() => void sendQueuedMessage(index)}
                    >
                      <Icon name="play" />
                    </button>
                  ) : null}
                </span>
              ))}
            </div>
          ) : null}
          {/* 原型 #workspaceCapMenu：cm-menu 能力菜单，锚在输入框内左下、hidden 属性切换 */}
          <div className="cm-menu workspace-cap-menu" role="menu" hidden={openMenu !== 'cap'}>
            <button
              type="button"
              className="cm-menu__item"
              role="menuitem"
              onClick={() => {
                setOpenMenu(null);
                setCapCenter('files');
              }}
            >
              <Icon name="folderopen" />
              <span>文件与目录</span>
              <small>@</small>
            </button>
            <button
              type="button"
              className="cm-menu__item"
              role="menuitem"
              onClick={() => {
                setOpenMenu(null);
                setCapCenter('commands');
              }}
            >
              <Icon name="terminal" />
              <span>命令与技能</span>
              <small>/</small>
            </button>
            <button
              type="button"
              className="cm-menu__item"
              role="menuitem"
              onClick={() => {
                setOpenMenu(null);
              }}
            >
              <Icon name="plug" />
              <span>连接器</span>
            </button>
          </div>
          <div className="composer__bar">
            {/* 原型 #attachBtn：一个「+」弹能力菜单 */}
            <button
              type="button"
              className="cm-tool"
              title="添加上下文与工具"
              aria-label="添加上下文与工具"
              aria-haspopup="menu"
              aria-expanded={openMenu === 'cap'}
              disabled={working}
              onClick={() => setOpenMenu(openMenu === 'cap' ? null : 'cap')}
            >
              <Icon name="plus" />
            </button>
            {/* 原型 #modeBtn：模式（构建⌄）→ floatmenu */}
            <button
              type="button"
              className="cm-tool"
              title="任务模式：构建可写文件执行命令；计划先出方案再执行；询问只读"
              aria-haspopup="menu"
              aria-expanded={openMenu === 'mode'}
              disabled={working}
              onClick={(event) => toggleFloat('mode', event.currentTarget)}
            >
              <Icon name="layers" />
              <span className="val">
                {mode === 'build' ? '构建' : mode === 'plan' ? '计划' : '询问'}
              </span>
              <Icon name="down" className="chev" />
            </button>
            {/* 原型 #profBtn：权限档位（标准⌄）→ floatmenu */}
            <button
              type="button"
              className={`cm-tool${permissionProfile === 'full_access' ? ' cm-tool--danger' : ''}`}
              title={`权限档位：${permissionDescription}；只影响当前任务，不改全局规则`}
              aria-haspopup="menu"
              aria-expanded={openMenu === 'profile'}
              disabled={working}
              onClick={(event) => toggleFloat('profile', event.currentTarget)}
            >
              <Icon name={permissionProfile === 'full_access' ? 'eyeoff' : 'shield'} />
              <span className="val">
                {permissionProfile === 'standard'
                  ? '标准'
                  : permissionProfile === 'auto'
                    ? '自动执行'
                    : '全部放开'}
              </span>
              <Icon name="down" className="chev" />
            </button>
            <div className="sp" />
            {/* 原型 #modelBtn：模型偏好（sparkles + mono 模型名 + ⌄）→ floatmenu */}
            <button
              type="button"
              className="cm-tool cm-tool--model"
              title="下一轮模型偏好；运行中不可切换"
              aria-haspopup="menu"
              aria-expanded={openMenu === 'model'}
              disabled={working}
              onClick={(event) => toggleFloat('model', event.currentTarget)}
            >
              <Icon name="sparkles" className="tool-ic tool-ic--accent" />
              <span className="mono" title={model}>
                {model}
              </span>
              <Icon name="down" className="chev" />
            </button>
            {/* 原型 #effortBtn：推理强度（自动⌄）→ floatmenu */}
            <button
              type="button"
              className="cm-tool"
              title="推理强度：独立于模型，档位只按当前 Engine 的真实支持范围提供；运行中锁定，下一轮生效"
              aria-haspopup="menu"
              aria-expanded={openMenu === 'effort'}
              disabled={working || reasoningDisabled}
              onClick={(event) => toggleFloat('effort', event.currentTarget)}
            >
              <Icon name="gauge" className="tool-ic" />
              <span className="val">{reasoningEffortLabel(reasoningEffort)}</span>
              <Icon name="down" className="chev" />
            </button>
            <div className="composer__session-status" aria-label="当前会话状态">
              {cost?.contextTokens != null || contextDetail ? (
                <ContextRing
                  detail={
                    contextDetail
                      ? { ...contextDetail, cost: contextDetail.cost ?? cost }
                      : cost
                        ? { cost }
                        : undefined
                  }
                  snapshot={contextSnapshot}
                  sessionContextEdit={sessionContextEdit}
                />
              ) : null}
            </div>
            {working ? (
              <button
                type="button"
                className="cm-tool cm-tool--danger"
                onClick={handleStop}
                title="停止当前轮次 · Ctrl+Shift+."
                aria-label="停止当前轮次"
              >
                <Icon name="stop" className="tool-ic" /> 停止
              </button>
            ) : (
              <button
                type="button"
                className="cm-tool cm-tool--send"
                onClick={() => void submitOrQueue()}
                disabled={!text.trim()}
                title="发送 · Enter"
                aria-label="发送"
              >
                <Icon name="send" />
              </button>
            )}
          </div>

          {/* 原型 openChoice 浮层：模式/权限/模型/强度共用一个 .floatmenu（body 级 fixed） */}
          <ComposerFloat anchor={openMenu === 'mode' ? floatAnchor : null} onClose={closeFloat}>
            {(
              [
                {
                  mode: 'build' as const,
                  icon: 'layers' as const,
                  label: '构建',
                  hint: '可执行',
                  desc: '可写文件、可执行命令；Runtime 询问时显示审批。',
                },
                {
                  mode: 'plan' as const,
                  icon: 'flag' as const,
                  label: '计划',
                  hint: '只规划',
                  desc: '先产出实施方案，确认后再执行。',
                },
                {
                  mode: 'ask' as const,
                  icon: 'helpcircle' as const,
                  label: '询问',
                  hint: '只读',
                  desc: '只读，不写文件、不执行写目标命令。',
                },
              ] as const
            ).map((item) => (
              <FloatItem
                key={item.mode}
                icon={item.icon}
                label={item.label}
                hint={item.hint}
                desc={item.desc}
                active={mode === item.mode}
                disabled={working}
                onClick={() => {
                  closeFloat();
                  onModeChange(item.mode);
                }}
              />
            ))}
          </ComposerFloat>
          <ComposerFloat anchor={openMenu === 'profile' ? floatAnchor : null} onClose={closeFloat}>
            {(
              [
                {
                  value: 'standard' as const,
                  icon: 'shield' as const,
                  label: '标准',
                  hint: '推荐',
                  desc: '读取直通，Runtime 询问时再审批。',
                },
                {
                  value: 'auto' as const,
                  icon: 'zap' as const,
                  label: '自动执行',
                  hint: '谨慎使用',
                  desc: '额外直通安全网络读取，减少打断。',
                },
                {
                  value: 'full_access' as const,
                  icon: 'alert' as const,
                  label: '全部放开',
                  hint: '高风险',
                  desc: '跳过审批 · 仅本任务，应用重启后失效。',
                },
              ] as const
            ).map((item) => (
              <FloatItem
                key={item.value}
                icon={item.value === 'full_access' ? 'eyeoff' : item.icon}
                label={item.label}
                hint={item.hint}
                desc={item.desc}
                active={permissionProfile === item.value}
                warn={item.value === 'auto'}
                danger={item.value === 'full_access'}
                disabled={working}
                onClick={() => {
                  closeFloat();
                  if (item.value === 'full_access') {
                    setFullAccessPending(true);
                    return;
                  }
                  onPermissionProfileChange?.(item.value as PermissionProfile);
                }}
              />
            ))}
          </ComposerFloat>
          <ComposerFloat anchor={openMenu === 'model' ? floatAnchor : null} onClose={closeFloat}>
            {modelOptions.length ? (
              modelOptions.map((item) => (
                <FloatItem
                  key={item.id}
                  icon="sparkles"
                  label={item.id}
                  mono
                  hint={
                    item.contextWindow ? `${Math.round(item.contextWindow / 1000)}K` : undefined
                  }
                  active={item.id === model}
                  disabled={working}
                  onClick={() => {
                    closeFloat();
                    onSelectModel?.(item.id);
                  }}
                />
              ))
            ) : (
              <div className="home-floatmenu__hint" style={{ padding: '7px 10px' }}>
                当前引擎没有可用模型（{modelProviderLabel ?? '未绑定服务商'}）
              </div>
            )}
            <div className="home-floatmenu__sep" />
            <FloatItem
              icon="plug"
              label="更改服务商绑定…"
              desc="AI 配置 → 执行引擎 · 下一次发送生效"
              onClick={() => {
                closeFloat();
                window.dispatchEvent(
                  new CustomEvent('helm:navigate', { detail: { page: 'providers' } }),
                );
              }}
            />
          </ComposerFloat>
          <ComposerFloat anchor={openMenu === 'effort' ? floatAnchor : null} onClose={closeFloat}>
            {reasoningLoading ? (
              <div className="home-floatmenu__hint" style={{ padding: '7px 10px' }}>
                正在读取 CLI 模型能力…
              </div>
            ) : (
              effortOptionsFor(reasoningCapability, engine).map((effort) => (
                <FloatItem
                  key={effort}
                  icon="gauge"
                  label={reasoningEffortLabel(effort)}
                  hint={effort === 'auto' ? '模型默认' : undefined}
                  desc={effortDescription(effort, engine)}
                  active={effort === reasoningEffort}
                  disabled={reasoningDisabled}
                  onClick={() => {
                    closeFloat();
                    onSelectReasoningEffort?.(effort);
                  }}
                />
              ))
            )}
            {!reasoningLoading && reasoningCapability?.support === 'unsupported' ? (
              <div className="home-floatmenu__hint" style={{ padding: '7px 10px' }}>
                当前 CLI 未声明可调档位，仅提供自动
              </div>
            ) : null}
          </ComposerFloat>

          {/* 原型 #workspaceCenter：能力菜单的居中搜索弹窗（文件与目录 / 命令与技能） */}
          <CapCenterModal
            cap={capCenter}
            cwd={cwd}
            commands={triggerCommands}
            onClose={() => setCapCenter(null)}
            onPickContext={(absolutePath) => {
              setCapCenter(null);
              addAttachments([absolutePath]);
            }}
            onPickCommand={(label) => {
              setCapCenter(null);
              setText((current) => {
                const spacer = current && !/\s$/.test(current) ? ' ' : '';
                return current + spacer + label + ' ';
              });
              window.setTimeout(() => ref.current?.focus(), 0);
            }}
            onPickNativeFile={() => {
              setCapCenter(null);
              void attachFiles();
            }}
          />
        </div>
        {/* 批次③：快捷键提示行撤除（原型无此行，占位符已含提示） */}
      </div>
      {fullAccessPending ? (
        <FullAccessConfirm
          titleId="workspaceFullAccessTitle"
          onCancel={() => setFullAccessPending(false)}
          onConfirm={() => {
            setFullAccessPending(false);
            onPermissionProfileChange?.('full_access');
          }}
        />
      ) : null}
    </div>
  );
}
