import type { EngineId } from '@helm/protocol';
// 用量与成本 API 层
import { invoke } from '@tauri-apps/api/core';

export interface UsageStats {
  total_cost: number;
  total_tokens: number;
  input_tokens: number;
  output_tokens: number;
  /** S4：缓存命中分子（分母是本窗口 input_tokens）；缓存列晚于最早记录，旧行回填 0 */
  cached_input_tokens: number;
  cache_write_input_tokens: number;
  request_count: number;
  session_count: number;
  actual_cost: number;
  estimated_cost: number;
  subscription_count: number;
  unknown_count: number;
  legacy_cost: number;
  legacy_count: number;
  previous_total_cost: number;
  previous_total_tokens: number;
  previous_request_count: number;
  previous_session_count: number;
}

/** S4 冻结：用量查询只支持这四个时间范围，后端对其他值 fail-closed */
export const USAGE_RANGE_DAYS = [7, 30, 90, 365] as const;
export type UsageRangeDays = (typeof USAGE_RANGE_DAYS)[number];

/**
 * S4 冻结的日粒度契约：每天返回真实调用次数与输入/输出/缓存 token；
 * 无 token 证据（当日全部是 legacy 行，只有金额）时 token 字段为 null（暂无），禁止估算。
 */
export interface DailyUsage {
  date: string;
  cost_usd: number;
  request_count: number;
  input_tokens: number | null;
  output_tokens: number | null;
  /** 缓存命中率分子；分母为同日 input_tokens */
  cached_input_tokens: number | null;
  cache_write_input_tokens: number | null;
}

/** S4 冻结的统一分组维度：model 按（模型, 引擎）成组；engine/provider 按单键成组 */
export type UsageBreakdownDimension = 'model' | 'engine' | 'provider';

/** 成本类型计数，口径与 UsageStats 的 cost_kind 一致 */
export interface UsageCostKindCounts {
  actual: number;
  estimated: number;
  subscription: number;
  unknown: number;
  legacy: number;
}

/**
 * 统一分组聚合行：
 * - model 维度：key = 模型 ID，engine = 该组运行引擎；
 * - engine 维度：key = engine id；
 * - provider 维度（P3-6）：key = provider_id，空串 = 未标注的旧会话。
 * 缓存命中率分子 = cached_input_tokens、分母 = 同组 input_tokens，两者同源可追溯；
 * 组内全部 legacy 时 token 字段为 null，禁止用费用反推。
 */
export interface UsageBreakdownRow {
  key: string;
  /** 运行引擎 id；后端按技术方案 §5.3 归属规则只产出 claude-code | codex */
  engine: EngineId;
  request_count: number;
  input_tokens: number | null;
  output_tokens: number | null;
  cached_input_tokens: number | null;
  cache_write_input_tokens: number | null;
  cost_usd: number;
  share: number;
  cost_kinds: UsageCostKindCounts;
}

export interface TopSession {
  id: string;
  title: string;
  model: string;
  engine: EngineId;
  cost_usd: number;
  total_tokens: number;
}

export interface Budget {
  monthly_limit: number;
  alert_at_80: boolean;
  stop_at_100: boolean;
  current_month_cost: number;
  percentage: number;
}

export async function getUsageStats(days: number): Promise<UsageStats> {
  return invoke<UsageStats>('get_usage_stats', { days });
}

export async function getUsageBreakdown(
  days: number,
  dimension: UsageBreakdownDimension,
): Promise<UsageBreakdownRow[]> {
  return invoke<UsageBreakdownRow[]>('get_usage_breakdown', { days, dimension });
}

export async function getDailyUsage(days: number): Promise<DailyUsage[]> {
  return invoke<DailyUsage[]>('get_daily_usage', { days });
}

export async function getTopSessions(days: number, limit: number): Promise<TopSession[]> {
  return invoke<TopSession[]>('get_top_sessions', { days, limit });
}

export async function getBudget(): Promise<Budget> {
  return invoke<Budget>('get_budget');
}

export async function setBudget(
  monthlyLimit: number,
  alertAt80: boolean,
  stopAt100: boolean,
): Promise<void> {
  return invoke<void>('set_budget', {
    monthlyLimit,
    alertAt80,
    stopAt100,
  });
}
