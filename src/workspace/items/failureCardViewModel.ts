import type { ThreadItem } from '../../engine/useSession';

// 变更-34 · C4：失败终态增强 —— 错误分类、已重试次数、能否自愈、下一步动作。
// 只基于工具的真实 outcome / denialSource / 输出文本归类，不伪造时序或百分比。

export type FailureKind = 'permission' | 'network' | 'auth' | 'timeout' | 'tool' | 'model';

export const FAILURE_KIND_LABELS: Record<FailureKind, string> = {
  permission: '权限失败',
  network: '网络失败',
  auth: '凭据失败',
  timeout: '超时失败',
  tool: '工具失败',
  model: '模型失败',
};

export interface FailureAdvice {
  kind: FailureKind;
  /** 重试能否自愈 —— 环境类失败重试无意义，不该让用户空等。 */
  selfHeal: boolean;
  note: string;
}

const ADVICE: Record<FailureKind, FailureAdvice> = {
  permission: {
    kind: 'permission',
    selfHeal: false,
    note: '该调用被权限系统拒绝。若在可授权范围，手动授权后重试可能通过；否则重试无效。',
  },
  network: {
    kind: 'network',
    selfHeal: true,
    note: '网络类失败通常短暂，稍后重试可能自愈；若持续，需检查代理或目标服务连通性。',
  },
  auth: {
    kind: 'auth',
    selfHeal: false,
    note: '凭据类失败重试无效，需先检查 API Key、登录态或服务商配置。',
  },
  timeout: {
    kind: 'timeout',
    selfHeal: true,
    note: '超时可能受负载影响，重试有机会自愈；持续超时可调大时限后重试。',
  },
  tool: {
    kind: 'tool',
    selfHeal: true,
    note: '工具已开始执行后失败，重试可能自愈；若是确定性报错，建议改指令后重试。',
  },
  model: {
    kind: 'model',
    selfHeal: true,
    note: '模型接口异常，重试通常可恢复。',
  },
};

const NETWORK_RE =
  /ECONNREFUSED|ETIMEDOUT|ENOTFOUND|EAI_AGAIN|connection refused|network error|fetch failed|socket hang up|no such host|unavailable/i;
const AUTH_RE =
  /unauthorized|invalid .*api.?key|authentication|credential|login required|api.?key.*invalid|401|403/i;
const TIMEOUT_RE = /timed? ?out|deadline exceeded/i;
const FS_PERM_RE = /EACCES|permission denied/i;
const MODEL_NAME_RE = /^(llm|model|generate|chat)/i;

export type ToolFailureSource = {
  name: string;
  input?: unknown;
  output?: string;
  outcome?: Extract<ThreadItem, { kind: 'tool' }>['outcome'];
  denialSource?: Extract<ThreadItem, { kind: 'tool' }>['denialSource'];
  nativeDenialCode?: Extract<ThreadItem, { kind: 'tool' }>['nativeDenialCode'];
};

export function classifyToolFailure(item: ToolFailureSource): FailureKind {
  // 9/4 修正：denial_source='tool' 表示「工具自身报错」（如 MCP 服务返回参数不兼容），
  // 不是 Helm 权限系统拒绝——只有 runtime/auto_reviewer 的拒绝才归「权限失败」，
  // 否则 tavily 参数错误这类普通失败会被误标成权限问题、误导用户去找授权入口。
  // tool 来源且无 outcome 拒绝标记时按输出文本正常归类。
  if (item.outcome === 'runtime_denied' || item.denialSource === 'runtime') {
    return 'permission';
  }
  if (item.denialSource === 'auto_reviewer' || item.nativeDenialCode) {
    return 'permission';
  }
  const text = `${item.output ?? ''}\n${JSON.stringify(item.input ?? {})}`;
  if (NETWORK_RE.test(text)) return 'network';
  if (TIMEOUT_RE.test(text)) return 'timeout';
  if (AUTH_RE.test(text)) return 'auth';
  if (FS_PERM_RE.test(text)) return 'tool';
  if (MODEL_NAME_RE.test(item.name)) return 'model';
  return 'tool';
}

export function failureAdvice(kind: FailureKind): FailureAdvice {
  return ADVICE[kind];
}

/** 同一 Turn 中目标工具之前的同名工具数 = 已重试次数（真实 Ledger 事实，不伪造）。 */
export function retryCountFor(items: ThreadItem[], targetId: string): number {
  const target = items.find((item) => item.kind === 'tool' && item.id === targetId);
  if (!target || target.kind !== 'tool' || !target.turnId) return 0;
  let count = 0;
  for (const item of items) {
    if (item.id === targetId) break;
    if (item.kind !== 'tool' || item.turnId !== target.turnId || item.name !== target.name)
      continue;
    count += 1;
  }
  return count;
}

/** 「重试这一步」实际是把失败工具作为一条真实用户消息发回给 Agent（Helm 不自行重放工具）。 */
export function retryRequestText(name: string, output?: string): string {
  const head = (output ?? '').split('\n').filter(Boolean).slice(0, 3).join('\n').slice(0, 300);
  return [
    `请重试上一步失败的工具：${name}。`,
    head ? `失败输出摘要：\n${head}` : '',
    '若仍失败，请说明原因并给出替代方案。',
  ]
    .filter(Boolean)
    .join('\n');
}
