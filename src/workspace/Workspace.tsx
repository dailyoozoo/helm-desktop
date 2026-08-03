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
import {
  closeSession,
  setSessionTurnPreference,
  setSessionPermissionProfile,
  getGitBranch,
  type PermissionProfile,
} from '../engine/transport';
import { showToast } from '../components/toast';
import { Icon } from '../shell/icons';
import { useDialogBehavior } from '../components/useDialogBehavior';
import { isTauriRuntime } from '../lib/env';
import { sendNotification } from '@tauri-apps/plugin-notification';
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
  createFolder,
  cancelBackgroundOperation,
  deleteFolder,
  deleteSession,
  getActiveSession,
  getBackgroundOperation,
  getSessionHistory,
  listFolders,
  listSessions,
  renameFolder,
  renameSession,
  resumeSession,
  retryBackgroundOperation,
  setFolderCollapsed,
  setSessionFolder,
  setSessionPinned,
  startSessionFork,
  type BackgroundOperation,
} from '../sessions/api';
import { publishResume } from '../sessions/resumeBridge';
import type { SessionFolder, SessionSummary } from '../sessions/sessionTypes';
import type { AppSettings } from '../settings/types';
import { selectDirectory } from '../settings/api';
import type { EngineId, ReasoningEffort } from '@helm/protocol';
import { useReasoningEffortCapability } from '../engine/useReasoningEffortCapability';
import { normalizeReasoningEffort } from '../reasoning';
import {
  defaultTurnModeForEngine,
  defaultTurnModeFromSettings,
  sessionDefaultsFromSettings,
  shouldReopenLastSession,
} from '../settings/settingsViewModel';
import type { TurnMode } from '../engine/transport';
import {
  defaultModelForEngine,
  sameWorkspacePath,
  workspaceEngineOptions,
  workspaceSessionIsActive,
} from './workspaceViewModel';
import { expandSlashCommandDetailed } from './slashCommands';

type IdentitySwitch = { kind: 'engine'; engine: EngineId; model: string; label: string };

type WorkspaceViewport = 'wide' | 'context-drawer' | 'dual-drawer' | 'compact';

function workspaceViewport(): WorkspaceViewport {
  if (typeof window === 'undefined') return 'wide';
  if (window.innerWidth >= 1280) return 'wide';
  if (window.innerWidth >= 960) return 'context-drawer';
  if (window.innerWidth >= 720) return 'dual-drawer';
  return 'compact';
}

export function Workspace({
  settings,
  onSettingsChange,
  newSessionRequest = 0,
  toggleContextRequest = 0,
  cycleEngineRequest = 0,
  pendingSessionId,
  onClearPendingSessionId,
  onGitInfoChange,
}: {
  settings: AppSettings;
  onSettingsChange?: (updater: (prev: AppSettings) => AppSettings) => void;
  newSessionRequest?: number;
  toggleContextRequest?: number;
  cycleEngineRequest?: number;
  pendingSessionId?: string | null;
  onClearPendingSessionId?: () => void;
  /** 当 git 信息变化时回调（用于标题栏显示） */
  onGitInfoChange?: (info: { projectName?: string; branchName?: string }) => void;
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
    toggleMcpServer,
    restoreCheckpoint,
    undoRevert,
  } = useSession(sessionDefaults);
  const engineDefaultTurnMode = useMemo(
    () => defaultTurnModeForEngine(settings, state.engine),
    [settings, state.engine],
  );
  // 会话模式（变更-04 B.1）：轮次级状态放 Workspace，Composer 受控；新会话回落设置默认
  const [turnMode, setTurnMode] = useState<TurnMode>(defaultTurnMode);
  const [permissionProfile, setPermissionProfile] = useState<PermissionProfile>('standard');
  const previousEngine = useRef(state.engine);
  const [reasoningEffort, setReasoningEffort] = useState<ReasoningEffort>('auto');
  const [viewport, setViewport] = useState<WorkspaceViewport>(workspaceViewport);
  const [showCtx, setShowCtx] = useState(() => workspaceViewport() === 'wide');
  const [contextTabRequest, setContextTabRequest] = useState(0);
  const [activityTarget, setActivityTarget] = useState<{ id: string; request: number } | null>(
    null,
  );
  const activityRequestRef = useRef(0);
  const activityClearTimerRef = useRef<number | null>(null);
  const [showSessions, setShowSessions] = useState(false);
  const [config, setConfig] = useState<AppConfig | null>(null);
  const [sessions, setSessions] = useState<SessionSummary[]>([]);
  const [folders, setFolders] = useState<SessionFolder[]>([]);
  const [newSessionFolderId, setNewSessionFolderId] = useState<string | undefined>();
  const [slashCommands, setSlashCommands] = useState<SlashCommand[]>([]);
  const [mcpServers, setMcpServers] = useState<McpServer[]>([]);
  const [skills, setSkills] = useState<Skill[]>([]);

  const locateActivityItem = useCallback((id: string) => {
    activityRequestRef.current += 1;
    const request = activityRequestRef.current;
    setActivityTarget({ id, request });
    if (activityClearTimerRef.current != null) {
      window.clearTimeout(activityClearTimerRef.current);
    }
    activityClearTimerRef.current = window.setTimeout(() => {
      setActivityTarget((current) => (current?.request === request ? null : current));
      activityClearTimerRef.current = null;
    }, 1_600);
  }, []);

  useEffect(
    () => () => {
      if (activityClearTimerRef.current != null) {
        window.clearTimeout(activityClearTimerRef.current);
      }
    },
    [],
  );
  const [mcpLoadError, setMcpLoadError] = useState<string | null>(null);
  const [skillsLoadError, setSkillsLoadError] = useState<string | null>(null);
  const [sessionError, setSessionError] = useState<string | null>(null);
  const [resumingId, setResumingId] = useState<string | null>(null);
  const [sendBlocker, setSendBlocker] = useState<{
    message: string;
    action: 'providers' | 'directory';
  } | null>(null);
  const [identitySwitch, setIdentitySwitch] = useState<IdentitySwitch | null>(null);
  const [forkOperation, setForkOperation] = useState<BackgroundOperation | null>(null);
  const [forkStarting, setForkStarting] = useState(false);
  // 初值取当前计数而不是 0：App 层计数器跨挂载存活，Workspace 重挂载时
  // 若从 0 起比会把历史请求重放一遍（凭空新建会话/切引擎，可靠性检查 C2）
  const lastNewSessionRequest = useRef(newSessionRequest);
  const lastToggleContextRequest = useRef(toggleContextRequest);

  // Git 信息（批次 E）：标题栏显示「项目目录名 › 分支名」
  useEffect(() => {
    if (!state.cwd) {
      onGitInfoChange?.({ projectName: undefined, branchName: undefined });
      return;
    }

    // 从 cwd 提取项目目录名
    const projectName = state.cwd.split(/[\\/]/).filter(Boolean).pop() || state.cwd;

    let cancelled = false;
    getGitBranch(state.cwd)
      .then((branch) => {
        if (cancelled) return;
        onGitInfoChange?.({ projectName, branchName: branch });
      })
      .catch(() => {
        if (cancelled) return;
        // git 获取失败（可能不是 git 仓库），只显示项目名
        onGitInfoChange?.({ projectName, branchName: undefined });
      });

    return () => {
      cancelled = true;
    };
  }, [state.cwd, onGitInfoChange]);

  useEffect(() => {
    const update = () => setViewport(workspaceViewport());
    window.addEventListener('resize', update);
    return () => window.removeEventListener('resize', update);
  }, []);

  useEffect(() => {
    if (viewport !== 'wide') setShowCtx(false);
    if (viewport === 'context-drawer' || viewport === 'wide') setShowSessions(false);
  }, [viewport]);

  const closeSessionDrawer = useCallback((restoreFocus = false) => {
    setShowSessions(false);
    if (restoreFocus) {
      window.setTimeout(
        () => document.querySelector<HTMLElement>('.ws-sidebar-toggle')?.focus(),
        0,
      );
    }
  }, []);

  const closeContextDrawer = useCallback((restoreFocus = false) => {
    setShowCtx(false);
    if (restoreFocus) {
      window.setTimeout(
        () => document.querySelector<HTMLElement>('.ws-context-toggle')?.focus(),
        0,
      );
    }
  }, []);

  useEffect(() => {
    if (viewport === 'wide') return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== 'Escape') return;
      if (showCtx) {
        event.preventDefault();
        closeContextDrawer(true);
      } else if (showSessions) {
        event.preventDefault();
        closeSessionDrawer(true);
      }
    };
    document.addEventListener('keydown', onKeyDown);
    return () => document.removeEventListener('keydown', onKeyDown);
  }, [closeContextDrawer, closeSessionDrawer, showCtx, showSessions, viewport]);
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
  const [folderDeleteTarget, setFolderDeleteTarget] = useState<SessionFolder | null>(null);
  const [folderEditor, setFolderEditor] = useState<{
    folder: SessionFolder | null;
    value: string;
    moveSession?: SessionSummary;
  } | null>(null);

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
      const [next, nextFolders] = await Promise.all([listSessions(), listFolders()]);
      setSessions(next);
      setFolders(nextFolders);
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
  const activeSession = sessions.find((session) => session.id === state.historyId);
  const {
    capability: reasoningCapability,
    loading: reasoningLoading,
    error: reasoningError,
  } = useReasoningEffortCapability(
    state.engine,
    activeModel?.id || state.model,
    activeOption?.provider?.id,
  );

  useEffect(() => {
    setReasoningEffort(
      activeSession?.preferredReasoningEffort ?? activeOption?.binding?.reasoningEffort ?? 'auto',
    );
  }, [
    activeOption?.binding?.reasoningEffort,
    activeSession?.preferredReasoningEffort,
    state.engine,
    state.model,
  ]);

  useEffect(() => {
    if (reasoningLoading) return;
    if (reasoningCapability) {
      const normalized = normalizeReasoningEffort(reasoningCapability, reasoningEffort);
      if (normalized !== reasoningEffort) {
        setReasoningEffort(normalized);
        if (state.handleId) {
          void setSessionTurnPreference(state.handleId, state.model, normalized).catch((error) =>
            showToast(error instanceof Error ? error.message : '保存推理强度回落结果失败', 'error'),
          );
        }
      }
    } else if (reasoningError) {
      if (reasoningEffort !== 'auto') {
        setReasoningEffort('auto');
        if (state.handleId) {
          void setSessionTurnPreference(state.handleId, state.model, 'auto').catch((error) =>
            showToast(error instanceof Error ? error.message : '保存推理强度回落结果失败', 'error'),
          );
        }
      }
    }
  }, [
    reasoningCapability,
    reasoningEffort,
    reasoningError,
    reasoningLoading,
    state.handleId,
    state.model,
  ]);

  useEffect(() => {
    if (!config || state.model) return;
    const nextModel = defaultModelForEngine(config, state.engine);
    if (nextModel) void selectModel(nextModel);
  }, [config, selectModel, state.engine, state.model]);

  // stale model 纠正（可靠性检查 A4）：localStorage 恢复的模型已不在当前绑定的
  // 可选清单里时回落默认，杜绝「头部显示 A、实际发送 B」
  useEffect(() => {
    if (!config || !state.model) return;
    if (state.handleId || state.historyId || state.items.length > 0) return;
    const option = engineOptions.find((item) => item.engine.id === state.engine);
    if (!option || !option.models.length) return;
    if (!option.models.some((model) => model.id === state.model)) {
      void selectModel(defaultModelForEngine(config, state.engine));
    }
  }, [
    config,
    engineOptions,
    selectModel,
    state.engine,
    state.handleId,
    state.historyId,
    state.items.length,
    state.model,
  ]);

  const handleSelectEngine = useCallback(
    (engineId: EngineId) => {
      if (!config) return;
      const model = defaultModelForEngine(config, engineId);
      if (engineId === state.engine && model === state.model) return;
      if (state.status === 'working') {
        showToast('当前轮次正在运行，请先停止后再切换引擎', 'error');
        return;
      }
      if (state.handleId || state.historyId || state.sessionId || state.items.length > 0) {
        setIdentitySwitch({
          kind: 'engine',
          engine: engineId,
          model,
          label: engineId === 'codex' ? 'Codex' : 'Claude Code',
        });
        return;
      }
      setReasoningEffort(
        config.bindings.find((binding) => binding.engineId === engineId)?.reasoningEffort ?? 'auto',
      );
      setTurnMode(defaultTurnModeForEngine(settings, engineId));
      setPermissionProfile('standard');
      selectEngine(engineId, model);
    },
    [
      config,
      selectEngine,
      settings,
      state.engine,
      state.handleId,
      state.historyId,
      state.items.length,
      state.model,
      state.sessionId,
      state.status,
    ],
  );

  const handleSelectModel = useCallback(
    async (model: string) => {
      if (model === state.model) return;
      if (state.status === 'working') {
        showToast('当前轮次正在运行，请先停止后再切换模型', 'error');
        return;
      }
      try {
        const nextEffort = reasoningEffort;
        await selectModel(model, nextEffort);
        setReasoningEffort(nextEffort);
      } catch (error) {
        showToast(error instanceof Error ? error.message : '保存下一轮模型偏好失败', 'error');
      }
    },
    [reasoningEffort, selectModel, state.model, state.status],
  );

  const confirmIdentitySwitch = useCallback(async () => {
    const pending = identitySwitch;
    if (!pending) return;
    if (!state.historyId) {
      reset();
      setPermissionProfile('standard');
      setReasoningEffort(
        config?.bindings.find((binding) => binding.engineId === pending.engine)?.reasoningEffort ??
          'auto',
      );
      setTurnMode(defaultTurnModeForEngine(settings, pending.engine));
      selectEngine(pending.engine, pending.model);
      setIdentitySwitch(null);
      return;
    }
    setForkStarting(true);
    try {
      const operation = await startSessionFork(state.historyId, pending.engine);
      setForkOperation(operation);
    } catch (error) {
      showToast(error instanceof Error ? error.message : '创建跨引擎交接任务失败', 'error');
    } finally {
      setForkStarting(false);
    }
  }, [config?.bindings, identitySwitch, reset, selectEngine, settings, state.historyId]);

  const handleSelectReasoningEffort = useCallback(
    async (effort: ReasoningEffort) => {
      if (state.status === 'working') return;
      try {
        if (state.handleId) {
          await setSessionTurnPreference(state.handleId, state.model, effort);
        }
        setReasoningEffort(effort);
      } catch (error) {
        showToast(error instanceof Error ? error.message : '保存下一轮推理强度失败', 'error');
      }
    },
    [state.handleId, state.model, state.status],
  );

  /** 新建会话：线程重置 + 模式回落设置默认（变更-04 B.1，新会话不沿用上一段的模式） */
  const startNewSession = useCallback(
    (folderId?: string) => {
      const defaultOption = engineOptions.find(
        (option) => option.engine.id === sessionDefaults.engine,
      );
      // 全局新建由后端按 canonical cwd 自动归类；只有 Folder 上下文入口才显式覆盖。
      setNewSessionFolderId(folderId);
      reset();
      setTurnMode(defaultTurnMode);
      setReasoningEffort(defaultOption?.binding?.reasoningEffort ?? 'auto');
    },
    [defaultTurnMode, engineOptions, reset, sessionDefaults.engine],
  );

  // 恢复历史会话或切换引擎后，不能继续沿用另一个 Engine 的模式。
  useEffect(() => {
    if (previousEngine.current === state.engine) return;
    previousEngine.current = state.engine;
    setTurnMode(engineDefaultTurnMode);
  }, [engineDefaultTurnMode, state.engine]);

  // 设置里的默认模式变化时，未开场的空会话跟随（对齐 apply_defaults 的语义）
  useEffect(() => {
    if (state.handleId || state.sessionId || state.items.length > 0) return;
    setTurnMode(engineDefaultTurnMode);
  }, [engineDefaultTurnMode, state.handleId, state.sessionId, state.items.length]);

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
    let settled = false;
    getActiveSession()
      .then(async (session) => {
        if (!active || !session) return;
        setResumingId(session.id);
        const handleId = await resumeSession(session.id);
        if (!active) return;
        publishResume({ handleId, session });
        setPermissionProfile(session.safePermissionProfile ?? 'standard');
        setSessionError(null);
        void refreshSessions();
      })
      .catch((err: unknown) => {
        if (!active) return;
        setSessionError(err instanceof Error ? err.message : '重新打开上次会话失败');
      })
      .finally(() => {
        settled = true;
        if (active) setResumingId(null);
      });
    return () => {
      active = false;
      // StrictMode 会清理后重跑 effect；未完成的首次尝试不能永久吃掉第二次恢复。
      if (!settled) autoResumeAttempted.current = false;
    };
  }, [settings, state.handleId, state.items.length, state.sessionId]);

  const handleSend = async (text: string, attachments: string[]): Promise<boolean> => {
    // 恢复进行中不接受发送（可靠性检查 B2）：此时句柄指向旧会话或为空，
    // 消息会发错地方且随后被恢复快照整体覆盖。
    if (resumingId) {
      showToast('正在恢复会话，请稍候再发送', 'error');
      return false;
    }
    // 发送前置校验（可靠性检查 §4.4）：不满足前置条件时消息不上屏，
    // 给出可点击的修复出路，而不是发送后用红色错误卡打脸。
    if (config && !(config.bindings ?? []).some((binding) => binding.engineId === state.engine)) {
      setSendBlocker({
        message: `当前引擎（${activeOption?.engine.name ?? state.engine}）还没有绑定服务商和模型，需要先完成绑定才能对话。`,
        action: 'providers',
      });
      return false;
    }
    if (!state.cwd.trim()) {
      setSendBlocker({
        message: '尚未设置工作目录。Agent 需要一个目录来读写文件，请先选择。',
        action: 'directory',
      });
      return false;
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
    const sent = await send(
      text,
      outgoingAttachments,
      turnMode,
      commandText,
      reasoningEffort,
      permissionProfile,
      newSessionFolderId,
    );
    if (!sent && !state.handleId && permissionProfile === 'full_access') {
      // 新会话的 FullAccessLease 只能在首次发送、Session id 已确定后签发。
      // 用户取消系统确认时立即回落安全档，不能让 UI 假装危险模式仍已开启。
      setPermissionProfile('standard');
    }
    void refreshSessions();
    return sent;
  };

  useEffect(() => {
    const onGlobalStop = (event: KeyboardEvent) => {
      if (!event.ctrlKey || !event.shiftKey || event.key !== '.') return;
      if (state.status !== 'working') return;
      event.preventDefault();
      void stop();
    };
    window.addEventListener('keydown', onGlobalStop);
    return () => window.removeEventListener('keydown', onGlobalStop);
  }, [state.status, stop]);

  // 系统通知（G-15）：轮次完成/出错时弹出 Windows 原生通知
  const prevStatusRef = useRef(state.status);
  useEffect(() => {
    const prev = prevStatusRef.current;
    prevStatusRef.current = state.status;
    // 只在从 working → idle 时触发（轮次结束）
    if (prev !== 'working' || state.status !== 'idle') return;
    // 检查通知是否启用
    const notifEnabled = settings.general.notifications?.enabled ?? true;
    if (!notifEnabled) return;
    // 检查是否在 Tauri 环境
    if (!isTauriRuntime()) return;
    // 判断是出错还是正常完成
    const lastItem = state.items[state.items.length - 1];
    const hasError = lastItem?.kind === 'error';
    try {
      if (hasError) {
        sendNotification({
          title: 'Helm',
          body: '轮次出错，请查看会话详情',
        });
      } else {
        sendNotification({
          title: 'Helm',
          body: '轮次已完成',
        });
      }
    } catch {
      // 通知发送失败静默处理
    }
  }, [state.status, state.items, settings.general.notifications]);

  const handlePermissionProfileChange = async (profile: PermissionProfile) => {
    if (!state.handleId) {
      setPermissionProfile(profile);
      return;
    }
    try {
      await setSessionPermissionProfile(state.handleId, profile);
      setPermissionProfile(profile);
    } catch (error) {
      showToast(error instanceof Error ? error.message : String(error), 'error');
    }
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
      setPermissionProfile(session.safePermissionProfile ?? 'standard');
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

  useEffect(() => {
    if (!forkOperation) return;
    let active = true;
    const openCompletedFork = async (operation: BackgroundOperation) => {
      const targetSessionId =
        operation.result && typeof operation.result === 'object'
          ? (operation.result as { targetSessionId?: unknown }).targetSessionId
          : null;
      if (typeof targetSessionId !== 'string' || !targetSessionId) {
        showToast('交接任务完成，但目标 Session 身份缺失', 'error');
        return;
      }
      setIdentitySwitch(null);
      setForkOperation(null);
      await refreshSessions();
      await handleOpenSession(targetSessionId);
      showToast('已通过交接摘要创建新会话', 'success');
    };
    const refresh = async (operation: BackgroundOperation) => {
      try {
        const latest =
          operation.status === 'succeeded' ? operation : await getBackgroundOperation(operation.id);
        if (!active || !latest) return;
        setForkOperation(latest);
        if (latest.status === 'succeeded') await openCompletedFork(latest);
      } catch (error) {
        if (active) {
          showToast(error instanceof Error ? error.message : '读取交接任务状态失败', 'error');
        }
      }
    };
    void refresh(forkOperation);
    const polling = ['committed', 'running'].includes(forkOperation.status);
    const timer = polling ? window.setInterval(() => void refresh(forkOperation), 750) : undefined;
    return () => {
      active = false;
      if (timer !== undefined) window.clearInterval(timer);
    };
  }, [forkOperation, handleOpenSession]);

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

  const handleMoveSession = async (session: SessionSummary, folderId: string) => {
    try {
      await setSessionFolder(session.id, folderId);
      setSessions((current) =>
        current.map((item) => (item.id === session.id ? { ...item, folderId } : item)),
      );
    } catch (err) {
      showToast(`移动会话失败：${err instanceof Error ? err.message : String(err)}`, 'error');
    }
  };

  const handleFolderEditorConfirm = async () => {
    if (!folderEditor?.value.trim()) return;
    try {
      if (folderEditor.folder) {
        await renameFolder(folderEditor.folder.id, folderEditor.value.trim());
      } else {
        const created = await createFolder(folderEditor.value.trim());
        if (folderEditor.moveSession) {
          await setSessionFolder(folderEditor.moveSession.id, created.id);
        }
      }
      setFolderEditor(null);
      void refreshSessions();
    } catch (err) {
      showToast(`保存文件夹失败：${err instanceof Error ? err.message : String(err)}`, 'error');
    }
  };

  const freshSession = !state.handleId && !state.sessionId && state.items.length === 0;
  const automaticFolderId = folders.find(
    (folder) => folder.cwd && sameWorkspacePath(folder.cwd, state.cwd),
  )?.id;
  const activeFolderId =
    sessions.find((session) => session.id === state.historyId)?.folderId ??
    newSessionFolderId ??
    automaticFolderId ??
    (freshSession ? null : 'folder-default');

  return (
    <main
      className={
        'ws' +
        (showCtx ? ' is-context-open' : ' no-ctx') +
        (showSessions ? ' is-sidebar-open' : '') +
        ` is-${viewport}`
      }
    >
      {viewport !== 'wide' && (showCtx || showSessions) ? (
        <button
          className="ws-drawer-backdrop"
          type="button"
          aria-label="关闭侧边面板"
          onClick={() => {
            if (showCtx) closeContextDrawer(true);
            if (showSessions) closeSessionDrawer(true);
          }}
        />
      ) : null}
      <SessionSidebar
        state={state}
        activeOption={activeOption}
        sessions={sessions}
        folders={folders}
        sessionError={sessionError}
        resumingId={resumingId}
        runningIds={runningIds}
        approvalIds={approvalIds}
        onNew={startNewSession}
        onCreateFolder={() => setFolderEditor({ folder: null, value: '' })}
        onToggleFolder={(folder) => {
          setFolders((current) =>
            current.map((item) =>
              item.id === folder.id ? { ...item, collapsed: !item.collapsed } : item,
            ),
          );
          void setFolderCollapsed(folder.id, !folder.collapsed);
        }}
        onMoveSession={(session, folderId) => void handleMoveSession(session, folderId)}
        onCreateFolderForSession={(session) =>
          setFolderEditor({ folder: null, value: '', moveSession: session })
        }
        onRenameFolder={(folder) => setFolderEditor({ folder, value: folder.name })}
        onDeleteFolder={setFolderDeleteTarget}
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
          folders={folders}
          folderId={activeFolderId}
          onSelectFolder={(folderId) => {
            if (state.historyId) {
              const session = sessions.find((item) => item.id === state.historyId);
              if (session) void handleMoveSession(session, folderId);
            } else {
              setNewSessionFolderId(folderId);
            }
          }}
          engineOptions={engineOptions}
          activeOption={activeOption}
          model={state.model || activeModel?.id || '默认模型'}
          runtimeModel={state.runtimeModel}
          onSelectEngine={handleSelectEngine}
          onSelectModel={(model) => void handleSelectModel(model)}
          reasoningEffort={reasoningEffort}
          reasoningCapability={reasoningCapability}
          reasoningLoading={reasoningLoading}
          reasoningDisabled={state.status === 'working'}
          onSelectReasoningEffort={(effort) => void handleSelectReasoningEffort(effort)}
          onToggleSessions={() => {
            setShowCtx(false);
            setShowSessions((value) => !value);
          }}
          sessionsExpanded={showSessions}
          onToggleCtx={() => {
            if (viewport !== 'wide') setShowSessions(false);
            setShowCtx((value) => !value);
          }}
          contextExpanded={showCtx}
          cost={state.cost}
        />
        {state.fork ? (
          <div className="ws-handoff-banner" role="status">
            <Icon name="gitbranch" />
            <span>
              由 {state.fork.sourceEngine === 'codex' ? 'Codex' : 'Claude Code'}{' '}
              会话摘要派生，细节可能有损
            </span>
            {state.fork.sourceSessionId ? (
              <button
                className="btn btn--subtle btn--sm"
                type="button"
                onClick={() => void handleOpenSession(state.fork?.sourceSessionId ?? '')}
              >
                查看源会话
              </button>
            ) : null}
          </div>
        ) : null}
        {!freshSession ? (
          <Thread
            key={state.historyId ?? state.sessionId ?? 'empty'}
            state={state}
            onApprove={approve}
            onRestoreCheckpoint={restoreCheckpoint}
            onUndoRevert={undoRevert}
            locateTarget={activityTarget}
          />
        ) : (
          <div className="ws-new-session">
            <div className="ws-new-session__intro">
              <Icon name="sparkles" />
              <h2>开始新会话</h2>
              <p>描述你希望 Agent 在当前项目中完成的任务。</p>
            </div>
            <div className="ws-new-session__cwd">
              <span>
                <Icon name="folderopen" />
                {state.cwd || '尚未选择工作目录'}
              </span>
              <button
                className="btn btn--subtle btn--sm"
                type="button"
                onClick={handlePickDirectory}
              >
                更换目录…
              </button>
              <small>会话开始后不可更改</small>
            </div>
          </div>
        )}
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
          model={state.model}
          reasoningEffort={reasoningEffort}
          cwd={state.cwd}
          slashCommands={slashCommands}
          skills={skills}
          onModeChange={setTurnMode}
          permissionProfile={permissionProfile}
          onPermissionProfileChange={(profile) => void handlePermissionProfileChange(profile)}
          onCommandAction={handleCommandAction}
          onSend={handleSend}
          onStop={stop}
          cost={state.cost}
          onOpenContext={() => {
            setShowSessions(false);
            setShowCtx(true);
            setContextTabRequest((value) => value + 1);
          }}
        />
      </section>
      <ContextPanel
        state={state}
        permissionProfile={permissionProfile}
        mcpServers={mcpServers}
        skills={skills}
        mcpLoadError={mcpLoadError}
        skillsLoadError={skillsLoadError}
        onRetryExtensions={loadContextExtensions}
        onToggleMcp={toggleMcpServer}
        onOpenExtensions={() =>
          window.dispatchEvent(new CustomEvent('helm:navigate', { detail: { page: 'extensions' } }))
        }
        openContextRequest={contextTabRequest}
        onLocateItem={locateActivityItem}
        onLocateChange={(path, lineNumber, toolId) => {
          // 批次 I：变更导航器跳转
          locateActivityItem(toolId);
          showToast(`跳转到 ${path}:${lineNumber}`, 'info', 2000);
        }}
      />
      {renameTarget ? (
        <RenameSessionDialog
          value={renameValue}
          onChange={setRenameValue}
          onCancel={() => setRenameTarget(null)}
          onConfirm={handleRenameConfirm}
        />
      ) : null}
      {deleteTarget ? (
        <DeleteSessionDialog
          title={deleteTarget.title}
          onCancel={() => setDeleteTarget(null)}
          onConfirm={handleDeleteConfirm}
        />
      ) : null}
      {folderDeleteTarget ? (
        <DeleteFolderDialog
          name={folderDeleteTarget.name}
          fallbackName={folders.find((folder) => folder.id === 'folder-default')?.name ?? '默认'}
          onCancel={() => setFolderDeleteTarget(null)}
          onConfirm={async () => {
            try {
              await deleteFolder(folderDeleteTarget.id);
              if (newSessionFolderId === folderDeleteTarget.id) {
                setNewSessionFolderId(undefined);
              }
              setFolderDeleteTarget(null);
              void refreshSessions();
            } catch (error) {
              showToast(error instanceof Error ? error.message : '删除文件夹失败', 'error');
            }
          }}
        />
      ) : null}
      {folderEditor ? (
        <FolderEditorDialog
          folder={folderEditor.folder}
          value={folderEditor.value}
          onChange={(value) =>
            setFolderEditor((current) => (current ? { ...current, value } : null))
          }
          onCancel={() => setFolderEditor(null)}
          onConfirm={handleFolderEditorConfirm}
        />
      ) : null}
      {identitySwitch ? (
        <IdentitySwitchDialog
          pending={identitySwitch}
          operation={forkOperation}
          starting={forkStarting}
          onCancel={() => {
            if (forkOperation && ['committed', 'running'].includes(forkOperation.status)) {
              void cancelBackgroundOperation(forkOperation.id).then(() => {
                setForkOperation((current) =>
                  current ? { ...current, status: 'cancelled' } : current,
                );
              });
              return;
            }
            setForkOperation(null);
            setIdentitySwitch(null);
          }}
          onRetry={() => {
            if (!forkOperation) return;
            void retryBackgroundOperation(forkOperation.id)
              .then(() => setForkOperation({ ...forkOperation, status: 'committed' }))
              .catch((error: unknown) =>
                showToast(error instanceof Error ? error.message : '重试交接任务失败', 'error'),
              );
          }}
          onConfirm={() => void confirmIdentitySwitch()}
        />
      ) : null}
    </main>
  );
}

function IdentitySwitchDialog({
  pending,
  operation,
  starting,
  onCancel,
  onRetry,
  onConfirm,
}: {
  pending: IdentitySwitch;
  operation: BackgroundOperation | null;
  starting: boolean;
  onCancel: () => void;
  onRetry: () => void;
  onConfirm: () => void;
}) {
  const dialogRef = useDialogBehavior(onCancel);
  const target = `引擎「${pending.label}」`;

  return (
    <div
      className="modal-overlay"
      onMouseDown={(event) => event.target === event.currentTarget && onCancel()}
    >
      <div
        ref={dialogRef}
        className="modal-panel"
        role="dialog"
        aria-modal="true"
        aria-labelledby="identity-switch-title"
        tabIndex={-1}
      >
        <div className="modal-panel__head">
          <b id="identity-switch-title">创建新会话</b>
          <button className="btn-icon sm" onClick={onCancel} aria-label="关闭" type="button">
            ×
          </button>
        </div>
        <div className="modal-panel__body">
          <p>
            切换到{target}会生成交接摘要并创建新的
            Session，当前会话、消息、用量和检查点会保留在侧栏中。
          </p>
          <p className="hint" style={{ marginTop: 8 }}>
            摘要由目标 Engine 的真实 CLI
            生成，细节可能有损。任务只读取已完成的轮次，不使用工具或已保存授权。
          </p>
          {operation ? (
            <p
              className={operation.status === 'failed' ? 'error' : 'hint'}
              style={{ marginTop: 8 }}
            >
              {operation.status === 'committed' || operation.status === 'running'
                ? '正在生成交接摘要…'
                : operation.status === 'failed' || operation.status === 'delivery_unknown'
                  ? operation.errorCode || '交接任务失败'
                  : operation.status === 'cancelled'
                    ? '交接任务已取消'
                    : null}
            </p>
          ) : null}
        </div>
        <div className="modal-panel__foot">
          <button className="btn btn--sm" onClick={onCancel} type="button">
            {operation && ['committed', 'running'].includes(operation.status) ? '停止任务' : '取消'}
          </button>
          {operation && ['failed', 'cancelled', 'delivery_unknown'].includes(operation.status) ? (
            <button className="btn btn--primary btn--sm" onClick={onRetry} type="button">
              重试
            </button>
          ) : operation ? null : (
            <button
              className="btn btn--primary btn--sm"
              onClick={onConfirm}
              disabled={starting}
              type="button"
            >
              {starting ? '正在创建…' : '生成摘要并继续'}
            </button>
          )}
        </div>
      </div>
    </div>
  );
}

function RenameSessionDialog({
  value,
  onChange,
  onCancel,
  onConfirm,
}: {
  value: string;
  onChange: (value: string) => void;
  onCancel: () => void;
  onConfirm: () => Promise<void>;
}) {
  const dialogRef = useDialogBehavior(onCancel);
  return (
    <div
      className="modal-overlay"
      onMouseDown={(event) => event.target === event.currentTarget && onCancel()}
    >
      <div
        ref={dialogRef}
        className="modal-panel"
        role="dialog"
        aria-modal="true"
        aria-labelledby="rename-session-title"
        tabIndex={-1}
      >
        <div className="modal-panel__head">
          <b id="rename-session-title">重命名会话</b>
          <button className="btn-icon sm" onClick={onCancel} aria-label="关闭" type="button">
            ×
          </button>
        </div>
        <div className="modal-panel__body">
          <input
            type="text"
            className="input"
            style={{ width: '100%' }}
            value={value}
            aria-label="会话标题"
            onChange={(event) => onChange(event.target.value)}
            onKeyDown={(event) => {
              if (event.nativeEvent.isComposing) return;
              if (event.key === 'Enter') void onConfirm();
            }}
          />
        </div>
        <div className="modal-panel__foot">
          <button className="btn btn--sm" type="button" onClick={onCancel}>
            取消
          </button>
          <button
            className="btn btn--primary btn--sm"
            type="button"
            disabled={!value.trim()}
            onClick={() => void onConfirm()}
          >
            保存
          </button>
        </div>
      </div>
    </div>
  );
}

function DeleteSessionDialog({
  title,
  onCancel,
  onConfirm,
}: {
  title: string;
  onCancel: () => void;
  onConfirm: () => Promise<void>;
}) {
  const dialogRef = useDialogBehavior(onCancel);
  return (
    <div
      className="modal-overlay"
      onMouseDown={(event) => event.target === event.currentTarget && onCancel()}
    >
      <div
        ref={dialogRef}
        className="modal-panel"
        role="dialog"
        aria-modal="true"
        aria-labelledby="delete-session-title"
        tabIndex={-1}
      >
        <div className="modal-panel__head">
          <b id="delete-session-title">删除会话</b>
          <button className="btn-icon sm" onClick={onCancel} aria-label="关闭" type="button">
            ×
          </button>
        </div>
        <div className="modal-panel__body">
          <p>
            确定删除「{title}
            」吗？会话的消息、工具记录、用量与检查点快照将一并删除，运行中的后台轮次会被终止。此操作不可撤销。
          </p>
        </div>
        <div className="modal-panel__foot">
          <button className="btn btn--sm" type="button" onClick={onCancel}>
            取消
          </button>
          <button
            className="btn btn--danger btn--sm"
            type="button"
            onClick={() => void onConfirm()}
          >
            删除
          </button>
        </div>
      </div>
    </div>
  );
}

function DeleteFolderDialog({
  name,
  fallbackName,
  onCancel,
  onConfirm,
}: {
  name: string;
  fallbackName: string;
  onCancel: () => void;
  onConfirm: () => Promise<void>;
}) {
  const dialogRef = useDialogBehavior(onCancel);
  return (
    <div
      className="modal-overlay"
      onMouseDown={(event) => event.target === event.currentTarget && onCancel()}
    >
      <div
        ref={dialogRef}
        className="modal-panel"
        role="dialog"
        aria-modal="true"
        aria-labelledby="delete-folder-title"
        tabIndex={-1}
      >
        <div className="modal-panel__head">
          <b id="delete-folder-title">删除文件夹</b>
          <button className="btn-icon sm" onClick={onCancel} aria-label="关闭" type="button">
            ×
          </button>
        </div>
        <div className="modal-panel__body">
          <p>
            确定删除「{name}」吗？其中的会话会移入「{fallbackName}」，不会删除任何会话内容。
          </p>
        </div>
        <div className="modal-panel__foot">
          <button className="btn btn--sm" type="button" onClick={onCancel}>
            取消
          </button>
          <button
            className="btn btn--danger btn--sm"
            type="button"
            onClick={() => void onConfirm()}
          >
            删除
          </button>
        </div>
      </div>
    </div>
  );
}

function FolderEditorDialog({
  folder,
  value,
  onChange,
  onCancel,
  onConfirm,
}: {
  folder: SessionFolder | null;
  value: string;
  onChange: (value: string) => void;
  onCancel: () => void;
  onConfirm: () => Promise<void>;
}) {
  const dialogRef = useDialogBehavior(onCancel);
  const title = folder ? '重命名文件夹' : '新建文件夹';
  return (
    <div
      className="modal-overlay"
      onMouseDown={(event) => event.target === event.currentTarget && onCancel()}
    >
      <div
        ref={dialogRef}
        className="modal-panel"
        role="dialog"
        aria-modal="true"
        aria-labelledby="folder-editor-title"
        tabIndex={-1}
      >
        <div className="modal-panel__head">
          <b id="folder-editor-title">{title}</b>
          <button className="btn-icon sm" onClick={onCancel} aria-label="关闭" type="button">
            ×
          </button>
        </div>
        <div className="modal-panel__body">
          <label className="field">
            <span className="label">文件夹名称</span>
            <input
              className="input"
              autoFocus
              maxLength={80}
              value={value}
              onChange={(event) => onChange(event.target.value)}
              onKeyDown={(event) => {
                if (!event.nativeEvent.isComposing && event.key === 'Enter' && value.trim())
                  void onConfirm();
              }}
            />
          </label>
        </div>
        <div className="modal-panel__foot">
          <button className="btn btn--sm" type="button" onClick={onCancel}>
            取消
          </button>
          <button
            className="btn btn--primary btn--sm"
            type="button"
            disabled={!value.trim()}
            onClick={() => void onConfirm()}
          >
            保存
          </button>
        </div>
      </div>
    </div>
  );
}
