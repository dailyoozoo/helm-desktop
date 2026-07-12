import { type ReactNode } from 'react';
import { Icon } from './icons';

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
  children,
}: {
  kind: WinAction;
  label: string;
  children: ReactNode;
}) {
  return (
    <button
      className={`wc wc--${kind}`}
      aria-label={label}
      title={label}
      onClick={() => void winAction(kind)}
    >
      <svg
        viewBox="0 0 10 10"
        width={10}
        height={10}
        fill="none"
        stroke="currentColor"
        strokeWidth={1.1}
      >
        {children}
      </svg>
    </button>
  );
}

export function Titlebar({
  title = '工作区',
  workspaceName = 'Helm 工作区',
  onOpenCommandPalette,
}: {
  title?: string;
  workspaceName?: string;
  onOpenCommandPalette?: () => void;
}) {
  return (
    <div className="titlebar" data-tauri-drag-region>
      <div className="titlebar__brand" data-tauri-drag-region>
        <span className="titlebar__mark">
          <Icon name="rocket" />
        </span>
        <b>Helm</b>
        <span className="titlebar__app" title={`${workspaceName} · CLI Agent 控制台`}>
          {workspaceName} · CLI Agent 控制台
        </span>
      </div>
      <div className="titlebar__center" data-tauri-drag-region>
        <span>{title}</span>
      </div>
      <div className="titlebar__right">
        <button
          className="titlebar__k"
          title="命令面板"
          onClick={onOpenCommandPalette}
          type="button"
        >
          <Icon name="search" style={{ width: 14, height: 14 }} />
          搜索命令、会话、服务商…
        </button>
        <span className="kbd">Ctrl K</span>
        <div className="win-caption">
          <CaptionButton kind="min" label="最小化">
            <path d="M0 5h10" />
          </CaptionButton>
          <CaptionButton kind="max" label="最大化">
            <rect x={0.7} y={0.7} width={8.6} height={8.6} />
          </CaptionButton>
          <CaptionButton kind="close" label="关闭">
            <path d="M0.6 0.6 9.4 9.4M9.4 0.6 0.6 9.4" />
          </CaptionButton>
        </div>
      </div>
    </div>
  );
}
