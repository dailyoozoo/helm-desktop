import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import type { ThreadItem } from '../engine/useSession';
import { Workstrip, workstripTodo } from './Workstrip';

const planItem = {
  kind: 'plan',
  id: 'p1',
  steps: [
    { text: '调研', status: 'done' },
    { text: '实现', status: 'active' },
    { text: '验证', status: 'pending' },
  ],
  turnId: 'turn-1',
} as unknown as ThreadItem;

describe('workstripTodo', () => {
  it('projects the latest plan steps with done/doing/todo states', () => {
    const rows = workstripTodo([planItem]);
    expect(rows.map((r) => r.state)).toEqual(['done', 'doing', 'todo']);
    expect(rows[1].text).toBe('实现');
  });

  it('returns empty without a plan', () => {
    expect(workstripTodo([])).toEqual([]);
  });
});

describe('Workstrip', () => {
  const agents = [
    { id: 'a1', name: 'scout', task: '调研', dur: '2s', state: 'ok', status: 'success' },
    { id: 'a2', name: 'runner', task: '测试', dur: '3s', state: 'err', status: 'error' },
  ] as const;

  it('renders main line, agent chips and todo toggle when running with data', () => {
    const html = renderToStaticMarkup(
      <Workstrip
        working
        waitingApproval={false}
        activityLabel="正在思考…"
        agents={[...agents]}
        todo={workstripTodo([planItem])}
        onLocateAgent={() => undefined}
      />,
    );
    expect(html).toContain('主 Agent');
    expect(html).toContain('正在思考…');
    expect(html).toContain('agentchip');
    expect(html).toContain('Todo 1/3');
  });

  it('shows the approval wording while waiting', () => {
    const html = renderToStaticMarkup(
      <Workstrip
        working={false}
        waitingApproval
        activityLabel={null}
        agents={[]}
        todo={workstripTodo([planItem])}
        onLocateAgent={() => undefined}
      />,
    );
    expect(html).toContain('等待你确认权限后继续');
  });

  it('renders nothing when idle without data', () => {
    const html = renderToStaticMarkup(
      <Workstrip
        working={false}
        waitingApproval={false}
        activityLabel={null}
        agents={[]}
        todo={[]}
        onLocateAgent={() => undefined}
      />,
    );
    expect(html).not.toContain('ws-workstrip__line');
  });
});
