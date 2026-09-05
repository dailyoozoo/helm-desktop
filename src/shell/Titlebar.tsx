import { useEffect, useState } from 'react';
import { Icon, type IconName } from './icons';

// 自定义标题栏（Tauri decorations:false）。右上角放 Windows 三键，贴合 Windows 交互。
// data-tauri-drag-region 让空白区域可拖拽窗口；三键各自处理点击，不触发拖拽。

type WinAction = 'min' | 'max' | 'close';

async function winAction(action: WinAction) {
  // 浏览器预览（无 Tauri）下没有窗口 API，动态读取并吞掉异常即可。
  try {
    const { getCurrentWindow } = await import('@tauri-apps/api/window');
    const w = getCurrentWindow();
    if (action === 'min') await w.minimize();
    else if (action === 'max') await w.toggleMaximize();
    else await w.close();
  } catch {
    /* 非 Tauri 环境：忽略 */
  }
}

function CaptionButton({
  kind,
  label,
  icon,
  onClick,
}: {
  kind: WinAction;
  label: string;
  icon: IconName;
  onClick?: () => void;
}) {
  return (
    <button
      className={`wc wc--${kind}`}
      aria-label={label}
      title={label}
      onClick={onClick ?? (() => void winAction(kind))}
      type="button"
    >
      <Icon name={icon} style={{ width: 12, height: 12 }} />
    </button>
  );
}

/** 原型 commercial.js titlebar()：btn-icon sm 同款紧凑搜索图标，点击打开命令面板。 */
function SearchIconButton({ onOpen }: { onOpen?: () => void }) {
  return (
    <button
      className="titlebar__search"
      title="搜索"
      aria-label="搜索"
      onClick={onOpen}
      type="button"
    >
      <Icon name="search" style={{ width: 14, height: 14 }} />
    </button>
  );
}

export function Titlebar({
  title = '工作区',
  workspaceName = 'Helm 工作区',
  projectName,
  branchName,
  onOpenCommandPalette,
  bare = false,
  searchMode = 'box',
  taskTitle,
  onToggleCtx,
  ctxExpanded,
  /** 设置等二级页：在标题栏品牌(logo)位置放置返回按钮，并显示页面标题。 */
  onBack,
  pageTitle,
}: {
  title?: string;
  workspaceName?: string;
  /** 项目目录名（从 cwd 提取） */
  projectName?: string;
  /** 当前 git 分支名 */
  branchName?: string;
  onOpenCommandPalette?: () => void;
  /** 新任务页（2026-08-23 三轮决议）：仅保留左侧品牌与右上角三键，无标题。 */
  bare?: boolean;
  /** 搜索入口形态（2026-08-24 五轮决议，参考原型 commercial.js titlebar()）：
   * box=宽搜索框+Ctrl K（工作区保留）、icon=紧凑搜索图标（新任务/AI 配置/插件/用量）、none=无（设置页）。 */
  searchMode?: 'box' | 'icon' | 'none';
  /** 工作区页：当前任务标题（原型 workspace-titlebar__task）。传入时中段显示任务标题而非 git 信息。 */
  taskTitle?: string;
  /** 工作区页：右栏开关（原型 #ctxToggle，位于搜索入口之前）。 */
  onToggleCtx?: () => void;
  /** 右栏展开态（aria-expanded 用）。 */
  ctxExpanded?: boolean;
  /** 二级页返回：提供时在最左侧(logo 位置)渲染返回按钮。 */
  onBack?: () => void;
  /** 二级页标题：提供时在标题栏中段显示，如「设置」。 */
  pageTitle?: string;
}) {
  const [maximized, setMaximized] = useState(false);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void import('@tauri-apps/api/window')
      .then(async ({ getCurrentWindow }) => {
        const appWindow = getCurrentWindow();
        if (!disposed) setMaximized(await appWindow.isMaximized());
        unlisten = await appWindow.onResized(async () => {
          if (!disposed) setMaximized(await appWindow.isMaximized());
        });
      })
      .catch(() => undefined);
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  const toggleMaximize = async () => {
    await winAction('max');
    try {
      const { getCurrentWindow } = await import('@tauri-apps/api/window');
      setMaximized(await getCurrentWindow().isMaximized());
    } catch {
      setMaximized((value) => !value);
    }
  };

  if (bare) {
    // 新任务页（第三轮决议修正）：原型同款——左品牌 logo+Helm，右仅窗口三键，
    // 无中间标题、无搜索按钮（第二轮决议「只要最右边三个按钮」继续有效）。
    return (
      <div className="titlebar titlebar--home" data-tauri-drag-region>
        <div className="titlebar__brand" data-tauri-drag-region title="Helm · 新任务">
          <span className="titlebar__mark">
            <Icon name="helm" />
          </span>
          <b>Helm</b>
        </div>
        <div className="titlebar__center" data-tauri-drag-region />
        <div className="titlebar__right">
          {searchMode !== 'none' ? <SearchIconButton onOpen={onOpenCommandPalette} /> : null}
          <div className="win-caption">
            <CaptionButton kind="min" label="最小化" icon="minus" />
            <CaptionButton
              kind="max"
              label={maximized ? '向下还原' : '最大化'}
              icon={maximized ? 'restore' : 'maximize'}
              onClick={() => void toggleMaximize()}
            />
            <CaptionButton kind="close" label="关闭" icon="x" />
          </div>
        </div>
      </div>
    );
  }
  return (
    <div className="titlebar" data-tauri-drag-region>
      <div className="titlebar__brand" data-tauri-drag-region title={`${workspaceName} · ${title}`}>
        {onBack ? (
          <button
            className="titlebar__back"
            type="button"
            aria-label="返回"
            title="返回"
            onClick={onBack}
          >
            <Icon name="left" style={{ width: 16, height: 16 }} />
          </button>
        ) : (
          <span className="titlebar__mark">
            <Icon name="helm" />
          </span>
        )}
        <b>Helm</b>
      </div>
      <div className="titlebar__center" data-tauri-drag-region>
        {pageTitle ? (
          <span className="titlebar__page">{pageTitle}</span>
        ) : taskTitle != null ? (
          // 原型 workspace-titlebar__page：任务标题（2026-08-27 对齐原型，ThreadHead 行退役）
          <div className="titlebar__task" data-tauri-drag-region>
            <span className="truncate" title={taskTitle}>
              {taskTitle}
            </span>
          </div>
        ) : searchMode === 'box' && projectName && branchName ? (
          <span className="titlebar__git" title={`${projectName} › ${branchName}`}>
            <Icon name="gitbranch" className="h-3 w-3" style={{ width: 12, height: 12 }} />
            <span className="titlebar__project">{projectName}</span>
            <span className="titlebar__sep">›</span>
            <span className="titlebar__branch">{branchName}</span>
          </span>
        ) : searchMode === 'box' ? (
          // 原型标题栏无页名文字：仅工作区（box 搜索模式）保留标题/git 信息，
          // icon/none 模式（新任务/AI 配置/插件/用量/设置）中间留空。
          <span>{title}</span>
        ) : null}
      </div>
      <div className="titlebar__right">
        {onToggleCtx ? (
          // 原型 #ctxToggle：右栏开关位于搜索入口之前（btn-icon sm 同款紧凑尺寸）
          <button
            className="btn-icon sm"
            onClick={onToggleCtx}
            title="右侧工作区 · Ctrl+."
            aria-label="显示或隐藏右侧工作区"
            aria-expanded={ctxExpanded}
            type="button"
          >
            <Icon name="panelright" style={{ width: 15, height: 15 }} />
          </button>
        ) : null}
        {searchMode === 'box' ? (
          <>
            <button
              className="titlebar__k"
              title="命令面板"
              onClick={onOpenCommandPalette}
              type="button"
            >
              <Icon name="search" className="h-3.5 w-3.5" style={{ width: 14, height: 14 }} />
              <span className="titlebar__k-label">搜索命令、会话、服务商…</span>
            </button>
            <span className="kbd titlebar__kbd">Ctrl K</span>
          </>
        ) : searchMode === 'icon' ? (
          <SearchIconButton onOpen={onOpenCommandPalette} />
        ) : null}
        <div className="win-caption">
          <CaptionButton kind="min" label="最小化" icon="minus" />
          <CaptionButton
            kind="max"
            label={maximized ? '向下还原' : '最大化'}
            icon={maximized ? 'restore' : 'maximize'}
            onClick={() => void toggleMaximize()}
          />
          <CaptionButton kind="close" label="关闭" icon="x" />
        </div>
      </div>
    </div>
  );
}
