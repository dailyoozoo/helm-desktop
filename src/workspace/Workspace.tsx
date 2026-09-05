import { useCallback, useEffect, useMemo, useRef, useState, useSyncExternalStore } from 'react';
import { listen as tauriListen } from '@tauri-apps/api/event';
import { markRailTaskSeen } from '../shell/railSeen';
import type { ActiveSessionIds } from '../shell/railViewModel';
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
  compactContext,
  type PermissionProfile,
} from '../engine/transport';
import { showToast } from '../components/toast';
import { Icon } from '../shell/icons';
import { bindingModelLabel } from '../providers/providerViewModel';
import {
  Dialog as ShadcnDialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogFooter,
} from '@/components/ui/dialog';
import { Button } from '@/components/ui/button';
import { isTauriRuntime } from '../lib/env';
import { sendNotification } from '@tauri-apps/plugin-notification';
import { SessionSidebar } from './SessionSidebar';
import { Thread } from './Thread';
import { Composer } from './Composer';
import { statusBarLabel } from './StatusBar';
import { Workstrip, collectWorkstripAgents, workstripTodo } from './Workstrip';
import { CompactBanner } from './CompactBanner';
import { contextRingState } from './ContextRing';
import { ContextPanel } from './ContextPanel';
import type { ArtifactPaneTab, ContextPanelFixedTab } from './contextPanelViewModel';
import { billingSummary } from './contextPanelViewModel';
import type { ContextRingDetail, SessionContextEditActions } from './ContextRing';
import { open as openPathDialog } from '@tauri-apps/plugin-dialog';
import { contextSnapshot, type ContextSnapshotViewModel } from './contextSnapshotViewModel';
import { retryRequestText } from './items/failureCardViewModel';
import { ResizablePane } from './ResizablePane';
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
  addSessionContext,
  cancelBackgroundOperation,
  deleteSession,
  getActiveSession,
  getBackgroundOperation,
  getSessionHistory,
  listFolders,
  listSessions,
  listSessionContexts,
  renameSession,
  resumeSession,
  retryBackgroundOperation,
  removeSessionContext,
  setFolderCollapsed,
  setSessionArchived,
  setSessionPinned,
  startSessionBranch,
  startSessionFork,
  type BackgroundOperation,
  type SessionContextRecord,
  type SessionDetail,
} from '../sessions/api';
import { discardHistoryPreview, publishHistoryOnly, publishResume } from '../sessions/resumeBridge';
import { forkTrace } from '../diag/forkTrace';
import type { SessionFolder, SessionSummary } from '../sessions/sessionTypes';
import type { AppSettings } from '../settings/types';
import { selectDirectory, detectEngine } from '../settings/api';
import {
  invalidateSharedDepsCache,
  probeSharedDepsCached,
  SetupGuide,
  type WorkspaceDeps,
} from './SetupGuide';
import type { EngineId, ReasoningEffort } from '@helm/protocol';
import { useReasoningEffortCapability } from '../engine/useReasoningEffortCapability';
import { normalizeReasoningEffort } from '../reasoning';
import {
  defaultTurnModeForEngine,
  defaultTurnModeFromSettings,
  sessionDefaultsFromSettings,
  shouldReopenLastSession,
} from '../settings/settingsViewModel';
import type { NewTaskLaunchConfig } from '../home/newTaskViewModel';
import type { TurnMode } from '../engine/transport';
import {
  defaultModelForEngine,
  workspaceEngineOptions,
  workspaceSessionIsActive,
} from './workspaceViewModel';
import { expandSlashCommandDetailed } from './slashCommands';
import {
  getTranscriptDensity,
  nextTranscriptDensity,
  setTranscriptDensity,
  useTranscriptDensity,
} from './transcriptDensity';
import { isGenuineTurnEnd } from './turnNotification';
import type { SessionTurn } from '../sessions/api';
import { lastTurnPrefs } from './lastTurnPrefs';

type IdentitySwitch = {
  kind: 'engine';
  engine: EngineId;
  model: string;
  label: string;
  /** 变更-34/35 · B4：同引擎派生（压缩 banner 触发），文案与跨引擎切换区分。 */
  sameEngine?: boolean;
};

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
  draftRequest,
  onDraftConsumed,
  launching = false,
  onGitInfoChange,
  onSessionTitleChange,
  onContextExpandedChange,
}: {
  settings: AppSettings;
  onSettingsChange?: (updater: (prev: AppSettings) => AppSettings) => void;
  newSessionRequest?: number;
  toggleContextRequest?: number;
  cycleEngineRequest?: number;
  pendingSessionId?: string | null;
  onClearPendingSessionId?: () => void;
  draftRequest?: {
    id: number;
    text: string;
    attachments?: string[];
    /** S2：新任务页的就绪选择（引擎/目录/模式/权限/模型/强度），随首条消息一次性生效 */
    launch?: NewTaskLaunchConfig;
  } | null;
  onDraftConsumed?: () => void;
  /**
   * 新任务启动过渡仍在进行（App 的 LaunchOverlay 尚未揭开）。draftRequest 会在 Composer
   * 挂载首帧被消费清空，不能再用它判断「会话创建中」，否则线程区会在会话建立前回落到
   * 与新任务页雷同的「开始新会话」空态（2026-08-30 用户报告的「跳回新任务页」）。
   */
  launching?: boolean;
  /** 当 git 信息变化时回调（用于标题栏显示） */
  onGitInfoChange?: (info: { projectName?: string; branchName?: string }) => void;
  /** 当前任务标题上报（2026-08-27 对齐原型：任务标题入全局标题栏，ThreadHead 行退役）。 */
  onSessionTitleChange?: (title: string | null) => void;
  /** 右栏展开态上报（标题栏开关 aria-expanded 用）。 */
  onContextExpandedChange?: (expanded: boolean) => void;
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
  // 会话模式（变更-04 B.1）：轮次级状态放 Workspace，Composer 受控；
  // 权限初始档位 = 新默认组合「自动执行」（2026-09-04，新对话默认构建+自动执行）。
  const [turnMode, setTurnMode] = useState<TurnMode>(defaultTurnMode);
  const [permissionProfile, setPermissionProfile] = useState<PermissionProfile>('auto');
  const [fullAccessConfirmed, setFullAccessConfirmed] = useState(false);
  const previousEngine = useRef(state.engine);
  const [reasoningEffort, setReasoningEffort] = useState<ReasoningEffort>('auto');
  const [viewport, setViewport] = useState<WorkspaceViewport>(workspaceViewport);
  // D-7（可靠性检查-工作区对话页-差异清单，2026-08-25 用户裁决「原则上跟原型一致」）：
  // 右栏默认关闭（对齐原型 boot 行为）；线程卡片/头部开关可打开。
  // 推翻 P0-01「有真实 diff 宽屏自动打开」——sbar 常驻后三栏并存会挤压线程。
  const [showCtx, setShowCtx] = useState(false);
  // 变更-34 · A4：请求打开交付物 tab（request 递增触发）；S3 后常驻 tab 直接切换、动态 tab 打开
  const [paneRequest, setPaneRequest] = useState<{
    tab: ContextPanelFixedTab | ArtifactPaneTab;
    request: number;
  } | null>(null);
  const openArtifactPane = useCallback((tab: ContextPanelFixedTab | ArtifactPaneTab) => {
    setShowSessions(false);
    setShowCtx(true);
    setPaneRequest((current) => ({ tab, request: (current?.request ?? 0) + 1 }));
  }, []);
  const [activityTarget, setActivityTarget] = useState<{ id: string; request: number } | null>(
    null,
  );
  const activityRequestRef = useRef(0);
  const activityClearTimerRef = useRef<number | null>(null);
  // 批次①用户裁决（覆盖原 D-1 常驻列方案）：任务列表侧栏全视口默认收起为抽屉
  // （原型 .sbar display:none，会话入口在主栏「最近任务」）；原型已删除抽屉唤起按钮，
  // 抽屉机制保留（Esc/背板关闭路径仍在）。
  const [showSessions, setShowSessions] = useState(false);
  const [config, setConfig] = useState<AppConfig | null>(null);
  const [sessions, setSessions] = useState<SessionSummary[]>([]);
  const [folders, setFolders] = useState<SessionFolder[]>([]);
  const [slashCommands, setSlashCommands] = useState<SlashCommand[]>([]);
  const [mcpServers, setMcpServers] = useState<McpServer[]>([]);
  // 变更-34/35 · D4/E2 · 切片 D（P1-02）：Composer 上下文圆环的完整明细。
  // 切片 D 把圆环与右栏「上下文」tab 改为共用同一份 ContextSnapshotViewModel 派生：
  // 历史附件 / 会话上下文 / 有效 MCP / 归因 一次性派生，避免两处各写口径。
  // 归因永远为空数组（context_usage 协议无逐来源字段；AGENTS.md 红线禁止估算）。
  const [ringSessionContexts, setRingSessionContexts] = useState<SessionContextRecord[]>([]);
  // S3：右栏「上下文」tab 撤销后，会话上下文的增删只从 Composer 圆环 popover 进入；
  // Workspace 持有真实 list/add/remove_session_contexts 命令与刷新。
  const reloadRingSessionContexts = useCallback(async () => {
    if (!state.historyId) {
      setRingSessionContexts([]);
      return;
    }
    try {
      setRingSessionContexts(await listSessionContexts(state.historyId));
    } catch {
      setRingSessionContexts([]);
    }
  }, [state.historyId]);
  useEffect(() => {
    void reloadRingSessionContexts();
  }, [reloadRingSessionContexts, state.status]);
  const [ctxEditBusy, setCtxEditBusy] = useState(false);
  const ctxErrorText = (label: string, error: unknown) =>
    label + (error instanceof Error ? error.message : String(error));
  const addRingSessionContext = useCallback(
    async (directory: boolean) => {
      if (!state.historyId || state.status === 'working' || ctxEditBusy) return;
      setCtxEditBusy(true);
      try {
        const selected = await openPathDialog({ multiple: true, directory });
        const paths = typeof selected === 'string' ? [selected] : (selected ?? []);
        let added = 0;
        for (const path of paths) {
          try {
            await addSessionContext(state.historyId, path);
            added += 1;
          } catch (error) {
            showToast(ctxErrorText('添加会话上下文失败：', error), 'error');
          }
        }
        if (added > 0) {
          showToast('已加入 ' + added + ' 项到会话上下文（只影响后续轮次）', 'info');
        }
        await reloadRingSessionContexts();
      } finally {
        setCtxEditBusy(false);
      }
    },
    [state.historyId, state.status, ctxEditBusy, reloadRingSessionContexts],
  );
  const removeRingSessionContext = useCallback(
    async (contextId: string) => {
      if (!state.historyId || state.status === 'working' || ctxEditBusy) return;
      setCtxEditBusy(true);
      try {
        await removeSessionContext(state.historyId, contextId);
        await reloadRingSessionContexts();
      } catch (error) {
        showToast(ctxErrorText('移除会话上下文失败：', error), 'error');
      } finally {
        setCtxEditBusy(false);
      }
    },
    [state.historyId, state.status, ctxEditBusy, reloadRingSessionContexts],
  );
  const sessionContextEdit = useMemo<SessionContextEditActions>(
    () => ({
      enabled: Boolean(state.historyId) && state.status !== 'working',
      busy: ctxEditBusy,
      onAddFile: () => void addRingSessionContext(false),
      onAddDirectory: () => void addRingSessionContext(true),
      onRemove: (contextId) => void removeRingSessionContext(contextId),
    }),
    [state.historyId, state.status, ctxEditBusy, addRingSessionContext, removeRingSessionContext],
  );
  const contextSnapshotView = useMemo<ContextSnapshotViewModel | undefined>(
    () =>
      contextSnapshot({
        items: state.items,
        cost: state.cost,
        mcpServers,
        disabledMcp: state.disabledMcp,
        sessionContexts: ringSessionContexts,
      }),
    [state.items, state.cost, mcpServers, state.disabledMcp, ringSessionContexts],
  );
  const contextDetail: ContextRingDetail = useMemo(() => {
    const billing = billingSummary(state.cost);
    return {
      cost: state.cost,
      billing,
      messageCount: contextSnapshotView?.messageCount,
      startedAt: state.startedAt,
      attribution: [],
    };
  }, [state.cost, state.startedAt, contextSnapshotView]);
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
  // 恢复窗口（2026-09-04 闪现修复第三刀）：Workspace 挂载首帧到 autoResume 真正起跑
  // （setResumingId）之间还有一帧「无会话且不在恢复」的空窗，会闪「开始新会话」空态。
  // 按设置先行假定「可能要恢复」渲染打开中占位，由 autoResume 的各早退路径显式关窗：
  // 草稿/启动流、明确不恢复、指针为 null（无可恢复任务）、恢复失败。恢复成功则整帧
  // 被线程内容替换，无需关窗。
  const [restoreWindowOpen, setRestoreWindowOpen] = useState(
    () => settings.general.reopenLastSession,
  );
  const [sendBlocker, setSendBlocker] = useState<{
    message: string;
    action: 'providers' | 'directory' | 'setup';
  } | null>(null);
  const [setupGuideVisible, setSetupGuideVisible] = useState(false);
  const [setupProbeRequest, setSetupProbeRequest] = useState(0);
  const [identitySwitch, setIdentitySwitch] = useState<IdentitySwitch | null>(null);
  const [forkOperation, setForkOperation] = useState<BackgroundOperation | null>(null);
  const [forkStarting, setForkStarting] = useState(false);
  // 变更-34/35 · B4 · P1-05：压缩提醒按 Session 隔离。
  // dismiss key 使用 historyId/sessionId（新会话即句柄 id）。
  // 切换/新建会话时新身份不在集合中 → 重新提醒。
  // 占用回落 80% 以下后再次跨阈值 → 允许重新提醒（清除该会话的 dismiss）。
  const compactDismissedRef = useRef<Set<string>>(new Set());
  const compactLastPercentRef = useRef<Map<string, number>>(new Map());
  // ref 变化不触发重渲染，用 epoch 计数器强制刷新 CompactBanner 区域
  const [, setCompactEpoch] = useState(0);
  const compactDismissKey = state.historyId ?? state.sessionId ?? state.handleId;
  const compactPercent =
    contextRingState(state.cost.contextTokens, state.cost.contextWindow).percent ?? 0;
  // 占用回落 80% 以下时清除当前会话的 dismiss，允许跨阈值时重新提醒
  useEffect(() => {
    if (!compactDismissKey) return;
    const last = compactLastPercentRef.current.get(compactDismissKey) ?? 0;
    if (last >= 80 && compactPercent < 80 && compactDismissedRef.current.has(compactDismissKey)) {
      compactDismissedRef.current.delete(compactDismissKey);
      setCompactEpoch((value) => value + 1);
    }
    compactLastPercentRef.current.set(compactDismissKey, compactPercent);
  }, [compactPercent, compactDismissKey]);
  const compactDismissed =
    compactDismissKey != null && compactDismissedRef.current.has(compactDismissKey);
  const dismissCompact = useCallback(() => {
    if (compactDismissKey) {
      compactDismissedRef.current.add(compactDismissKey);
      setCompactEpoch((value) => value + 1);
    }
  }, [compactDismissKey]);
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
    // 批次①用户裁决：任务列表侧栏全视口默认收起（原型隐藏该列），仅按钮唤起抽屉。
    setShowSessions(false);
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
  // 变更-34/35 · B1：线程视图密度（全局，localStorage 持久化），供密度下拉与 Ctrl+O。
  const transcriptDensity = useTranscriptDensity();
  // 变更-34/35 · B2：当前会话的 TurnLedger 逐轮真值，供轮次摘要头读取模型等字段。
  const [turnLedger, setTurnLedger] = useState<SessionTurn[]>([]);

  // 2026-08-28 修复「失败轮次卡进行中」：TurnLedger 只在打开会话时加载一次，
  // 轮次失败（如模型不可用错误卡）后若不重拉，ledger 里该轮仍是 running，
  // TurnProcess 的 completed 判定永远为 false，线程头部一直显示「进行中」。
  // 轮次从运行中回到空闲（成功/失败/中断都会走）时刷新一次账本。
  const prevStatusRef = useRef(state.status);
  useEffect(() => {
    const wasWorking = prevStatusRef.current === 'working';
    prevStatusRef.current = state.status;
    if (!wasWorking || state.status === 'working') return;
    const sessionId = state.historyId;
    if (!sessionId) return;
    let active = true;
    getSessionHistory(sessionId)
      .then((session) => {
        if (active) setTurnLedger(session.turns ?? []);
      })
      .catch(() => {
        // 刷新失败保持旧账本；下次打开会话时会重新加载
      });
    return () => {
      active = false;
    };
  }, [state.status, state.historyId]);

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

  // 价格目录刷新成功（设置页「立即更新价格」/ 后台自动更新 / 离线导入）后
  // 后端 emit("helm-pricing-catalog-updated")。重拉服务商配置，让模型列表
  // input/output 价格和 priceSource 即时反映新目录，不必重启或手动同步。
  // 新会话刚建立（sessionId/historyId 从空变为有值）时主动刷一次侧栏，
  // 确保发送后新建的任务立即出现在左侧「任务列表」中（对齐原型：任务已触发即入列）。
  const hadActiveSessionRef = useRef(false);
  useEffect(() => {
    const has = Boolean(state.sessionId) || Boolean(state.historyId);
    if (has && !hadActiveSessionRef.current) {
      hadActiveSessionRef.current = true;
      void refreshSessions();
    } else if (!has) {
      hadActiveSessionRef.current = false;
    }
  }, [state.sessionId, state.historyId]);

  useEffect(() => {
    let active = true;
    let unlisten: (() => void) | null = null;
    void tauriListen('helm-pricing-catalog-updated', () => {
      if (!active) return;
      void getProviderConfig()
        .then((next) => {
          if (active) setConfig(next);
        })
        .catch((err) => {
          if (active && isTauriRuntime()) {
            console.error('价格目录刷新后重拉服务商配置失败:', err);
          }
        });
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
    activeOption?.models.find((model) => model.id === activeOption.binding?.primaryModel) ??
    null;
  // 方案 b：绑定到角色时显示角色名（发送仍由后端按角色解析）
  const activeModelLabel = activeModel
    ? activeModel.displayName && activeModel.displayName !== activeModel.id
      ? `${activeModel.displayName} · ${activeModel.id}`
      : activeModel.id
    : activeOption?.binding?.primaryModel
      ? bindingModelLabel(activeOption.binding.primaryModel)
      : '';
  const activeSession = sessions.find((session) => session.id === state.historyId);
  const { capability: reasoningCapability, loading: reasoningLoading } =
    useReasoningEffortCapability(
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
    // 2026-08-27 用户裁决：档位跟随 Agent——normalize 内部以 effortOptionsFor 口径处理
    // （探测 supported 以探测为准 / unknown 回落引擎档位表 / unsupported 仅自动）；
    // 探测失败（capability 为 null）不强行改写用户选择，菜单按引擎档位表兜底展示。
    if (!reasoningCapability) return;
    const normalized = normalizeReasoningEffort(reasoningCapability, reasoningEffort, state.engine);
    if (normalized !== reasoningEffort) {
      setReasoningEffort(normalized);
      if (state.handleId) {
        void setSessionTurnPreference(state.handleId, state.model, normalized).catch((error) =>
          showToast(error instanceof Error ? error.message : '保存推理强度回落结果失败', 'error'),
        );
      }
    }
  }, [
    reasoningCapability,
    reasoningEffort,
    reasoningLoading,
    state.engine,
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
      // 新默认组合（9/4）：新对话/切引擎按「构建+自动执行」起步，不再是 standard。
      setPermissionProfile('auto');
      setFullAccessConfirmed(false);
      selectEngine(engineId, model);
      // 变更-37 · 切引擎重新探测当前引擎 CLI（共享依赖缓存复用，CLI 行刷新）
      invalidateSharedDepsCache();
      setSetupProbeRequest((value) => value + 1);
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

  // 同引擎派生（2026-09-02）：优先走无损分支（Codex `thread/fork` /
  // claude `--fork-session`），CLI 不支持时才回退摘要派生轮询。
  // 对话内「分叉」图标直接调它、不弹确认框（对齐 ChatGPT/Claude：点击即分叉 + 轻提示）；
  // 跨引擎切换仍走 IdentitySwitchDialog（有损、需确认）。
  // 修复前对话内分叉无条件调 startSessionFork，Codex 的 model_only_operation
  // 硬编码 Unsupported，必报 [operation_tools_not_disableable]。
  const runSameEngineBranch = useCallback(
    async (historyId: string, boundaryTurnId?: string) => {
      setForkStarting(true);
      forkTrace('workspace_fork_click', `source=${historyId}`);
      try {
        const outcome = await startSessionBranch(historyId, boundaryTurnId);
        if (outcome.mode === 'lossless') {
          setIdentitySwitch(null);
          // 契约防御（2026-09-04 埋点实证）：后端序列化错位曾让 sessionId=undefined，
          // 事件带着空值派发 → App 静默丢弃 →「点了没反应」。坏载荷就地暴露。
          if (!outcome.sessionId) {
            forkTrace('workspace_fork_bad_payload', `raw=${JSON.stringify(outcome)}`);
            showToast('分叉已创建，但返回载荷缺少会话标识（前端契约错误，请反馈）', 'error');
            void refreshSessions();
            return;
          }
          showToast('已创建分支 · 原会话完整保留', 'success');
          void refreshSessions();
          forkTrace('workspace_fork_dispatch', `target=${outcome.sessionId}`);
          window.dispatchEvent(
            new CustomEvent('helm:open-session', { detail: { sessionId: outcome.sessionId } }),
          );
          return;
        }
        // CLI 不支持无损分支：回退摘要派生，用弹窗承载进度/重试。
        setIdentitySwitch({
          kind: 'engine',
          engine: state.engine,
          model: state.model,
          label: state.engine === 'codex' ? 'Codex' : 'Claude Code',
          sameEngine: true,
        });
        setForkOperation(outcome.operation);
      } catch (error) {
        // Tauri invoke 失败以字符串 reject（十一次反馈）：原样透出后端 tag，不吞原因。
        const reason =
          typeof error === 'string' && error.trim()
            ? '派生失败：' + error
            : error instanceof Error && error.message
              ? error.message
              : '创建派生会话失败';
        forkTrace('workspace_fork_error', `source=${historyId} err=${reason}`);
        showToast(reason, 'error');
      } finally {
        setForkStarting(false);
      }
    },
    [refreshSessions, showToast, state.engine, state.model],
  );

  const confirmIdentitySwitch = useCallback(async () => {
    const pending = identitySwitch;
    if (!pending) return;
    if (!state.historyId) {
      reset();
      // 新默认组合（9/4）：新对话按「构建+自动执行」起步。
      setPermissionProfile('auto');
      setFullAccessConfirmed(false);
      setReasoningEffort(
        config?.bindings.find((binding) => binding.engineId === pending.engine)?.reasoningEffort ??
          'auto',
      );
      setTurnMode(defaultTurnModeForEngine(settings, pending.engine));
      selectEngine(pending.engine, pending.model);
      setIdentitySwitch(null);
      return;
    }
    // 同引擎派生：直接执行，不弹确认（对话内分叉图标已走这条；从弹窗进来的兜底也走这里）。
    if (pending.sameEngine) {
      setIdentitySwitch(null);
      await runSameEngineBranch(state.historyId);
      return;
    }
    setForkStarting(true);
    try {
      const operation = await startSessionFork(state.historyId, pending.engine);
      setForkOperation(operation);
    } catch (error) {
      const reason =
        typeof error === 'string' && error.trim()
          ? '跨引擎交接失败：' + error
          : error instanceof Error && error.message
            ? error.message
            : '创建跨引擎交接任务失败';
      showToast(reason, 'error');
    } finally {
      setForkStarting(false);
    }
  }, [
    config?.bindings,
    identitySwitch,
    reset,
    runSameEngineBranch,
    selectEngine,
    settings,
    showToast,
    state.historyId,
  ]);

  // 变更-34/35 · B4：同引擎派生（对话内分叉图标）。2026-09-02 简化（对齐同业）：
  // 点击即无损分支 + 轻提示，不再弹「创建新会话」确认框——大段机制说明与实际
  // 逻辑对不上；只有 CLI 不支持无损分支、回退摘要派生时才由 runSameEngineBranch
  // 拉起弹窗承载进度与重试。
  const handleForkSameEngine = useCallback(
    (turnId?: string) => {
      if (!state.historyId) {
        showToast('尚未开始会话，无法派生', 'error');
        return;
      }
      if (state.status === 'working') {
        showToast('当前轮次运行中，请先停止再派生', 'error');
        return;
      }
      void runSameEngineBranch(state.historyId, turnId);
    },
    [runSameEngineBranch, showToast, state.historyId, state.status],
  );

  // 变更-34/35 · B4：Codex 原生压缩（app-server `thread/compact/start`，2026-08-12 更正）。
  // P0-04：RPC 立即返回时只提示「已提交压缩」，完成事件由 contextCompaction 生命周期上报，
  // 线程内渲染 compact 记录卡（submitted/running/succeeded/failed）。
  const handleCompactContext = useCallback(async () => {
    if (!state.handleId) {
      showToast('尚未开始会话，无法压缩', 'error');
      return;
    }
    try {
      await compactContext(state.handleId);
      showToast('已提交上下文压缩，完成后线程内会保留压缩记录', 'info');
    } catch (error) {
      showToast(`压缩失败：${String(error)}`, 'error');
    }
  }, [showToast, state.handleId]);

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

  /**
   * 新建会话（2026-09-04 组合持久化口径更新）：
   * - 模式沿用当前选择（对话内切换过就取最新），不再无条件回落设置默认；
   * - 权限沿用当前选择，但 full_access lease 只对单个会话生效、不跨会话携带，
   *   新会话降档到「自动执行」；首个会话/初始态落到新默认「构建+自动执行」。
   */
  const startNewSession = useCallback(() => {
    const defaultOption = engineOptions.find(
      (option) => option.engine.id === sessionDefaults.engine,
    );
    // 新建会话沿用当前会话的工作目录；只有当前草稿尚未有目录时才使用设置默认值。
    reset({
      engine: sessionDefaults.engine,
      cwd: state.cwd || sessionDefaults.cwd,
    });
    // 模式沿用当前值不动（对话内切换过就取最新；Workspace 层 turnMode 本就不随 reset 重置）
    setFullAccessConfirmed(false);
    setPermissionProfile((current) => (current === 'full_access' ? 'auto' : current));
    setReasoningEffort(defaultOption?.binding?.reasoningEffort ?? 'auto');
  }, [engineOptions, reset, sessionDefaults.cwd, sessionDefaults.engine, state.cwd]);
  // 恢复历史会话或切换引擎后，不能继续沿用另一个 Engine 的模式。
  // S2：新任务启动配置显式带模式时，抑制一次回落（launchModeOnceRef）。
  useEffect(() => {
    if (previousEngine.current === state.engine) return;
    previousEngine.current = state.engine;
    setTurnMode(engineDefaultTurnMode);
  }, [engineDefaultTurnMode, state.engine]);

  // 设置里的默认模式变化时，未开场的空会话跟随（对齐 apply_defaults 的语义）
  useEffect(() => {
    if (state.handleId || state.sessionId || state.items.length > 0) return;
    if (launchModeOnceRef.current) return;
    setTurnMode(engineDefaultTurnMode);
  }, [engineDefaultTurnMode, state.handleId, state.sessionId, state.items.length]);

  useEffect(() => {
    if (newSessionRequest <= lastNewSessionRequest.current) return;
    lastNewSessionRequest.current = newSessionRequest;
    startNewSession();
  }, [newSessionRequest, startNewSession]);

  // 2026-08-27 对齐原型：任务标题入全局标题栏（ThreadHead 行退役），右栏展开态供标题栏 aria。
  const activeSessionTitle = activeSession?.title ?? null;
  useEffect(() => {
    onSessionTitleChange?.(activeSessionTitle);
  }, [activeSessionTitle, onSessionTitleChange]);
  useEffect(() => {
    onContextExpandedChange?.(showCtx);
  }, [showCtx, onContextExpandedChange]);

  // 左栏选中态的唯一真值源（2026-09-04 用户报告「发起了新对话，左栏选中的还是老对话」）：
  // 会话身份一变（新建会话 / 打开历史 / 启动恢复 / 分叉跳转 / 句柄重建）就广播一次，
  // 主侧栏据此高亮「当前正在跑的那一个」；三个 id 全空 = 新建未落库的空会话，左栏不选任何行。
  useEffect(() => {
    const detail: ActiveSessionIds = {
      historyId: state.historyId ?? null,
      handleId: state.handleId ?? null,
      cliSessionId: state.sessionId ?? null,
    };
    window.dispatchEvent(new CustomEvent<ActiveSessionIds>('helm:session-active', { detail }));
  }, [state.historyId, state.handleId, state.sessionId]);

  // S2：新任务页就绪选择随首条消息一次性生效（引擎/目录/模式/权限/模型/强度）。
  // 与 Composer 的 draft 消费同 id 对齐；ref 初值 0 保证挂载即消费（首页跳转场景）。
  const consumedLaunchIdRef = useRef<number | null>(null);
  // 启动配置显式指定模式后，抑制一次「引擎默认模式回落」，避免把用户选择覆盖掉
  const launchModeOnceRef = useRef(false);
  const applyLaunch = useCallback(
    (launch: NewTaskLaunchConfig) => {
      const option = engineOptions.find((item) => item.engine.id === launch.engine);
      reset({ engine: launch.engine, cwd: launch.cwd || sessionDefaults.cwd });
      const validModel =
        launch.model && option?.models.some((model) => model.id === launch.model)
          ? launch.model
          : undefined;
      if (validModel) void selectModel(validModel);
      // 引擎守卫直接视为已处理，避免同一提交里两个默认模式回落互相触发
      previousEngine.current = launch.engine;
      launchModeOnceRef.current = true;
      setTurnMode(launch.mode);
      setPermissionProfile(launch.permissionProfile);
      setFullAccessConfirmed(Boolean(launch.fullAccessConfirmed));
      setReasoningEffort(launch.reasoningEffort ?? option?.binding?.reasoningEffort ?? 'auto');
    },
    [engineOptions, reset, selectModel, sessionDefaults.cwd],
  );

  useEffect(() => {
    const launch = draftRequest?.launch;
    if (!draftRequest || !launch) return;
    if (consumedLaunchIdRef.current === draftRequest.id) return;
    consumedLaunchIdRef.current = draftRequest.id;
    applyLaunch(launch);
  }, [draftRequest, applyLaunch]);

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
    // 新任务流（新任务页发送跳转）带着未消费草稿进场时禁止自动恢复：否则恢复旧会话与
    // startNewSession 竞速，首条消息会挂进已有会话、页面像「没反应」（2026-08-27 用户报告）。
    if (draftRequest || launching) {
      autoResumeAttempted.current = true;
      setRestoreWindowOpen(false); // 走会话创建流，不再假定恢复窗口
      return;
    }
    if (
      !shouldReopenLastSession(settings, {
        handleId: state.handleId,
        sessionId: state.sessionId,
        itemsLength: state.items.length,
      })
    ) {
      setRestoreWindowOpen(false); // 明确不恢复：回落「开始新会话」空态
      return;
    }
    autoResumeAttempted.current = true;
    let active = true;
    let settled = false;
    getActiveSession()
      .then(async (session) => {
        if (!active) return;
        if (!session) {
          setRestoreWindowOpen(false); // 指针为空：没有可恢复任务，回落空态
          return;
        }
        setResumingId(session.id);
        // B 方案：getActiveSession 已带回完整快照，先渲染线程再后台重建运行时；
        // restoreWindowOpen 保持开启，占位/重建闸门由 engineRebuilding 接管 Composer。
        publishHistoryOnly({ session });
        setTurnLedger(session.turns ?? []);
        // 模式+权限组合持久化（9/4）：恢复会话沿用该会话最后一轮的组合；
        // full_access 是高风险 lease 不自动恢复，回落 auto（lastTurnPrefs 内约束）。
        const restored = lastTurnPrefs(session.turns);
        setTurnMode(restored?.mode ?? defaultTurnMode);
        setPermissionProfile(
          restored?.permissionProfile ?? session.safePermissionProfile ?? 'standard',
        );
        setSessionError(null);
        let handleId: string;
        try {
          handleId = await resumeSession(session.id);
        } catch (err) {
          if (active) discardHistoryPreview(session.id);
          throw err;
        }
        if (!active) {
          void closeSession(handleId).catch(() => {});
          return;
        }
        publishResume({ handleId, session });
        setFullAccessConfirmed(false);
        void refreshSessions();
      })
      .catch((err: unknown) => {
        if (!active) return;
        setRestoreWindowOpen(false); // 恢复失败：回落空态，错误提示照常展示
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
  }, [draftRequest, launching, settings, state.handleId, state.items.length, state.sessionId]);

  const handleSend = async (text: string, attachments: string[]): Promise<boolean> => {
    // 无句柄的恢复窗口不接受直发（可靠性检查 B2）：句柄为空时 send() 会走
    // 「无句柄→createSession」分支误开新会话。B 方案历史先行后，engineRebuilding
    // 窗口的 Enter/发送已被 Composer 的 holding 挡进队列，flush 时句柄必已就绪
    // （publishResume 先于 resumingId 清理，两者同批可能拆两次渲染——所以闸门
    // 只拦「没句柄」，不能拦「resumingId 还挂着但句柄已到位」，否则 flush 误报失败）。
    if (resumingId && !state.handleId) {
      showToast('正在恢复会话，请稍候再发送', 'error');
      return false;
    }
    // 变更-37 · 发送前置校验：CLI 环境缺失就地拦截 + 引导卡（仅新会话需要判缺；
    // 已有 handleId 的会话已通过此闸门，复检由运行期兜底覆盖）。
    if (!state.handleId) {
      let deps: WorkspaceDeps | null = null;
      try {
        deps = await probeSharedDepsCached();
      } catch {
        // 探测失败保持原校验流向（绑定 → 目录 → 发送），不阻断用户使用 Node/git 已有的会话
      }
      const cliReady = await detectEngine(state.engine)
        .then(() => true)
        .catch(() => false);
      if (deps && (!deps.node.available || !deps.git.available || !cliReady)) {
        setSendBlocker({
          message:
            '本机尚未配置当前引擎所需的环境（Node.js / Git / CLI）。请按引导卡完成安装后再发送。',
          action: 'setup',
        });
        setSetupGuideVisible(true);
        setSetupProbeRequest((value) => value + 1);
        return false;
      }
      invalidateSharedDepsCache();
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
      permissionProfile === 'full_access' && fullAccessConfirmed,
    );
    if (!sent && !state.handleId && permissionProfile === 'full_access') {
      // 新会话的 FullAccessLease 只能在首次发送、Session id 已确定后签发。
      // 未带确认标记被后端拒绝时立即回落安全档，不能让 UI 假装危险模式仍已开启。
      setPermissionProfile('standard');
      setFullAccessConfirmed(false);
    }
    void refreshSessions();
    return sent;
  };

  // 变更-34 · C4：失败卡「重试这一步」把失败工具作为一条真实用户消息发回 Agent。
  const handleRetryTool = (toolId: string) => {
    const item = state.items.find((entry) => entry.kind === 'tool' && entry.id === toolId);
    if (!item || item.kind !== 'tool') return;
    void handleSend(retryRequestText(item.name, item.output), []);
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

  // 变更-34/35 · B1：Ctrl+O 循环切换视图密度（标准 ↔ 专注，v3 对齐 focusView）。
  useEffect(() => {
    const onDensityCycle = (event: KeyboardEvent) => {
      if (!event.ctrlKey || event.shiftKey || event.altKey) return;
      if (event.key !== 'o' && event.key !== 'O') return;
      event.preventDefault();
      setTranscriptDensity(nextTranscriptDensity(getTranscriptDensity()));
    };
    window.addEventListener('keydown', onDensityCycle);
    return () => window.removeEventListener('keydown', onDensityCycle);
  }, []);

  // 系统通知（G-15）：轮次完成/出错时弹出 Windows 原生通知
  const prevTurnRef = useRef({ status: state.status, historyId: state.historyId });
  useEffect(() => {
    const prev = prevTurnRef.current;
    prevTurnRef.current = { status: state.status, historyId: state.historyId };
    // 只在同线程内从 working → idle 时触发（轮次真实结束）。切会话/新建会话会
    // 整体换线程并置 idle，不代表旧会话轮次结束（旧轮次照常后台跑，P3-3）。
    if (!isGenuineTurnEnd(prev, prevTurnRef.current)) return;
    // 检查通知是否启用
    const notifEnabled = settings.general.notifications?.enabled ?? true;
    if (!notifEnabled) return;
    // 检查是否在 Tauri 环境
    if (!isTauriRuntime()) return;
    // 判断终止原因：正常完成 / 等待用户确认 / 其它未完成
    const lastItem = state.items[state.items.length - 1];
    const isWaitingApproval =
      lastItem?.kind === 'error' &&
      lastItem.errorKind === 'tool_stalled' &&
      lastItem.stalledKind === 'waiting_approval';
    try {
      if (isWaitingApproval) {
        sendNotification({
          title: 'Helm',
          body: '有一项操作在等待你确认',
        });
      } else if (lastItem?.kind === 'error') {
        sendNotification({
          title: 'Helm',
          body: '当前轮次未完成，可到会话中查看',
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
    const confirmed = profile === 'full_access';
    if (!state.handleId) {
      setPermissionProfile(profile);
      setFullAccessConfirmed(confirmed);
      return;
    }
    try {
      await setSessionPermissionProfile(state.handleId, profile, confirmed || undefined);
      setPermissionProfile(profile);
      setFullAccessConfirmed(confirmed);
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

  // B 方案（历史先行）：打开任务先渲染已入库的历史线程，CLI 运行时在后台重建；
  // 重建完成（publishResume）前 Composer 保持隐藏（engineRebuilding 闸门），
  // 不存在「对着半恢复会话发消息」。重建失败则丢弃先行渲染，回落到失败前画面 + toast。
  const applyHistorySnapshot = useCallback(
    (session: SessionDetail) => {
      publishHistoryOnly({ session });
      setTurnLedger(session.turns ?? []);
      // 模式+权限组合持久化（9/4）：打开会话沿用该会话最后一轮的组合（切换过取最新）。
      const restored = lastTurnPrefs(session.turns);
      setTurnMode(restored?.mode ?? defaultTurnMode);
      setPermissionProfile(
        restored?.permissionProfile ?? session.safePermissionProfile ?? 'standard',
      );
      setSessionError(null);
    },
    [defaultTurnMode],
  );

  const handleOpenSession = useCallback(
    async (sessionId: string) => {
      // 主侧栏「处理完成」徽标：任何入口打开任务都算「已查看」（railSeen 本机记录）
      markRailTaskSeen(sessionId);
      const token = ++resumeSeq.current;
      setResumingId(sessionId);
      forkTrace('workspace_open_enter', `target=${sessionId}`);
      try {
        // 并行会话（P3-3）：还有存活句柄的会话直接复用，不重启 CLI；
        // 后台仍在跑的轮次结束后事件会照常写入历史。
        const live = liveSessionHandle(sessionId);
        if (live) {
          const session = await getSessionHistory(sessionId);
          if (token !== resumeSeq.current) return;
          publishResume({ handleId: live, session });
          setTurnLedger(session.turns ?? []);
          // 模式+权限组合持久化（9/4）：与历史先行路径同口径，取最后一轮组合。
          const restored = lastTurnPrefs(session.turns);
          setTurnMode(restored?.mode ?? defaultTurnMode);
          setPermissionProfile(
            restored?.permissionProfile ?? session.safePermissionProfile ?? 'standard',
          );
          setFullAccessConfirmed(false);
          setSessionError(null);
          void refreshSessions();
          return;
        }
        // 历史先行：先画线程，再后台重建运行时（旧行为是 Promise.allSettled
        // 等两者都齐才画——Codex 冷启动 3~9s 里主区零反馈，2026-09-04 用户报「恢复慢」）。
        const session = await getSessionHistory(sessionId);
        if (token !== resumeSeq.current) return;
        applyHistorySnapshot(session);
        let handleId: string;
        try {
          handleId = await resumeSession(sessionId);
        } catch (err) {
          if (token === resumeSeq.current) {
            discardHistoryPreview(sessionId); // 回滚先行渲染，不留「只读死线程」
            const reason = err instanceof Error ? err.message : '恢复会话失败';
            setSessionError(reason);
            // 恢复失败过去只落在会话抽屉（.sbar-error），主区无任何反馈——分叉后自动
            // 跳转失败时表现为「停在原会话、点了没反应」（2026-09-02/09-04 两度复现）。
            showToast('打开任务失败：' + reason, 'error');
          }
          return;
        }
        if (token !== resumeSeq.current) {
          // 已被更晚的打开操作取代（快速连点）：新启的运行时立即回收防泄漏
          forkTrace('workspace_open_stale_seq', `target=${sessionId}`);
          void closeSession(handleId).catch(() => {});
          return;
        }
        forkTrace('workspace_open_done', `target=${sessionId} handle=${handleId}`);
        publishResume({ handleId, session });
        setFullAccessConfirmed(false);
        void refreshSessions();
      } catch (err) {
        if (token === resumeSeq.current) {
          forkTrace(
            'workspace_open_error',
            `target=${sessionId} err=${err instanceof Error ? err.message : String(err)}`,
          );
          discardHistoryPreview(sessionId);
          const reason = err instanceof Error ? err.message : '恢复会话失败';
          setSessionError(reason);
          showToast('打开任务失败：' + reason, 'error');
        }
      } finally {
        if (token === resumeSeq.current) setResumingId(null);
      }
    },
    [applyHistorySnapshot],
  );

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
    forkTrace('workspace_pending_consumed', `target=${pendingSessionId}`);
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

  const handleToggleArchived = async (session: SessionSummary) => {
    try {
      await setSessionArchived(session.id, !session.archived);
      void refreshSessions();
      showToast(session.archived ? '已取消归档' : '已归档 · 可在「已归档」筛选中找回');
    } catch (err) {
      showToast(`归档失败：${err instanceof Error ? err.message : String(err)}`, 'error');
    }
  };

  const freshSession = !state.handleId && !state.sessionId && state.items.length === 0;
  // 恢复窗口（「正在打开任务…」占位期）：句柄未就绪，Composer 随占位一并隐藏——
  // 恢复期间可输入是假可供性：resumingId 下发送已被拦截弹 toast，pending/restore
  // 窗口甚至可能抢在恢复完成前把消息发进新建会话（2026-09-04 用户报告「为啥还放输入框」）。
  // 与上方占位渲染的两个恢复分支同口径；「正在创建会话…」（draftRequest||launching）
  // 不在此列——Composer 需在场消费草稿，用户也要看到任务文本没丢。
  const resumingPlaceholder =
    (resumingId !== null && resumingId !== state.historyId) ||
    (freshSession && Boolean(resumingId || pendingSessionId || restoreWindowOpen));
  // B 方案恢复闸门（09-04 用户裁决改无感版）：历史已先行渲染、句柄未到位期间，
  // Composer 保持在场，holding 把 Enter/发送路由进既有排队机制（句柄就绪自动
  // flush）；绝不直发——send() 的「无句柄→createSession」分支会误开新会话
  // （可靠性检查 B2 同族风险）。
  const engineRebuilding = resumingId !== null && resumingId === state.historyId && !state.handleId;

  // 2026-08-28 对齐原型 syncTitleAxis：工作区页任务标题左缘与 composer 输入框左缘对齐。
  // 2026-09-04 修复：恢复窗口（resumingPlaceholder）会整块隐藏 Composer，原先只跑一次的
  // 挂载观测会盯在随后被卸载的节点上——恢复完成后 Composer 重新挂载也不再同步，任务标题
  // 就一直贴在标题栏最左边（用户报告「标题跟 composer 不对齐了」）。改为随 Composer 在场
  // 与否重建观测；同时观测父级 .composer——右栏开合/轮次轨道出现只改内边距，.composer__inner
  // 自身宽度可能不变，只盯它会漏触发。
  useEffect(() => {
    const center = document.querySelector<HTMLElement>('.titlebar__center');
    const inner = document.querySelector<HTMLElement>('.composer__inner');
    if (!center || !inner) return;
    const sync = () => {
      const gap = Math.max(
        0,
        Math.round(inner.getBoundingClientRect().left - center.getBoundingClientRect().left),
      );
      center.style.paddingLeft = gap + 'px';
    };
    sync();
    const ro = new ResizeObserver(sync);
    ro.observe(inner);
    if (inner.parentElement) ro.observe(inner.parentElement);
    ro.observe(center);
    window.addEventListener('resize', sync);
    return () => {
      ro.disconnect();
      window.removeEventListener('resize', sync);
      center.style.paddingLeft = '';
    };
  }, [resumingPlaceholder, showCtx]);

  // D-2：workstrip 派生（真实数据）—— 子代理聚合 / Todo 计划投影 / 等审批判定
  const workstripAgents = useMemo(() => collectWorkstripAgents(state.items), [state.items]);
  const workstripTodoRows = useMemo(() => workstripTodo(state.items), [state.items]);
  const waitingApproval = useMemo(
    () =>
      state.items.some(
        (item) =>
          item.kind === 'approval' && (item.status === 'pending' || item.status === 'applying'),
      ),
    [state.items],
  );

  return (
    <main
      className={
        'ws' +
        (showCtx ? ' is-context-open' : ' no-ctx') +
        (showSessions ? ' is-sidebar-open' : '') +
        ` is-${viewport}`
      }
    >
      {/* 批次①：任务列表侧栏全视口改为抽屉（原型隐藏该列），抽屉打开时显示背板 */}
      {showSessions || (viewport !== 'wide' && showCtx) ? (
        <button
          className="ws-drawer-backdrop"
          type="button"
          aria-label="关闭侧边面板"
          onClick={() => {
            if (showSessions) closeSessionDrawer(true);
            else if (showCtx) closeContextDrawer(true);
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
        onToggleFolder={(folder) => {
          setFolders((current) =>
            current.map((item) =>
              item.id === folder.id ? { ...item, collapsed: !item.collapsed } : item,
            ),
          );
          void setFolderCollapsed(folder.id, !folder.collapsed);
        }}
        onOpenSession={handleOpenSession}
        onRenameSession={(session) => {
          setRenameValue(session.title);
          setRenameTarget(session);
        }}
        onDeleteSession={(session) => setDeleteTarget(session)}
        onTogglePinned={(session) => void handleTogglePinned(session)}
        onToggleArchived={(session) => void handleToggleArchived(session)}
        isSessionActive={(session) =>
          workspaceSessionIsActive(session, {
            historyId: state.historyId,
            handleId: state.handleId,
            cliSessionId: state.sessionId,
          })
        }
      />
      {/* D-10：>3 轮挂 has-rail，内容列与 Composer 腾出轨道右侧空间（原型 .thread.has-rail） */}
      <section
        className={`thread dens-${transcriptDensity}${turnLedger.length > 3 ? ' has-rail' : ''}`}
      >
        {/* 批次①：派生提示横幅退役 —— 信息改由线程内原型 .swch 派生胶囊承载 */}
        {resumingId && resumingId !== state.historyId ? (
          // 打开其他任务进行中（分叉自动跳转 / 侧栏切换 / 派生会话打开）：句柄未就绪前
          // 不再让旧线程原地常驻——否则分叉后主区毫无变化，看起来「停留在原会话、点了
          // 没反应」（2026-09-04 用户报告）。恢复成功后整帧替换为线程内容；恢复失败时
          // resumingId 归位，旧线程与失败 toast（handleOpenSession catch）照常回落。
          <div className="ws-new-session ws-new-session--creating">
            <div className="ws-new-session__intro">
              <Icon name="sparkles" />
              <h2>正在打开任务…</h2>
              <p>正在恢复该会话的运行时与线程内容。</p>
            </div>
          </div>
        ) : !freshSession ? (
          <Thread
            key={state.historyId ?? state.sessionId ?? 'empty'}
            state={state}
            onApprove={approve}
            onRestoreCheckpoint={restoreCheckpoint}
            onUndoRevert={undoRevert}
            locateTarget={activityTarget}
            onOpenPane={(tab) => openArtifactPane(tab)}
            onRetryTool={handleRetryTool}
            turns={turnLedger}
            onForkAnswer={handleForkSameEngine}
            onOpenSourceSession={(sessionId) => void handleOpenSession(sessionId)}
          />
        ) : draftRequest || launching ? (
          // 新任务流带着草稿进场时，会话还在创建中：不要渲染「开始新会话」空态
          // （与「新任务页」视觉雷同，会造成跳回新任务页的闪屏）；改为创建中占位。
          <div className="ws-new-session ws-new-session--creating">
            <div className="ws-new-session__intro">
              <Icon name="sparkles" />
              <h2>正在创建会话…</h2>
              <p>已收到任务，正在启动 Agent 并打开工作区。</p>
            </div>
          </div>
        ) : resumingId || pendingSessionId || restoreWindowOpen ? (
          // 恢复进行中 / 恢复窗口未关（启动自动恢复 / 兜底打开最近会话 / 切页重挂载复用）：
          // 句柄尚未就绪，不要闪「开始新会话」空态——恢复完成后会整体替换为线程内容，闪现
          // 的空态与最终页面无关（2026-09-04 用户报告「跳回原任务前闪现开始新会话」）。
          // 各早退路径会关窗回落空态；恢复失败时空态与错误提示照常回落。
          <div className="ws-new-session ws-new-session--creating">
            <div className="ws-new-session__intro">
              <Icon name="sparkles" />
              <h2>正在打开任务…</h2>
              <p>正在恢复该会话的运行时与线程内容。</p>
            </div>
          </div>
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
          sendBlocker.action === 'setup' && setupGuideVisible ? (
            <SetupGuide
              key={`${state.engine}-${setupProbeRequest}`}
              engine={state.engine}
              onReady={() => {
                invalidateSharedDepsCache();
                setSetupGuideVisible(false);
                setSendBlocker(null);
                showToast('环境引导完成，可以开始对话了', 'info');
              }}
            />
          ) : (
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
                ) : sendBlocker.action === 'setup' ? (
                  <button
                    type="button"
                    className="btn btn--sm"
                    onClick={() => {
                      setSetupProbeRequest((value) => value + 1);
                      setSetupGuideVisible(true);
                    }}
                  >
                    打开环境引导
                  </button>
                ) : (
                  <button type="button" className="btn btn--sm" onClick={handlePickDirectory}>
                    选择工作目录
                  </button>
                )}
                {sendBlocker.action === 'setup' ? null : (
                  <button
                    type="button"
                    className="btn btn--sm"
                    onClick={() => setSendBlocker(null)}
                  >
                    知道了
                  </button>
                )}
              </div>
            </div>
          )
        ) : null}
        {!compactDismissed ? (
          <CompactBanner
            percent={compactPercent}
            engine={state.engine}
            working={state.status === 'working'}
            onCompact={handleCompactContext}
            onFork={handleForkSameEngine}
            onClose={dismissCompact}
          />
        ) : null}
        {/* D-2：workstrip 执行态条（主Agent行+子代理chips+Todo 投影，真实数据） */}
        <Workstrip
          working={state.status === 'working'}
          waitingApproval={waitingApproval}
          activityLabel={state.turnActivity ? statusBarLabel(state.turnActivity) : null}
          agents={workstripAgents}
          todo={workstripTodoRows}
          onLocateAgent={locateActivityItem}
        />
        {/* 原型明确没有 execbar：运行态只体现在 workstrip（有子代理/Todo 时）与任务列表，
          故不渲染常驻执行状态条，执行态由 workstrip 与任务列表承载。 */}
        {resumingPlaceholder ? null : (
          <Composer
            working={state.status === 'working'}
            holding={engineRebuilding}
            mode={turnMode}
            engine={state.engine}
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
            model={state.model || activeModelLabel || activeModel?.id || '默认模型'}
            modelOptions={activeOption?.models ?? []}
            modelProviderLabel={activeOption?.provider?.name}
            onSelectModel={(model) => void handleSelectModel(model)}
            reasoningCapability={reasoningCapability}
            reasoningLoading={reasoningLoading}
            reasoningDisabled={state.status === 'working'}
            onSelectReasoningEffort={(effort) => void handleSelectReasoningEffort(effort)}
            contextDetail={contextDetail}
            contextSnapshot={contextSnapshotView}
            draftRequest={draftRequest}
            onDraftConsumed={onDraftConsumed}
            sessionContextEdit={sessionContextEdit}
          />
        )}
      </section>
      <ResizablePane visible={showCtx} />
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
        openPaneRequest={paneRequest}
        onLocateItem={locateActivityItem}
        onStopTask={() => stop()}
        onCollapse={() => setShowCtx(false)}
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
  const target = `引擎「${pending.label}」`;

  return (
    <ShadcnDialog
      open
      onOpenChange={(open) => {
        if (!open) onCancel();
      }}
    >
      <DialogContent>
        <DialogHeader>
          <DialogTitle>创建新会话</DialogTitle>
        </DialogHeader>
        <div className="space-y-2 text-xs text-fg-2">
          {pending.sameEngine ? (
            // 走到这里说明无损分支不可用（CLI 不支持或源会话没跑过轮次），已回退摘要派生。
            // 弹窗只承载摘要任务的进度/取消/重试，不再复述无损派生机制。
            <>
              <p>
                当前无法无损复制完整历史，改用交接摘要创建新会话。摘要生成需要一点时间，
                细节可能有损；原会话完整保留。
              </p>
            </>
          ) : (
            <>
              <p>
                切换到{target}会生成交接摘要并创建新的
                Session，当前会话、消息、用量和检查点会保留在侧栏中。
              </p>
              <p className="text-fg-3">
                摘要由目标 Engine 的真实 CLI
                生成，细节可能有损。任务只读取已完成的轮次，不使用工具或已保存授权。
              </p>
            </>
          )}
          {operation ? (
            <p className={operation.status === 'failed' ? 'text-danger' : 'text-fg-3'}>
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
        <DialogFooter>
          <Button variant="ghost" size="sm" onClick={onCancel} type="button">
            {operation && ['committed', 'running'].includes(operation.status) ? '停止任务' : '取消'}
          </Button>
          {operation && ['failed', 'cancelled', 'delivery_unknown'].includes(operation.status) ? (
            <Button variant="primary" size="sm" onClick={onRetry} type="button">
              重试
            </Button>
          ) : operation ? null : (
            <Button
              variant="primary"
              size="sm"
              onClick={onConfirm}
              disabled={starting}
              type="button"
            >
              {starting ? '正在创建…' : pending.sameEngine ? '派生新会话' : '生成摘要并继续'}
            </Button>
          )}
        </DialogFooter>
      </DialogContent>
    </ShadcnDialog>
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
  return (
    <ShadcnDialog
      open
      onOpenChange={(open) => {
        if (!open) onCancel();
      }}
    >
      <DialogContent className="cm-modal--xs">
        <DialogHeader>
          <DialogTitle>重命名会话</DialogTitle>
        </DialogHeader>
        <input
          type="text"
          className="w-full rounded-sm border border-border bg-surface px-2.5 py-2 text-sm text-fg outline-none focus:border-accent"
          value={value}
          aria-label="会话标题"
          onChange={(event) => onChange(event.target.value)}
          onKeyDown={(event) => {
            if (event.nativeEvent.isComposing) return;
            if (event.key === 'Enter') void onConfirm();
          }}
        />
        <DialogFooter>
          <Button variant="ghost" size="sm" type="button" onClick={onCancel}>
            取消
          </Button>
          <Button
            variant="primary"
            size="sm"
            type="button"
            disabled={!value.trim()}
            onClick={() => void onConfirm()}
          >
            保存
          </Button>
        </DialogFooter>
      </DialogContent>
    </ShadcnDialog>
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
  return (
    <ShadcnDialog
      open
      onOpenChange={(open) => {
        if (!open) onCancel();
      }}
    >
      <DialogContent className="cm-modal--xs">
        <DialogHeader>
          <DialogTitle>删除会话</DialogTitle>
        </DialogHeader>
        <p className="text-xs text-fg-2">
          确定删除「{title}
          」吗？会话的消息、工具记录、用量与检查点快照将一并删除，运行中的后台轮次会被终止。此操作不可撤销。
        </p>
        <DialogFooter>
          <Button variant="ghost" size="sm" type="button" onClick={onCancel}>
            取消
          </Button>
          <Button variant="danger" size="sm" type="button" onClick={() => void onConfirm()}>
            删除
          </Button>
        </DialogFooter>
      </DialogContent>
    </ShadcnDialog>
  );
}
