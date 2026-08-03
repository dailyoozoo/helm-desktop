import { memo, useEffect, useState } from 'react';
import { toolTarget } from '../toolTarget';
import { Icon, type IconName } from '../../shell/icons';
import type { ThreadItem } from '../../engine/useSession';
import type { Diff } from '@helm/protocol';
import { copyText } from '../../lib/markdown';
import { showToast } from '../../components/toast';
import { isTerminalToolName } from '../threadGroups';

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

function DiffView({ diff }: { diff: Diff }) {
  let added = 0;
  let removed = 0;
  diff.hunks.forEach((h) =>
    h.lines.forEach((l) => {
      if (l.kind === 'add') added++;
      else if (l.kind === 'del') removed++;
    }),
  );

  return (
    <div className="diff">
      <div className="diff__head">
        <Icon name="file" />
        <span>{diff.path}</span>
        <span className="diff__stat">
          <span className="a">+{added}</span>
          <span className="d">-{removed}</span>
        </span>
      </div>
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
    </div>
  );
}

/** Bash 深色终端卡（原型 term 卡）：$ 命令头 + 输出 + exit 药丸 */
function TerminalView({ item }: { item: ToolItem }) {
  const rawCommand =
    item.input && typeof item.input === 'object'
      ? (item.input as Record<string, unknown>).command
      : '';
  const command = Array.isArray(rawCommand)
    ? rawCommand.map(String).join(' ')
    : String(rawCommand ?? '');
  return (
    <div className="term">
      <div className="term__head">
        <span className="term__prompt">$</span>
        <span className="term__cmd">{command}</span>
        <span
          className={
            'pill ' +
            (item.status === 'success'
              ? 'pill--success'
              : isDeniedOutcome(item)
                ? 'pill--warn'
                : item.status === 'error'
                  ? 'pill--danger'
                  : 'pill--warn')
          }
        >
          {item.status === 'success'
            ? 'exit 0'
            : isDeniedOutcome(item)
              ? item.started === false
                ? '未执行'
                : '已拒绝'
              : item.status === 'error'
                ? '出错'
                : '运行中'}
        </span>
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
      </div>
      {item.output ? <div className="term__out">{item.output}</div> : null}
    </div>
  );
}

export const ToolBlock = memo(function ToolBlock({
  item,
  className,
  locateTarget,
}: {
  item: ToolItem;
  className?: string;
  locateTarget?: { id: string; request: number } | null;
}) {
  const located = locateTarget?.id === item.id;
  const [manualOpen, setManualOpen] = useState<boolean | null>(() => (located ? true : null));
  const open = manualOpen ?? item.status === 'pending';
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
  const isTerminal = isTerminalToolName(item.name);
  const result = resultSummary(item);
  const duration = durationSummary(item);

  return (
    <div className={className ? `item ${className}` : 'item'}>
      <div className="item__gut" />
      <div className="item__main">
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
              {item.name}
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
          <div className="tool__body">
            {isTerminal ? (
              <TerminalView item={item} />
            ) : (
              <>
                {item.diff ? <DiffView diff={item.diff} /> : null}
                {/* 入参 / 输出分区（变更-10）：有输出后仍可查看调用参数 */}
                <div className="tool__section">
                  <div className="tool__section-label">参数</div>
                  <div className="code">{preview(item.input)}</div>
                </div>
                {item.output ? (
                  <div className="tool__section">
                    <div className="tool__section-label">输出</div>
                    <div className="code code--wrap">{item.output}</div>
                  </div>
                ) : null}
              </>
            )}
          </div>
        </div>
      </div>
    </div>
  );
});
