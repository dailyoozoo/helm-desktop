import { describe, expect, it } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import type { CliLoginState, ProviderConfig } from '../providers/api';
import type { DailyUsage, TopSession, UsageBreakdownRow, UsageStats } from './api';
import { TOP_TASKS_LIMIT } from './metrics';
import {
  UsageErrorView,
  UsageLoadedView,
  UsageLoadingView,
  type UsageLoadedProps,
} from './UsagePage';

const noop = () => undefined;

function daily(date: string, over: Partial<DailyUsage> = {}): DailyUsage {
  return {
    date,
    cost_usd: 2,
    request_count: 12,
    input_tokens: 800_000,
    output_tokens: 200_000,
    cached_input_tokens: 400_000,
    cache_write_input_tokens: 50_000,
    ...over,
  };
}

const stats: UsageStats = {
  total_cost: 42.81,
  total_tokens: 18_600_000,
  input_tokens: 14_200_000,
  output_tokens: 4_400_000,
  cached_input_tokens: 8_920_000,
  cache_write_input_tokens: 900_000,
  request_count: 286,
  session_count: 24,
  actual_cost: 40.03,
  estimated_cost: 2.78,
  subscription_count: 0,
  unknown_count: 0,
  legacy_cost: 0,
  legacy_count: 0,
  previous_total_cost: 30,
  previous_total_tokens: 15_000_000,
  previous_request_count: 200,
  previous_session_count: 18,
};

function breakdownRow(over: Partial<UsageBreakdownRow> = {}): UsageBreakdownRow {
  return {
    key: 'claude-sonnet-4',
    engine: 'claude-code',
    request_count: 126,
    input_tokens: 6_800_000,
    output_tokens: 1_620_000,
    cached_input_tokens: 4_650_000,
    cache_write_input_tokens: 480_000,
    cost_usd: 20.18,
    share: 0.47,
    cost_kinds: { actual: 120, estimated: 6, subscription: 0, unknown: 0, legacy: 0 },
    ...over,
  };
}

const providers: ProviderConfig[] = [
  {
    id: 'anthropic-sub',
    name: 'Anthropic 订阅',
    kind: 'subscription',
    baseUrl: '',
    keyRef: null,
    ready: true,
    lastTest: null,
    protocol: 'anthropic',
    authMethod: 'oauth',
  },
];

const logins: Record<string, CliLoginState | null> = {
  'anthropic-sub': { state: 'ok', authMethod: 'subscription', detail: 'ok' },
};

function topSession(i: number): TopSession {
  return {
    id: 's' + i,
    title: '任务 ' + i,
    model: 'claude-sonnet-4',
    engine: 'claude-code',
    cost_usd: 3 + i * 0.5,
    total_tokens: 1_000_000 + i * 100_000,
  };
}

function loadedProps(over: Partial<UsageLoadedProps> = {}): UsageLoadedProps {
  return {
    days: 30,
    loading: false,
    loadError: null,
    updatedAt: new Date(2026, 7, 22, 14, 32).getTime(),
    stats,
    dailyUsage: [daily('2026-08-21'), daily('2026-08-22', { request_count: 30 })],
    breakdown: { model: [breakdownRow()], engine: [], provider: [] },
    topSessions: Array.from({ length: TOP_TASKS_LIMIT }, (_, i) => topSession(i + 1)),
    providers,
    logins,
    activeDim: 'model',
    onDaysChange: noop,
    onDimensionChange: noop,
    onRetry: noop,
    ...over,
  };
}

describe('UsagePage 视图（S5）', () => {
  it('首屏加载态不编造数据，只显示加载占位', () => {
    const markup = renderToStaticMarkup(<UsageLoadingView />);
    expect(markup).toContain('正在加载用量');
    expect(markup).toContain('role="status"');
  });

  it('整页错误态提供失败信息与重试入口', () => {
    const markup = renderToStaticMarkup(
      <UsageErrorView message="加载用量数据失败：boom" onRetry={noop} />,
    );
    expect(markup).toContain('加载用量数据失败：boom');
    expect(markup).toContain('重试');
    expect(markup).toContain('role="alert"');
  });

  it('四个时间范围分段可切换，默认 30 天激活', () => {
    const markup = renderToStaticMarkup(<UsageLoadedView {...loadedProps()} />);
    for (const label of ['7 天', '30 天', '90 天', '365 天']) {
      expect(markup).toContain(`>${label}</button>`);
    }
    // 仅当前范围按钮处于按下态；激活 Tab 另有 aria-selected 表达
    expect(markup.match(/aria-pressed="true"/g)).toHaveLength(1);
    expect(markup.match(/aria-pressed="false"/g)).toHaveLength(3);
    expect(markup.match(/aria-selected="true"/g)).toHaveLength(1);
  });

  it('四 KPI 卡片展示真实聚合：Token、调用、缓存率与费用口径', () => {
    const markup = renderToStaticMarkup(<UsageLoadedView {...loadedProps()} />);
    for (const label of ['总 Token', '调用次数', '缓存命中率', '预估费用']) {
      expect(markup).toContain(label);
    }
    expect(markup).toContain('18.6M');
    expect(markup).toContain('输入 14.2M');
    expect(markup).toContain('输出 4.4M');
    expect(markup).toContain('>286<');
    expect(markup).toContain('62.8%'); // 8_920_000 / 14_200_000
    expect(markup).toContain('缓存读取 8.92M / 总输入 14.2M');
    expect(markup).toContain('$42.81');
    expect(markup).toContain('实际 $40.03');
    expect(markup).toContain('估算 $2.78');
  });

  it('每日用量图渲染输入/输出双柱与调用折线，空数据走空态', () => {
    const markup = renderToStaticMarkup(<UsageLoadedView {...loadedProps()} />);
    expect(markup).toContain('每日用量');
    expect(markup).toContain('过去 30 天');
    expect(markup).toContain('cm-bar__input');
    expect(markup).toContain('cm-bar__output');
    expect(markup).toContain('<svg class="cm-chart__line"');
    expect(markup).toContain('更新于');

    const empty = renderToStaticMarkup(<UsageLoadedView {...loadedProps({ dailyUsage: [] })} />);
    expect(empty).toContain('当前范围暂无用量记录');
    expect(empty).not.toContain('cm-bar__input');
  });

  it('用量构成提供模型/引擎/服务商三维 Tab 与相同口径表格', () => {
    const markup = renderToStaticMarkup(
      <UsageLoadedView
        {...loadedProps({ breakdown: { model: [breakdownRow()], engine: [], provider: [] } })}
      />,
    );
    expect(markup).toContain('role="tablist"');
    for (const label of ['模型', '引擎', '服务商']) {
      expect(markup).toContain(`>${label}</button>`);
    }
    expect(markup).toContain('claude-sonnet-4');
    expect(markup).toContain('Claude Code');
    expect(markup).toContain('缓存命中率');
  });

  it('构成行缺失 token/缓存证据时显示暂无并标注成本口径', () => {
    const markup = renderToStaticMarkup(
      <UsageLoadedView
        {...loadedProps({
          breakdown: {
            model: [
              breakdownRow({
                key: 'custom-reasoner-v2',
                request_count: 17,
                input_tokens: null,
                output_tokens: null,
                cached_input_tokens: null,
                cache_write_input_tokens: null,
                cost_kinds: { actual: 0, estimated: 0, subscription: 0, unknown: 17, legacy: 0 },
                cost_usd: 0,
              }),
            ],
            engine: [],
            provider: [],
          },
        })}
      />,
    );
    expect(markup.match(/暂无/g)?.length).toBeGreaterThanOrEqual(2);
    expect(markup).toContain('未计价');
  });

  it('服务商维度并入真实服务商配置与登录就绪状态', () => {
    const markup = renderToStaticMarkup(
      <UsageLoadedView
        {...loadedProps({
          activeDim: 'provider',
          breakdown: {
            model: [],
            engine: [],
            provider: [breakdownRow({ key: 'anthropic-sub' })],
          },
        })}
      />,
    );
    expect(markup).toContain('Anthropic 订阅');
    expect(markup).toContain('订阅');
    expect(markup).toContain('已就绪');
  });

  it('高用量任务固定渲染 limit=5 的真实会话磁贴并可打开', () => {
    const props = loadedProps();
    const markup = renderToStaticMarkup(<UsageLoadedView {...props} />);
    expect(props.topSessions).toHaveLength(5);
    expect(markup.match(/class="cm-usage-task"/g)).toHaveLength(5);
    expect(markup).toContain('花费最高的前 5 个任务');
    // 原型 settings.html#tasks 的等价入口：深链进设置页任务 Tab
    expect(markup).toContain('全部任务');
    expect(markup).toContain('cm-inline-link');
    for (let i = 1; i <= 5; i += 1) {
      expect(markup).toContain(`任务 ${i}`);
    }

    const empty = renderToStaticMarkup(<UsageLoadedView {...loadedProps({ topSessions: [] })} />);
    expect(empty).toContain('当前范围暂无任务花费');
  });

  it('热力图固定 365 天且不随时间范围切换', () => {
    for (const days of [7, 90, 365] as const) {
      const markup = renderToStaticMarkup(
        <UsageLoadedView {...loadedProps({ days, dailyUsage: [daily('2026-08-22')] })} />,
      );
      expect(markup.match(/data-level="/g)).toHaveLength(365);
      expect(markup).toContain('365 天 Token 活跃度');
      expect(markup).toContain('固定展示最近一年');
      // 图例五档与格子 data-level 0-4 一一对应（用户决议 2026-09）
      expect(markup.match(/usage-heat-swatch--\d/g)).toHaveLength(5);
    }
  });

  it('空数据库下所有分区给出可验证的空态', () => {
    const markup = renderToStaticMarkup(
      <UsageLoadedView
        {...loadedProps({
          dailyUsage: [],
          breakdown: { model: [], engine: [], provider: [] },
          topSessions: [],
        })}
      />,
    );
    expect(markup).toContain('当前范围暂无用量记录');
    expect(markup).toContain('当前范围暂无模型维度用量');
    expect(markup).toContain('当前范围暂无任务花费');
    expect(markup.match(/data-level="0"/g)).toHaveLength(365);
  });

  it('局部刷新失败保留旧数据并显示可重试的内联错误', () => {
    const markup = renderToStaticMarkup(
      <UsageLoadedView {...loadedProps({ loadError: '加载用量数据失败：boom' })} />,
    );
    expect(markup).toContain('usage-inline-error');
    expect(markup).toContain('加载用量数据失败：boom');
    expect(markup).toContain('重试');
    // 旧数据仍在页面上
    expect(markup).toContain('18.6M');
  });
});
