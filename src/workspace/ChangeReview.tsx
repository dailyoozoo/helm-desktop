import { Fragment, useEffect, useMemo, useRef, useState } from 'react';
import type { ThreadItem } from '../engine/useSession';
import { Icon } from '../shell/icons';
import { changeReviewFiles, flattenHunkTargets } from './changeReviewViewModel';
import { DiffView, type DiffExpandState, type DiffViewMode } from './DiffView';
import { DiffNavigation } from './DiffNavigation';

function clampIndex(index: number, total: number): number {
  if (total === 0) return 0;
  return Math.min(index, total - 1);
}

/**
 * 变更审阅面 —— 对齐原型单列手风琴（ftop + flist + afile 行 + 行内展开统一 diff，
 * 同一时刻只展开一个）；保留上一处/下一处跨文件跳转与统一/并排切换。
 * 原型无行级批注与审阅工具条，故移除批注/自评审/攒批回灌（变更-34 · R1）。
 */
export function ChangeReview({ items }: { items: ThreadItem[] }) {
  const model = useMemo(() => changeReviewFiles(items), [items]);
  const files = model.files;
  const targets = useMemo(() => flattenHunkTargets(files), [files]);
  // 手风琴开合：同一时刻只展开一个文件（原型 curFile 语义，null = 全收起）
  const [openPath, setOpenPath] = useState<string | null>(null);
  const [mode, setMode] = useState<DiffViewMode>('unified');
  const [activeIndex, setActiveIndex] = useState(0);
  const [expanded, setExpanded] = useState<DiffExpandState>({});
  const viewRef = useRef<HTMLDivElement>(null);

  const openFile = files.find((file) => file.path === openPath) ?? null;
  const safeActive = clampIndex(activeIndex, targets.length);

  // 首次出现变更时默认展开第一个文件（原型 curFile 初始 0）
  useEffect(() => {
    if (!openPath && files.length > 0) setOpenPath(files[0].path);
  }, [openPath, files]);

  const toggleFile = (path: string) => {
    setOpenPath((current) => (current === path ? null : path));
  };

  const stepHunk = (delta: number) => {
    if (targets.length === 0) return;
    const next = clampIndex(activeIndex + delta, targets.length);
    if (next === activeIndex) return;
    const target = targets[next];
    if (target && target.path !== openFile?.path) setOpenPath(target.path);
    setActiveIndex(next);
  };

  const toggleSkip = (hunkKey: string) => {
    setExpanded((prev) => ({ ...prev, [hunkKey]: !prev[hunkKey] }));
  };

  useEffect(() => {
    const target = targets[safeActive];
    const container = viewRef.current;
    if (!target || !container) return;
    const el = Array.from(container.querySelectorAll<HTMLElement>('.dvw__hunk')).find(
      (candidate) => candidate.dataset.hunk === target.hunkKey,
    );
    el?.scrollIntoView({ behavior: 'smooth', block: 'center' });
  }, [activeIndex, openFile?.path, safeActive, targets]);

  return (
    <div className="achg">
      <div className="achg__ftop">
        <span className="t">本任务变更</span>
        {files.length > 0 ? (
          <span className="sum">
            <span className="a">+{model.totalAdded}</span>{' '}
            <span className="d">−{model.totalRemoved}</span>
          </span>
        ) : null}
      </div>
      <div className="achg__flist">
        {files.length === 0 ? (
          <div className="achg__empty">本任务还没有文件变更</div>
        ) : (
          files.map((file) => {
            const open = file.path === openPath;
            return (
              <Fragment key={file.path}>
                <button
                  type="button"
                  className={'afile' + (open ? ' is-on' : '')}
                  aria-expanded={open}
                  title={file.path}
                  onClick={() => toggleFile(file.path)}
                >
                  <span className={'st ' + file.status}>{file.status.toUpperCase()}</span>
                  <span className="nm">{file.path}</span>
                  <span className="dd">
                    <span className="a">+{file.added}</span>{' '}
                    <span className="d">−{file.removed}</span>
                  </span>
                  <span className="chev" aria-hidden="true">
                    <Icon name="down" />
                  </span>
                </button>
                {open ? (
                  <div className="afile__diff">
                    <div className="achg__bar">
                      <DiffNavigation
                        total={targets.length}
                        current={safeActive}
                        onPrev={() => stepHunk(-1)}
                        onNext={() => stepHunk(1)}
                      />
                      <span className="sp" />
                      <div className="seg">
                        <button
                          type="button"
                          className={mode === 'unified' ? 'is-active' : ''}
                          onClick={() => setMode('unified')}
                        >
                          统一
                        </button>
                        <button
                          type="button"
                          className={mode === 'split' ? 'is-active' : ''}
                          onClick={() => setMode('split')}
                        >
                          并排
                        </button>
                      </div>
                    </div>
                    <div className="achg__view" ref={viewRef}>
                      <DiffView
                        file={file}
                        mode={mode}
                        expanded={expanded}
                        activeHunkKey={targets[safeActive]?.hunkKey ?? null}
                        onToggleSkip={toggleSkip}
                      />
                    </div>
                  </div>
                ) : null}
              </Fragment>
            );
          })
        )}
      </div>
    </div>
  );
}
