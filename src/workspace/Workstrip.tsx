import { memo, useMemo, useState } from 'react';
import type { ThreadItem } from '../engine/useSession';
import { Icon } from '../shell/icons';
import { collectSubagents, type SubagentEntry } from './items/taskViewModel';

/**
 * D-2（可靠性检查-工作区对话页-差异清单）：workstrip —— 线程下方的执行态条。
 * 对齐原型 ws.html L65-73 / ws.js L385-412：主 Agent 状态行 + 子代理 chips（≤3 + N，
 * 点击定位线程内卡片）+ Todo 清单开关。可见性 = (运行中或等审批) 且 (有子代理或 Todo)。
 * Todo 数据源：线程内最近一个未回退 plan 的 steps 投影（原型同款兜底，真实数据，不伪造）。
 */

export type WorkstripTodoState = 'done' | 'doing' | 'todo';

export interface WorkstripTodoRow {
  state: WorkstripTodoState;
  text: string;
}

/** 取线程内最近一个未回退 plan 的步骤作为 Todo 投影；无 plan 返回空。 */
export function workstripTodo(items: ThreadItem[]): WorkstripTodoRow[] {
  for (let index = items.length - 1; index >= 0; index -= 1) {
    const item = items[index];
    if (item.kind === 'plan' && !('reverted' in item && item.reverted)) {
      return item.steps.map((step) => ({
        state: step.status === 'done' ? 'done' : step.status === 'active' ? 'doing' : 'todo',
        text: step.text,
      }));
    }
  }
  return [];
}

export function collectWorkstripAgents(items: ThreadItem[]): SubagentEntry[] {
  return collectSubagents(items);
}

export interface WorkstripProps {
  working: boolean;
  waitingApproval: boolean;
  /** 当前动作文案（真实 TurnStage 派生，来自 statusBarLabel） */
  activityLabel: string | null;
  agents: SubagentEntry[];
  todo: WorkstripTodoRow[];
  onLocateAgent: (id: string) => void;
}

function TodoStateIcon({ state }: { state: WorkstripTodoState }) {
  if (state === 'done') return <Icon name="checkc" />;
  if (state === 'doing') return <Icon name="right" />;
  return <Icon name="dot" />;
}

export const Workstrip = memo(function Workstrip({
  working,
  waitingApproval,
  activityLabel,
  agents,
  todo,
  onLocateAgent,
}: WorkstripProps) {
  const [todoOpen, setTodoOpen] = useState(false);
  const visibleChips = useMemo(() => agents.slice(0, 3), [agents]);
  const overflow = agents.length - visibleChips.length;
  const doneCount = todo.filter((row) => row.state === 'done').length;

  if (!(working || waitingApproval) || (agents.length === 0 && todo.length === 0)) return null;

  const mainText = waitingApproval
    ? '等待你确认权限后继续'
    : working
      ? (activityLabel ?? '正在执行任务')
      : '正在执行任务';

  return (
    <div
      className={'workstrip' + (todoOpen && todo.length ? ' is-todo-open' : '')}
      role="status"
      aria-live="polite"
    >
      <div className="workstrip__line">
        <span className="workstrip__dot" />
        <span className="workstrip__main">
          <b>主 Agent</b>
          <span>{mainText}</span>
        </span>
        <span className="workstrip__agents">
          {visibleChips.map((agent) => (
            <button
              key={agent.id}
              type="button"
              className={
                'agentchip' +
                (agent.state === 'ok' ? ' is-done' : agent.state === 'err' ? ' is-error' : '')
              }
              title={agent.task}
              onClick={() => onLocateAgent(agent.id)}
            >
              <i />
              <span>{agent.name}</span>
            </button>
          ))}
          {overflow > 0 ? (
            <span
              className="agentchip is-more"
              title={agents
                .slice(3)
                .map((a) => a.name)
                .join('、')}
            >
              <span>+{overflow}</span>
            </span>
          ) : null}
        </span>
        {todo.length ? (
          <button
            type="button"
            className="workstrip__todo-btn btn btn--subtle btn--sm"
            aria-expanded={todoOpen}
            onClick={() => setTodoOpen((value) => !value)}
          >
            <Icon name="checkc" />
            <span>
              Todo {doneCount}/{todo.length}
            </span>
            <Icon name={todoOpen ? 'up' : 'down'} />
          </button>
        ) : null}
      </div>
      {todo.length && todoOpen ? (
        <div className="workstrip__todo">
          {todo.map((row, index) => (
            <div
              key={index + '-' + row.text}
              className={
                'workstrip__todo-row' +
                (row.state === 'done' ? ' is-done' : row.state === 'doing' ? ' is-doing' : '')
              }
            >
              <TodoStateIcon state={row.state} />
              <span>{row.text}</span>
            </div>
          ))}
        </div>
      ) : null}
    </div>
  );
});
