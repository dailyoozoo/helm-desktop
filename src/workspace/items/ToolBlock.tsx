import { memo, useEffect, useState } from 'react';
import { toolTarget } from '../toolTarget';
import { Icon, type IconName } from '../../shell/icons';
import type { ThreadItem } from '../../engine/useSession';
import type { Diff } from '@helm/protocol';
import { copyText } from '../../lib/markdown';
import { showToast } from '../../components/toast';
import { isTerminalToolName } from '../threadGroups';
import { FailureCard } from './FailureCard';
import { AnsiText } from './AnsiText';

type ToolItem = Extract<ThreadItem, { kind: 'tool' }>;

function isDeniedOutcome(item: ToolItem): boolean {
  return (
    item.outcome === 'auto_review_unavailable' ||
    item.outcome === 'auto_review_parse_error' ||
    item.outcome === 'auto_review_blocked' ||
    item.outcome === 'runtime_denied'
  );
}

// 工具名 → 图标 / 着色，覆盖 Claude Code 常见工具；未知工具回退到通用文件图标。
const TOOL_ICON: Record<string, IconName> = {
  Read: 'file',
  Write: 'file',
  Edit: 'edit',
  Bash: 'terminal',
  Grep: 'search',
  Glob: 'search',
  LS: 'folder',
  WebFetch: 'upright',
  WebSearch: 'search',
};
const TOOL_COLOR: Record<string, string> = {
  Edit: 'k-edit',
  Write: 'k-edit',
  Bash: 'k-bash',
  Grep: 'k-search',
  Glob: 'k-search',
};

// 工具名 → 中文动作词（渲染形态 B 对齐 WorkBuddy 轻量行「运行命令/读取文件…」）；
// 未知工具回退显示原名，不猜。
const TOOL_VERB: Record<string, string> = {
  Read: '读取文件',
  Write: '写入文件',
  Edit: '编辑文件',
  NotebookEdit: '编辑笔记本',
  Bash: '运行命令',
  Grep: '搜索内容',
  Glob: '查找文件',
  LS: '列出目录',
  WebFetch: '抓取网页',
  WebSearch: '联网搜索',
  Task: '派发子代理',
};

function preview(input: unknown): string {
  if (typeof input === 'string') return input;
  try {
    return JSON.stringify(input, null, 2);
  } catch {
    // 豁免提示：循环引用等无法序列化的输入退化为 String()，仅影响预览文本
    return String(input);
  }
}

function resultSummary(item: ToolItem): string {
  if (item.status === 'pending') return '';
  if (item.diff) {
    let added = 0;
    let removed = 0;
    item.diff.hunks.forEach((hunk) =>
      hunk.lines.forEach((line) => {
        if (line.kind === 'add') added += 1;
        if (line.kind === 'del') removed += 1;
      }),
    );
    return `diff +${added}/-${removed}`;
  }
  if (item.status === 'error' && item.output) {
    const firstLine = item.output.split(/\r?\n/, 1)[0].trim();
    if (firstLine) return firstLine.length > 72 ? `${firstLine.slice(0, 71)}…` : firstLine;
  }
  if (item.output) return `${item.output.split(/\r?\n/).length} 行输出`;
  return item.status === 'error' ? '执行失败' : '已完成';
}

function durationSummary(item: ToolItem): string {
  if (!item.startedAt || !item.endedAt) return '';
  return `${Math.max(0, (item.endedAt - item.startedAt) / 1000).toFixed(1)}s`;
}

function diffStats(diff: Diff): { added: number; removed: number } {
  let added = 0;
  let removed = 0;
  diff.hunks.forEach((h) =>
    h.lines.forEach((l) => {
      if (l.kind === 'add') added += 1;
      else if (l.kind === 'del') removed += 1;
    }),
  );
  return { added, removed };
}

/** diff 独立可折叠卡（原型 .diff，ws.js L112）：默认折叠，点头部展开。 */
function DiffCard({ diff }: { diff: Diff }) {
  const [open, setOpen] = useState(false);
  const { added, removed } = diffStats(diff);
  return (
    <div className={'diff' + (open ? '' : ' collapsed')} data-kind="diff">
      <div
        className="diff__head"
        role="button"
        tabIndex={0}
        aria-expanded={open}
        onClick={() => setOpen(!open)}
        onKeyDown={(event) => {
          if (event.key === 'Enter' || event.key === ' ') {
            event.preventDefault();
            setOpen(!open);
          }
        }}
      >
        <div style={{ display: 'flex', alignItems: 'center', gap: 9, flex: 1, minWidth: 0 }}>
          <Icon name="file" />
          <span>{diff.path}</span>
        </div>
        <span className="diff__stat">
          <span className="a">+{added}</span>
          <span className="d">-{removed}</span>
        </span>
        <Icon name="down" />
      </div>
      {open ? (
        <div className="diff__lines">
          {diff.hunks.map((hunk, hi) => {
            // 行号双列（变更-10 修复）：删除/上下文行推进旧行号，新增/上下文行推进新行号
            let oldLine = hunk.oldStart;
            let newLine = hunk.newStart;
            return hunk.lines.map((line, li) => {
              const cls = line.kind === 'add' ? 'add' : line.kind === 'del' ? 'del' : '';
              const sign = line.kind === 'add' ? '+' : line.kind === 'del' ? '-' : ' ';
              const oldNo = line.kind === 'add' ? '' : String(oldLine++);
              const newNo = line.kind === 'del' ? '' : String(newLine++);
              return (
                <div key={`${hi}-${li}`} className={`dl ${cls}`}>
                  <div className="dl__n">{oldNo}</div>
                  <div className="dl__n dl__n--new">{newNo}</div>
                  <div className="dl__c">
                    {sign} {line.text}
                  </div>
                </div>
              );
            });
          })}
        </div>
      ) : null}
    </div>
  );
}

/** Bash 命令执行（渲染形态 B，对齐 WorkBuddy 截图 2026-08-31）：
 * **默认折叠为轻量单行**（图标 + 「运行命令」+ 命令摘要 + 状态药丸），无论完成/运行中/出错，
 * 点开后才渲染完整终端卡（$ 命令 + 输出 + exit 药丸 + 复制）。
 * 运行中收起时药丸显示「运行中」，完成后翻为「exit 0」。
 * 注意：open 不依赖初始 status，避免「pending 挂载 → 完成后仍展开」的状态未回灌问题。 */
function TerminalView({ item, className }: { item: ToolItem; className?: string }) {
  const [open, setOpen] = useState(false);
  const rawCommand =
    item.input && typeof item.input === 'object'
      ? (item.input as Record<string, unknown>).command
      : '';
  const command = Array.isArray(rawCommand)
    ? rawCommand.map(String).join(' ')
    : String(rawCommand ?? '');
  const collapsed = !open;
  const pillClass =
    item.status === 'success'
      ? 'pill--success'
      : isDeniedOutcome(item)
        ? 'pill--warn'
        : item.status === 'error'
          ? 'pill--danger'
          : 'pill--warn';
  const pillText =
    item.status === 'success'
      ? 'exit 0'
      : isDeniedOutcome(item)
        ? item.started === false
          ? '未执行'
          : '已拒绝'
        : item.status === 'error'
          ? '出错'
          : '运行中';
  if (collapsed) {
    // 轻量单行（对齐 WorkBuddy「运行命令」行）：深色终端大卡只在展开后出现
    const firstLine = command.trim().split('\n', 1)[0] ?? '';
    const summary = firstLine.length > 60 ? `${firstLine.slice(0, 59)}…` : firstLine;
    return (
      <div className={className} data-kind="term">
        {/* is-lite：复用 .tool.collapsed 的轻量行样式（无边框、与正文平级） */}
        <div className="tool is-lite">
          <div
            className="tool__head"
            role="button"
            tabIndex={0}
            aria-expanded={false}
            aria-label="展开命令与输出"
            onClick={() => setOpen(true)}
            onKeyDown={(event) => {
              if (event.key === 'Enter' || event.key === ' ') {
                event.preventDefault();
                setOpen(true);
              }
            }}
          >
            <div className="tool__ic k-bash">
              <Icon name="terminal" />
            </div>
            <div className="tool__name">
              运行命令
              {summary ? (
                <>
                  {' '}
                  <b>{summary}</b>
                </>
              ) : null}
            </div>
            <span className={'pill ' + pillClass} style={{ height: 19 }}>
              {pillText}
            </span>
            <span className="tool__chev">
              <Icon name="down" />
            </span>
          </div>
        </div>
      </div>
    );
  }
  return (
    <div className={'term ' + (className ?? '')} data-kind="term">
      <div
        className="term__head"
        role="button"
        tabIndex={0}
        aria-expanded={open}
        onClick={() => setOpen(!open)}
        onKeyDown={(event) => {
          if (event.key === 'Enter' || event.key === ' ') {
            event.preventDefault();
            setOpen(!open);
          }
        }}
      >
        <span className="pr">$</span>
        <span className="term__cmd">{command}</span>
        <span className={'pill ' + pillClass}>{pillText}</span>
        <button
          className="term__copy"
          type="button"
          title="复制命令"
          aria-label="复制命令"
          onClick={async (event) => {
            event.stopPropagation();
            if (await copyText(command)) showToast('命令已复制', 'success');
            else showToast('复制命令失败', 'error');
          }}
        >
          <Icon name="copy" />
        </button>
        <span className="term__chev">
          <Icon name="down" />
        </span>
      </div>
      {item.output && open ? (
        <div className="term__out">
          <AnsiText text={item.output} />
        </div>
      ) : null}
    </div>
  );
}

export const ToolBlock = memo(function ToolBlock({
  item,
  className,
  locateTarget,
  retryCount,
  onRetry,
  working = false,
}: {
  item: ToolItem;
  className?: string;
  locateTarget?: { id: string; request: number } | null;
  /** 变更-34 · C4：同一 Turn 中同名工具已重试次数（真实 Ledger 事实）。 */
  retryCount?: number;
  /** 变更-34 · C4：把失败工具作为真实用户消息发回 Agent。
   *  传工具 id 而非无参闭包：调用方得以复用稳定引用，避免每次渲染都击穿本组件的 memo。 */
  onRetry?: (toolId: string) => void;
  /** 轮次仍在运行时禁用失败卡的重试按钮。 */
  working?: boolean;
}) {
  const located = locateTarget?.id === item.id;
  const [manualOpen, setManualOpen] = useState<boolean | null>(() => (located ? true : null));
  // 默认收起（含运行中）；仅被定位（locateTarget，点文件引用）时自动展开。
  // 不再以初始 status==='pending' 决定展开，避免「运行中挂载 → 完成后仍展开」的状态未回灌问题。
  const open = manualOpen ?? false;
  useEffect(() => {
    if (located) setManualOpen(true);
  }, [located, locateTarget?.request]);
  const icon = TOOL_ICON[item.name] ?? 'file';
  const color = TOOL_COLOR[item.name] ?? '';
  const denied = isDeniedOutcome(item);
  const pillClass =
    item.status === 'success'
      ? 'pill--success'
      : denied
        ? 'pill--warn'
        : item.status === 'error'
          ? 'pill--danger'
          : 'pill--warn';
  const pillText =
    item.status === 'pending'
      ? '运行中'
      : item.outcome === 'auto_review_unavailable' || item.outcome === 'auto_review_parse_error'
        ? '未执行'
        : item.outcome === 'auto_review_blocked' || item.outcome === 'runtime_denied'
          ? '已拒绝'
          : item.status === 'success'
            ? '完成'
            : '出错';
  const target = toolTarget(item.name, item.input);
  const verb = TOOL_VERB[item.name] ?? item.name;
  const isTerminal = isTerminalToolName(item.name);
  const result = resultSummary(item);
  const duration = durationSummary(item);

  // 终端：原型单层头，直接渲染 .term（无外层 .tool 包裹，避免双层头）。
  if (isTerminal) {
    return <TerminalView item={item} className={className} />;
  }
  // 非拒绝的错误：原型把失败提成线程顶层独立 .failc 卡（ws.js L174）。
  if (item.status === 'error' && !denied) {
    return (
      <div className={className} data-kind="fail">
        <FailureCard
          item={item}
          toolId={item.id}
          title={target ? `${verb} ${target}` : verb}
          retryCount={retryCount}
          onRetry={onRetry}
          working={working}
        />
      </div>
    );
  }
  return (
    /* 批次①：过程/交付物卡片不再自带 .item 头像壳，由所在轮次 .ai-turn 统一承担 */
    <div className={className} data-kind={item.diff ? 'diff' : 'tool'}>
      <div className={'tool' + (open ? '' : ' collapsed')}>
        <div
          className="tool__head"
          role="button"
          tabIndex={0}
          aria-expanded={open}
          onClick={() => setManualOpen(!open)}
          onKeyDown={(e) => {
            if (e.key === 'Enter' || e.key === ' ') {
              e.preventDefault();
              setManualOpen(!open);
            }
          }}
        >
          <div className={'tool__ic ' + color}>
            <Icon name={icon} />
          </div>
          <div className="tool__name">
            {verb}
            {target ? (
              <>
                {' '}
                <b>{target}</b>
              </>
            ) : null}
          </div>
          {result || duration ? (
            <span className="tool__meta">{[result, duration].filter(Boolean).join(' · ')}</span>
          ) : null}
          <span className={'pill ' + pillClass} style={{ height: 19 }}>
            {pillText}
          </span>
          <span className="tool__chev">
            <Icon name="down" />
          </span>
        </div>
        {/* 原型工具体为单 .code（ws.js L72）；参数/输出合并为一块结果，去「变更」按钮（T4）。
            AnsiText 让 Read/Edit/Grep 等带 ANSI 着色的输出也能正确上色（此前只有 Bash 终端上色）。
            折叠时不渲染 .tool__body（与 .tool.collapsed 的 display:none 视觉一致），
            展开时才把完整输出交给 AnsiText —— 流式 delta 不再触发全量 parseAnsi 重扫。 */}
        {open ? (
          <ExpandedToolBodyMemo text={item.output ? item.output : preview(item.input)} />
        ) : (
          <CollapsedToolBody />
        )}
      </div>
      {item.diff ? <DiffCard diff={item.diff} /> : null}
    </div>
  );
});

/** 折叠态工具体零渲染：展开才付 parseAnsi / pretty-print 的成本。
 *  已核实 workspace.css 无 tool__body 高度动画（975 行仅 display:none），条件渲染与
 *  CSS 隐藏视觉完全一致；只有展开的卡片付解析成本，流式 delta 不再重复扫描全部输出。 */
const CollapsedToolBody = memo(function CollapsedToolBody() {
  return null;
});

/** 惰性 AnsilBody：仅展开时把完整输出交给 parseAnsi，避免折叠卡在流式期间反复重解析。 */
function ExpandedToolBody({ text }: { text: string }) {
  return (
    <div className="tool__body">
      <div className="code">
        <AnsiText text={text} />
      </div>
    </div>
  );
}

const ExpandedToolBodyMemo = memo(ExpandedToolBody);
