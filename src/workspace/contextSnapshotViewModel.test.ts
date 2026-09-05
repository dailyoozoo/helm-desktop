import { describe, expect, it } from 'vitest';
import type { ThreadItem } from '../engine/useSession';
import type { McpServer } from '../extensions/extensionsApi';
import type { SessionContextRecord } from '../sessions/api';
import {
  contextRingHoverSummary,
  contextSnapshot,
  type ContextSnapshotInput,
} from './contextSnapshotViewModel';

const items: ThreadItem[] = [
  {
    kind: 'user',
    id: 'u1',
    text: '修改鉴权',
    attachments: ['D:\\work\\app.ts'],
  },
  {
    kind: 'assistant',
    id: 'a1',
    text: 'ok',
  },
];

const mcpServers: McpServer[] = [
  {
    name: 'github',
    command: 'npx',
    args: [],
    env: {},
    transport: 'stdio',
    enabled: true,
    status: 'connected',
    toolCount: 12,
    lastError: null,
  },
  {
    name: 'context7',
    command: 'npx',
    args: [],
    env: {},
    transport: 'stdio',
    enabled: true,
    status: 'connected',
    toolCount: 2,
    lastError: null,
  },
  {
    name: 'broken',
    command: 'npx',
    args: [],
    env: {},
    transport: 'stdio',
    enabled: true,
    status: 'error',
    toolCount: 0,
    lastError: ' spawn failed',
  },
];

const sessionContexts: SessionContextRecord[] = [
  {
    id: 'ctx-1',
    kind: 'file',
    sourcePath: 'D:\\work\\docs\\intro.md',
    canonicalPath: 'D:\\work\\docs\\intro.md',
    displayName: 'intro.md',
    status: 'ready',
    createdAt: 1,
    updatedAt: 1,
  },
];

function build(overrides: Partial<ContextSnapshotInput> = {}) {
  return contextSnapshot({
    items,
    cost: {
      inputTokens: 1_000,
      cachedInputTokens: 700,
      cacheWriteInputTokens: 200,
      outputTokens: 100,
      contextTokens: 62_000,
      contextWindow: 200_000,
    },
    mcpServers,
    disabledMcp: ['context7'],
    sessionContexts,
    ...overrides,
  });
}

describe('contextSnapshot', () => {
  it('派生历史附件、会话上下文与有效 MCP（按 disabledMcp 过滤）', () => {
    const snap = build();
    expect(snap.historicalAttachments).toEqual(['D:\\work\\app.ts']);
    expect(snap.sessionContexts).toHaveLength(1);
    // github 已连接且未停用 → 启用；context7 已连接但被本会话停用 → 不在 enabled 中；
    // broken 有 lastError → 不算已连接，也不在 enabled
    expect(snap.mcpEnabled.map((mcp) => mcp.name)).toEqual(['github']);
    expect(snap.mcpDisabled.map((mcp) => mcp.name)).toEqual(['context7']);
  });

  it('圆环 hover 摘要：合并历史附件 + 会话上下文，只算启用的 MCP', () => {
    const snap = build();
    const summary = contextRingHoverSummary(snap);
    expect(summary?.percent).toBe(31);
    expect(summary?.tokens).toBe(62_000);
    expect(summary?.maxTokens).toBe(200_000);
    // 历史附件 1 + 会话上下文 1 = 2
    expect(summary?.files).toBe(2);
    expect(summary?.mcp).toBe(1);
  });

  it('缺逐调用 context_usage 时圆环返回占位摘要（不估算）', () => {
    const snap = build({
      cost: { inputTokens: 1_000, outputTokens: 0 },
    });
    const summary = contextRingHoverSummary(snap);
    expect(summary?.percent).toBeNull();
    expect(summary?.tokens).toBeNull();
    // 仍提供真实计数，不伪造占用
    expect(summary?.files).toBe(2);
    expect(summary?.mcp).toBe(1);
  });

  it('归因恒为空数组（协议无逐来源字段，AGENTS.md 红线禁止估算）', () => {
    const snap = build();
    expect(snap.attribution).toEqual([]);
  });

  it('无 mcpServers 时 mcpEnabled 与 mcpDisabled 均为空', () => {
    const snap = build({ mcpServers: [], disabledMcp: ['nothing'] });
    expect(snap.mcpEnabled).toEqual([]);
    expect(snap.mcpDisabled).toEqual([]);
  });
});
