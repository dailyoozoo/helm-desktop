import { describe, expect, it } from 'vitest';
import type { ThreadItem } from '../../engine/useSession';
import {
  collectBackgroundCommands,
  collectSubagents,
  durationLabel,
  formatDuration,
  isBackgroundCommand,
  isSubagentToolName,
  subagentDisplayName,
  subagentTaskLabel,
} from './taskViewModel';

type ToolItem = Extract<ThreadItem, { kind: 'tool' }>;

const tool = (id: string, partial: Partial<ToolItem> = {}): ToolItem => ({
  kind: 'tool',
  id,
  name: 'Read',
  input: {},
  status: 'success',
  ...partial,
});

describe('isSubagentToolName', () => {
  it('识别 Claude Task / Codex Agent / subagent', () => {
    expect(isSubagentToolName('Task')).toBe(true);
    expect(isSubagentToolName('Agent')).toBe(true);
    expect(isSubagentToolName('subagent')).toBe(true);
    expect(isSubagentToolName('Task/specialized')).toBe(true);
    expect(isSubagentToolName('Read')).toBe(false);
    expect(isSubagentToolName('Grep')).toBe(false);
  });
});

describe('subagentTaskLabel / subagentDisplayName', () => {
  it('按权威顺序取任务描述', () => {
    expect(subagentTaskLabel({ description: '改 API 层', prompt: '长篇' })).toBe('改 API 层');
    expect(subagentTaskLabel({ instructions: '补测试' })).toBe('补测试');
    expect(subagentTaskLabel({ command: 'run' })).toBe('run');
    expect(subagentTaskLabel({ other: 1 })).toBe('');
  });

  it('优先 CLI 提供的 name', () => {
    expect(subagentDisplayName({ name: 'api-layer' }, 'Task')).toBe('api-layer');
    expect(subagentDisplayName({ tool_name: 'ui-layer' }, 'Task')).toBe('ui-layer');
    expect(subagentDisplayName({}, 'Task')).toBe('Task');
  });
});

describe('formatDuration / durationLabel', () => {
  it('毫秒转为原型同款文案', () => {
    expect(formatDuration(1_100)).toBe('1.1s');
    expect(formatDuration(42_000)).toBe('42s');
    expect(formatDuration(130_000)).toBe('2m 10s');
    expect(formatDuration(3_600_000)).toBe('1h');
    expect(formatDuration(5_400_000)).toBe('1.5h');
    expect(formatDuration(0)).toBe('0s');
  });

  it('无开始时间返回空；运行中取 now，完成后取 endedAt', () => {
    expect(durationLabel(undefined, undefined, 1000)).toBe('');
    expect(durationLabel(1000, undefined, 65_000)).toBe('1m 4s');
    expect(durationLabel(1000, 50_000, 999_999)).toBe('49s');
  });
});

describe('collectSubagents', () => {
  it('只收集子代理工具并映射状态/耗时', () => {
    const result = collectSubagents(
      [
        tool('a', {
          name: 'Task',
          input: { description: '改 API 层' },
          status: 'success',
          startedAt: 1000,
          endedAt: 5000,
        }),
        tool('b', {
          name: 'Task',
          input: { prompt: '补测试' },
          status: 'pending',
          startedAt: 3000,
        }),
        tool('c', { name: 'Read', status: 'success' }),
      ],
      63_000,
    );
    expect(result).toHaveLength(2);
    expect(result[0]).toMatchObject({
      id: 'a',
      name: 'Task',
      task: '改 API 层',
      dur: '4s',
      state: 'ok',
      status: 'success',
    });
    expect(result[1]).toMatchObject({ id: 'b', state: 'run', dur: '1m' });
  });

  it('子代理错误映射为 err', () => {
    const [entry] = collectSubagents([tool('a', { name: 'Agent', status: 'error' })]);
    expect(entry.state).toBe('err');
  });
});

describe('isBackgroundCommand / collectBackgroundCommands', () => {
  it('识别 is_background / 长 timeout 的终端工具', () => {
    expect(
      isBackgroundCommand(
        tool('a', { name: 'Bash', input: { command: 'npm run dev', is_background: true } }),
      ),
    ).toBe(true);
    expect(
      isBackgroundCommand(
        tool('b', { name: 'Bash', input: { command: 'npm test', timeout: 900_000 } }),
      ),
    ).toBe(true);
    expect(
      isBackgroundCommand(
        tool('c', { name: 'Bash', input: { command: 'echo hi', timeout: 30_000 } }),
      ),
    ).toBe(false);
    expect(isBackgroundCommand(tool('d', { name: 'Read' }))).toBe(false);
  });

  it('收集后台命令，未开始执行即失败的不算', () => {
    const result = collectBackgroundCommands(
      [
        tool('a', {
          name: 'Bash',
          input: { command: 'npm run dev', timeout: 900_000 },
          status: 'success',
          startedAt: 1000,
          endedAt: 2000,
        }),
        tool('b', {
          name: 'Bash',
          input: { command: 'bad', timeout: 900_000 },
          status: 'error',
          started: false,
        }),
        tool('c', { name: 'Read' }),
      ],
      5000,
    );
    expect(result).toHaveLength(1);
    expect(result[0]).toMatchObject({ id: 'a', command: 'npm run dev', dur: '1s', state: 'ok' });
  });
});
