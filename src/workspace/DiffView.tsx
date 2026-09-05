import type { ReviewChangeFile, ReviewHunk } from './changeReviewViewModel';

export type DiffViewMode = 'unified' | 'split';
export type DiffExpandState = Record<string, boolean>;

const SIGN: Record<'add' | 'del' | 'ctx', string> = { add: '+', del: '−', ctx: ' ' };

type DiffLine = ReviewHunk['lines'][number];
type Side = 'l' | 'r' | null;

/** 行号标记：add/ctx 取新行号，del 取旧行号。 */
function lineMarker(line: DiffLine): number | null {
  return line.newNo ?? line.oldNo;
}

/** 单行 diff：统一视图渲染旧/新双行号，并排视图只渲染各自一侧；缺省侧补空行占位。 */
function DiffLineRow({ line, side }: { line: DiffLine; side: Side }) {
  // 并排视图里，add 行只出现在右栏、del 行只出现在左栏；对应栏之外补占位行。
  if (side === 'l' && line.kind === 'add') {
    return (
      <div className="dvl pad">
        <span className="n" />
        <span className="tx" />
      </div>
    );
  }
  if (side === 'r' && line.kind === 'del') {
    return (
      <div className="dvl pad">
        <span className="n" />
        <span className="tx" />
      </div>
    );
  }
  const cls = line.kind === 'add' ? ' add' : line.kind === 'del' ? ' del' : '';
  const oldNo = side === 'r' ? '' : (line.oldNo ?? '');
  const newNo = side === 'l' ? '' : (line.newNo ?? '');
  const marker = lineMarker(line);
  return (
    <div className={`dvl${cls}`} data-line={marker === null ? undefined : String(marker)}>
      <span className="n">{oldNo}</span>
      {side === null ? <span className="n">{newNo}</span> : null}
      <span className="tx" data-sig={SIGN[line.kind]}>
        {line.text}
      </span>
    </div>
  );
}

/** 折叠未变更行：默认收成一行「⋯ 折叠 N 行未变更」；展开后显示真实行号区间（内容未随 Diff 记录）。 */
function SkipRow({
  hunk,
  expanded,
  onToggle,
}: {
  hunk: ReviewHunk;
  expanded: boolean;
  onToggle: (hunkKey: string) => void;
}) {
  const from = hunk.newStart - hunk.skip;
  const to = hunk.newStart - 1;
  const label = expanded
    ? `已展开 · 第 ${from}–${to} 行未变更（内容未随变更记录） · 点击收起`
    : `⋯ 折叠 ${hunk.skip} 行未变更`;
  return (
    <button
      type="button"
      className={`dskip${expanded ? ' is-open' : ''}`}
      onClick={() => onToggle(hunk.key)}
    >
      {label}
    </button>
  );
}

/** 纯 diff 查看器（原型无行级批注/审阅）：统一或并排渲染，支持未变更行折叠与跨文件导航高亮。 */
export function DiffView({
  file,
  mode,
  expanded,
  activeHunkKey,
  onToggleSkip,
}: {
  file: ReviewChangeFile;
  mode: DiffViewMode;
  expanded: DiffExpandState;
  /** 当前导航命中的 hunk（跨文件导航高亮） */
  activeHunkKey: string | null;
  onToggleSkip: (hunkKey: string) => void;
}) {
  return (
    <div className={'dvw' + (mode === 'split' ? ' is-split' : '')}>
      {file.hunks.map((hunk) => (
        <div
          key={hunk.key}
          className={'dvw__hunk' + (hunk.key === activeHunkKey ? ' is-nav' : '')}
          data-hunk={hunk.key}
        >
          {hunk.skip > 0 ? (
            <SkipRow hunk={hunk} expanded={Boolean(expanded[hunk.key])} onToggle={onToggleSkip} />
          ) : null}
          {mode === 'split' ? (
            <>
              <div className="dside">
                {hunk.lines.map((line, index) => (
                  <DiffLineRow key={index} line={line} side="l" />
                ))}
              </div>
              <div className="dside">
                {hunk.lines.map((line, index) => (
                  <DiffLineRow key={index} line={line} side="r" />
                ))}
              </div>
            </>
          ) : (
            hunk.lines.map((line, index) => <DiffLineRow key={index} line={line} side={null} />)
          )}
        </div>
      ))}
    </div>
  );
}
