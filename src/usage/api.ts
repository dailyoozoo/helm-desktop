// 用量与成本 API 层
import { invoke } from '@tauri-apps/api/core';

export interface UsageStats {
  total_cost: number;
  total_tokens: number;
  input_tokens: number;
  output_tokens: number;
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

export interface ModelUsage {
  model: string;
  engine: string;
  request_count: number;
  input_tokens: number;
  output_tokens: number;
  cost_usd: number;
  share: number;
}

export interface DailyUsage {
  date: string;
  cost_usd: number;
}

/** 按服务商聚合（P3-6）：provider 是 provider_id，空串 = 未标注的旧会话 */
export interface ProviderUsage {
  provider: string;
  cost_usd: number;
  share: number;
}

export interface TopSession {
  id: string;
  title: string;
  model: string;
  engine: string;
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

export async function getUsageByModel(days: number): Promise<ModelUsage[]> {
  return invoke<ModelUsage[]>('get_usage_by_model', { days });
}

export async function getUsageByProvider(days: number): Promise<ProviderUsage[]> {
  return invoke<ProviderUsage[]>('get_usage_by_provider', { days });
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
