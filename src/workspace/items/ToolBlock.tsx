import { memo, useState } from 'react';
import { Icon, type IconName } from '../../shell/icons';
import type { ThreadItem } from '../../engine/useSession';
import type { Diff } from '@helm/protocol';

type ToolItem = Extract<ThreadItem, { kind: 'tool' }>;

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

/** 工具头部的目标摘要（原型：工具名后跟加粗文件名/命令），从入参提取，无则为空 */
function toolTarget(name: string, input: unknown): string {
  if (!input || typeof input !== 'object') return '';
  const record = input as Record<string, unknown>;
  const candidate =
    record.file_path ?? record.notebook_path ?? record.path ?? record.pattern ?? record.url;
  if (typeof candidate === 'string' && candidate) return candidate;
  if (name === 'Bash' && typeof record.command === 'string') {
    const first = record.command.trim().split('\n')[0];
    return first.length > 60 ? `${first.slice(0, 60)}…` : first;
  }
  return '';
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
  const command =
    item.input && typeof item.input === 'object'
      ? String((item.input as Record<string, unknown>).command ?? '')
      : '';
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
              : item.status === 'error'
                ? 'pill--danger'
                : 'pill--warn')
          }
        >
          {item.status === 'success' ? 'exit 0' : item.status === 'error' ? '出错' : '运行中'}
        </span>
      </div>
      {item.output ? <div className="term__out">{item.output}</div> : null}
    </div>
  );
}

export const ToolBlock = memo(function ToolBlock({
  item,
  className,
}: {
  item: ToolItem;
  className?: string;
}) {
  const [open, setOpen] = useState(false);
  const icon = TOOL_ICON[item.name] ?? 'file';
  const color = TOOL_COLOR[item.name] ?? '';
  const pillClass =
    item.status === 'success'
      ? 'pill--success'
      : item.status === 'error'
        ? 'pill--danger'
        : 'pill--warn';
  const pillText =
    item.status === 'pending' ? '运行中' : item.status === 'success' ? '完成' : '出错';
  const target = toolTarget(item.name, item.input);
  const isBash = item.name === 'Bash';

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
            onClick={() => setOpen((v) => !v)}
            onKeyDown={(e) => {
              if (e.key === 'Enter' || e.key === ' ') {
                e.preventDefault();
                setOpen((v) => !v);
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
            <span className={'pill ' + pillClass} style={{ height: 19 }}>
              {pillText}
            </span>
            <span className="tool__chev">
              <Icon name="down" />
            </span>
          </div>
          <div className="tool__body">
            {isBash ? (
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
