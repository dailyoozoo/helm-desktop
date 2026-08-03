import { describe, expect, it } from 'vitest';
import { describePermissionRule } from './permissionRules';

describe('describePermissionRule', () => {
  it('renders a migrated command rule with human-readable engine, scope and capability', () => {
    expect(
      describePermissionRule({
        id: 'legacy-1',
        effect: 'allow',
        scope: 'global',
        engine: 'claude-code',
        capability: 'process_exec',
        operation: 'ls',
        resourcePattern: null,
        createdAt: 1,
        expiresAt: null,
        maxUses: null,
      }),
    ).toEqual({
      title: '允许执行命令：ls',
      meta: 'Claude Code · 全局',
    });
  });

  it('keeps unknown capabilities visible instead of pretending they are safe', () => {
    expect(
      describePermissionRule({
        id: 'unknown-1',
        effect: 'deny',
        scope: 'project',
        engine: null,
        capability: 'unknown:CustomTool',
        operation: 'CustomTool',
        resourcePattern: null,
        createdAt: 1,
        expiresAt: null,
        maxUses: null,
      }).title,
    ).toBe('拒绝未知能力（CustomTool）');
  });
});
