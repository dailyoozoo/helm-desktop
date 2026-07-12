import { useCallback, useEffect, useMemo, useRef, useState, useSyncExternalStore } from 'react';
import { listen as tauriListen } from '@tauri-apps/api/event';
import {
  dropLiveSession,
  lastOpenWorkspaceSession,
  livePendingApprovalSessionIds,
  liveSessionHandle,
  liveWorkingSessionIds,
  subscribeLiveSessions,
  useSession,
} from '../engine/useSession';
import { closeSession } from '../engine/transport';
import { showToast } from '../components/toast';
import { isTauriRuntime } from '../lib/env';
import { SessionSidebar } from './SessionSidebar';
import { ThreadHead } from './ThreadHead';
import { Thread } from './Thread';
import { Composer } from './Composer';
import { ContextPanel } from './ContextPanel';
import { getProviderConfig, type AppConfig } from '../providers/api';
import {
  listMcpServers,
  listSkills,
  listSlashCommands,
  type McpServer,
  type Skill,
  type SlashCommand,
} from '../extensions/extensionsApi';
import {
  deleteSession,
  getActiveSession,
  getSessionHistory,
  listSessions,
  renameSession,
  resumeSession,
  setSessionPinned,
} from '../sessions/api';
import { createSessionWorktree } from '../sessions/api';
import { publishResume } from '../sessions/resumeBridge';
import type { SessionSummary } from '../sessions/sessionTypes';
import type { AppSettings } from '../settings/types';
import { selectDirectory } from '../settings/api';
import type { EngineId } from '@helm/protocol';
import {
  defaultTurnModeFromSettings,
  sessionDefaultsFromSettings,
  shouldReopenLastSession,
} from '../settings/settingsViewModel';
import type { TurnMode } from '../engine/transport';
import {
  defaultModelForEngine,
  workspaceEngineOptions,
  workspaceSessionIsActive,
} from './workspaceViewModel';
import { expandSlashCommandDetailed } from './slashCommands';

export function Workspace({
  settings,
  onSettingsChange,
  newSessionRequest = 0,
  toggleContextRequest = 0,
  cycleEngineRequest = 0,
  pendingSessionId,
  onClearPendingSessionId,
}: {
  settings: AppSettings;
  onSettingsChange?: (updater: (prev: AppSettings) => AppSettings) => void;
  newSessionRequest?: number;
  toggleContextRequest?: number;
  cycleEngineRequest?: number;
  pendingSessionId?: string | null;
  onClearPendingSessionId?: () => void;
}) {
  const sessionDefaults = useMemo(() => sessionDefaultsFromSettings(settings), [settings]);
  const defaultTurnMode = useMemo(() => defaultTurnModeFromSettings(settings), [settings]);
  const {
    state,
    send,
    stop,
    reset,
    approve,
    selectEngine,
    selectModel,
    setCwd,
    toggleMcpServer,
    restoreCheckpoint,
    undoRevert,
  } = useSession(sessionDefaults);
  // 会话模式（变更-04 B.1）：轮次级状态放 Workspace，Composer 受控；新会话回落设置默认
  const [turnMode, setTurnMode] = useState<TurnMode>(defaultTurnMode);
  const [showCtx, setShowCtx] = useState(true);
  const [config, setConfig] = useState<AppConfig | null>(null);
  const [sessions, setSessions] = useState<SessionSummary[]>([]);
  const [slashCommands, setSlashCommands] = useState<SlashCommand[]>([]);
  const [mcpServers, setMcpServers] = useState<McpServer[]>([]);
  const [skills, setSkills] = useState<Skill[]>([]);
  const [mcpLoadError, setMcpLoadError] = useState<string | null>(null);
  const [skillsLoadError, setSkillsLoadError] = useState<string | null>(null);
  const [sessionError, setSessionError] = useState<string | null>(null);
  const [resumingId, setResumingId] = useState<string | null>(null);
  const [sendBlocker, setSendBlocker] = useState<{
    message: string;
    action: 'providers' | 'directory';
  } | null>(null);
  const [creatingWorktree, setCreatingWorktree] = useState(false);
  // 初值取当前计数而不是 0：App 层计数器跨挂载存活，Workspace 重挂载时
  // 若从 0 起比会把历史请求重放一遍（凭空新建会话/切引擎，可靠性检查 C2）
  const lastNewSessionRequest = useRef(newSessionRequest);
  const lastToggleContextRequest = useRef(toggleContextRequest);
  const lastCycleEngineRequest = useRef(cycleEngineRequest);
  const autoResumeAttempted = useRef(false);
  const remountRestoreAttempted = useRef(false);
  // 恢复操作序号（可靠性检查 B1）：快速连点两个会话时只应用最后一次
  const resumeSeq = useRef(0);
  // 侧栏「运行中」标记：订阅注册表变化（turn_complete 后即时消失，不再永久误报）
  const runningIds = useSyncExternalStore(subscribeLiveSessions, liveWorkingSessionIds);
  // 侧栏「待审批」徽标（变更-12）
  const approvalIds = useSyncExternalStore(subscribeLiveSessions, livePendingApprovalSessionIds);
  // 会话管理对话框（变更-12）
  const [renameTarget, setRenameTarget] = useState<SessionSummary | null>(null);
  const [renameValue, setRenameValue] = useState('');
  const [deleteTarget, setDeleteTarget] = useState<SessionSummary | null>(null);

  useEffect(() => {
    let active = true;
    getProviderConfig()
      .then((next) => {
        if (active) setConfig(next);
      })
      .catch((err) => {
        if (!active) return;
        setConfig(null);
        // 浏览器预览没有 Tauri 桥，保持静默；桌面环境下这是真实故障，必须可见
        if (isTauriRuntime()) {
          console.error('读取服务商配置失败:', err);
          showToast('读取服务商配置失败，引擎与绑定信息可能不完整', 'error');
        }
      });
    return () => {
      active = false;
    };
  }, []);

  useEffect(() => {
    let active = true;
    listSlashCommands(state.engine, state.cwd || undefined)
      .then((commands) => {
        if (active) {
          setSlashCommands(commands);
        }
      })
      .catch((err) => {
        console.error('[slash] failed to load for', state.engine, err);
        if (active) setSlashCommands([]);
        // 浏览器预览下静默；桌面环境提示 / 菜单不可用的原因
        if (active && isTauriRuntime()) {
          showToast('斜杠命令加载失败，输入 / 时菜单可能为空', 'error');
        }
      });
    return () => {
      active = false;
    };
  }, [state.engine, state.cwd]);

  // 右栏工具 tab 数据（变更-11）：真实 MCP 配置与技能清单；错误与空配置必须区分。
  const loadContextExtensions = useCallback(() => {
    setMcpLoadError(null);
    setSkillsLoadError(null);
    listMcpServers()
      .then((servers) => {
        setMcpServers(servers);
      })
      .catch((error: unknown) => {
        setMcpLoadError(error instanceof Error ? error.message : 'MCP 配置读取失败');
      });
    listSkills(state.engine, state.cwd || undefined)
      .then((next) => {
        setSkills(next);
      })
      .catch((error: unknown) => {
        setSkillsLoadError(error instanceof Error ? error.message : '技能清单读取失败');
      });
  }, [state.engine, state.cwd]);

  useEffect(() => {
    loadContextExtensions();
  }, [loadContextExtensions]);

  // stale model 纠正（可靠性检查 A4）：localStorage 恢复的模型已不在当前绑定的
  // 可选清单里时回落默认，杜绝「头部显示 A、实际发送 B」

  const refreshSessions = async () => {
    try {
      const next = await listSessions();
      setSessions(next);
      setSessionError(null);
    } catch (err) {
      setSessionError(err instanceof Error ? err.message : '无法读取会话历史');
    }
  };

  useEffect(() => {
    void refreshSessions();
  }, []);

  // 自动起标题完成后（P3-5）后端会广播 helm-sessions-changed，侧栏标题即时更新
  useEffect(() => {
    let active = true;
    let unlisten: (() => void) | null = null;
    void tauriListen('helm-sessions-changed', () => {
      if (active) void refreshSessions();
    })
      .then((stop) => {
        if (active) unlisten = stop;
        else stop();
      })
      .catch(() => {
        // 浏览器预览没有 Tauri 事件桥，忽略
      });
    return () => {
      active = false;
      unlisten?.();
    };
  }, []);

  const engineOptions = useMemo(() => (config ? workspaceEngineOptions(config) : []), [config]);
  const activeOption = engineOptions.find((option) => option.engine.id === state.engine);
  const activeModel =
    activeOption?.models.find((model) => model.id === state.model) ??
    activeOption?.models.find((model) => model.id === activeOption.binding?.primaryModel);

  useEffect(() => {
    if (!config || state.model) return;
    const nextModel = defaultModelForEngine(config, state.engine);
    if (nextModel) selectModel(nextModel);
  }, [config, selectModel, state.engine, state.model]);

  // stale model 纠正（可靠性检查 A4）：localStorage 恢复的模型已不在当前绑定的
  // 可选清单里时回落默认，杜绝「头部显示 A、实际发送 B」
  useEffect(() => {
    if (!config || !state.model) return;
    const option = engineOptions.find((item) => item.engine.id === state.engine);
    if (!option || !option.models.length) return;
    if (!option.models.some((model) => model.id === state.model)) {
      selectModel(defaultModelForEngine(config, state.engine));
    }
  }, [config, engineOptions, selectModel, state.engine, state.model]);

  const handleSelectEngine = useCallback(
    (engineId: EngineId) => {
      if (!config) return;
      selectEngine(engineId, defaultModelForEngine(config, engineId));
    },
    [config, selectEngine],
  );

  /** 新建会话：线程重置 + 模式回落设置默认（变更-04 B.1，新会话不沿用上一段的模式） */
  const startNewSession = useCallback(() => {
    reset();
    setTurnMode(defaultTurnMode);
  }, [reset, defaultTurnMode]);

  // 设置里的默认模式变化时，未开场的空会话跟随（对齐 apply_defaults 的语义）
  useEffect(() => {
    if (state.handleId || state.sessionId || state.items.length > 0) return;
    setTurnMode(defaultTurnMode);
  }, [defaultTurnMode, state.handleId, state.sessionId, state.items.length]);

  useEffect(() => {
    if (newSessionRequest <= lastNewSessionRequest.current) return;
    lastNewSessionRequest.current = newSessionRequest;
    startNewSession();
  }, [newSessionRequest, startNewSession]);

  useEffect(() => {
    if (toggleContextRequest <= lastToggleContextRequest.current) return;
    lastToggleContextRequest.current = toggleContextRequest;
    setShowCtx((value) => !value);
  }, [toggleContextRequest]);

  useEffect(() => {
    if (cycleEngineRequest <= lastCycleEngineRequest.current) return;
    if (engineOptions.length < 2) return;
    const currentIndex = engineOptions.findIndex((option) => option.engine.id === state.engine);
    const next = engineOptions[(currentIndex + 1) % engineOptions.length] ?? engineOptions[0];
    lastCycleEngineRequest.current = cycleEngineRequest;
    handleSelectEngine(next.engine.id);
  }, [cycleEngineRequest, engineOptions, handleSelectEngine, state.engine]);

  useEffect(() => {
    if (autoResumeAttempted.current) return;
    if (
      !shouldReopenLastSession(settings, {
        handleId: state.handleId,
        sessionId: state.sessionId,
        itemsLength: state.items.length,
      })
    ) {
      return;
    }
    autoResumeAttempted.current = true;
    let active = true;
    getActiveSession()
      .then(async (session) => {
        if (!active || !session) return;
        setResumingId(session.id);
        const handleId = await resumeSession(session.id);
        if (!active) return;
        publishResume({ handleId, session });
        setSessionError(null);
        void refreshSessions();
      })
      .catch((err: unknown) => {
        if (!active) return;
        setSessionError(err instanceof Error ? err.message : '重新打开上次会话失败');
      })
      .finally(() => {
        if (active) setResumingId(null);
      });
    return () => {
      active = false;
    };
  }, [settings, state.handleId, state.items.length, state.sessionId]);

  const handleSend = async (text: string, attachments: string[]) => {
    // 恢复进行中不接受发送（可靠性检查 B2）：此时句柄指向旧会话或为空，
    // 消息会发错地方且随后被恢复快照整体覆盖。
    if (resumingId) {
      showToast('正在恢复会话，请稍候再发送', 'error');
      return;
    }
    // 发送前置校验（可靠性检查 §4.4）：不满足前置条件时消息不上屏，
    // 给出可点击的修复出路，而不是发送后用红色错误卡打脸。
    if (config && !(config.bindings ?? []).some((binding) => binding.engineId === state.engine)) {
      setSendBlocker({
        message: `当前引擎（${activeOption?.engine.name ?? state.engine}）还没有绑定服务商和模型，需要先完成绑定才能对话。`,
        action: 'providers',
      });
      return;
    }
    if (!state.cwd.trim()) {
      setSendBlocker({
        message: '尚未设置工作目录。Agent 需要一个目录来读写文件，请先选择。',
        action: 'directory',
      });
      return;
    }
    setSendBlocker(null);
    // 斜杠命令展开（变更-03/08）：线程/历史保留 /trigger args 原文，只有发给 CLI 的文本被展开。
    const trimmed = text.trim();
    const firstToken = trimmed.split(/\s/, 1)[0]?.toLowerCase();
    const matchedSkill = skills.find(
      (skill) =>
        skill.enabled &&
        skill.engine === state.engine &&
        skill.trigger.toLowerCase() === firstToken,
    );
    const expansion = matchedSkill
      ? {
          expanded:
            state.engine === 'codex'
              ? trimmed
              : `请明确使用技能 ${matchedSkill.id.replace(/^proj:/, '')} 完成本次任务。\n\n${trimmed.slice(firstToken.length).trim()}`.trim(),
          matched: true,
          passthrough: state.engine === 'codex',
        }
      : expandSlashCommandDetailed(slashCommands, text, state.engine);
    let outgoingAttachments = attachments;
    if (expansion.matched && expansion.passthrough && attachments.length) {
      // 透传命令要求「命令位于 prompt 开头」，附件说明若追加在尾部会污染 $ARGUMENTS，
      // 本轮不注入附件（可靠性检查 C2），提示用户改用普通消息引用文件。
      outgoingAttachments = [];
      showToast('原生斜杠命令本轮不随附件发送；如需引用文件请在普通消息中提及路径', 'error');
    }
    const commandText = expansion.matched ? expansion.expanded : undefined;
    await send(text, outgoingAttachments, turnMode, commandText);
    void refreshSessions();
  };

  const handlePickDirectory = async () => {
    try {
      const dir = await selectDirectory();
      if (!dir) return;
      onSettingsChange?.((prev) => ({
        ...prev,
        general: { ...prev.general, defaultDirectory: dir },
      }));
      setSendBlocker(null);
    } catch {
      // 目录选择器失败保持 blocker 显示，用户可去设置页配置
    }
  };

  const navigate = (page: string) =>
    window.dispatchEvent(new CustomEvent('helm:navigate', { detail: { page } }));

  const handleCommandAction = (action: string) => {
    if (action === 'new-session') return startNewSession();
    if (action === 'resume-session') return navigate('sessions');
    if (action === 'open-extensions') return navigate('extensions');
    if (action === 'open-permissions') {
      sessionStorage.setItem('helm:settings-tab', 'permissions');
      return navigate('settings');
    }
    if (action === 'toggle-context') return setShowCtx((value) => !value);
    if (action === 'show-status') return setShowCtx(true);
    if (action === 'stop-turn') {
      if (state.status === 'working') stop();
      else showToast('当前没有正在运行的轮次');
      return;
    }
    if (action === 'show-help') {
      showToast('命令分为 Helm 操作、提示词命令、引擎扩展和 Skills；输入 / 或 $ 可搜索');
    }
  };

  const handleOpenSession = useCallback(async (sessionId: string) => {
    const token = ++resumeSeq.current;
    setResumingId(sessionId);
    try {
      // 并行会话（P3-3）：还有存活句柄的会话直接复用，不重启 CLI；
      // 后台仍在跑的轮次结束后事件会照常写入历史。
      const live = liveSessionHandle(sessionId);
      let session;
      let handleId: string;
      if (live) {
        session = await getSessionHistory(sessionId);
        handleId = live;
      } else {
        const [historyResult, resumeResult] = await Promise.allSettled([
          getSessionHistory(sessionId),
          resumeSession(sessionId),
        ]);
        if (historyResult.status === 'rejected' || resumeResult.status === 'rejected') {
          if (resumeResult.status === 'fulfilled') {
            void closeSession(resumeResult.value).catch(() => {});
          }
          if (historyResult.status === 'rejected') throw historyResult.reason;
          if (resumeResult.status === 'rejected') throw resumeResult.reason;
          throw new Error('恢复会话失败');
        }
        session = historyResult.value;
        handleId = resumeResult.value;
      }
      if (token !== resumeSeq.current) {
        // 已被更晚的打开操作取代（快速连点）：慢返回的不应用；
        // 新启的运行时立即回收防泄漏，复用的存活句柄保持原状
        if (!live) void closeSession(handleId).catch(() => {});
        return;
      }
      publishResume({ handleId, session });
      setSessionError(null);
      void refreshSessions();
    } catch (err) {
      if (token === resumeSeq.current) {
        setSessionError(err instanceof Error ? err.message : '恢复会话失败');
      }
    } finally {
      if (token === resumeSeq.current) setResumingId(null);
    }
  }, []);

  // 重挂载恢复（可靠性检查 C1）：切页回来时若上一个打开的会话还有存活句柄，
  // 直接复用恢复线程视图，不再显示空白线程、也不遗留孤儿句柄。
  useEffect(() => {
    if (remountRestoreAttempted.current) return;
    remountRestoreAttempted.current = true;
    if (pendingSessionId) return; // 命令面板等入口指定了要打开的会话，让它优先
    const last = lastOpenWorkspaceSession();
    if (!last || !liveSessionHandle(last)) return;
    autoResumeAttempted.current = true; // 已有明确恢复目标，跳过「重开上次会话」
    void handleOpenSession(last);
  }, [handleOpenSession, pendingSessionId]);

  /** 在独立 Git worktree 中开一个并行会话（P3-3） */
  const handleNewWorktreeSession = async () => {
    const baseCwd = (state.cwd || settings.general.defaultDirectory).trim();
    if (!baseCwd) {
      setSendBlocker({
        message: '尚未设置工作目录，无法创建 worktree。请先选择一个 Git 仓库目录。',
        action: 'directory',
      });
      return;
    }
    setCreatingWorktree(true);
    try {
      const info = await createSessionWorktree(baseCwd, '');
      // 当前会话若在跑会自动挂后台（releaseHandle keepAlive）；新线程落到 worktree 目录
      startNewSession();
      setCwd(info.path);
      if (info.setupOutput.startsWith('[setup 脚本失败]')) {
        showToast(`worktree 已创建，但初始化命令失败：${info.setupOutput.slice(0, 160)}`, 'error');
      } else {
        showToast(`已创建并行 worktree：${info.path}（分支 ${info.branch}）`, 'success');
      }
    } catch (err) {
      showToast(`创建 worktree 失败：${err instanceof Error ? err.message : String(err)}`, 'error');
    } finally {
      setCreatingWorktree(false);
    }
  };

  useEffect(() => {
    if (!pendingSessionId || !onClearPendingSessionId) return;
    void handleOpenSession(pendingSessionId);
    onClearPendingSessionId();
  }, [pendingSessionId, handleOpenSession, onClearPendingSessionId]);

  // —— 会话管理动作（变更-12） ——
  const handleRenameConfirm = async () => {
    if (!renameTarget) return;
    try {
      await renameSession(renameTarget.id, renameValue);
      setRenameTarget(null);
      void refreshSessions();
    } catch (err) {
      showToast(`重命名失败：${err instanceof Error ? err.message : String(err)}`, 'error');
    }
  };

  const handleDeleteConfirm = async () => {
    if (!deleteTarget) return;
    const target = deleteTarget;
    try {
      await deleteSession(target.id);
      dropLiveSession(target.id);
      setDeleteTarget(null);
      // 删的是当前打开的会话：清空线程视图，回到全新会话
      if (state.historyId === target.id) startNewSession();
      showToast('会话已删除', 'success');
      void refreshSessions();
    } catch (err) {
      showToast(`删除失败：${err instanceof Error ? err.message : String(err)}`, 'error');
    }
  };

  const handleTogglePinned = async (session: SessionSummary) => {
    try {
      await setSessionPinned(session.id, !session.pinned);
      void refreshSessions();
    } catch (err) {
      showToast(`置顶失败：${err instanceof Error ? err.message : String(err)}`, 'error');
    }
  };

  return (
    <main className={'ws' + (showCtx ? '' : ' no-ctx')}>
      <SessionSidebar
        state={state}
        engineOptions={engineOptions}
        activeOption={activeOption}
        sessions={sessions}
        sessionError={sessionError}
        resumingId={resumingId}
        runningIds={runningIds}
        approvalIds={approvalIds}
        worktreeEnabled={settings.worktree.enabled}
        creatingWorktree={creatingWorktree}
        onNew={startNewSession}
        onNewWorktree={() => void handleNewWorktreeSession()}
        onSelectEngine={handleSelectEngine}
        onOpenSession={handleOpenSession}
        onRenameSession={(session) => {
          setRenameValue(session.title);
          setRenameTarget(session);
        }}
        onDeleteSession={(session) => setDeleteTarget(session)}
        onTogglePinned={(session) => void handleTogglePinned(session)}
        isSessionActive={(session) =>
          workspaceSessionIsActive(session, {
            historyId: state.historyId,
            handleId: state.handleId,
            cliSessionId: state.sessionId,
          })
        }
      />
      <section className="thread">
        <ThreadHead
          title={sessions.find((session) => session.id === state.historyId)?.title}
          engineOptions={engineOptions}
          activeOption={activeOption}
          model={activeModel?.id || state.model || '默认模型'}
          onSelectEngine={handleSelectEngine}
          onSelectModel={selectModel}
          onToggleCtx={() => setShowCtx((v) => !v)}
        />
        <Thread
          key={state.historyId ?? state.sessionId ?? 'empty'}
          state={state}
          onApprove={approve}
          onRestoreCheckpoint={restoreCheckpoint}
          onUndoRevert={undoRevert}
        />
        {sendBlocker ? (
          <div className="ws-send-blocker">
            <span>{sendBlocker.message}</span>
            <div className="ws-send-blocker__actions">
              {sendBlocker.action === 'providers' ? (
                <button
                  type="button"
                  className="btn btn--sm"
                  onClick={() =>
                    window.dispatchEvent(
                      new CustomEvent('helm:navigate', { detail: { page: 'providers' } }),
                    )
                  }
                >
                  去服务商页配置
                </button>
              ) : (
                <button type="button" className="btn btn--sm" onClick={handlePickDirectory}>
                  选择工作目录
                </button>
              )}
              <button type="button" className="btn btn--sm" onClick={() => setSendBlocker(null)}>
                知道了
              </button>
            </div>
          </div>
        ) : null}
        <Composer
          working={state.status === 'working'}
          mode={turnMode}
          engine={state.engine}
          cwd={state.cwd}
          slashCommands={slashCommands}
          skills={skills}
          onModeChange={setTurnMode}
          onCommandAction={handleCommandAction}
          onSend={handleSend}
          onStop={stop}
        />
      </section>
      <ContextPanel
        state={state}
        settings={settings}
        mcpServers={mcpServers}
        skills={skills}
        mcpLoadError={mcpLoadError}
        skillsLoadError={skillsLoadError}
        onRetryExtensions={loadContextExtensions}
        onToggleMcp={toggleMcpServer}
        onOpenExtensions={() =>
          window.dispatchEvent(new CustomEvent('helm:navigate', { detail: { page: 'extensions' } }))
        }
        onOpenSettings={() =>
          window.dispatchEvent(new CustomEvent('helm:navigate', { detail: { page: 'settings' } }))
        }
      />
      {renameTarget ? (
        <div className="modal-overlay" role="dialog" aria-modal="true" aria-label="重命名会话">
          <div className="modal-panel">
            <div className="modal-panel__head">
              <b>重命名会话</b>
              <button
                className="btn-icon sm"
                onClick={() => setRenameTarget(null)}
                aria-label="关闭"
                type="button"
              >
                ×
              </button>
            </div>
            <div className="modal-panel__body">
              <input
                type="text"
                className="input"
                style={{ width: '100%' }}
                value={renameValue}
                autoFocus
                aria-label="会话标题"
                onChange={(event) => setRenameValue(event.target.value)}
                onKeyDown={(event) => {
                  if (event.nativeEvent.isComposing) return;
                  if (event.key === 'Enter') void handleRenameConfirm();
                  if (event.key === 'Escape') setRenameTarget(null);
                }}
              />
            </div>
            <div className="modal-panel__foot">
              <button className="btn btn--sm" type="button" onClick={() => setRenameTarget(null)}>
                取消
              </button>
              <button
                className="btn btn--primary btn--sm"
                type="button"
                disabled={!renameValue.trim()}
                onClick={() => void handleRenameConfirm()}
              >
                保存
              </button>
            </div>
          </div>
        </div>
      ) : null}
      {deleteTarget ? (
        <div className="modal-overlay" role="dialog" aria-modal="true" aria-label="删除会话">
          <div className="modal-panel">
            <div className="modal-panel__head">
              <b>删除会话</b>
              <button
                className="btn-icon sm"
                onClick={() => setDeleteTarget(null)}
                aria-label="关闭"
                type="button"
              >
                ×
              </button>
            </div>
            <div className="modal-panel__body">
              <p>
                确定删除「{deleteTarget.title}
                」吗？会话的消息、工具记录、用量与检查点快照将一并删除，
                运行中的后台轮次会被终止。此操作不可撤销。
              </p>
            </div>
            <div className="modal-panel__foot">
              <button className="btn btn--sm" type="button" onClick={() => setDeleteTarget(null)}>
                取消
              </button>
              <button
                className="btn btn--danger btn--sm"
                type="button"
                onClick={() => void handleDeleteConfirm()}
              >
                删除
              </button>
            </div>
          </div>
        </div>
      ) : null}
    </main>
  );
}
