import { useCallback, useEffect, useRef } from 'react';

/**
 * 变更-34 · E1：右栏宽度可拖拽并记忆。
 * 拖拽分隔线更新 `--ctx-w`，松手后写入 localStorage（key: helm:ctxw）。
 * 最小 360px（对齐原型 token clamp(360px, 34vw, 560px)）、最大 60vw；仅在宽屏（非 drawer 态）显示分隔线。
 */
const MIN_WIDTH = 360;
const STORAGE_KEY = 'helm:ctxw';

/** 变更-34 · E1：把鼠标横向位置换算为右栏宽度，钳制在 [minWidth, 60vw]。 */
export function clampPaneWidth(
  clientX: number,
  windowWidth: number,
  minWidth: number = MIN_WIDTH,
): number {
  const max = Math.round(windowWidth * 0.6);
  return Math.round(Math.min(max, Math.max(minWidth, windowWidth - clientX)));
}

export function ResizablePane({
  visible,
  minWidth = MIN_WIDTH,
  storageKey = STORAGE_KEY,
}: {
  /** 是否允许拖拽（宽屏 && 右栏打开）。drawer 态下不渲染手柄。 */
  visible: boolean;
  minWidth?: number;
  storageKey?: string;
}) {
  const root = document.documentElement;
  const draggingRef = useRef(false);
  const handleRef = useRef<HTMLButtonElement>(null);

  // 挂载时读取上次拖拽宽度；旧值低于新最小宽度（360px）时重新钳制。
  useEffect(() => {
    try {
      const saved = localStorage.getItem(storageKey);
      if (saved) {
        const numeric = parseInt(saved, 10);
        if (!Number.isNaN(numeric) && saved.endsWith('px') && numeric < minWidth) {
          // 旧持久化值低于新最小宽度，丢弃让它回落到 CSS 默认 clamp 值。
          localStorage.removeItem(storageKey);
        } else {
          root.style.setProperty('--ctx-w', saved);
        }
      }
    } catch {
      // localStorage 不可用时保持默认宽度
    }
  }, [root, storageKey, minWidth]);

  const applyWidth = useCallback(
    (clientX: number) => {
      const width = clampPaneWidth(clientX, window.innerWidth, minWidth);
      root.style.setProperty('--ctx-w', `${width}px`);
    },
    [minWidth, root],
  );

  const onPointerDown = useCallback((event: React.PointerEvent<HTMLButtonElement>) => {
    draggingRef.current = true;
    handleRef.current?.classList.add('is-drag');
    document.body.classList.add('is-splitting-v');
    event.preventDefault();
  }, []);

  useEffect(() => {
    const onMove = (event: PointerEvent) => {
      if (!draggingRef.current) return;
      applyWidth(event.clientX);
    };
    const onUp = () => {
      if (!draggingRef.current) return;
      draggingRef.current = false;
      handleRef.current?.classList.remove('is-drag');
      document.body.classList.remove('is-splitting-v');
      try {
        localStorage.setItem(storageKey, getComputedStyle(root).getPropertyValue('--ctx-w').trim());
      } catch {
        // localStorage 不可用时忽略持久化
      }
    };
    window.addEventListener('pointermove', onMove);
    window.addEventListener('pointerup', onUp);
    return () => {
      window.removeEventListener('pointermove', onMove);
      window.removeEventListener('pointerup', onUp);
    };
  }, [applyWidth, root, storageKey]);

  if (!visible) return null;
  return (
    <button
      ref={handleRef}
      className="splitter splitter--v"
      type="button"
      aria-label="拖动调整右栏宽度"
      onPointerDown={onPointerDown}
    />
  );
}
