import { describe, expect, it } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import type { ThreadItem } from '../engine/useSession';
import { TasksPanel } from './TasksPanel';

const tool = (
  id: string,
  partial: Partial<Extract<ThreadItem, { kind: 'tool' }>> = {},
): ThreadItem => ({
  kind: 'tool',
  id,
  name: 'Read',
  input: {},
  status: 'success',
  ...partial,
});

describe('TasksPanel', () => {
  it('列出子代理与后台命令，运行中的后台命令显示停止按钮', () => {
    const markup = renderToStaticMarkup(
      <TasksPanel
        items={[
          tool('task-1', { name: 'Task', input: { description: '改 API 层' }, status: 'success' }),
          tool('bg-1', {
            name: 'Bash',
            input: { command: 'npm run dev', timeout: 900_000 },
            status: 'pending',
          }),
          tool('bg-2', {
            name: 'Bash',
            input: { command: 'npm run build', timeout: 900_000 },
            status: 'success',
          }),
        ]}
        onStopTask={() => undefined}
      />,
    );
    expect(markup).toContain('子代理');
    expect(markup).toContain('改 API 层');
    expect(markup).toContain('后台命令');
    expect(markup).toContain('npm run dev');
    expect(markup).toContain('npm run build');
    expect(markup).toContain('停止本轮');
    expect(markup).toContain('中断当前轮次');
    expect(markup).toContain('1 个');
  });

  it('子代理为空时显示空态', () => {
    const markup = renderToStaticMarkup(<TasksPanel items={[]} />);
    expect(markup).toContain('本会话没有子代理');
    expect(markup).toContain('没有后台命令');
    expect(markup).not.toContain('停止本轮');
  });
  it('提供 onLocate 时子代理行渲染定位按钮', () => {
    const markup = renderToStaticMarkup(
      <TasksPanel
        items={[
          tool('task-1', { name: 'Agent', input: { instructions: '补测试' }, status: 'success' }),
        ]}
        onLocate={() => undefined}
      />,
    );
    expect(markup).toContain('在线程中定位');
  });
});
