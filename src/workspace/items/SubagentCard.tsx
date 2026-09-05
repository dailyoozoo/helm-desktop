import { memo, useState } from 'react';
import { Icon } from '../../shell/icons';
import { agentStateLabel, type SubagentEntry } from './taskViewModel';

// 变更-34 · C1：并行子代理卡。线程是线性的，Agent 的工作是并行的 ——
// 并行那部分只能靠这张卡呈现：名称 / 任务 / 状态 / 耗时 / 可展开产出。

export const SubagentCard = memo(function SubagentCard({
  items,
  onOpenPane,
}: {
  items: SubagentEntry[];
  /** 在交付物区打开「任务」tab 查看全部。 */
  onOpenPane?: () => void;
}) {
  const [openId, setOpenId] = useState<string | null>(null);
  const run = items.filter((entry) => entry.state === 'run').length;
  const err = items.filter((entry) => entry.state === 'err').length;
  const meta = run ? `${run} 个运行中` : err ? `${err} 个失败` : `${items.length} 个已完成`;

  return (
    /* 批次①：子代理卡不再自带 .item 头像壳，由所在轮次 .ai-turn 统一承担 */
    <div data-kind="sagent">
      <div className="sagent">
        <div className="sagent__head">
          <Icon name="users" />
          <span className="t">并行子代理</span>
          <span className="m">{meta}</span>
          {onOpenPane ? (
            <button
              type="button"
              className="plan__open"
              title="在交付物区查看全部任务"
              aria-label="在交付物区查看全部任务"
              onClick={onOpenPane}
            >
              <Icon name="panelright" />
            </button>
          ) : null}
        </div>
        {items.map((entry) => (
          <div key={entry.id}>
            <button
              type="button"
              className={'sarow' + (openId === entry.id ? ' is-open' : '')}
              aria-expanded={openId === entry.id}
              onClick={() => setOpenId(openId === entry.id ? null : entry.id)}
            >
              <Icon name="bot" />
              <span className="nm">{entry.name}</span>
              <span className="task" title={entry.task}>
                {entry.task || '—'}
              </span>
              {entry.dur ? <span className="dur">{entry.dur}</span> : null}
              <span className={'st ' + entry.state}>
                {entry.state === 'run' ? (
                  <>
                    <i aria-hidden="true" />
                    {agentStateLabel(entry.state)}
                  </>
                ) : (
                  agentStateLabel(entry.state)
                )}
              </span>
            </button>
            {entry.output ? <div className="sarow__out">{entry.output}</div> : null}
          </div>
        ))}
      </div>
    </div>
  );
});
