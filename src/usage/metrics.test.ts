import { describe, expect, it } from 'vitest';
import type { CliLoginState, ProviderConfig } from '../providers/api';
import type { DailyUsage, UsageBreakdownRow } from './api';
import {
  HEATMAP_DAYS,
  TOP_TASKS_LIMIT,
  breakdownCostNote,
  breakdownTotalTokens,
  buildDailyChart,
  buildHeatmapCells,
  cacheRate,
  comparisonText,
  createRequestGate,
  formatCompactTokens,
  formatMonthDay,
  mergeProviderBreakdown,
  percentageChange,
  projectedMonthEndCost,
} from './metrics';

const providers: ProviderConfig[] = [
  {
    id: 'api',
    name: 'API Provider',
    kind: 'api',
    baseUrl: 'https://example.com',
    keyRef: 'key',
    ready: true,
    lastTest: { result: 'ok', at: 1 },
    protocol: 'anthropic',
    authMethod: 'apikey',
  },
  {
    id: 'subscription',
    name: 'Subscription Provider',
    kind: 'subscription',
    baseUrl: '',
    keyRef: null,
    ready: true,
    lastTest: null,
    protocol: 'openai-responses',
    authMethod: 'oauth',
  },
];

function kinds(
  partial: Partial<UsageBreakdownRow['cost_kinds']> = {},
): UsageBreakdownRow['cost_kinds'] {
  return { actual: 0, estimated: 0, subscription: 0, unknown: 0, legacy: 0, ...partial };
}

describe('usage metrics', () => {
  it('calculates comparisons without inventing a percentage from a zero baseline', () => {
    expect(percentageChange(12, 10)).toBe(20);
    expect(percentageChange(0, 0)).toBe(0);
    expect(percentageChange(5, 0)).toBeNull();
    expect(comparisonText(5, 0)).toBe('较前一期无基数');
    // 视觉入口 fixture 缺基期字段时曾渲染 NaN%（2026-09 回归）：非有限基期一律按无基数
    expect(percentageChange(12, Number.NaN)).toBeNull();
    expect(percentageChange(12, undefined as unknown as number)).toBeNull();
    expect(comparisonText(12, undefined as unknown as number)).toBe('较前一期无基数');
  });

  it('uses one calendar-aware month-end projection algorithm', () => {
    expect(projectedMonthEndCost(140, new Date(2026, 1, 14))).toBe(280);
    expect(projectedMonthEndCost(31, new Date(2026, 0, 1))).toBe(961);
  });

  it('accepts only the newest result during concurrent refreshes', () => {
    const gate = createRequestGate();
    const first = gate.begin();
    const second = gate.begin();
    expect(gate.isCurrent(first)).toBe(false);
    expect(gate.isCurrent(second)).toBe(true);
  });

  it('computes cache rate only from real numerator and denominator pairs', () => {
    // S4 契约：分子/分母来自同一聚合；任一字段缺失（null）显示暂无，禁止估算
    expect(cacheRate(400, 1000)).toBeCloseTo(0.4, 9);
    expect(cacheRate(null, 1000)).toBeNull();
    expect(cacheRate(400, null)).toBeNull();
    expect(cacheRate(0, 0)).toBeNull();
    expect(cacheRate(0, 800)).toBe(0);
    expect(cacheRate(-1, 800)).toBeNull();
  });

  it('formats compact token counts without changing magnitudes', () => {
    expect(formatCompactTokens(18_600_000)).toBe('18.6M');
    expect(formatCompactTokens(14_200_000)).toBe('14.2M');
    expect(formatCompactTokens(4_400_000)).toBe('4.4M');
    expect(formatCompactTokens(842_000)).toBe('842K');
    expect(formatCompactTokens(999_999)).toBe('1000K');
    expect(formatCompactTokens(940)).toBe('940');
    expect(formatCompactTokens(0)).toBe('0');
  });

  it('renders SQL dates as compact month-day labels', () => {
    expect(formatMonthDay('2026-08-02')).toBe('8月2日');
    expect(formatMonthDay('2026-12-31')).toBe('12月31日');
    expect(formatMonthDay('not-a-date')).toBe('not-a-date');
  });

  describe('buildHeatmapCells', () => {
    const now = new Date(2026, 7, 22); // 2026-08-22，本地时区

    function daily(date: string, over: Partial<DailyUsage> = {}): DailyUsage {
      return {
        date,
        cost_usd: 1,
        request_count: 1,
        input_tokens: 0,
        output_tokens: 0,
        cached_input_tokens: 0,
        cache_write_input_tokens: 0,
        ...over,
      };
    }

    it('always renders a fixed 365-day window ending today', () => {
      const cells = buildHeatmapCells([], now);
      expect(cells).toHaveLength(365);
      expect(cells[0].date).toBe('2025-08-23');
      expect(cells[364].date).toBe('2026-08-22');
      expect(cells.every((cell) => cell.tokens === null && cell.requests === 0)).toBe(true);
      expect(cells.every((cell) => cell.level === 0)).toBe(true);
      expect(HEATMAP_DAYS).toBe(365);
    });

    it('maps recorded days by date and sums real input/output tokens', () => {
      const cells = buildHeatmapCells(
        [
          daily('2026-08-21', { request_count: 3, input_tokens: 100, output_tokens: 900 }),
          daily('2026-08-22', { request_count: 10, input_tokens: 5_000, output_tokens: 500 }),
        ],
        now,
      );
      const byDate = new Map(cells.map((cell) => [cell.date, cell]));
      expect(byDate.get('2026-08-21')).toMatchObject({ tokens: 1000, requests: 3 });
      expect(byDate.get('2026-08-22')).toMatchObject({ tokens: 5500, requests: 10 });
      // 更高的真实 token 必须得到不低于低值日的档位
      expect(byDate.get('2026-08-22')!.level).toBeGreaterThanOrEqual(
        byDate.get('2026-08-21')!.level,
      );
    });

    it('keeps legacy-only days as level 0 with request counts intact', () => {
      const cells = buildHeatmapCells(
        [
          daily('2026-08-19', {
            request_count: 2,
            input_tokens: null,
            output_tokens: null,
            cached_input_tokens: null,
            cache_write_input_tokens: null,
          }),
          daily('2026-08-20', { request_count: 5, input_tokens: 4000, output_tokens: 1000 }),
        ],
        now,
      );
      const byDate = new Map(cells.map((cell) => [cell.date, cell]));
      expect(byDate.get('2026-08-19')).toMatchObject({ tokens: null, requests: 2, level: 0 });
      expect(byDate.get('2026-08-20')!.level).toBeGreaterThan(0);
    });
  });

  describe('buildDailyChart', () => {
    it('scales bars and call line from window maxima, ignoring null tokens', () => {
      const chart = buildDailyChart([
        {
          date: '2026-08-20',
          cost_usd: 1,
          request_count: 30,
          input_tokens: 900,
          output_tokens: 300,
          cached_input_tokens: 0,
          cache_write_input_tokens: 0,
        },
        {
          date: '2026-08-21',
          cost_usd: 1,
          request_count: 10,
          input_tokens: null,
          output_tokens: null,
          cached_input_tokens: null,
          cache_write_input_tokens: null,
        },
        {
          date: '2026-08-22',
          cost_usd: 1,
          request_count: 20,
          input_tokens: 1800,
          output_tokens: 600,
          cached_input_tokens: 0,
          cache_write_input_tokens: 0,
        },
      ]);
      expect(chart.maxTokens).toBe(1800);
      expect(chart.maxRequests).toBe(30);
      expect(chart.points).toHaveLength(3);
      expect(chart.points[1]).toMatchObject({
        inputTokens: null,
        outputTokens: null,
        requests: 10,
      });
    });
  });

  describe('breakdown helpers', () => {
    it('returns total tokens only when both directions are known', () => {
      expect(breakdownTotalTokens({ input_tokens: 700, output_tokens: 300 })).toBe(1000);
      expect(breakdownTotalTokens({ input_tokens: null, output_tokens: 300 })).toBeNull();
      expect(breakdownTotalTokens({ input_tokens: 700, output_tokens: null })).toBeNull();
    });

    it('annotates cost notes strictly from returned cost-kind counts', () => {
      expect(breakdownCostNote({ cost_kinds: kinds({ unknown: 17 }), request_count: 17 })).toBe(
        '未计价',
      );
      expect(breakdownCostNote({ cost_kinds: kinds({ legacy: 9 }), request_count: 9 })).toBe(
        '历史金额',
      );
      expect(
        breakdownCostNote({
          cost_kinds: kinds({ estimated: 45, subscription: 0 }),
          request_count: 45,
        }),
      ).toBe('等效折算');
      expect(
        breakdownCostNote({
          cost_kinds: kinds({ actual: 100, estimated: 26 }),
          request_count: 126,
        }),
      ).toBeNull();
      expect(breakdownCostNote({ cost_kinds: kinds(), request_count: 0 })).toBeNull();
    });
  });

  describe('mergeProviderBreakdown', () => {
    const rows: UsageBreakdownRow[] = [
      {
        key: 'api',
        engine: 'claude-code',
        request_count: 98,
        input_tokens: 5_000_000,
        output_tokens: 1_180_000,
        cached_input_tokens: 2_960_000,
        cache_write_input_tokens: 210_000,
        cost_usd: 14.72,
        share: 0.34,
        cost_kinds: kinds({ actual: 98 }),
      },
      {
        key: '',
        engine: 'codex',
        request_count: 25,
        input_tokens: 1_430_000,
        output_tokens: 260_000,
        cached_input_tokens: null,
        cache_write_input_tokens: null,
        cost_usd: 2.78,
        share: 0.07,
        cost_kinds: kinds({ actual: 20, unknown: 5 }),
      },
    ];

    it('resolves provider names and keeps every frozen aggregate field', () => {
      const merged = mergeProviderBreakdown(providers, rows);
      expect(merged).toHaveLength(2);
      expect(merged[0]).toMatchObject({
        key: 'api',
        name: 'API Provider',
        kind: 'api',
        cost_usd: 14.72,
      });
      expect(merged[1]).toMatchObject({
        key: '',
        name: '未标注（旧会话）',
        kind: 'unknown',
        ready: false,
      });
      expect(merged[0].cached_input_tokens).toBe(2_960_000);
    });

    it('does not call an API-key CLI login a ready subscription', () => {
      const subscriptionRow = { ...rows[0], key: 'subscription' };
      const withApikeyLogin = mergeProviderBreakdown(providers, [subscriptionRow], {
        subscription: {
          state: 'ok',
          authMethod: 'apikey',
          detail: 'API key',
        } satisfies CliLoginState,
      });
      expect(withApikeyLogin[0].ready).toBe(false);
      const withSubscriptionLogin = mergeProviderBreakdown(providers, [subscriptionRow], {
        subscription: {
          state: 'ok',
          authMethod: 'subscription',
          detail: 'ok',
        } satisfies CliLoginState,
      });
      expect(withSubscriptionLogin[0].ready).toBe(true);
    });

    it('keeps zero-cost runtime-ready providers out of the usage table (rows come from backend)', () => {
      expect(mergeProviderBreakdown([], [])).toEqual([]);
      expect(TOP_TASKS_LIMIT).toBe(5);
    });
  });
});
