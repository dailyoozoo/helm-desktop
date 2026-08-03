export interface PermissionRule {
  id: string;
  effect: 'allow' | 'ask' | 'deny';
  scope: 'once' | 'turn' | 'session' | 'project' | 'global';
  engine: string | null;
  capability: string;
  operation: string | null;
  resourcePattern: string | null;
  createdAt: number;
  expiresAt: number | null;
  maxUses: number | null;
  uses?: number;
  principal?: string;
  scopeBinding?: {
    toolCallId?: string | null;
    turnId?: string | null;
    sessionId?: string | null;
    projectRoot?: string | null;
  };
}

const effectLabels: Record<PermissionRule['effect'], string> = {
  allow: '允许',
  ask: '每次询问',
  deny: '拒绝',
};

const scopeLabels: Record<PermissionRule['scope'], string> = {
  once: '仅一次',
  turn: '本轮',
  session: '本会话',
  project: '本项目',
  global: '全局',
};

function capabilityLabel(rule: PermissionRule): string {
  const target = rule.resourcePattern ? `：${rule.resourcePattern}` : '';
  switch (rule.capability) {
    case 'file_read':
      return `读取文件${target}`;
    case 'file_write':
      return `修改文件${target}`;
    case 'process_exec':
      return rule.operation ? `执行命令：${rule.operation}` : '执行命令';
    case 'network_request':
      return `访问网络${target}`;
    case 'mcp_invoke':
      return rule.operation ? `调用 MCP：${rule.operation}` : '调用 MCP';
    default: {
      const unknown = rule.capability.startsWith('unknown:')
        ? rule.capability.slice('unknown:'.length)
        : rule.capability;
      return `未知能力（${unknown}）`;
    }
  }
}

export function describePermissionRule(rule: PermissionRule): { title: string; meta: string } {
  const engine =
    rule.engine === 'claude-code' ? 'Claude Code' : rule.engine === 'codex' ? 'Codex' : '所有引擎';
  const project =
    rule.scope === 'project' && rule.scopeBinding?.projectRoot
      ? ` · ${rule.scopeBinding.projectRoot}`
      : '';
  return {
    title: `${effectLabels[rule.effect]}${capabilityLabel(rule)}`,
    meta: `${engine} · ${scopeLabels[rule.scope]}${project}`,
  };
}
