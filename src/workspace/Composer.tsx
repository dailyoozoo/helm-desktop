import {
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type ClipboardEvent,
  type KeyboardEvent,
  type CompositionEvent,
} from 'react';
import { open } from '@tauri-apps/plugin-dialog';
import { Icon } from '../shell/icons';
import { showToast } from '../components/toast';
import type { Skill, SlashCommand } from '../extensions/extensionsApi';
import type { PermissionProfile, TurnMode } from '../engine/transport';
import type { SessionState } from '../engine/useSession';
import type { ReasoningEffort } from '@helm/protocol';
import { reasoningEffortLabel } from '../reasoning';
import { savePastedImage, searchWorkspaceFiles } from './workspaceApi';
import {
  completeSlashCommand,
  estimateTokens,
  filterSlashCommands,
  helmCommandAction,
  matchSlashCommand,
  resolveEnterAction,
} from './slashCommands';

const useClientLayoutEffect = typeof window === 'undefined' ? useEffect : useLayoutEffect;

function sourceLabel(source: SlashCommand['source']) {
  if (source === 'extension') return '扩展中心';
  if (source === 'engine-project') return '项目';
  if (source === 'engine-user') return '引擎';
  return '内置';
}

/** 模式对应的输入框提示（变更-04 B.2：切了要有感觉） */
const MODE_PLACEHOLDER: Record<TurnMode, string> = {
  build: '让 Helm 构建或修改点什么…  Enter 发送 · / 命令 · @ 文件',
  plan: '描述要做的事，本轮只调研并产出实施计划，不会改动文件…',
  ask: '问点什么…（询问模式只读，不会修改文件）',
};

interface QueuedMessage {
  text: string;
  attachments: string[];
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

export function Composer({
  working,
  mode,
  engine,
  model = '',
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
  onOpenContext,
}: {
  working: boolean;
  /** 会话模式（变更-04）：状态在 Workspace，Composer 受控展示 */
  mode: TurnMode;
  engine?: string;
  model?: string;
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
  cost?: SessionState['cost'];
  onOpenContext?: () => void;
}) {
  const [text, setText] = useState('');
  const permissionDescription =
    permissionProfile === 'standard'
      ? '危险操作会询问'
      : permissionProfile === 'auto'
        ? '安全操作自动执行'
        : '跳过 Helm 审批';
  const contextRatio =
    cost?.contextTokens != null && cost.contextWindow
      ? Math.min(1, cost.contextTokens / cost.contextWindow)
      : undefined;
  const [attachments, setAttachments] = useState<string[]>([]);
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
  const ref = useRef<HTMLTextAreaElement>(null);
  const onSendRef = useRef(onSend);
  const queueSendingRef = useRef(false);
  const menuRef = useRef<HTMLDivElement>(null);
  const activeItemRef = useRef<HTMLButtonElement>(null);
  // 输入历史（变更-12）：↑↓ 在空输入框时回溯已发送消息
  const historyRef = useRef<string[]>([]);
  const historyPosRef = useRef(-1);
  const draftRef = useRef('');

  useEffect(() => {
    onSendRef.current = onSend;
  }, [onSend]);

  const modeTitle = (target: TurnMode) => {
    if (working) return '轮次进行中，结束后可切换模式';
    if (target === 'plan' && engine === 'codex') {
      return 'Codex 无原生计划模式，以只读沙箱 + 计划指令近似';
    }
    if (target === 'plan') return '本轮只调研并产出实施计划，不改动文件';
    if (target === 'ask') return '只读问答：不修改文件、不执行写命令';
    if (target === 'build' && engine === 'codex') {
      return 'Codex 构建模式由 Codex Runtime 管理权限；仅显式启用 Helm 保护路径时才要求强制验证';
    }
    return '正常执行（按权限设置审批）';
  };

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
    if (working) {
      setQueue((current) => [...current, { text: t, attachments }]);
      setText('');
      setAttachments([]);
    } else {
      const sent = await onSendRef.current(t, attachments);
      if (sent === false) return;
      setText('');
      setAttachments([]);
    }
  };

  // 本轮结束后自动发出队首消息；手动 Stop 后由用户显式恢复，避免意外继续执行。
  useEffect(() => {
    if (working || queuePaused || queue.length === 0 || queueSendingRef.current) return;
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
  }, [working, queue, queuePaused]);

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
    setAttachments((current) => Array.from(new Set([...removed.attachments, ...current])));
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
    setText((current) => {
      const idx = current.lastIndexOf(lastToken);
      const head = idx >= 0 ? current.slice(0, idx) : current;
      return `${head}@${relativePath} `;
    });
    const absolute = `${cwd.replace(/[\\/]+$/, '')}/${relativePath}`;
    setAttachments((current) => Array.from(new Set([...current, absolute])));
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
        working,
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

  // token 估算（变更-08）：CJK 按 ~1.6 字符/token、其余按 ~4 字符/token 粗估，
  // 仅供参考、非后端真实值。
  const tok = estimateTokens(text);

  const addAttachments = (paths: string[]) => {
    setAttachments((current) => Array.from(new Set([...current, ...paths])));
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

  const attachDirectories = async () => {
    try {
      const selected = await open({ multiple: true, directory: true });
      addAttachments(normalizeSelection(selected));
    } catch {
      // 浏览器预览没有 Tauri dialog；Tauri 环境下取消选择会返回 null。
    }
  };

  const removeAttachment = (path: string) => {
    setAttachments((current) => current.filter((item) => item !== path));
  };

  const attachmentLabel = (path: string) => path.split(/[\\/]/).filter(Boolean).pop() ?? path;

  return (
    <div className="composer">
      <div className="composer__inner">
        <div className="seg" style={{ marginBottom: 9 }}>
          <button
            className={mode === 'build' ? 'is-active' : ''}
            disabled={working}
            title={modeTitle('build')}
            onClick={() => onModeChange('build')}
          >
            构建
          </button>
          <button
            className={mode === 'plan' ? 'is-active' : ''}
            disabled={working}
            title={modeTitle('plan')}
            onClick={() => onModeChange('plan')}
          >
            计划
          </button>
          <button
            className={mode === 'ask' ? 'is-active' : ''}
            disabled={working}
            title={modeTitle('ask')}
            onClick={() => onModeChange('ask')}
          >
            询问
          </button>
          <label
            className={`ws-permission-profile${permissionProfile === 'full_access' ? ' is-danger' : ''}`}
            title={`当前会话权限档位：${permissionDescription}`}
          >
            <Icon name={permissionProfile === 'full_access' ? 'eyeoff' : 'shield'} />
            <select
              value={permissionProfile}
              disabled={working}
              aria-label="权限档位"
              onChange={(event) =>
                onPermissionProfileChange?.(event.target.value as PermissionProfile)
              }
            >
              <option value="standard">标准</option>
              <option value="auto">自动执行</option>
              <option value="full_access">全部放开</option>
            </select>
            <span className="ws-permission-profile__hint">{permissionDescription}</span>
          </label>
        </div>
        <div className="composer__box">
          <textarea
            ref={ref}
            rows={1}
            value={text}
            placeholder={MODE_PLACEHOLDER[mode]}
            aria-label="消息输入框"
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
                    <Icon name="file" style={{ width: 13, height: 13, flex: 'none' }} />
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
            <div className="composer__attachments" aria-label="排队中的消息">
              {queue.map((message, index) => (
                <span className="composer__attachment ws-queued" key={`${index}-${message.text}`}>
                  <Icon name="clock" />
                  <span title={message.text}>
                    {queuePaused ? '已暂停' : `排队 ${index + 1}`}：{message.text.slice(0, 24)}
                    {message.text.length > 24 ? '…' : ''}
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
          {attachments.length ? (
            <div className="composer__attachments" aria-label="已挂载上下文">
              {attachments.map((path) => (
                <span className="composer__attachment" key={path} title={path}>
                  <Icon name="file" />
                  <span>{attachmentLabel(path)}</span>
                  <button
                    type="button"
                    title="移除"
                    aria-label={`移除 ${path}`}
                    onClick={() => removeAttachment(path)}
                  >
                    <Icon name="x" />
                  </button>
                </span>
              ))}
            </div>
          ) : null}
          <div className="composer__bar">
            <button
              type="button"
              className="btn-icon sm"
              title="斜杠命令（也可直接输入 /）"
              disabled={working}
              onClick={() => {
                setText((current) => (current.trimStart().startsWith('/') ? current : '/'));
                setDismissed(false);
                focusEnd();
              }}
            >
              <Icon name="terminal" />
            </button>
            <button
              type="button"
              className="btn-icon sm"
              title="扩展中心（技能 / MCP / 子代理 / 斜杠命令）"
              onClick={() =>
                window.dispatchEvent(
                  new CustomEvent('helm:navigate', { detail: { page: 'extensions' } }),
                )
              }
            >
              <Icon name="puzzle" />
            </button>
            <button
              type="button"
              className="btn-icon sm"
              title="附加文件"
              disabled={working}
              onClick={attachFiles}
            >
              <Icon name="clip" />
            </button>
            <button
              type="button"
              className="btn-icon sm"
              title="将文件夹加入上下文"
              disabled={working}
              onClick={attachDirectories}
            >
              <Icon name="folderopen" />
            </button>
            <div className="sp" />
            <div className="composer__session-status" aria-label="当前会话状态">
              <span title={`模型 ${model || '未选择'}`}>模型 {model || '未选择'}</span>
              <span title={`推理强度 ${reasoningEffortLabel(reasoningEffort)}`}>
                强度 {reasoningEffortLabel(reasoningEffort)}
              </span>
              {cost?.contextTokens != null ? (
                <button
                  type="button"
                  className={`composer__context-status${contextRatio != null && contextRatio >= 0.95 ? ' is-danger' : contextRatio != null && contextRatio >= 0.8 ? ' is-warning' : ''}`}
                  title="当前上下文占用，点击查看明细"
                  aria-label={`当前上下文占用${contextRatio == null ? '' : ` ${Math.round(contextRatio * 100)}%`}，点击查看明细`}
                  onClick={onOpenContext}
                >
                  上下文{' '}
                  {contextRatio == null
                    ? `${Math.round(cost.contextTokens / 1000)}K`
                    : `${Math.round(contextRatio * 100)}% · ${Math.round(cost.contextTokens / 1000)}K/${Math.round((cost.contextWindow ?? 0) / 1000)}K`}
                </button>
              ) : (
                <span>上下文 暂无</span>
              )}
              <span>花费 {cost ? `$${cost.costUsd.toFixed(4)}` : '暂无'}</span>
            </div>
            <span className="mono faint" style={{ fontSize: 11 }} title="客户端粗估，非真实计费值">
              本条约 {tok} token
            </span>
            {working ? (
              <button
                type="button"
                className="btn btn--danger btn--sm"
                onClick={handleStop}
                title="停止当前轮次 · Ctrl+Shift+."
              >
                <Icon name="stop" style={{ width: 14, height: 14 }} /> 停止
              </button>
            ) : null}
            <button
              type="button"
              className="btn btn--primary btn--sm"
              onClick={() => void submitOrQueue()}
              disabled={!text.trim()}
              title={working ? '排队发送（当前轮结束后自动发出）' : '发送'}
            >
              <Icon name={working ? 'clock' : 'send'} style={{ width: 14, height: 14 }} />
              {working ? '排队' : '发送'}
            </button>
          </div>
        </div>
        <div className="composer__hint">
          <span>
            <span className="kbd">Enter</span> 发送
          </span>
          <span>
            <span className="kbd">Shift+Enter</span> 换行
          </span>
          <span>
            <span className="kbd">/</span> 命令
          </span>
          <span>
            <span className="kbd">@</span> 文件
          </span>
        </div>
      </div>
    </div>
  );
}
