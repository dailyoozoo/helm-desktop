import { describe, expect, it } from 'vitest';
import type { ThreadItem } from '../../engine/useSession';
import {
  classifyToolFailure,
  FAILURE_KIND_LABELS,
  failureAdvice,
  retryCountFor,
  retryRequestText,
} from './failureCardViewModel';

type ToolItem = Extract<ThreadItem, { kind: 'tool' }>;

const tool = (id: string, partial: Partial<ToolItem> = {}): ToolItem => ({
  kind: 'tool',
  id,
  name: 'Bash',
  input: {},
  status: 'error',
  ...partial,
});

describe('classifyToolFailure', () => {
  it('权限拒绝优先，不依赖文本', () => {
    expect(classifyToolFailure(tool('a', { outcome: 'runtime_denied', output: 'any text' }))).toBe(
      'permission',
    );
    expect(classifyToolFailure(tool('b', { denialSource: 'runtime' }))).toBe('permission');
    expect(classifyToolFailure(tool('c', { denialSource: 'auto_reviewer' }))).toBe('permission');
    expect(classifyToolFailure(tool('d', { nativeDenialCode: 'NO_MATCHER' }))).toBe('permission');
  });

  it('工具自身报错（denial_source=tool）按输出归类，不误判为权限', () => {
    // MCP 服务自身报错（如 tavily 参数不兼容）不是 Helm 权限拒绝（9/4 修正）
    expect(classifyToolFailure(tool('a', { denialSource: 'tool' }))).toBe('tool');
    expect(
      classifyToolFailure(
        tool('b', { denialSource: 'tool', output: 'Country parameter is not supported' }),
      ),
    ).toBe('tool');
    expect(classifyToolFailure(tool('c', { denialSource: 'tool', output: 'ECONNREFUSED' }))).toBe(
      'network',
    );
  });

  it('按输出/输入文本归类网络、超时、凭据', () => {
    expect(classifyToolFailure(tool('a', { output: 'ECONNREFUSED' }))).toBe('network');
    expect(classifyToolFailure(tool('b', { output: 'Connection refused' }))).toBe('network');
    expect(classifyToolFailure(tool('c', { output: 'request timed out' }))).toBe('timeout');
    expect(classifyToolFailure(tool('d', { output: '401 Unauthorized' }))).toBe('auth');
    expect(
      classifyToolFailure(tool('e', { input: { apiKey: 'invalid' }, output: 'invalid key' })),
    ).toBe('auth');
  });

  it('文件系统权限错误归工具，不误判为凭据', () => {
    expect(classifyToolFailure(tool('a', { output: 'EACCES: permission denied' }))).toBe('tool');
  });

  it('模型类工具名归模型，其余兜底工具', () => {
    expect(classifyToolFailure(tool('a', { name: 'Chat', output: 'boom' }))).toBe('model');
    expect(classifyToolFailure(tool('b', { output: 'unknown' }))).toBe('tool');
  });
});

describe('failureAdvice / labels', () => {
  it('每个分类都有中文标签和自愈说明', () => {
    for (const kind of Object.keys(FAILURE_KIND_LABELS) as (keyof typeof FAILURE_KIND_LABELS)[]) {
      const advice = failureAdvice(kind);
      expect(FAILURE_KIND_LABELS[kind]).toBeTruthy();
      expect(advice.note).toBeTruthy();
      expect(typeof advice.selfHeal).toBe('boolean');
    }
  });

  it('权限/凭据不可自愈，网络/超时/工具/模型可自愈', () => {
    expect(failureAdvice('permission').selfHeal).toBe(false);
    expect(failureAdvice('auth').selfHeal).toBe(false);
    expect(failureAdvice('network').selfHeal).toBe(true);
    expect(failureAdvice('timeout').selfHeal).toBe(true);
    expect(failureAdvice('tool').selfHeal).toBe(true);
  });
});

describe('retryCountFor', () => {
  it('同一 Turn 同名工具出现次数减一为重试次数', () => {
    const items = [
      tool('a', { name: 'Bash', turnId: 't1', input: { command: 'npm test' } }),
      tool('b', { name: 'Bash', turnId: 't1', input: { command: 'npm test' } }),
      tool('c', { name: 'Bash', turnId: 't1', input: { command: 'npm test' } }),
    ];
    expect(retryCountFor(items, 'c')).toBe(2);
    expect(retryCountFor(items, 'a')).toBe(0);
  });

  it('跨 Turn 与异名工具不计数；无 turnId 返回 0', () => {
    const items = [
      tool('a', { name: 'Bash', turnId: 't1' }),
      tool('b', { name: 'Bash', turnId: 't2' }),
      tool('c', { name: 'Read', turnId: 't1' }),
      tool('d', { name: 'Bash' }),
    ];
    expect(retryCountFor(items, 'b')).toBe(0);
    expect(retryCountFor(items, 'c')).toBe(0);
    expect(retryCountFor(items, 'd')).toBe(0);
  });
});

describe('retryRequestText', () => {
  it('构造一条带失败摘要的用户消息', () => {
    const text = retryRequestText('Bash', 'psql: error\nconnection refused');
    expect(text).toContain('请重试上一步失败的工具：Bash');
    expect(text).toContain('connection refused');
    expect(text).toContain('替代方案');
  });

  it('无输出时省略摘要段', () => {
    const text = retryRequestText('Read');
    expect(text).not.toContain('失败输出摘要');
  });
});
