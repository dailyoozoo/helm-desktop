import type { CliLoginState, ProviderConfig } from '../providers/api';
import { providerRuntimeReady } from '../providers/providerViewModel';
import type { DailyUsage, UsageBreakdownRow } from './api';

/**
 * 缓存命中率 = 分子（cached_input_tokens）/ 分母（同窗口 input_tokens）。
 * 任一字段为 null（无 token 证据，界面显示「暂无」）或分母非正数时返回 null；
 * 禁止用累计花费或其它口径估算。
 */
export function cacheRate(numerator: number | null, denominator: number | null): number | null {
  if (numerator === null || denominator === null) return null;
  if (denominator <= 0 || numerator < 0 || denominator < 0) return null;
  return numerator / denominator;
}

export function percentageChange(current: number, previous: number): number | null {
  // 后端未回传基期字段（undefined/null/非有限值）时按无基数处理，禁止渲染 NaN
  if (!Number.isFinite(previous) || !Number.isFinite(current)) return null;
  if (previous === 0) return current === 0 ? 0 : null;
  return ((current - previous) / previous) * 100;
}

export function comparisonText(current: number, previous: number, label = '较前一期'): string {
  const change = percentageChange(current, previous);
  if (change === null) return `${label}无基数`;
  if (Math.abs(change) < 0.5) return `${label}持平`;
  return `${label} ${change > 0 ? '+' : ''}${Math.round(change)}%`;
}

export function projectedMonthEndCost(currentMonthCost: number, now = new Date()): number {
  const day = Math.max(1, now.getDate());
  const daysInMonth = new Date(now.getFullYear(), now.getMonth() + 1, 0).getDate();
  return (currentMonthCost / day) * daysInMonth;
}

export function createRequestGate() {
  let current = 0;
  return {
    begin: () => ++current,
    isCurrent: (generation: number) => generation === current,
  };
}

/** S5 验收口径：高用量任务固定取 limit=5。 */
export const TOP_TASKS_LIMIT = 5;

/** S5 验收口径：热力图窗口固定 365 天，不随时间范围分段切换。 */
export const HEATMAP_DAYS = 365;

function trimTrailingZeros(value: number): string {
  const s = value >= 100 ? value.toFixed(0) : value >= 10 ? value.toFixed(1) : value.toFixed(2);
  return s.replace(/(\.\d*[1-9])0+$/, '$1').replace(/\.0$/, '');
}

/** 紧凑 token 展示：18.6M / 842K / 940；只做格式化，不改变数值口径。 */
export function formatCompactTokens(value: number): string {
  if (!Number.isFinite(value) || value <= 0) return '0';
  if (value >= 1_000_000) return `${trimTrailingZeros(value / 1_000_000)}M`;
  if (value >= 1_000) return `${trimTrailingZeros(value / 1_000)}K`;
  return String(Math.round(value));
}

/** SQL DATE（'2026-08-02'）→ '8月2日'；非法输入原样返回。 */
export function formatMonthDay(date: string): string {
  const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(date);
  if (!m) return date;
  return `${Number(m[2])}月${Number(m[3])}日`;
}

function dateKey(date: Date): string {
  const y = date.getFullYear();
  const m = String(date.getMonth() + 1).padStart(2, '0');
  const d = String(date.getDate()).padStart(2, '0');
  return `${y}-${m}-${d}`;
}

export interface HeatmapCell {
  date: string;
  /** 当日输入+输出 token；无记录或当日全部为 legacy 行（无 token 证据）时为 null。 */
  tokens: number | null;
  requests: number;
  /** 0..4；只按真实 token 值在有值日期中的分位分档，不用花费/调用数反推 token。 */
  level: number;
}

/**
 * 固定 days（默认 365）天、从最早一天到今天的活跃度格子。
 * 后端只返回有记录的日期，缺失日期在此补齐为「无记录」；
 * legacy 日保留真实调用次数但 tokens 为 null，档位为 0，由 tooltip 说明差异。
 */
export function buildHeatmapCells(
  dailyUsage: readonly DailyUsage[],
  now: Date,
  days = HEATMAP_DAYS,
): HeatmapCell[] {
  const byDate = new Map(dailyUsage.map((d) => [d.date, d]));
  const cells: HeatmapCell[] = [];
  for (let i = days - 1; i >= 0; i -= 1) {
    const day = new Date(now.getFullYear(), now.getMonth(), now.getDate() - i);
    const key = dateKey(day);
    const row = byDate.get(key);
    const tokens =
      row && row.input_tokens !== null && row.output_tokens !== null
        ? row.input_tokens + row.output_tokens
        : null;
    cells.push({ date: key, tokens, requests: row?.request_count ?? 0, level: 0 });
  }
  const positives = cells
    .map((c) => c.tokens ?? 0)
    .filter((v) => v > 0)
    .sort((a, b) => a - b);
  if (positives.length > 0) {
    const quantile = (p: number) =>
      positives[Math.min(positives.length - 1, Math.floor(p * positives.length))];
    const t1 = quantile(0.25);
    const t2 = quantile(0.5);
    const t3 = quantile(0.75);
    for (const cell of cells) {
      const v = cell.tokens ?? 0;
      cell.level = v <= 0 ? 0 : v <= t1 ? 1 : v <= t2 ? 2 : v <= t3 ? 3 : 4;
    }
  }
  return cells;
}

export interface DailyChartPoint {
  date: string;
  inputTokens: number | null;
  outputTokens: number | null;
  requests: number;
}

export interface DailyChartModel {
  points: DailyChartPoint[];
  /** 输入/输出柱共用的峰值刻度；null 字段按 0 参与缩放但不渲染柱体。 */
  maxTokens: number;
  maxRequests: number;
}

/** 每日用量双轴缩放模型：输入/输出 token 走柱体，调用次数走折线，各自以窗口峰值为满刻度。 */
export function buildDailyChart(data: readonly DailyUsage[]): DailyChartModel {
  const points = data.map((d) => ({
    date: d.date,
    inputTokens: d.input_tokens,
    outputTokens: d.output_tokens,
    requests: d.request_count,
  }));
  const maxTokens = Math.max(
    0,
    ...data.flatMap((d) => [d.input_tokens ?? 0, d.output_tokens ?? 0]),
  );
  const maxRequests = Math.max(0, ...data.map((d) => d.request_count));
  return { points, maxTokens, maxRequests };
}

/** 构成行总 token：任一方向缺失即返回 null（界面显示「暂无」），禁止相加估算。 */
export function breakdownTotalTokens(
  row: Pick<UsageBreakdownRow, 'input_tokens' | 'output_tokens'>,
): number | null {
  return row.input_tokens !== null && row.output_tokens !== null
    ? row.input_tokens + row.output_tokens
    : null;
}

export type BreakdownCostNote = '未计价' | '历史金额' | '等效折算' | null;

/**
 * 成本注记只来自后端返回的 cost_kinds 计数：
 * 全部无价格数据 → 未计价；全部历史金额 → 历史金额；
 * 无 actual 但有订阅/估算折算 → 等效折算；混合口径不加注记。
 */
export function breakdownCostNote(
  row: Pick<UsageBreakdownRow, 'cost_kinds' | 'request_count'>,
): BreakdownCostNote {
  const kinds = row.cost_kinds;
  if (row.request_count <= 0) return null;
  if (kinds.unknown === row.request_count) return '未计价';
  if (kinds.legacy === row.request_count) return '历史金额';
  if (kinds.actual === 0 && kinds.subscription + kinds.estimated > 0) return '等效折算';
  return null;
}

/** 服务商维度构成行：在 S4 冻结聚合行上补充真实服务商配置信息。 */
export interface ProviderBreakdownRow extends UsageBreakdownRow {
  name: string;
  kind: ProviderConfig['kind'] | 'unknown';
  ready: boolean;
}

/**
 * 服务商构成行并入服务商配置（P3-6：key = provider_id，空串 = 未标注旧会话）。
 * 登录状态沿用现有真实 detectCliLogin 结果，不在本层探测或伪造。
 */
export function mergeProviderBreakdown(
  providers: readonly ProviderConfig[],
  rows: readonly UsageBreakdownRow[],
  loginByProvider: Readonly<Record<string, CliLoginState | null>> = {},
): ProviderBreakdownRow[] {
  const providerById = new Map(providers.map((provider) => [provider.id, provider]));
  return rows.map((row) => {
    const provider = providerById.get(row.key);
    return {
      ...row,
      name: provider?.name ?? (row.key ? row.key : '未标注（旧会话）'),
      kind: provider?.kind ?? 'unknown',
      ready: provider
        ? providerRuntimeReady(provider, loginByProvider[provider.id] ?? null)
        : false,
    };
  });
}
