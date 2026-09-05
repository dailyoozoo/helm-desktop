import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  useLayoutEffect,
  type ReactNode,
} from 'react';
import { createPortal } from 'react-dom';
import { open } from '@tauri-apps/plugin-dialog';
import claudeBrand from '../assets/brands/claude.svg';
import openaiBrand from '../assets/brands/openai.svg';
import type { EngineId, ReasoningEffort } from '@helm/protocol';
import type { PermissionProfile, TurnMode } from '../engine/transport';
import { Icon } from '../shell/icons';
import { EngineBrand } from '../shell/EngineBrand';
import { showToast } from '../components/toast';
import { FullAccessConfirm } from '../components/FullAccessConfirm';
import { ContextPill, contextPillLabel, type ContextPillItem } from '../workspace/ContextPill';
import { defaultModelForEngine, workspaceEngineOptions } from '../workspace/workspaceViewModel';
import { searchWorkspaceFiles } from '../workspace/workspaceApi';
import { listSessions } from '../sessions/api';
import {
  listSkills,
  listSlashCommands,
  type Skill,
  type SlashCommand,
} from '../extensions/extensionsApi';
import { getProviderConfig, type AppConfig } from '../providers/api';
import {
  detectWorkspaceDeps,
  getReadinessReport,
  installCliEngine,
  installGit,
  installNode,
  selectDirectory,
  type ReadinessReport,
  type WorkspaceDeps,
} from '../settings/api';
import {
  buildReadinessItems,
  engineDisplayName,
  engineEffortTiers,
  engineReadiness,
  isTaskReady,
  PERMISSION_LABELS,
  permissionOptions,
  planAgentInstall,
  readyCount,
  REASONING_EFFORT_LABELS,
  stashHomeDraft,
  takeHomeDraft,
  TURN_MODE_LABELS,
  turnModeOptions,
  type NewTaskLaunchConfig,
  type ReadinessDep,
  type ReadinessItem,
} from './newTaskViewModel';

/** 发送给工作区的首条任务载荷：文本 + 附件 + 会话启动配置。 */
export interface NewTaskDraft {
  text: string;
  attachments: string[];
  config: NewTaskLaunchConfig;
}

type CenterKind = 'file' | 'command';
type BarMenu = 'mode' | 'permission' | 'model' | 'effort' | 'engine' | null;

interface NewTaskPageProps {
  defaultEngine: EngineId;
  defaultDirectory: string;
  onNavigate: (page: string) => void;
  onStartTask: (draft: NewTaskDraft) => void;
}
export function NewTaskPage({
  defaultEngine,
  defaultDirectory,
  onNavigate,
  onStartTask,
}: NewTaskPageProps) {
  // 草稿保护（D-13）：跳去服务商/插件页前暂存过草稿时，返回本页恢复一次。
  const [text, setText] = useState(() => takeHomeDraft());
  const [pills, setPills] = useState<ContextPillItem[]>([]);
  const [engine, setEngine] = useState<EngineId>(defaultEngine);
  const [mode, setMode] = useState<TurnMode>('build');
  // 新对话默认组合（2026-09-04 用户规格）：构建+自动执行，不再是 standard。
  const [permission, setPermission] = useState<PermissionProfile>('auto');
  const [model, setModel] = useState('');
  const [effort, setEffort] = useState<ReasoningEffort>('auto');

  const [config, setConfig] = useState<AppConfig | null>(null);
  const [report, setReport] = useState<ReadinessReport | null>(null);
  const [deps, setDeps] = useState<WorkspaceDeps | null>(null);
  // null = 尚未拿到真实报告；不提前假成就绪。
  // 主侧栏「选择工作目录」跳转过来时以所选目录为初始选中态（五次反馈），
  // 就绪报告返回后仍以其 cwd 为准做存在性校验（?? 守卫不覆盖已设值）。
  const [directory, setDirectory] = useState<{ path: string; exists: boolean } | null>(() => {
    const seed = defaultDirectory.trim();
    return seed ? { path: seed, exists: true } : null;
  });
  // 六次反馈修复：上面的种子只在挂载时取一次 prop。用户停留在本页时点主侧栏
  // 「选择工作目录」，组件不会重挂载，种子永不生效——这里对 prop 的后续变化做
  // 同步；外部显式选择优先于既有选中值（页内手选后若无新的外部动作不被覆盖）。
  const seededDirectoryRef = useRef(defaultDirectory.trim());
  useEffect(() => {
    const seed = defaultDirectory.trim();
    if (!seed || seed === seededDirectoryRef.current) return;
    seededDirectoryRef.current = seed;
    setDirectory((prev) => (prev?.path === seed ? prev : { path: seed, exists: true }));
  }, [defaultDirectory]);

  const [agentInstalling, setAgentInstalling] = useState(false);
  const [installNote, setInstallNote] = useState<string | null>(null);
  const [readinessOpen, setReadinessOpen] = useState(false);

  const [barMenu, setBarMenu] = useState<BarMenu>(null);
  const [capMenuOpen, setCapMenuOpen] = useState(false);
  const [center, setCenter] = useState<CenterKind | null>(null);
  const [centerQuery, setCenterQuery] = useState('');
  const [fileResults, setFileResults] = useState<string[]>([]);
  /** 文件中心应用内浏览的当前位置（2026-09 四次修订：Windows 系统弹窗无法混选
   *  文件与文件夹，改为弹框内自绘浏览列表；null = 未选起点）。 */
  const [browsePath, setBrowsePath] = useState<string | null>(null);
  const [slashCommands, setSlashCommands] = useState<SlashCommand[]>([]);
  const [skills, setSkills] = useState<Skill[]>([]);
  const [fullAccessPending, setFullAccessPending] = useState(false);
  const [fileDialogBusy, setFileDialogBusy] = useState(false);
  /** 浮层菜单锚点（原型 openMenuFloat：以触发钮 rect 定位的 fixed 浮层）。 */
  const [barAnchor, setBarAnchor] = useState<{ left: number; top: number } | null>(null);
  /** 「选择工作目录」弹层（原型 folderCenter）：最近使用 + 从电脑选择。 */
  const [dirModalOpen, setDirModalOpen] = useState(false);

  /** 启动序列（真实链接 Agent 的进度提示）：发送后原位展开分步启动卡，
   *  4 步全亮（任务已触发）后再把草稿交给工作区直接开跑。 */
  /** 从文件中心/@ 引导来选目录时置位：选完直接回到文件中心继续搜索。 */
  const [dirQuery, setDirQuery] = useState('');
  const [recentDirs, setRecentDirs] = useState<string[]>([]);

  const inputRef = useRef<HTMLTextAreaElement | null>(null);
  const engineOptions = useMemo(() => (config ? workspaceEngineOptions(config) : []), [config]);
  const activeOption = engineOptions.find((option) => option.engine.id === engine);
  const modelChoices = activeOption?.models ?? [];
  // 第四轮用户决议：推理强度跟随 Agent（引擎）而非逐模型探测——
  // Claude Code 与 Codex 各自固定展示其 CLI 声明过的档位集（docs/变更-17）。
  const effortChoices = engineEffortTiers(engine);

  const readiness = useMemo(
    () =>
      buildReadinessItems({
        report,
        deps,
        engine,
        directory: directory ?? { path: '', exists: false },
        agentInstalling,
      }),
    [report, deps, engine, directory, agentInstalling],
  );
  const items = readiness.items;
  const taskReady = isTaskReady(items);

  // Esc 自上而下关闭：浮层菜单 → 目录/中心/就绪弹层（原型同行为）。
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== 'Escape') return;
      if (barMenu !== null) {
        setBarMenu(null);
        return;
      }
      if (capMenuOpen) {
        setCapMenuOpen(false);
        return;
      }
      if (dirModalOpen) {
        setDirModalOpen(false);
        return;
      }
      if (center) {
        setCenter(null);
        return;
      }
      if (readinessOpen) setReadinessOpen(false);
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [barMenu, capMenuOpen, dirModalOpen, center, readinessOpen]);
  const refreshReadiness = useCallback(async () => {
    const [nextReport, nextDeps] = await Promise.all([getReadinessReport(), detectWorkspaceDeps()]);
    setReport(nextReport);
    setDeps(nextDeps);
    setDirectory((prev) => {
      if (!prev) return { path: nextReport.cwd.path, exists: nextReport.cwd.exists };
      // 种子/外选目录与报告同源（都来自默认目录设置）时，用报告校准真实存在性：
      // 过期的默认目录曾让文件中心一直空转搜不到文件（路径本身不被报告覆盖）。
      if (prev.path === nextReport.cwd.path && prev.exists !== nextReport.cwd.exists) {
        return { path: prev.path, exists: nextReport.cwd.exists };
      }
      return prev;
    });
    return { report: nextReport, deps: nextDeps };
  }, []);

  // 挂载即拉真实就绪报告 + 依赖探测 + Provider 配置（无 mock 初值）
  useEffect(() => {
    let active = true;
    refreshReadiness().catch(() => {
      if (active) showToast('就绪检查失败：无法读取本地环境报告', 'error');
    });
    getProviderConfig()
      .then((next) => {
        if (active) setConfig(next);
      })
      .catch(() => {
        if (active) showToast('AI 配置读取失败，请前往「AI 配置」检查', 'error');
      });
    return () => {
      active = false;
    };
  }, [refreshReadiness]);

  // 配置到达或切换引擎后，模型回落到当前引擎的真实绑定默认
  useEffect(() => {
    if (!config) return;
    setModel((current) => {
      const choices = engineOptions.find((option) => option.engine.id === engine)?.models ?? [];
      if (current && choices.some((choice) => choice.id === current)) return current;
      return defaultModelForEngine(config, engine);
    });
  }, [config, engine, engineOptions]);

  // 命令与技能来自真实动态发现（与工作区同一 API）
  const cwd = directory?.path ?? '';
  useEffect(() => {
    if (!cwd.trim()) return;
    let active = true;
    listSlashCommands(engine, cwd)
      .then((commands) => {
        if (active) setSlashCommands(commands.filter((command) => command.enabled));
      })
      .catch(() => {
        if (active) setSlashCommands([]);
      });
    listSkills(engine, cwd)
      .then((nextSkills) => {
        if (active) setSkills(nextSkills.filter((skill) => skill.enabled));
      })
      .catch(() => {
        if (active) setSkills([]);
      });
    return () => {
      active = false;
    };
  }, [engine, cwd]);

  // 打开文件中心时初始化应用内浏览起点：有工作目录从工作目录开始，否则待选起点。
  // 用户决议（2026-09）：新对话不展示「最近」（mock 概念）；浏览当前层不是「最近」。
  useEffect(() => {
    if (center === 'file') {
      setBrowsePath(cwd.trim() || null);
      setCenterQuery('');
    }
  }, [center, cwd]);

  // 文件中心：真实遍历浏览位置（后端限流，空查询返回浅层优先的全部条目），
  // 150ms 防抖。空关键词 = 浏览当前层；有关键词 = 当前层下递归搜索。
  useEffect(() => {
    if (center !== 'file' || !browsePath) {
      setFileResults([]);
      return;
    }
    let active = true;
    const timer = window.setTimeout(() => {
      searchWorkspaceFiles(browsePath, centerQuery)
        .then((files) => {
          if (active) setFileResults(files);
        })
        .catch(() => {
          if (active) setFileResults([]);
        });
    }, 150);
    return () => {
      active = false;
      window.clearTimeout(timer);
    };
  }, [center, browsePath, centerQuery]);

  /** 跳离新任务页（服务商/插件）前暂存草稿，返回时由 takeHomeDraft 恢复（D-13）。 */
  const navigateAway = (page: Parameters<typeof onNavigate>[0]) => {
    stashHomeDraft(text);
    setBarMenu(null);
    setCapMenuOpen(false);
    setReadinessOpen(false);
    onNavigate(page);
  };

  const submit = useCallback(() => {
    const value = text.trim();
    if (!value) {
      inputRef.current?.focus();
      return;
    }
    if (!taskReady) {
      // 未就绪不能发送：打开就绪检查弹层（验收标准）
      setReadinessOpen(true);
      showToast('请先完成任务就绪检查', 'error');
      return;
    }
    // 立即进入工作区；启动进度由 App 级 LaunchOverlay 监听真实后端事件驱动
    //（turn_stage / session_started），不再使用固定延时假动画。
    onStartTask({
      text: value,
      attachments: pills.map((pill) => pill.path),
      config: {
        engine,
        cwd: directory?.path ?? '',
        mode,
        permissionProfile: permission,
        fullAccessConfirmed: permission === 'full_access' ? true : undefined,
        model: model || undefined,
        reasoningEffort: effort,
      },
    });
  }, [text, taskReady, onStartTask, pills, directory, engine, mode, permission, model, effort]);

  const handleEngineChange = (next: EngineId) => {
    setEngine(next);
    setEffort('auto');
    if (config) setModel(defaultModelForEngine(config, next));
  };

  const handlePickDirectory = async () => {
    try {
      const dir = await selectDirectory();
      if (!dir) return;
      // 系统目录选择器只返回真实存在的目录
      setDirectory({ path: dir, exists: true });
      setInstallNote(null);
    } catch {
      showToast('目录选择器不可用，请在设置中配置默认目录', 'error');
    }
  };

  /** 打开浮层菜单：记录触发钮 rect（原型 openMenuFloat 行为）。 */
  const toggleBar = (kind: Exclude<BarMenu, null>, el: HTMLElement) => {
    if (barMenu === kind) {
      setBarMenu(null);
      return;
    }
    const rect = el.getBoundingClientRect();
    // 原型 openMenuFloat：菜单底边位于按钮顶边上方 8px，故锚点取按钮 top。
    setBarAnchor({ left: rect.left, top: rect.top });
    setBarMenu(kind);
  };

  /** 打开「选择工作目录」弹层（运行于按钮专用），并从真实最近任务提取去重后的工作目录。 */
  const openDirModal = () => {
    setDirQuery('');
    setDirModalOpen(true);
    listSessions()
      .then((sessions) => {
        const seen = new Set<string>();
        const dirs: string[] = [];
        for (const session of [...sessions].sort((a, b) => b.updatedAt - a.updatedAt)) {
          const cwd = session.cwd?.trim();
          if (!cwd || seen.has(cwd)) continue;
          seen.add(cwd);
          dirs.push(cwd);
          if (dirs.length >= 5) break;
        }
        setRecentDirs(dirs);
      })
      .catch(() => setRecentDirs([]));
  };

  /** 选择弹层中的目录行：乐观置为存在并立即复检。 */
  const chooseDirectoryPath = (path: string) => {
    setDirectory({ path, exists: true });
    setInstallNote(null);
    setDirModalOpen(false);
    refreshReadiness().catch(() => undefined);
  };

  /** 文件中心「选择开始位置…」：系统目录选择器（仅用于给应用内浏览挑一个起点；
   *  浏览/添加本身都在弹框内完成，见文件中心渲染块）。
   *  背景：Windows 系统弹窗无法混选文件与文件夹（FOS_PICKFOLDERS 模式下文件置灰，
   *  rfd 的 pick_file_or_folder 仅 macOS），2026-09 四次修订改为应用内浏览。 */
  const handlePickBrowseStart = async () => {
    if (fileDialogBusy) return;
    setFileDialogBusy(true);
    try {
      const dir = await selectDirectory();
      if (dir) {
        setBrowsePath(dir);
        setCenterQuery('');
      }
    } catch {
      showToast('目录选择器不可用，请在设置中配置默认目录', 'error');
    } finally {
      setFileDialogBusy(false);
    }
  };

  const handleInstallAgent = async () => {
    if (agentInstalling || !report || !deps) return;
    setAgentInstalling(true);
    setInstallNote(null);
    const cliInstalled = engineReadiness(report, engine).installed;
    const gitAvailable = deps.git.available;
    const steps = planAgentInstall({ cliInstalled, gitAvailable });
    let restartRequired = false;
    let failure: string | null = null;
    try {
      for (const step of steps) {
        if (step === 'node' && !deps.node.available) {
          const result = await installNode();
          restartRequired = restartRequired || result.restartRequired;
        }
        if (step === 'cli' && !cliInstalled) {
          // npm 源失败自动切国内镜像（installer.rs 真实安装链）
          await installCliEngine(engine);
        }
        if (step === 'git' && !gitAvailable) {
          const result = await installGit();
          restartRequired = restartRequired || result.restartRequired;
        }
      }
    } catch (error) {
      failure = error instanceof Error ? error.message : String(error);
    } finally {
      setAgentInstalling(false);
    }
    // 安装后立即复检（验收标准），以复检结果为准更新三项状态
    try {
      const next = await refreshReadiness();
      if (failure) {
        setInstallNote(failure);
        showToast('安装未完成：' + failure, 'error');
      } else {
        const stillMissing =
          !engineReadiness(next.report, engine).installed || !next.deps.git.available;
        showToast(
          stillMissing && restartRequired
            ? '安装完成，但需要重启 Helm 刷新 PATH 后复检才会通过'
            : stillMissing
              ? '安装动作已执行，复检仍未通过'
              : '复检通过：Agent 与 Git 均已就绪',
          stillMissing ? 'info' : 'success',
        );
        if (stillMissing) setInstallNote('复检未通过：请查看安装输出或重启 Helm 后重试。');
      }
    } catch {
      setInstallNote('复检失败：无法读取本地环境报告');
    }
  };

  const addPill = (absolutePath: string) => {
    const pill: ContextPillItem = {
      kind: 'mention',
      path: absolutePath,
      label: contextPillLabel(absolutePath, 'mention', cwd || undefined),
    };
    setPills((current) =>
      current.some((item) => item.path === absolutePath) ? current : [...current, pill],
    );
  };

  const chooseFileResult = (base: string, relativePath: string) => {
    const absolute = base.replace(/[\\/]+$/, '') + '/' + relativePath;
    addPill(absolute);
    setCenter(null);
    setCenterQuery('');
  };

  /** 「从电脑选择文件…」：系统文件选择器（多选），选中即挂药丸并关闭弹窗。 */
  const handlePickNativeFile = async () => {
    if (fileDialogBusy) return;
    setFileDialogBusy(true);
    try {
      const selected = await open({ multiple: true, directory: false });
      const paths = (Array.isArray(selected) ? selected : selected ? [selected] : [])
        .map((path) => path.trim())
        .filter(Boolean);
      if (paths.length) {
        paths.forEach((path) => addPill(path));
        setCenter(null);
        setCenterQuery('');
      }
    } catch {
      // 浏览器预览没有 Tauri dialog；取消选择返回 null。
    } finally {
      setFileDialogBusy(false);
    }
  };

  const insertTrigger = (trigger: string) => {
    setText((current) => {
      const spacer = current && !/\s$/.test(current) ? ' ' : '';
      return current + spacer + trigger + ' ';
    });
    setCenter(null);
    setCenterQuery('');
    window.setTimeout(() => inputRef.current?.focus(), 0);
  };

  const applyPermission = (next: PermissionProfile) => {
    if (next === 'full_access') {
      // 全部放开：先确认，再应用；仅当前任务生效（随首条消息冻结进会话）
      setFullAccessPending(true);
      return;
    }
    setPermission(next);
  };

  const skillCommandsForEngine = skills.filter((skill) => skill.engine === engine);
  const commandRows = slashCommands.filter(
    (command) => command.engine === 'all' || command.engine === engine,
  );
  const modeChoices = turnModeOptions(engine);
  const permissionChoices = permissionOptions();
  const query = centerQuery.trim().toLocaleLowerCase('zh-CN');

  return (
    <div className="home home--start">
      <main className="cm-start">
        <header className="cm-start__heading">
          <div className="cm-start__titleline">
            <span className="cm-start__logo" aria-hidden="true">
              <Icon name="helm" />
            </span>
            <h1>今天想和 Agent 一起完成什么？</h1>
          </div>
          <p>描述想完成的目标；Helm 会确认运行所需配置，再调用真实 CLI 开始任务。</p>
        </header>

        <div className="cm-compose-shell">
          <form
            className="cm-composer"
            onSubmit={(event) => {
              event.preventDefault();
              submit();
            }}
          >
            <div className="cm-composer__body">
              <textarea
                ref={inputRef}
                value={text}
                aria-label="任务说明"
                placeholder={
                  engine === 'codex'
                    ? '说说你想完成的事，@ 引用文件，/ 调用命令，$ 调用技能…'
                    : '说说你想完成的事，@ 引用文件，/ 调用命令或技能…'
                }
                onChange={(event) => {
                  const next = event.target.value;
                  setText(next);
                  // 原型行为：输入 / 或 @ 立即打开对应中心
                  if (next === '/') {
                    setCenter('command');
                    setCapMenuOpen(false);
                  } else if (next === '@') {
                    setCapMenuOpen(false);
                    // 用户决议（2026-09 三次修订）：@ 直接开文件中心；框内不放
                    // 「选择工作目录」入口，未选目录时也不两步跳目录弹层。
                    // 文件/目录系统选择器各一个入口（Windows 弹窗无法混选，见下注释）。
                    setCenter('file');
                  }
                }}
                onKeyDown={(event) => {
                  if (event.nativeEvent.isComposing || event.key !== 'Enter' || event.shiftKey)
                    return;
                  event.preventDefault();
                  submit();
                }}
              />
              {/* 原型 .cm-pills 空容器也占 margin-top:8px，保持常驻以对齐 composer 高度。 */}
              <div className="cm-pills">
                {pills.map((pill) => (
                  <ContextPill
                    key={pill.path}
                    item={pill}
                    onRemove={(path) =>
                      setPills((current) => current.filter((item) => item.path !== path))
                    }
                  />
                ))}
              </div>
            </div>
            <div className="cm-composer__bar">
              <div className="cm-composer__group">
                <div className="cap-anchor">
                  <button
                    className="cm-tool"
                    type="button"
                    title="添加上下文与工具"
                    aria-label="添加上下文与工具"
                    aria-expanded={capMenuOpen}
                    onClick={() => {
                      setBarMenu(null);
                      setCapMenuOpen((open) => !open);
                    }}
                  >
                    <Icon name="plus" />
                  </button>
                  {capMenuOpen ? (
                    <div className="cm-menu cm-menu--above" role="menu">
                      <button
                        className="cm-menu__item"
                        type="button"
                        role="menuitem"
                        onClick={() => {
                          setCapMenuOpen(false);
                          // 同 @ 输入：弹框直接可选文件或目录，未选工作目录时在弹框内选。
                          setCenterQuery('');
                          setCenter('file');
                        }}
                      >
                        <Icon name="folderopen" />
                        <span>文件与目录</span>
                        <small>@</small>
                      </button>
                      <button
                        className="cm-menu__item"
                        type="button"
                        role="menuitem"
                        onClick={() => {
                          setCapMenuOpen(false);
                          setCenterQuery('');
                          setCenter('command');
                        }}
                      >
                        <Icon name="terminal" />
                        <span>命令与技能</span>
                        <small>/</small>
                      </button>
                      <button
                        className="cm-menu__item"
                        type="button"
                        role="menuitem"
                        onClick={() => {
                          setCapMenuOpen(false);
                          navigateAway('extensions');
                        }}
                      >
                        <Icon name="plug" />
                        <span>连接器</span>
                      </button>
                    </div>
                  ) : null}
                </div>
                <button
                  id="modeSelect"
                  className="cm-tool"
                  type="button"
                  title="任务模式"
                  aria-expanded={barMenu === 'mode'}
                  onClick={(event) => toggleBar('mode', event.currentTarget)}
                >
                  <Icon name="layers" />
                  <span>{TURN_MODE_LABELS[mode]}</span>
                  <Icon name="down" />
                </button>
                <button
                  id="permissionSelect"
                  className={
                    'cm-tool' +
                    (permission === 'full_access'
                      ? ' is-danger'
                      : permission === 'auto'
                        ? ' is-warning'
                        : '')
                  }
                  type="button"
                  title="权限"
                  aria-expanded={barMenu === 'permission'}
                  onClick={(event) => toggleBar('permission', event.currentTarget)}
                >
                  <Icon name="shield" />
                  <span>{PERMISSION_LABELS[permission]}</span>
                  <Icon name="down" />
                </button>
              </div>
              <div className="cm-composer__group cm-composer__group--end">
                <button
                  id="modelSelect"
                  className="cm-tool"
                  type="button"
                  title="模型"
                  aria-expanded={barMenu === 'model'}
                  onClick={(event) => toggleBar('model', event.currentTarget)}
                >
                  <Icon name="sparkles" />
                  <span className="mono">{model || '未绑定模型'}</span>
                  <Icon name="down" />
                </button>
                <button
                  id="effortSelect"
                  className="cm-tool"
                  type="button"
                  title="推理强度"
                  aria-label="选择推理强度"
                  aria-expanded={barMenu === 'effort'}
                  onClick={(event) => toggleBar('effort', event.currentTarget)}
                >
                  <Icon name="gauge" />
                  <span>{REASONING_EFFORT_LABELS[effort]}</span>
                  <Icon name="down" />
                </button>
                <button
                  className={'cm-tool cm-tool--send' + (taskReady ? '' : ' is-blocked')}
                  type="submit"
                  title={taskReady ? '发送' : '发送前检查任务配置'}
                  aria-label={taskReady ? '发送' : '发送前检查任务配置'}
                >
                  <Icon name="send" />
                </button>
              </div>
              {barMenu === 'mode' ? (
                <HomeFloat anchor={barAnchor}>
                  {modeChoices.map((choice) => (
                    <button
                      key={choice.value}
                      className={
                        'home-floatmenu__item' + (choice.value === mode ? ' is-active' : '')
                      }
                      type="button"
                      role="menuitem"
                      onClick={() => {
                        setMode(choice.value);
                        setBarMenu(null);
                      }}
                    >
                      <Icon
                        name={
                          choice.value === 'build'
                            ? 'layers'
                            : choice.value === 'plan'
                              ? 'flag'
                              : 'helpcircle'
                        }
                      />
                      <span className="home-floatmenu__copy">
                        <span>{choice.label}</span>
                        <small>{choice.desc}</small>
                      </span>
                      <span className="home-floatmenu__hint">{choice.hint}</span>
                    </button>
                  ))}
                </HomeFloat>
              ) : null}
              {barMenu === 'permission' ? (
                <HomeFloat anchor={barAnchor}>
                  {permissionChoices.map((choice) => (
                    <button
                      key={choice.value}
                      className={
                        'home-floatmenu__item' +
                        (choice.value === permission ? ' is-active' : '') +
                        (choice.tone === 'danger'
                          ? ' is-danger'
                          : choice.tone === 'warning'
                            ? ' is-warn'
                            : '')
                      }
                      type="button"
                      role="menuitem"
                      onClick={() => {
                        applyPermission(choice.value);
                        setBarMenu(null);
                      }}
                    >
                      <Icon
                        name={
                          choice.value === 'standard'
                            ? 'shield'
                            : choice.value === 'auto'
                              ? 'zap'
                              : 'alert'
                        }
                      />
                      <span className="home-floatmenu__copy">
                        <span>{choice.label}</span>
                        <small>{choice.desc}</small>
                      </span>
                      <span className="home-floatmenu__hint">{choice.hint}</span>
                    </button>
                  ))}
                </HomeFloat>
              ) : null}
              {barMenu === 'model' ? (
                <HomeFloat anchor={barAnchor}>
                  {modelChoices.length === 0 ? (
                    <div className="home-menu__empty">
                      当前引擎尚未绑定模型
                      <small>到 AI 配置 → 执行引擎绑定后，这里会列出可选型号</small>
                    </div>
                  ) : (
                    modelChoices.map((choice) => (
                      <button
                        key={choice.id}
                        className={
                          'home-floatmenu__item' + (choice.id === model ? ' is-active' : '')
                        }
                        type="button"
                        role="menuitem"
                        onClick={() => {
                          // 强度跟随 Agent（第五轮决议口径）：同引擎内换模型不重置强度。
                          setModel(choice.id);
                          setBarMenu(null);
                        }}
                      >
                        <Icon name="zap" />
                        <span className="home-floatmenu__copy">
                          <span className="mono">{choice.id}</span>
                        </span>
                        {choice.contextWindow ? (
                          <span className="home-floatmenu__hint">
                            {Math.round(choice.contextWindow / 1000)}K
                          </span>
                        ) : null}
                      </button>
                    ))
                  )}
                  <div className="home-floatmenu__sep" role="separator" />
                  <button
                    className="home-floatmenu__item"
                    type="button"
                    role="menuitem"
                    onClick={() => {
                      navigateAway('providers');
                    }}
                  >
                    <Icon name="plug" />
                    <span className="home-floatmenu__copy">
                      <span>更改服务商绑定…</span>
                      <small>AI 配置 → 执行引擎</small>
                    </span>
                  </button>
                </HomeFloat>
              ) : null}
              {barMenu === 'effort' ? (
                <HomeFloat anchor={barAnchor}>
                  {effortChoices.map((choice) => (
                    <button
                      key={choice}
                      className={'home-floatmenu__item' + (choice === effort ? ' is-active' : '')}
                      type="button"
                      role="menuitem"
                      onClick={() => {
                        setEffort(choice);
                        setBarMenu(null);
                      }}
                    >
                      <span className="home-floatmenu__copy">
                        <span>{REASONING_EFFORT_LABELS[choice]}</span>
                        <small>{effortDesc(choice, engine)}</small>
                      </span>
                      {choice === 'auto' ? (
                        <span className="home-floatmenu__hint">模型默认</span>
                      ) : null}
                    </button>
                  ))}
                </HomeFloat>
              ) : null}
            </div>
          </form>

          <div className="cm-start-meta" aria-label="任务执行上下文">
            <span className="cm-start-meta__lead">运行于</span>
            {/* 引擎菜单走 HomeFloat portal（body 级 fixed），无需锚点包装层（原型同为裸按钮）。 */}
            <button
              id="engineSelect"
              className="cm-meta-select"
              type="button"
              title="更换 Agent"
              aria-label={'选择 Agent，当前 ' + engineDisplayName(engine)}
              aria-expanded={barMenu === 'engine'}
              onClick={(event) => toggleBar('engine', event.currentTarget)}
            >
              <span className="cm-meta-select__icon">
                <img src={engine === 'codex' ? openaiBrand : claudeBrand} alt="" />
              </span>
              <span className="cm-meta-select__main">
                <b>{engineDisplayName(engine)}</b>
              </span>
              <Icon name="down" />
            </button>
            {barMenu === 'engine' ? (
              <HomeFloat anchor={barAnchor}>
                {(['claude-code', 'codex'] as EngineId[]).map((id) => {
                  const installed = report ? engineReadiness(report, id).installed : null;
                  return (
                    <button
                      key={id}
                      className={'home-floatmenu__item' + (id === engine ? ' is-active' : '')}
                      type="button"
                      role="menuitem"
                      onClick={() => {
                        handleEngineChange(id);
                        setBarMenu(null);
                      }}
                    >
                      {/* R4#6 决议：引擎菜单行首用内联品牌 SVG，偏离原型 zap/terminal 语义图。 */}
                      <span className="cm-meta-select__icon">
                        <EngineBrand engine={id} size={16} />
                      </span>
                      <span className="home-floatmenu__copy">
                        <span>{engineDisplayName(id)}</span>
                        <small>
                          {id === 'codex'
                            ? '适合原生线程续接、沙箱和结构化工具调用。'
                            : '适合长任务、复杂代码理解和多步执行。'}
                        </small>
                      </span>
                      <span className="home-floatmenu__hint">
                        {installed == null
                          ? '检测中…'
                          : installed
                            ? id === 'codex'
                              ? 'OpenAI · 已就绪'
                              : 'Anthropic · 已就绪'
                            : '未安装'}
                      </span>
                    </button>
                  );
                })}
              </HomeFloat>
            ) : null}
            <span className="cm-start-meta__dot" aria-hidden="true" />
            {/* 原型 is-placeholder 挂在按钮上（.cm-start-meta .cm-meta-select.is-placeholder）。 */}
            <button
              className={'cm-meta-select' + (cwd ? '' : ' is-placeholder')}
              type="button"
              title="更换工作目录"
              aria-label={'选择工作目录，当前 ' + (cwd || '未选择')}
              onClick={() => {
                openDirModal();
              }}
            >
              <span className="cm-meta-select__icon">
                <Icon name="folder" />
              </span>
              <span className="cm-meta-select__main">
                <b>{cwd ? cwd.split(/[\\/]/).filter(Boolean).pop() || cwd : '选择目录'}</b>
              </span>
              <Icon name="down" />
            </button>
          </div>
        </div>
      </main>

      {barMenu !== null || capMenuOpen ? (
        <div
          className="home-overlay"
          aria-hidden="true"
          onClick={() => {
            setBarMenu(null);
            setCapMenuOpen(false);
          }}
        />
      ) : null}

      {readinessOpen ? (
        <div
          className="cm-modal-backdrop is-open"
          onClick={(event) => {
            if (event.target === event.currentTarget) setReadinessOpen(false);
          }}
        >
          <section
            className="home-modal cm-readiness"
            role="dialog"
            aria-modal="true"
            aria-labelledby="homeReadinessTitle"
          >
            {/* 原型同款双类：home-modal__head 提供 flex 骨架（本地 chrome），cm-readiness__head 只调间距。 */}
            <div className="home-modal__head cm-readiness__head">
              <div className="cm-readiness__intro">
                <span className="cm-readiness__mark">
                  <Icon name="helm" />
                </span>
                <div>
                  <h2 id="homeReadinessTitle">开始任务前，再确认 3 项</h2>
                  <p>当前输入会保留，补齐后可以直接继续发送。</p>
                </div>
              </div>
              <div className="cm-readiness__head-actions">
                <span className="cm-readiness__count">{readyCount(items)} / 3 项就绪</span>
                <button
                  className="btn-icon"
                  type="button"
                  aria-label="关闭"
                  onClick={() => setReadinessOpen(false)}
                >
                  <Icon name="x" />
                </button>
              </div>
            </div>

            <div className="cm-readiness__rail" aria-hidden="true">
              {items.map((item) => (
                <span
                  key={item.key}
                  className={
                    item.state === 'ready'
                      ? 'is-ready'
                      : item.state === 'installing'
                        ? 'is-installing'
                        : ''
                  }
                />
              ))}
            </div>

            <div className="cm-readiness__list" aria-label="任务就绪检查">
              {items.map((item) => (
                <ReadinessRow
                  key={item.key}
                  item={item}
                  engine={engine}
                  deps={item.key === 'agent' ? readiness.agentDeps : undefined}
                  onAction={() => {
                    if (item.key === 'agent') void handleInstallAgent();
                    if (item.key === 'provider') {
                      navigateAway('providers');
                    }
                    if (item.key === 'directory') openDirModal();
                  }}
                />
              ))}
            </div>

            <div className="cm-readiness__foot">
              <span className="cm-readiness__note">
                <Icon name="info" />
                <span>
                  {taskReady
                    ? '三项均已就绪，可以继续发送任务。'
                    : (installNote ?? '安装项优先走国内可直连源，完成后自动复检。')}
                </span>
              </span>
              {taskReady ? (
                <button
                  className="cm-action cm-action--primary"
                  type="button"
                  onClick={() => {
                    setReadinessOpen(false);
                    submit();
                  }}
                >
                  继续发送
                </button>
              ) : null}
            </div>
          </section>
        </div>
      ) : null}

      {dirModalOpen ? (
        <div
          className="cm-modal-backdrop is-open"
          onClick={(event) => {
            if (event.target === event.currentTarget) setDirModalOpen(false);
          }}
        >
          <section
            className="home-modal"
            role="dialog"
            aria-modal="true"
            aria-labelledby="homeDirTitle"
          >
            <div className="home-modal__head">
              <div>
                <h2 id="homeDirTitle">选择工作目录</h2>
                <p>首条消息发送后，当前任务的工作目录将锁定。</p>
              </div>
              <button
                className="btn-icon"
                type="button"
                aria-label="关闭"
                onClick={() => setDirModalOpen(false)}
              >
                <Icon name="x" />
              </button>
            </div>
            <label className="cm-search cm-search--block">
              <Icon name="search" />
              <input
                value={dirQuery}
                onChange={(event) => setDirQuery(event.target.value)}
                placeholder="搜索文件夹"
                aria-label="搜索文件夹"
              />
            </label>
            <div className="cm-list mt-12" aria-label="工作目录候选">
              {recentDirs
                .filter((dir) => {
                  const query = dirQuery.trim().toLocaleLowerCase('zh-CN');
                  return query ? dir.toLocaleLowerCase('zh-CN').includes(query) : true;
                })
                .map((dir, index) => (
                  <button
                    key={dir}
                    className="cm-menu__item"
                    type="button"
                    data-home-recent-dir={dir}
                    onClick={() => chooseDirectoryPath(dir)}
                  >
                    <Icon name="folder" />
                    <span>{dir.split(/[\\/]/).filter(Boolean).pop() || dir}</span>
                    {index === 0 ? <small>最近使用</small> : null}
                  </button>
                ))}
              {recentDirs.length === 0 ? (
                <div className="home-menu__empty">暂无最近使用的目录</div>
              ) : null}
              <button
                className="cm-menu__item"
                type="button"
                data-home-pick-dir
                onClick={() => void handlePickDirectory()}
              >
                <Icon name="folderopen" />
                <span>从电脑选择…</span>
              </button>
            </div>
          </section>
        </div>
      ) : null}

      {center ? (
        <div
          className="cm-modal-backdrop is-open"
          onClick={(event) => {
            if (event.target === event.currentTarget) setCenter(null);
          }}
        >
          <section
            className="home-modal"
            role="dialog"
            aria-modal="true"
            aria-labelledby="homeCenterTitle"
          >
            <div className="home-modal__head">
              <div>
                {/* 原型 compactCenter 标题随内容切换：fileCenter=文件与目录、commandCenter=命令与技能。 */}
                <h2 id="homeCenterTitle">{center === 'file' ? '文件与目录' : '命令与技能'}</h2>
                <p>
                  {center === 'file'
                    ? '浏览电脑上的文件与目录：点文件夹进入，点文件挂到输入框。'
                    : engine === 'codex'
                      ? '按来源分组；选择后插入输入框，命令以 / 调用、技能以 $ 调用。'
                      : '按来源分组；选择后插入输入框，技能同样以 / 调用。'}
                </p>
              </div>
              <button
                className="btn-icon"
                type="button"
                aria-label="关闭"
                onClick={() => setCenter(null)}
              >
                <Icon name="x" />
              </button>
            </div>
            <div className="cm-search cm-search--block">
              <Icon name="search" />
              {/* 原型 index.html:450：占位随中心切换——文件中心「搜索文件与目录」、命令中心「搜索命令」。 */}
              <input
                value={centerQuery}
                autoFocus
                placeholder={center === 'file' ? '搜索文件与目录' : '搜索命令'}
                aria-label="搜索"
                onChange={(event) => setCenterQuery(event.target.value)}
              />
            </div>
            <div className="cm-command-list mt-12">
              {center === 'file' ? (
                /* 用户决议（2026-09 四次修订）：应用内浏览——Windows 系统弹窗无法混选
                 * 文件与文件夹（FOS_PICKFOLDERS 下文件置灰、文件模式选不了文件夹，
                 * rfd 的混选 API 仅 macOS），故在弹框内自绘浏览列表：
                 * 点文件夹进入、点文件挂药丸并关闭、可从任意位置开始（含工作目录外）。 */
                browsePath ? (
                  <>
                    {/* 2026-08-28 用户决议：去掉路径头/添加此目录/上一层三行，
                        只留前 5 条结果 +「从电脑选择文件…」；点文件夹仍可进入（单向浏览）。 */}
                    {fileResults.length > 0 ? (
                      // 2026-08-28 用户决议：只展示前 5 条，其下直接是「从电脑选择文件…」。
                      fileResults.slice(0, 5).map((relativePath) => {
                        const isDir = /[\\/]$/.test(relativePath);
                        // 原型 context 行 label 不用 mono（mono 只给命令/技能触发词），
                        // 目录名不带尾斜杠展示（尾斜杠只是后端目录条目契约）。
                        const displayLabel = relativePath.replace(/[\\/]+$/, '');
                        return (
                          <button
                            key={relativePath}
                            className="cm-command-row"
                            type="button"
                            onClick={() =>
                              isDir
                                ? (setBrowsePath(
                                    browsePath.replace(/[\\/]+$/, '') + '/' + displayLabel,
                                  ),
                                  setCenterQuery(''))
                                : chooseFileResult(browsePath, displayLabel)
                            }
                          >
                            <span className="cm-command-row__icon">
                              <Icon name={isDir ? 'folder' : 'file'} />
                            </span>
                            <span className="cm-command-row__copy">
                              <b>{displayLabel}</b>
                              <small>{dirLabelOf(relativePath)}</small>
                            </span>
                            <span className="cm-command-row__meta">{isDir ? '进入' : '文件'}</span>
                          </button>
                        );
                      })
                    ) : (
                      <div className="home-menu__empty">
                        {centerQuery.trim() ? '没有匹配的文件或目录' : '此目录为空或不可读'}
                      </div>
                    )}
                    <button
                      className="cm-command-row"
                      type="button"
                      data-home-pick-file
                      disabled={fileDialogBusy}
                      onClick={() => void handlePickNativeFile()}
                    >
                      <span className="cm-command-row__icon">
                        <Icon name="folderopen" />
                      </span>
                      <span className="cm-command-row__copy">
                        <b>从电脑选择文件…</b>
                        <small>选择文件</small>
                      </span>
                    </button>
                  </>
                ) : (
                  <>
                    <div className="home-menu__empty">
                      尚未选择工作目录；从电脑上任意位置开始浏览并选择文件。
                    </div>
                    <button
                      className="cm-command-row"
                      type="button"
                      data-home-browse-start
                      disabled={fileDialogBusy}
                      onClick={() => void handlePickBrowseStart()}
                    >
                      <span className="cm-command-row__icon">
                        <Icon name="folderopen" />
                      </span>
                      <span className="cm-command-row__copy">
                        <b>选择开始位置…</b>
                        <small>电脑任意目录，之后可逐层浏览</small>
                      </span>
                    </button>
                  </>
                )
              ) : (
                <>
                  <BuiltinHead rows={commandRows} query={query} />
                  {commandRows
                    .filter((command) =>
                      query
                        ? (command.trigger + ' ' + command.description)
                            .toLocaleLowerCase('zh-CN')
                            .includes(query)
                        : true,
                    )
                    .map((command) => (
                      <button
                        key={command.id}
                        className="cm-command-row"
                        type="button"
                        onClick={() => insertTrigger(command.trigger)}
                      >
                        <span className="cm-command-row__icon">
                          <Icon name="terminal" />
                        </span>
                        <span className="cm-command-row__copy">
                          <b className="mono">{command.trigger}</b>
                          <small>{command.description}</small>
                        </span>
                        <span className="cm-command-row__meta">命令</span>
                      </button>
                    ))}
                  {skillCommandsForEngine.filter((skill) =>
                    query
                      ? (skill.trigger + ' ' + skill.description)
                          .toLocaleLowerCase('zh-CN')
                          .includes(query)
                      : true,
                  ).length > 0 ? (
                    <div className="cm-command-head">技能 Skills</div>
                  ) : null}
                  {skillCommandsForEngine
                    .filter((skill) =>
                      query
                        ? (skill.trigger + ' ' + skill.description)
                            .toLocaleLowerCase('zh-CN')
                            .includes(query)
                        : true,
                    )
                    .map((skill) => (
                      <button
                        key={skill.id}
                        className="cm-command-row"
                        type="button"
                        onClick={() => insertTrigger(skill.trigger)}
                      >
                        <span className="cm-command-row__icon">
                          <Icon name="sparkles" />
                        </span>
                        <span className="cm-command-row__copy">
                          {/* trigger 由后端按引擎给前缀（list_skills：Claude /、Codex $），
                              与工作区插入口径一致；插件页展示层会再剥插件命名空间段。 */}
                          <b className="mono">{skill.trigger}</b>
                          <small>{skill.description}</small>
                        </span>
                        <span className="cm-command-row__meta">技能</span>
                      </button>
                    ))}
                  {commandRows.length === 0 && skillCommandsForEngine.length === 0 ? (
                    <div className="home-menu__empty">当前环境没有可用命令或技能</div>
                  ) : null}
                </>
              )}
            </div>
          </section>
        </div>
      ) : null}

      {fullAccessPending ? (
        <FullAccessConfirm
          titleId="homeFullAccessTitle"
          onCancel={() => setFullAccessPending(false)}
          onConfirm={() => {
            setFullAccessPending(false);
            setPermission('full_access');
          }}
        />
      ) : null}
    </div>
  );
}

function ReadinessRow({
  item,
  engine,
  deps,
  onAction,
}: {
  item: ReadinessItem;
  /** Agent 行图标随引擎切换品牌图（原型 index.html:117 readinessAgentIcon）。 */
  engine: EngineId;
  deps?: ReadinessDep[];
  onAction: () => void;
}) {
  const stateClass =
    item.state === 'ready'
      ? 'is-ready'
      : item.state === 'installing'
        ? 'is-installing'
        : 'is-missing';
  const stateIcon =
    item.state === 'ready' ? 'checkc' : item.state === 'installing' ? 'clock' : 'xc';
  return (
    <div className={'cm-readiness__row ' + stateClass} data-readiness-key={item.key}>
      <span className="cm-readiness__state">
        <Icon name={stateIcon} />
      </span>
      <span className="cm-readiness__item-icon">
        {item.key === 'agent' ? (
          <img src={engine === 'codex' ? openaiBrand : claudeBrand} alt="" />
        ) : (
          <Icon name={item.key === 'provider' ? 'slidersh' : 'folder'} />
        )}
      </span>
      <span className="cm-readiness__copy">
        <b>{item.title}</b>
        <small>{item.detail}</small>
        {deps ? (
          <span className="cm-readiness__deps" aria-label="Agent 运行依赖">
            {deps.map((dep) => (
              <span
                key={dep.id}
                className={
                  'cm-readiness__dep ' +
                  (dep.state === 'ok'
                    ? 'is-ready'
                    : dep.state === 'installing'
                      ? 'is-installing'
                      : 'is-missing')
                }
              >
                <Icon name={dep.id === 'cli' ? 'terminal' : 'gitbranch'} />
                <span>{dep.label}</span>
              </span>
            ))}
          </span>
        ) : null}
      </span>
      {item.state === 'ready' ? (
        <span className="cm-readiness__done">已就绪</span>
      ) : (
        <button
          className="cm-action"
          type="button"
          disabled={item.state === 'installing'}
          onClick={onAction}
        >
          {item.state === 'installing'
            ? '正在准备…'
            : (item.actionLabel ?? (item.key === 'provider' ? '去配置' : '选择目录'))}
        </button>
      )}
    </div>
  );
}

/** 原型同款推理强度说明文案（openChoice 的 desc 列）。 */
const EFFORT_DESCS: Record<ReasoningEffort, string> = {
  auto: '使用当前模型的默认推理预算。',
  none: '不进行额外推理，直接给出回答。',
  minimal: '极少推理预算，速度优先。',
  low: '更快返回，适合简单修改与明确指令。',
  medium: '在速度和分析深度之间保持平衡。',
  high: '增加分析预算，适合复杂重构和排障。',
  xhigh: '投入更多分析预算，适合跨模块改造和疑难问题。',
  max: '使用当前模型支持的最大推理预算。',
};

/** 「自动/最大」档说明带引擎名（原型 openChoice desc 按引擎分组文案）。 */
function effortDesc(choice: ReasoningEffort, engine: EngineId): string {
  if (choice === 'auto') return `使用 ${engineDisplayName(engine)} 当前模型的默认推理预算。`;
  if (choice === 'max') return `使用 ${engineDisplayName(engine)} 当前支持的最大推理预算。`;
  return EFFORT_DESCS[choice];
}

/** 文件行副行：展示所在目录；根级文件显示「当前目录」。 */
function dirLabelOf(relativePath: string): string {
  const parts = relativePath.replace(/[\\/]+$/, '').split(/[\\/]/);
  return parts.length > 1 ? parts.slice(0, -1).join('/') : '当前目录';
}
/**
 * 原型 openMenuFloat：以触发钮 rect 定位的 fixed 浮层（左对齐、悬于按钮上方 8px、视口内夹紧）。
 * 经 portal 挂到 document.body（原型同为 body 级元素）：fixed 定位不再受任何
 * 祖先 transform / stacking context 影响——此前 composer :focus-within 的
 * translateY 曾把浮层整体右移出视口（模型/强度弹层「没有内容」的直接根因）。
 */
function HomeFloat({
  anchor,
  children,
}: {
  anchor: { left: number; top: number } | null;
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
/** 命令中心「内置命令」分组头（D-11）：有可见命令行才渲染。 */
function BuiltinHead({ rows, query }: { rows: SlashCommand[]; query: string }) {
  const visible = rows.some((command) =>
    query
      ? (command.trigger + ' ' + command.description).toLocaleLowerCase('zh-CN').includes(query)
      : true,
  );
  return visible ? <div className="cm-command-head">内置命令</div> : null;
}
