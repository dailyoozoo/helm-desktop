import { describe, expect, it } from 'vitest';
import type { Budget, DailyUsage, ModelUsage, TopSession, UsageStats } from './api';
import { buildUsageCsv } from './exportCsv';

describe('buildUsageCsv', () => {
  it('exports usage dashboard data with csv escaping', () => {
    const stats: UsageStats = {
      total_cost: 12.3456,
      total_tokens: 1500,
      input_tokens: 1000,
      output_tokens: 500,
      request_count: 3,
      session_count: 2,
      actual_cost: 9.5,
      estimated_cost: 2.8456,
      subscription_count: 0,
      unknown_count: 0,
      legacy_cost: 0,
      legacy_count: 0,
      previous_total_cost: 10,
      previous_total_tokens: 1200,
      previous_request_count: 2,
      previous_session_count: 1,
    };
    const dailyUsage: DailyUsage[] = [{ date: '2026-06-15', cost_usd: 1.25 }];
    const modelUsage: ModelUsage[] = [
      {
        model: 'gpt-5, codex',
        engine: 'codex',
        request_count: 2,
        input_tokens: 700,
        output_tokens: 300,
        cost_usd: 9.5,
        share: 0.75,
      },
    ];
    const topSessions: TopSession[] = [
      {
        id: 's-1',
        title: '修复 "CSV" 导出',
        model: 'gpt-5',
        engine: 'codex',
        cost_usd: 9.5,
        total_tokens: 1000,
      },
    ];
    const budget: Budget = {
      monthly_limit: 20,
      alert_at_80: true,
      stop_at_100: true,
      current_month_cost: 12.3456,
      percentage: 61.728,
    };

    const csv = buildUsageCsv({
      periodLabel: '近 7 天',
      stats,
      dailyUsage,
      modelUsage,
      topSessions,
      budget,
      generatedAt: new Date('2026-06-15T02:00:00.000Z'),
    });

    expect(csv).toContain('Helm 用量与成本导出');
    expect(csv).toContain('统计项,值');
    expect(csv).toContain('总花费 USD,12.3456');
    expect(csv).toContain('实际花费 USD,9.5000');
    expect(csv).toContain('每日花费');
    expect(csv).toContain('2026-06-15,1.2500');
    expect(csv).toContain('"gpt-5, codex",codex,2,700,300,9.5000,75.00%');
    expect(csv).toContain('s-1,"修复 ""CSV"" 导出",gpt-5,codex,9.5000,1000');
    expect(csv).toContain('月度预算 USD,20.0000');
  });
});
