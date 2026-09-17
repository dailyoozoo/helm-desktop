import { useCallback, useEffect, useRef, useState, type DragEvent, type RefObject } from 'react';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { isTauriRuntime } from './env';

/**
 * 把文件拖进输入框（变更-37 · 拖拽附加）。
 *
 * 两条事件流必须配合，缺一不可：
 * - Tauri 的 `onDragDropEvent` 是唯一能拿到**真实绝对路径**的来源；HTML5 的
 *   `dataTransfer.files` 在 webview 里只有 File 对象，拿不到磁盘路径，无法生成附件药丸。
 * - 但 `onDragDropEvent` 是整个窗口级的，无法区分「拖到输入框上」还是「拖到页面别处」，
 *   所以悬停高亮和落点判定由 HTML5 的 dragenter/dragleave 计数 + 元素矩形命中共同决定。
 *   矩形命中用 devicePixelRatio 把 CSS 坐标换算到 Tauri 报告的物理坐标（Windows 常见 1.5/2）。
 */
export interface FileDropBinding {
  /** 拖拽悬停在目标元素上（用于高亮与提示层） */
  dragging: boolean;
  /** 展开到目标容器上即可，默认行为与落点判定都已处理 */
  onDragEnter: (event: DragEvent<HTMLElement>) => void;
  onDragOver: (event: DragEvent<HTMLElement>) => void;
  onDragLeave: (event: DragEvent<HTMLElement>) => void;
  onDrop: (event: DragEvent<HTMLElement>) => void;
}

function dragCarriesFiles(event: DragEvent<HTMLElement>): boolean {
  return Array.from(event.dataTransfer?.types ?? []).includes('Files');
}

export function useFileDrop(
  ref: RefObject<HTMLElement | null>,
  onPaths: (paths: string[]) => void,
  enabled = true,
): FileDropBinding {
  const [dragging, setDragging] = useState(false);
  // HTML5 enter/leave 会成对穿透子元素，用计数而不是布尔值，避免掠过 textarea 就误判离开。
  const hoverDepth = useRef(0);
  const htmlSeen = useRef(false);
  const onPathsRef = useRef(onPaths);
  onPathsRef.current = onPaths;

  const hitTarget = useCallback(
    (x: number, y: number): boolean => {
      const element = ref.current;
      if (!element) return false;
      const rect = element.getBoundingClientRect();
      const scale = typeof window === 'undefined' ? 1 : window.devicePixelRatio || 1;
      return (
        x >= rect.left * scale &&
        x <= rect.right * scale &&
        y >= rect.top * scale &&
        y <= rect.bottom * scale
      );
    },
    [ref],
  );

  useEffect(() => {
    if (!enabled || !isTauriRuntime()) return;
    let disposed = false;
    let unlisten: (() => void) | null = null;
    void getCurrentWindow()
      .onDragDropEvent((event) => {
        const payload = event.payload;
        if (payload.type === 'drop') {
          const inside = payload.position
            ? hitTarget(payload.position.x, payload.position.y)
            : true;
          hoverDepth.current = 0;
          htmlSeen.current = false;
          setDragging(false);
          if (inside && payload.paths.length) onPathsRef.current(payload.paths);
          return;
        }
        if (payload.type === 'leave') {
          hoverDepth.current = 0;
          htmlSeen.current = false;
          setDragging(false);
          return;
        }
        // enter / over：HTML5 事件不可用时（或光标尚未进入元素）也给出提示，
        // 但只有真正落在目标范围内才高亮，避免拖过整个窗口都亮。
        if (payload.position && !hitTarget(payload.position.x, payload.position.y)) {
          setDragging(false);
          return;
        }
        setDragging(true);
      })
      .then((fn) => {
        if (disposed) fn();
        else unlisten = fn;
      })
      .catch(() => {
        // 非 Tauri 环境或 webview 不支持拖拽事件：退化为「不支持拖拽」，不抛错。
      });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [enabled, hitTarget]);

  return {
    dragging,
    onDragEnter: (event) => {
      if (!enabled || !dragCarriesFiles(event)) return;
      event.preventDefault();
      htmlSeen.current = true;
      hoverDepth.current += 1;
      setDragging(true);
    },
    onDragOver: (event) => {
      if (!enabled || !dragCarriesFiles(event)) return;
      // 不 preventDefault 浏览器会判定「不接受放下」，Tauri 的 drop 事件也就不会触发。
      event.preventDefault();
      if (event.dataTransfer) event.dataTransfer.dropEffect = 'copy';
    },
    onDragLeave: () => {
      if (!enabled) return;
      hoverDepth.current = Math.max(0, hoverDepth.current - 1);
      if (hoverDepth.current === 0) setDragging(false);
    },
    onDrop: (event) => {
      if (!enabled) return;
      // 阻止 webview 直接导航到被拖入的文件；真实路径由 Tauri 的 drop 事件投递。
      event.preventDefault();
      hoverDepth.current = 0;
      setDragging(false);
    },
  };
}
