import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import type { KeyboardEvent as ReactKeyboardEvent, ReactNode } from 'react';
import { Icon } from '../shell/icons';
import { EngineBrand } from '../shell/EngineBrand';
import { EmptyState } from '../components/EmptyState';
import {
  USAGE_RANGE_DAYS,
  getDailyUsage,
  getTopSessions,
  getUsageBreakdown,
  getUsageStats,
  type DailyUsage,
  type TopSession,
  type UsageBreakdownDimension,
  type UsageBreakdownRow,
  type UsageRangeDays,
  type UsageStats,
} from './api';
import {
  detectCliLogin,
  getProviderConfig,
  type CliLoginState,
  type ProviderConfig,
} from '../providers/api';
import {
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
  type DailyChartModel,
  type HeatmapCell,
  type ProviderBreakdownRow,
} from './metrics';
import './usage.css';

const BREAKDOWN_DIMENSIONS: { id: UsageBreakdownDimension; label: string }[] = [
  { id: 'model', label: '模型' },
  { id: 'engine', label: '引擎' },
  { id: 'provider', label: '服务商' },
];

const ENGINE_LABELS: Record<string, string> = {
  'claude-code': 'Claude Code',
  codex: 'Codex',
};

function engineLabel(engine: string): string {
  return ENGINE_LABELS[engine] ?? engine;
}

function providerKindLabel(kind: ProviderBreakdownRow['kind']): string {
  if (kind === 'subscription') return '订阅';
  if (kind === 'api') return 'API';
  if (kind === 'local') return '本地';
  return '未标注';
}

type BreakdownByDimension = Record<UsageBreakdownDimension, UsageBreakdownRow[]>;

/** 调用折线平滑画法（Catmull-Rom → 三次贝塞尔），与原型 cm-chart__line 的曲线观感一致。 */
function smoothLinePath(points: { x: number; y: number }[]): string {
  if (points.length === 0) return '';
  if (points.length === 1) return `M${points[0].x.toFixed(2)} ${points[0].y.toFixed(2)}`;
  let d = `M${points[0].x.toFixed(2)} ${points[0].y.toFixed(2)}`;
  for (let i = 0; i < points.length - 1; i += 1) {
    const p0 = points[i - 1] ?? points[i];
    const p1 = points[i];
    const p2 = points[i + 1];
    const p3 = points[i + 2] ?? p2;
    const c1x = p1.x + (p2.x - p0.x) / 6;
    const c1y = p1.y + (p2.y - p0.y) / 6;
    const c2x = p2.x - (p3.x - p1.x) / 6;
    const c2y = p2.y - (p3.y - p1.y) / 6;
    d += ` C${c1x.toFixed(2)} ${c1y.toFixed(2)} ${c2x.toFixed(2)} ${c2y.toFixed(2)} ${p2.x.toFixed(2)} ${p2.y.toFixed(2)}`;
  }
  return d;
}

export function UsagePage() {
  const [days, setDays] = useState<UsageRangeDays>(30);
  const [activeDim, setActiveDim] = useState<UsageBreakdownDimension>('model');
  const [stats, setStats] = useState<UsageStats | null>(null);
  const [dailyUsage, setDailyUsage] = useState<DailyUsage[]>([]);
  const [breakdown, setBreakdown] = useState<BreakdownByDimension>({
    model: [],
    engine: [],
    provider: [],
  });
  const [topSessions, setTopSessions] = useState<TopSession[]>([]);
  const [providers, setProviders] = useState<ProviderConfig[]>([]);
  const [logins, setLogins] = useState<Record<string, CliLoginState | null>>({});
  const [loading, setLoading] = useState(true);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [updatedAt, setUpdatedAt] = useState<number | null>(null);
  const loadGate = useRef(createRequestGate());

  const loadData = useCallback(async (range: UsageRangeDays) => {
    const generation = loadGate.current.begin();
    setLoading(true);
    setLoadError(null);
    try {
      const [statsData, dailyData, modelRows, engineRows, providerRows, topData, providerCfg] =
        await Promise.all([
          getUsageStats(range),
          getDailyUsage(range),
          getUsageBreakdown(range, 'model'),
          getUsageBreakdown(range, 'engine'),
          getUsageBreakdown(range, 'provider'),
          // S5 验收口径：高用量任务固定 limit=5
          getTopSessions(range, TOP_TASKS_LIMIT),
          getProviderConfig()
            .then((config) => config.providers)
            .catch(() => [] as ProviderConfig[]),
        ]);
      // 订阅服务商登录状态沿用现有真实 API；单个探测失败不阻塞整页
      const loginEntries = await Promise.all(
        providerCfg
          .filter((provider) => provider.kind === 'subscription')
          .map(async (provider) => {
            const engine = provider.protocol === 'anthropic' ? 'claude-code' : 'codex';
            try {
              return [provider.id, await detectCliLogin(engine)] as const;
            } catch {
              return [
                provider.id,
                { state: 'unknown', authMethod: 'unknown', detail: 'CLI 登录状态检测失败' },
              ] as const;
            }
          }),
      );
      if (!loadGate.current.isCurrent(generation)) return;
      setStats(statsData);
      setDailyUsage(dailyData);
      setBreakdown({ model: modelRows, engine: engineRows, provider: providerRows });
      setTopSessions(topData);
      setProviders(providerCfg);
      setLogins(Object.fromEntries(loginEntries));
      setUpdatedAt(Date.now());
    } catch (err) {
      if (!loadGate.current.isCurrent(generation)) return;
      console.error('加载用量数据失败:', err);
      setLoadError('加载用量数据失败：' + (err instanceof Error ? err.message : String(err)));
    } finally {
      if (loadGate.current.isCurrent(generation)) setLoading(false);
    }
  }, []);

  useEffect(() => {
    void loadData(days);
  }, [loadData, days]);

  // 首次加载才整页占位；已有数据时切换范围走局部刷新，不清空已渲染内容
  if (loading && !stats) {
    return <UsageLoadingView />;
  }
  if (loadError && !stats) {
    return <UsageErrorView message={loadError} onRetry={() => void loadData(days)} />;
  }

  return (
    <UsageLoadedView
      days={days}
      loading={loading}
      loadError={loadError}
      updatedAt={updatedAt}
      stats={stats!}
      dailyUsage={dailyUsage}
      breakdown={breakdown}
      topSessions={topSessions}
      providers={providers}
      logins={logins}
      activeDim={activeDim}
      onDaysChange={setDays}
      onDimensionChange={setActiveDim}
      onRetry={() => void loadData(days)}
    />
  );
}

export function UsageLoadingView() {
  return (
    <div className="page">
      <div className="cm-pagebody usage-page">
        <div className="usage-loading" role="status">
          正在加载用量…
        </div>
      </div>
    </div>
  );
}

export function UsageErrorView({ message, onRetry }: { message: string; onRetry: () => void }) {
  return (
    <div className="page">
      <div className="cm-pagebody usage-page">
        <div className="usage-inline-error" role="alert">
          <Icon name="alert" />
          <span>{message}</span>
          <button className="btn btn--subtle btn--sm" onClick={onRetry} type="button">
            重试
          </button>
        </div>
      </div>
    </div>
  );
}

export interface UsageLoadedProps {
  days: UsageRangeDays;
  loading: boolean;
  loadError: string | null;
  updatedAt: number | null;
  stats: UsageStats;
  dailyUsage: DailyUsage[];
  breakdown: BreakdownByDimension;
  topSessions: TopSession[];
  providers: ProviderConfig[];
  logins: Record<string, CliLoginState | null>;
  activeDim: UsageBreakdownDimension;
  onDaysChange: (days: UsageRangeDays) => void;
  onDimensionChange: (dimension: UsageBreakdownDimension) => void;
  onRetry: () => void;
}

/* 页面骨架消费共享组件库（cm-pagebody/cm-pagehead/cm-section/cm-kpi…），
   与 prototype/usage.html 同构；.usage-page 仅作为本页覆盖层作用域钩子。 */
export function UsageLoadedView(props: UsageLoadedProps) {
  const {
    days,
    loading,
    loadError,
    updatedAt,
    stats,
    dailyUsage,
    breakdown,
    topSessions,
    providers,
    logins,
    activeDim,
    onDaysChange,
    onDimensionChange,
    onRetry,
  } = props;

  return (
    <div className="page scroll">
      <div
        className={'cm-pagebody usage-page' + (loading ? ' usage-refreshing' : '')}
        aria-busy={loading}
      >
        <header className="cm-pagehead">
          <div>
            <h1 className="cm-pagehead__title">用量</h1>
            <p className="cm-pagehead__desc">
              按真实 Usage 汇总 Token、调用、缓存与 Helm 预估费用。
            </p>
          </div>
          <div className="cm-pagehead__actions">
            <div className="cm-segment" role="group" aria-label="统计时间范围">
              {USAGE_RANGE_DAYS.map((range) => (
                <button
                  key={range}
                  type="button"
                  className={days === range ? 'is-active' : ''}
                  aria-pressed={days === range}
                  disabled={loading}
                  onClick={() => onDaysChange(range)}
                >
                  {range} 天
                </button>
              ))}
            </div>
          </div>
        </header>

        {loadError ? (
          <div className="usage-inline-error" role="alert">
            <Icon name="alert" />
            <span>{loadError}</span>
            <button className="btn btn--subtle btn--sm" onClick={onRetry} type="button">
              重试
            </button>
          </div>
        ) : null}

        <UsageKpis stats={stats} />

        <section className="cm-section" data-od-id="daily-usage" aria-label="每日用量">
          <div className="cm-section__head">
            <div>
              <h2>
                <Icon name="chartcolumn" />
                每日用量
              </h2>
              <p>输入与输出 Token 使用左侧刻度，调用次数使用右侧刻度。</p>
            </div>
            {updatedAt ? (
              <span className="cm-activity">
                更新于{' '}
                {new Date(updatedAt).toLocaleTimeString('zh-CN', {
                  hour: '2-digit',
                  minute: '2-digit',
                  hour12: false,
                })}
              </span>
            ) : null}
          </div>
          <div className="cm-chart" aria-label={`过去 ${days} 天 Token 与调用次数图表`}>
            <div className="cm-chart__head">
              <div className="cm-chart__title">过去 {days} 天</div>
              <div className="cm-chart__legend">
                <span className="cm-legend cm-legend--input">输入</span>
                <span className="cm-legend cm-legend--output">输出</span>
                <span className="cm-legend cm-legend--calls">调用次数</span>
              </div>
            </div>
            {dailyUsage.length === 0 ? (
              <EmptyState
                icon="chart"
                title="当前范围暂无用量记录"
                hint="发起一次真实会话后，这里会出现每日输入/输出与调用趋势。"
                action={{
                  label: '去工作区发起会话',
                  onClick: () =>
                    window.dispatchEvent(
                      new CustomEvent('helm:navigate', { detail: { page: 'workspace' } }),
                    ),
                }}
              />
            ) : (
              <DailyBars chart={buildDailyChart(dailyUsage)} />
            )}
          </div>
        </section>

        <BreakdownSection
          rows={breakdown}
          activeDim={activeDim}
          onDimensionChange={onDimensionChange}
          providers={providers}
          logins={logins}
        />

        <TopTasksSection sessions={topSessions} />

        <HeatmapSection dailyUsage={dailyUsage} />
      </div>
    </div>
  );
}

/** 成本口径摘要：只罗列后端返回的非零计数，不做换算。 */
function describeCostKinds(s: UsageStats): string {
  const parts: string[] = [`实际 $${s.actual_cost.toFixed(2)}`];
  if (s.estimated_cost > 0) parts.push(`估算 $${s.estimated_cost.toFixed(2)}`);
  if (s.subscription_count > 0) parts.push(`订阅内 ${s.subscription_count} 次`);
  if (s.unknown_count > 0) parts.push(`${s.unknown_count} 次无价格数据`);
  if (s.legacy_count > 0)
    parts.push(`历史未分类 $${s.legacy_cost.toFixed(2)}（${s.legacy_count} 次）`);
  return parts.join(' · ');
}

function UsageKpis({ stats }: { stats: UsageStats }) {
  const rate = cacheRate(stats.cached_input_tokens, stats.input_tokens);
  return (
    <div className="cm-kpi-grid">
      <article className="cm-panel cm-kpi">
        <span className="cm-kpi__badge">
          <Icon name="coins" />
        </span>
        <div className="cm-kpi__label">总 Token</div>
        <div className="cm-kpi__value">{formatCompactTokens(stats.total_tokens)}</div>
        <div className="cm-kpi__split">
          <span>输入 {formatCompactTokens(stats.input_tokens)}</span>
          <span>输出 {formatCompactTokens(stats.output_tokens)}</span>
        </div>
        <div className="cm-kpi__meta">
          {comparisonText(stats.total_tokens, stats.previous_total_tokens, '较前一期')}
        </div>
      </article>

      <article className="cm-panel cm-kpi">
        <span className="cm-kpi__badge">
          <Icon name="gauge" />
        </span>
        <div className="cm-kpi__label">调用次数</div>
        <div className="cm-kpi__value">{stats.request_count}</div>
        <div className="cm-kpi__split">
          <span>跨 {stats.session_count} 个会话</span>
        </div>
        <div className="cm-kpi__meta">
          {comparisonText(stats.request_count, stats.previous_request_count, '较前一期')}
        </div>
      </article>

      <article className="cm-panel cm-kpi">
        <span className="cm-kpi__badge">
          <Icon name="database" />
        </span>
        <div className="cm-kpi__label">缓存命中率</div>
        <div className="cm-kpi__value">
          {rate === null ? '暂无' : `${(rate * 100).toFixed(1)}%`}
        </div>
        <div className="cm-kpi__meta">
          {rate === null
            ? '暂无 Token 证据，无法计算命中率'
            : `缓存读取 ${formatCompactTokens(stats.cached_input_tokens)} / 总输入 ${formatCompactTokens(stats.input_tokens)}`}
        </div>
      </article>

      <article className="cm-panel cm-kpi">
        <span className="cm-kpi__badge">
          <Icon name="dollar" />
        </span>
        <div className="cm-kpi__label">预估费用</div>
        <div className="cm-kpi__value">${stats.total_cost.toFixed(2)}</div>
        <div className="cm-kpi__meta">{describeCostKinds(stats)}</div>
      </article>
    </div>
  );
}

function DailyBars({ chart }: { chart: DailyChartModel }) {
  const { points, maxTokens, maxRequests } = chart;
  const labelStep = Math.max(1, Math.ceil(points.length / 8));
  // 实测 .cm-bars 宽度后按 flex 几何精确对位柱心；SSR/首帧回退到均分估算
  const barsRef = useRef<HTMLDivElement | null>(null);
  const [measuredWidth, setMeasuredWidth] = useState<number | null>(null);
  useEffect(() => {
    const el = barsRef.current;
    if (!el) return;
    const update = () => setMeasuredWidth(el.clientWidth);
    update();
    const observer = new ResizeObserver(update);
    observer.observe(el);
    return () => observer.disconnect();
  }, []);
  const width = measuredWidth ?? Math.max(points.length * 12, 60);
  // 折线画布 130 单位高（与 cm-chart__line 元素 1:1，无拉伸畸变）；126 为零点基线。
  const linePoints = (() => {
    const n = points.length;
    if (n === 0 || width <= 0) return [];
    if (measuredWidth == null) {
      return points.map((_, index) => ({
        x: ((index + 0.5) / n) * width,
        y:
          maxRequests > 0 && points[index].requests > 0
            ? 126 - (points[index].requests / maxRequests) * 104
            : 126,
      }));
    }
    // 与 .cm-bars 的 space-between + gap(12px) + 柱宽上限(38px) 严格一致的柱心坐标
    const gap = 12;
    const barMax = 38;
    const slot = (width - (n - 1) * gap) / n;
    const center = (index: number) =>
      slot <= barMax
        ? (index + 0.5) * slot + index * gap
        : index * (barMax + (width - n * barMax) / (n - 1)) + barMax / 2;
    return points.map((point, index) => ({
      x: center(index),
      y: maxRequests > 0 && point.requests > 0 ? 126 - (point.requests / maxRequests) * 104 : 126,
    }));
  })();
  const path = smoothLinePath(linePoints);

  return (
    <>
      <div className="cm-bars" ref={barsRef}>
        {points.map((point, index) => {
          const inputHeight =
            maxTokens > 0 && point.inputTokens !== null && point.inputTokens > 0
              ? Math.max(1.5, (point.inputTokens / maxTokens) * 100)
              : 0;
          const outputHeight =
            maxTokens > 0 && point.outputTokens !== null && point.outputTokens > 0
              ? Math.max(1.5, (point.outputTokens / maxTokens) * 100)
              : 0;
          const showLabel = (points.length - 1 - index) % labelStep === 0;
          const tooltip = `${formatMonthDay(point.date)} · 输入 ${
            point.inputTokens === null ? '暂无' : formatCompactTokens(point.inputTokens)
          } · 输出 ${
            point.outputTokens === null ? '暂无' : formatCompactTokens(point.outputTokens)
          } · ${point.requests} 次`;
          return (
            <div key={point.date} className="cm-bar" title={tooltip}>
              <i className="cm-bar__input" style={{ height: `${inputHeight}%` }} />
              <i className="cm-bar__output" style={{ height: `${outputHeight}%` }} />
              {showLabel ? (
                <span className="cm-bar__label">{formatMonthDay(point.date)}</span>
              ) : null}
            </div>
          );
        })}
      </div>
      {maxRequests > 0 && linePoints.length > 0 ? (
        <svg
          className="cm-chart__line"
          viewBox={`0 0 ${width} 130`}
          preserveAspectRatio="none"
          aria-hidden="true"
        >
          <path d={path} vectorEffect="non-scaling-stroke" />
          {linePoints.map((point, index) => (
            <circle key={points[index].date} cx={point.x} cy={point.y} r="3" />
          ))}
        </svg>
      ) : null}
    </>
  );
}

function BreakdownSection({
  rows,
  activeDim,
  onDimensionChange,
  providers,
  logins,
}: {
  rows: BreakdownByDimension;
  activeDim: UsageBreakdownDimension;
  onDimensionChange: (dimension: UsageBreakdownDimension) => void;
  providers: ProviderConfig[];
  logins: Record<string, CliLoginState | null>;
}) {
  const activeMeta = BREAKDOWN_DIMENSIONS.find((d) => d.id === activeDim)!;
  const tableRows =
    activeDim === 'provider'
      ? mergeProviderBreakdown(providers, rows.provider, logins)
      : rows[activeDim];

  function handleTabKeyDown(event: ReactKeyboardEvent<HTMLDivElement>) {
    const count = BREAKDOWN_DIMENSIONS.length;
    const index = BREAKDOWN_DIMENSIONS.findIndex((d) => d.id === activeDim);
    let next: number | null = null;
    if (event.key === 'ArrowRight') next = (index + 1) % count;
    else if (event.key === 'ArrowLeft') next = (index + count - 1) % count;
    else if (event.key === 'Home') next = 0;
    else if (event.key === 'End') next = count - 1;
    if (next === null) return;
    event.preventDefault();
    onDimensionChange(BREAKDOWN_DIMENSIONS[next].id);
    document.getElementById(`usage-tab-${BREAKDOWN_DIMENSIONS[next].id}`)?.focus();
  }

  return (
    <section className="cm-section" data-od-id="usage-breakdown" aria-label="用量构成">
      <div className="cm-section__head">
        <div>
          <h2>
            <Icon name="layers" />
            用量构成
          </h2>
          <p>切换模型、引擎或服务商查看相同口径的汇总。</p>
        </div>
      </div>
      <div
        className="cm-tabs"
        role="tablist"
        aria-label="用量构成维度"
        onKeyDown={handleTabKeyDown}
      >
        {BREAKDOWN_DIMENSIONS.map((dimension) => (
          <button
            key={dimension.id}
            id={`usage-tab-${dimension.id}`}
            type="button"
            role="tab"
            aria-selected={activeDim === dimension.id}
            aria-controls={`usage-panel-${dimension.id}`}
            tabIndex={activeDim === dimension.id ? 0 : -1}
            className={activeDim === dimension.id ? 'is-active' : ''}
            onClick={() => onDimensionChange(dimension.id)}
          >
            {dimension.label}
          </button>
        ))}
      </div>
      {/* 实现只渲染当前维度单面板；原型为三面板预置隐藏 DOM，行为等价（aria 由 tab/tabpanel 表达） */}
      <div
        className="cm-tabpanel is-active cm-tab-view cm-panel cm-table-wrap"
        role="tabpanel"
        id={`usage-panel-${activeDim}`}
        aria-labelledby={`usage-tab-${activeDim}`}
      >
        {tableRows.length === 0 ? (
          <EmptyState
            icon="layers"
            title={`当前范围暂无${activeMeta.label}维度用量`}
            hint="发起会话并产生真实用量后，这里会展示相同口径的聚合数据。"
          />
        ) : (
          <table className="cm-table">
            <thead>
              <tr>
                <th>{activeMeta.label}</th>
                <th className="num">Token</th>
                <th className="num">调用次数</th>
                <th className="num">缓存命中率</th>
                <th className="num">预估费用</th>
              </tr>
            </thead>
            <tbody>
              {tableRows.map((row) => (
                <BreakdownRowItem key={`${row.engine}:${row.key}`} row={row} dim={activeDim} />
              ))}
            </tbody>
          </table>
        )}
      </div>
    </section>
  );
}

function BreakdownRowItem({ row, dim }: { row: UsageBreakdownRow; dim: UsageBreakdownDimension }) {
  const total = breakdownTotalTokens(row);
  const rate = cacheRate(row.cached_input_tokens, row.input_tokens);
  const note = breakdownCostNote(row);

  let nameCell: ReactNode;
  if (dim === 'model') {
    // 原型结构：品牌块 + 主行/副行两行文本；契约只有模型 ID 与引擎，副行展示引擎标签
    nameCell = (
      <span className="cm-table__primary">
        <span className="cm-brand cm-brand--light">
          <EngineBrand engine={row.engine} size={20} />
        </span>
        <span>
          <b>{row.key}</b>
          <small className="mono">{engineLabel(row.engine)}</small>
        </span>
      </span>
    );
  } else if (dim === 'engine') {
    nameCell = <b>{engineLabel(row.key)}</b>;
  } else {
    const providerRow = row as ProviderBreakdownRow;
    nameCell = (
      <span>
        <b>{providerRow.name}</b>
        <small className="cm-table__sub">
          {providerKindLabel(providerRow.kind)}
          {providerRow.ready ? ' · 已就绪' : ''}
        </small>
      </span>
    );
  }

  return (
    <tr>
      <td>{nameCell}</td>
      <td className={'num' + (total === null ? ' is-empty' : '')}>
        {total === null ? '暂无' : formatCompactTokens(total)}
      </td>
      <td className="num">{row.request_count}</td>
      <td className={'num' + (rate === null ? ' is-empty' : '')}>
        {rate === null ? '暂无' : `${(rate * 100).toFixed(1)}%`}
      </td>
      <td className="num">
        ${row.cost_usd.toFixed(2)}
        {note ? <small className="cm-table__sub">{note}</small> : null}
      </td>
    </tr>
  );
}

function TopTasksSection({ sessions }: { sessions: TopSession[] }) {
  return (
    <section className="cm-section" data-od-id="high-usage-tasks" aria-label="高用量任务">
      <div className="cm-section__head">
        <div>
          <h2>
            <Icon name="listtodo" />
            高用量任务
          </h2>
          {/* 后端 get_top_sessions 契约按窗口内花费降序返回前 limit 条；文案保持真实排序 */}
          <p>当前时间范围内花费最高的前 5 个任务。</p>
        </div>
        {/* 原型 settings.html#tasks 的实现等价物：设置页挂载时读取 helm:settings-tab（一次性） */}
        <a
          className="cm-inline-link"
          href="#tasks"
          onClick={(event) => {
            event.preventDefault();
            sessionStorage.setItem('helm:settings-tab', 'tasks');
            window.dispatchEvent(
              new CustomEvent('helm:navigate', { detail: { page: 'settings' } }),
            );
          }}
        >
          全部任务
        </a>
      </div>
      {sessions.length === 0 ? (
        <EmptyState
          icon="inbox"
          title="当前范围暂无任务花费"
          hint="发起会话后，花费最高的任务会出现在这里。"
        />
      ) : (
        <div className="cm-usage-task-list">
          {sessions.map((session) => (
            <button
              key={session.id}
              type="button"
              className="cm-usage-task"
              onClick={() =>
                window.dispatchEvent(
                  new CustomEvent('helm:open-session', { detail: { sessionId: session.id } }),
                )
              }
            >
              <span className="cm-usage-task__brand">
                <EngineBrand engine={session.engine} size={20} />
              </span>
              <span className="cm-usage-task__main">
                <b>{session.title}</b>
                <small>
                  {session.model} · {engineLabel(session.engine)}
                </small>
              </span>
              <span className="cm-usage-task__stat">
                <b>{formatCompactTokens(session.total_tokens)}</b>
                <small>${session.cost_usd.toFixed(2)}</small>
              </span>
            </button>
          ))}
        </div>
      )}
    </section>
  );
}

function heatmapCellTitle(cell: HeatmapCell): string {
  const label = formatMonthDay(cell.date);
  if (cell.requests === 0) return `${label} · 无用量记录`;
  if (cell.tokens === null) return `${label} · ${cell.requests} 次调用 · 无 Token 记录`;
  return `${label} · ${formatCompactTokens(cell.tokens)} Token`;
}

function HeatmapSection({ dailyUsage }: { dailyUsage: DailyUsage[] }) {
  // 窗口固定 365 天，不随上方时间范围分段切换（S5 验收口径）
  const cells = useMemo(() => buildHeatmapCells(dailyUsage, new Date()), [dailyUsage]);
  return (
    <section className="cm-section" data-od-id="usage-heatmap" aria-label="365 天 Token 活跃度">
      <div className="cm-section__head">
        <div>
          <h2>
            <Icon name="clock" />
            365 天 Token 活跃度
          </h2>
          <p>固定展示最近一年，不随上方时间范围切换。</p>
        </div>
        {/* 图例五档（0-4 分位档），与热力图格子 data-level 一一对应；用户决议 2026-09 */}
        <span className="usage-heat-legend">
          少
          {[0, 1, 2, 3, 4].map((level) => (
            <i key={level} className={`usage-heat-swatch usage-heat-swatch--${level}`} />
          ))}
          多
        </span>
      </div>
      <div className="cm-panel cm-panel--pad">
        <div className="usage-heatmap-scroll">
          <div className="cm-heatmap" role="img" aria-label="365 天 Token 活跃度热力图">
            {cells.map((cell) => (
              <i key={cell.date} data-level={cell.level} title={heatmapCellTitle(cell)} />
            ))}
          </div>
        </div>
      </div>
    </section>
  );
}
