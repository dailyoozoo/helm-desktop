import { useCallback, useEffect, useRef, useState } from 'react';
import { Icon } from '../shell/icons';
import { EmptyState } from '../components/EmptyState';
import { useDialogBehavior } from '../components/useDialogBehavior';
import {
  getBudget,
  getDailyUsage,
  getTopSessions,
  getUsageByModel,
  getUsageByProvider,
  getUsageStats,
  setBudget,
  type Budget,
  type DailyUsage,
  type ModelUsage,
  type TopSession,
  type UsageStats,
} from './api';
import { detectCliLogin, getProviderConfig } from '../providers/api';
import { buildUsageCsv } from './exportCsv';
import {
  comparisonText,
  createRequestGate,
  mergeProviderUsage,
  projectedMonthEndCost,
  type ProviderUsageRow,
} from './metrics';
import './usage.css';

type Period = '7d' | '30d';

export function UsagePage() {
  const [period, setPeriod] = useState<Period>('7d');
  const [stats, setStats] = useState<UsageStats | null>(null);
  const [modelUsage, setModelUsage] = useState<ModelUsage[]>([]);
  const [providerUsage, setProviderUsage] = useState<ProviderUsageRow[]>([]);
  const [dailyUsage, setDailyUsage] = useState<DailyUsage[]>([]);
  const [topSessions, setTopSessions] = useState<TopSession[]>([]);
  const [budget, setBudget] = useState<Budget | null>(null);
  const [loading, setLoading] = useState(true);
  const [loadError, setLoadError] = useState<string | null>(null);
  const loadGate = useRef(createRequestGate());

  const days = period === '7d' ? 7 : 30;

  const loadData = useCallback(async () => {
    const generation = loadGate.current.begin();
    setLoading(true);
    setLoadError(null);
    try {
      const [statsData, modelData, providerData, dailyData, topData, budgetData, providers] =
        await Promise.all([
          getUsageStats(days),
          getUsageByModel(days),
          getUsageByProvider(days),
          getDailyUsage(days),
          getTopSessions(days, 3),
          getBudget(),
          getProviderConfig()
            .then((config) => config.providers)
            .catch(() => []),
        ]);
      const loginEntries = await Promise.all(
        providers
          .filter((provider) => provider.kind === 'subscription')
          .map(async (provider) => {
            const engine = provider.protocol === 'anthropic' ? 'claude-code' : 'codex';
            try {
              return [provider.id, await detectCliLogin(engine)] as const;
            } catch {
              return [
                provider.id,
                {
                  state: 'unknown',
                  authMethod: 'unknown',
                  detail: 'CLI 登录状态检测失败',
                },
              ] as const;
            }
          }),
      );
      if (!loadGate.current.isCurrent(generation)) return;
      setStats(statsData);
      setModelUsage(modelData);
      setProviderUsage(
        mergeProviderUsage(providers, providerData, Object.fromEntries(loginEntries)),
      );
      setDailyUsage(dailyData);
      setTopSessions(topData);
      setBudget(budgetData);
    } catch (err) {
      if (!loadGate.current.isCurrent(generation)) return;
      console.error('加载用量数据失败:', err);
      setLoadError('加载用量数据失败：' + (err instanceof Error ? err.message : String(err)));
    } finally {
      if (loadGate.current.isCurrent(generation)) setLoading(false);
    }
  }, [days]);

  useEffect(() => {
    loadData();
  }, [loadData]);

  // 首次加载才整页占位（B1-3）；已有数据时切周期走局部刷新，不清空已渲染内容
  if (loading && !stats) {
    return (
      <div className="page">
        <div className="page__head">
          <div className="page__title">用量与成本</div>
        </div>
        <div style={{ padding: '2rem', textAlign: 'center', color: 'var(--fg-4)' }}>加载中...</div>
      </div>
    );
  }

  if (loadError && !stats) {
    return (
      <div className="page">
        <div className="page__head">
          <div>
            <div className="page__title">用量与成本</div>
            <div className="page__sub">当前无法读取用量数据，原有数据不会被覆盖。</div>
          </div>
        </div>
        <div className="usage-wrap">
          <div className="usage-inline-error" role="alert">
            <Icon name="alert" />
            <span>{loadError}</span>
            <button
              className="btn btn--subtle btn--sm"
              onClick={() => void loadData()}
              type="button"
            >
              重试
            </button>
          </div>
        </div>
      </div>
    );
  }

  const avgPerSession =
    stats && stats.session_count > 0 ? stats.total_cost / stats.session_count : 0;
  const previousAvgPerSession =
    stats && stats.previous_session_count > 0
      ? stats.previous_total_cost / stats.previous_session_count
      : 0;
  const comparisonLabel = period === '7d' ? '较前 7 天' : '较前 30 天';

  function exportCsv() {
    const csv = buildUsageCsv({
      periodLabel: period === '7d' ? '近 7 天' : '近 30 天',
      stats,
      modelUsage,
      dailyUsage,
      topSessions,
      budget,
      generatedAt: new Date(),
    });
    const blob = new Blob([`\uFEFF${csv}`], { type: 'text/csv;charset=utf-8' });
    const url = URL.createObjectURL(blob);
    const anchor = document.createElement('a');
    anchor.href = url;
    anchor.download = `helm-usage-${period}-${new Date().toISOString().slice(0, 10)}.csv`;
    document.body.appendChild(anchor);
    anchor.click();
    anchor.remove();
    URL.revokeObjectURL(url);
  }

  return (
    <div className="page scroll">
      <div className="page__head">
        <div>
          <div className="page__title">用量与成本</div>
          <div className="page__sub">
            跨每个引擎、服务商和会话的 token 吞吐量与花费 — 并附预算护栏。
          </div>
        </div>
        <div className="row gap-sm">
          <div className="seg">
            <button
              className={period === '7d' ? 'is-active' : ''}
              disabled={loading}
              onClick={() => setPeriod('7d')}
            >
              近 7 天
            </button>
            <button
              className={period === '30d' ? 'is-active' : ''}
              disabled={loading}
              onClick={() => setPeriod('30d')}
            >
              近 30 天
            </button>
          </div>
          <button className="btn btn--subtle" disabled={loading} onClick={exportCsv}>
            <Icon name="upright" /> 导出 CSV
          </button>
        </div>
      </div>

      <div
        className={'usage-wrap' + (loading ? ' usage-wrap--refreshing' : '')}
        aria-busy={loading}
      >
        {loadError ? (
          <div className="usage-inline-error">
            <Icon name="alert" />
            <span>{loadError}</span>
            <button className="btn btn--subtle btn--sm" onClick={() => void loadData()}>
              重试
            </button>
          </div>
        ) : null}
        {/* 顶部统计卡 */}
        <div className="usage-stats">
          <div className="card stat">
            <div className="stat__l">
              <Icon name="dollar" />
              花费 <small>· {period === '7d' ? '近 7 天' : '近 30 天'}</small>
            </div>
            <div className="stat__v">
              ${Math.floor(stats?.total_cost || 0)}
              <small>.{((stats?.total_cost || 0) % 1).toFixed(2).slice(2)}</small>
            </div>
            <div className="stat__d">
              {comparisonText(
                stats?.total_cost ?? 0,
                stats?.previous_total_cost ?? 0,
                comparisonLabel,
              )}
            </div>
            <div className="usage-cost-kinds">
              实际 ${(stats?.actual_cost ?? 0).toFixed(2)} · 估算 $
              {(stats?.estimated_cost ?? 0).toFixed(2)}
              {(stats?.subscription_count ?? 0) > 0
                ? ` · 订阅内 ${stats?.subscription_count ?? 0} 次`
                : ''}
              {(stats?.unknown_count ?? 0) > 0
                ? ` · ${stats?.unknown_count ?? 0} 次无价格数据`
                : ''}
              {(stats?.legacy_count ?? 0) > 0
                ? ` · 历史未分类 $${(stats?.legacy_cost ?? 0).toFixed(2)}（${stats?.legacy_count ?? 0} 次）`
                : ''}
            </div>
          </div>

          <div className="card stat">
            <div className="stat__l">
              <Icon name="layers" />
              Token <small>· {period === '7d' ? '近 7 天' : '近 30 天'}</small>
            </div>
            <div className="stat__v">
              {((stats?.total_tokens || 0) / 1_000_000).toFixed(2)}
              <small>M</small>
            </div>
            <div className="stat__d">
              {comparisonText(
                stats?.total_tokens ?? 0,
                stats?.previous_total_tokens ?? 0,
                '吞吐量较前一期',
              )}
            </div>
          </div>

          <div className="card stat">
            <div className="stat__l">
              <Icon name="chat" />
              请求数 <small>· {period === '7d' ? '近 7 天' : '近 30 天'}</small>
            </div>
            <div className="stat__v">{stats?.request_count || 0}</div>
            <div className="stat__d">
              {comparisonText(
                stats?.request_count ?? 0,
                stats?.previous_request_count ?? 0,
                comparisonLabel,
              )}
            </div>
            <div className="stat__d flat">跨 {stats?.session_count || 0} 个会话</div>
          </div>

          <div className="card stat">
            <div className="stat__l">
              <Icon name="history" />
              平均 / 会话
            </div>
            <div className="stat__v">
              ${Math.floor(avgPerSession)}
              <small>.{(avgPerSession % 1).toFixed(2).slice(2)}</small>
            </div>
            <div className="stat__d">
              {comparisonText(avgPerSession, previousAvgPerSession, '较前一期')}
            </div>
          </div>
        </div>

        {/* 花费趋势图 + 预算 */}
        <div className="usage-grid2">
          <div className="card">
            <div className="panel__h">
              <b>花费趋势</b>
              <span className="legend">
                <span>
                  <i />
                  每日花费 (USD)
                </span>
              </span>
            </div>
            <div className="panel__b">
              <DailyChart data={dailyUsage} />
            </div>
          </div>

          <BudgetCard budget={budget} onUpdate={loadData} />
        </div>

        {/* 按模型花费 */}
        {modelUsage.length > 0 && (
          <div className="card" style={{ marginBottom: 14 }}>
            <div className="panel__h">
              <b>按模型花费</b>
              <small>
                近 {days} 天 · {modelUsage.length} 个模型活跃
              </small>
            </div>
            <table className="table">
              <thead>
                <tr>
                  <th>模型</th>
                  <th>引擎</th>
                  <th>请求数</th>
                  <th>输入 / 输出 token</th>
                  <th>花费</th>
                  <th style={{ width: 190 }}>占比</th>
                </tr>
              </thead>
              <tbody>
                {modelUsage.map((m, i) => (
                  <tr key={i}>
                    <td>
                      <span className="row gap-sm">
                        <Icon name="sparkles" style={{ width: 15, height: 15 }} />
                        <span className="strong mono">{m.model}</span>
                      </span>
                    </td>
                    <td>
                      <span className="row gap-sm">
                        <Icon
                          name={m.engine === 'claude-code' ? 'zap' : 'cpu'}
                          style={{ width: 14, height: 14 }}
                        />
                        {m.engine === 'claude-code' ? 'Claude Code' : 'Codex'}
                      </span>
                    </td>
                    <td className="mono">{m.request_count}</td>
                    <td className="mono">
                      {(m.input_tokens / 1000).toFixed(0)}K / {(m.output_tokens / 1000).toFixed(0)}K
                    </td>
                    <td className="mono strong">${m.cost_usd.toFixed(2)}</td>
                    <td>
                      <div className="sharecell">
                        <div className="meter">
                          <i style={{ width: `${m.share * 100}%` }} />
                        </div>
                        <span className="pct">{Math.round(m.share * 100)}%</span>
                      </div>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}

        {/* 按服务商花费 + 花费最高的会话 */}
        <div className="usage-grid2 usage-grid2--equal">
          {/* 按服务商花费 */}
          <div className="card">
            <div className="panel__h">
              <b>按服务商花费</b>
              <small>真实归属 · 近 {days} 天</small>
            </div>
            <div className="panel__b" style={{ paddingTop: 6, paddingBottom: 8 }}>
              {providerUsage.map((p) => {
                return (
                  <div key={p.provider || 'unknown'} className="brow">
                    <div className="brow__ic">
                      <Icon name={p.provider ? 'server' : 'history'} />
                    </div>
                    <div className="brow__m">
                      <b>{p.name}</b>
                      <small>
                        {p.kind === 'subscription'
                          ? `订阅${p.ready ? ' · 已就绪' : ''}`
                          : p.kind === 'local'
                            ? `本地${p.ready ? ' · 已就绪' : ''}`
                            : p.kind === 'api'
                              ? `API${p.ready ? ' · 已就绪' : ''}`
                              : '历史记录'}
                        {p.cost_usd === 0 && p.ready ? ' · 本期未使用' : ''}
                      </small>
                      <div className="meter">
                        <i style={{ width: `${p.share * 100}%` }} />
                      </div>
                    </div>
                    <div className="brow__v">
                      <b>${p.cost_usd.toFixed(2)}</b>
                      <small>{Math.round(p.share * 100)}%</small>
                    </div>
                  </div>
                );
              })}
              {providerUsage.length === 0 && (
                <EmptyState
                  icon="chart"
                  title="还没有任何用量记录"
                  hint="发起一次真实会话后，这里会展示按服务商聚合的花费与占比。"
                  action={{
                    label: '去工作区发起会话',
                    onClick: () =>
                      window.dispatchEvent(
                        new CustomEvent('helm:navigate', { detail: { page: 'workspace' } }),
                      ),
                  }}
                />
              )}
            </div>
          </div>

          {/* 花费最高的会话 */}
          {topSessions.length > 0 && (
            <div className="card">
              <div className="panel__h">
                <b>花费最高的会话</b>
              </div>
              <div className="panel__b" style={{ paddingTop: 6, paddingBottom: 8 }}>
                {topSessions.map((s) => (
                  <button
                    key={s.id}
                    className="brow usage-top-session"
                    type="button"
                    onClick={() =>
                      window.dispatchEvent(
                        new CustomEvent('helm:open-session', { detail: { sessionId: s.id } }),
                      )
                    }
                  >
                    <div className="brow__ic">
                      <Icon name={s.engine === 'claude-code' ? 'zap' : 'cpu'} />
                    </div>
                    <div className="brow__m">
                      <b>{s.title}</b>
                      <small className="mono">{s.model}</small>
                    </div>
                    <div className="brow__v">
                      <b>${s.cost_usd.toFixed(2)}</b>
                      <small>{(s.total_tokens / 1000).toFixed(1)}K token</small>
                    </div>
                  </button>
                ))}
              </div>
            </div>
          )}
        </div>
      </div>
    </div>
  );
}

// 按服务商聚合
// 每日花费图表
function DailyChart({ data }: { data: DailyUsage[] }) {
  if (data.length === 0) {
    return (
      <div style={{ padding: '2rem', textAlign: 'center', color: 'var(--fg-4)', fontSize: 13 }}>
        暂无花费记录，发起会话后这里会出现每日趋势
      </div>
    );
  }

  const maxCost = Math.max(...data.map((d) => d.cost_usd));

  return (
    <>
      <div className="chart">
        {data.map((d, i) => {
          const height = maxCost > 0 ? (d.cost_usd / maxCost) * 100 : 0;
          const date = new Date(d.date);
          const label = date.toLocaleDateString('zh-CN', { month: 'numeric', day: 'numeric' });
          const weekday = ['周日', '周一', '周二', '周三', '周四', '周五', '周六'][date.getDay()];
          const daysAgo = data.length - 1 - i;
          const isToday = daysAgo === 0;
          const showLabel = data.length <= 7 || isToday || daysAgo % 7 === 0;
          return (
            <div key={i} className="col">
              <span className="tip">
                {isToday ? '今天' : `${label} ${weekday}`} · ${d.cost_usd.toFixed(2)}
              </span>
              <div className="bcell">
                <div className="bar" style={{ height: `${height}%` }} />
              </div>
              {showLabel ? (
                <div className="cl">{isToday ? '今天' : label}</div>
              ) : (
                <div className="cl" style={{ visibility: 'hidden' }}>
                  ·
                </div>
              )}
            </div>
          );
        })}
      </div>
      <div className="chart__foot">
        <span>{data.length} 天前</span>
        <span>峰值 ${maxCost.toFixed(2)}</span>
        <span>今天</span>
      </div>
    </>
  );
}

// 预算卡片
function BudgetCard({ budget, onUpdate }: { budget: Budget | null; onUpdate: () => void }) {
  const [editing, setEditing] = useState(false);

  if (!budget) {
    return (
      <div className="card">
        <div className="panel__h">
          <b>本月预算</b>
        </div>
        <div
          className="panel__b budget"
          style={{ textAlign: 'center', padding: '2rem 0', color: 'var(--fg-4)' }}
        >
          <div>预算数据加载失败</div>
          <button
            className="btn btn--subtle btn--sm"
            style={{ marginTop: 12 }}
            onClick={onUpdate}
            type="button"
          >
            重试
          </button>
        </div>
      </div>
    );
  }

  const remaining = budget.monthly_limit - budget.current_month_cost;
  // 阈值提醒档位与托盘通知一致（P3-2）：70% 起视为已触发
  const isAlert = budget.alert_at_80 && budget.percentage >= 70;
  const isStop = budget.stop_at_100 && budget.percentage >= 100;

  return (
    <div className="card">
      <div className="panel__h">
        <b>本月预算</b>
        <button className="btn-icon sm" onClick={() => setEditing(true)} title="编辑预算">
          <Icon name="settings" />
        </button>
      </div>
      <div className="panel__b budget">
        {budget.monthly_limit > 0 ? (
          <>
            <div className="budget__big">
              ${Math.floor(budget.current_month_cost)}
              <small>.{(budget.current_month_cost % 1).toFixed(2).slice(2)}</small>
              <span style={{ color: 'var(--fg-4)', fontSize: 15 }}>
                {' '}
                / ${budget.monthly_limit.toFixed(0)}
              </span>
            </div>
            <div className="budget__sub">
              {new Date().getFullYear()} 年 {new Date().getMonth() + 1} 月 · 已用{' '}
              {budget.percentage.toFixed(0)}% · 预计月末达 $
              {projectedMonthEndCost(budget.current_month_cost).toFixed(0)}
            </div>
            <div className={`meter ${isAlert ? 'meter--warn' : ''}`}>
              <i style={{ width: `${Math.min(budget.percentage, 100)}%` }} />
            </div>
            <div
              className="row row--between"
              style={{ fontSize: 11.5, color: 'var(--fg-4)', marginTop: 10 }}
            >
              <span>下月 1 日重置</span>
              <span
                className="mono"
                style={remaining < 0 ? { color: 'var(--danger)', fontWeight: 600 } : undefined}
              >
                剩余 ${remaining.toFixed(2)}
              </span>
            </div>
            <div className="alerts">
              {budget.alert_at_80 && (
                <div className="alert-row">
                  <Icon name={isAlert ? 'alert' : 'checkc'} />
                  阈值提醒（70% / 90% 系统通知）· {isAlert ? '已触发' : '已启用'}
                </div>
              )}
              {budget.stop_at_100 && (
                <div className="alert-row">
                  <Icon name="shield" />达 100% 自动停止 · {isStop ? '已阻止新任务' : '已启用'}
                </div>
              )}
            </div>
          </>
        ) : (
          <div style={{ textAlign: 'center', padding: '2rem 0', color: 'var(--fg-4)' }}>
            <div style={{ marginBottom: 12 }}>未设置预算</div>
            <button className="btn btn--primary" onClick={() => setEditing(true)}>
              设置预算
            </button>
          </div>
        )}
      </div>

      {editing && (
        <BudgetDialog
          budget={budget}
          onClose={() => setEditing(false)}
          onSave={() => {
            setEditing(false);
            onUpdate();
          }}
        />
      )}
    </div>
  );
}

// 预算编辑弹窗
function BudgetDialog({
  budget,
  onClose,
  onSave,
}: {
  budget: Budget;
  onClose: () => void;
  onSave: () => void;
}) {
  const dialogRef = useDialogBehavior(onClose);
  const [limit, setLimit] = useState(budget.monthly_limit.toString());
  const [alert80, setAlert80] = useState(budget.alert_at_80);
  const [stop100, setStop100] = useState(budget.stop_at_100);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function handleSave() {
    setSaving(true);
    setError(null);
    try {
      await setBudget(parseFloat(limit) || 0, alert80, stop100);
      onSave();
    } catch (err) {
      console.error('保存预算失败:', err);
      setError('保存失败：' + (err instanceof Error ? err.message : String(err)));
    } finally {
      setSaving(false);
    }
  }

  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div
        ref={dialogRef}
        className="modal"
        role="dialog"
        aria-modal="true"
        aria-labelledby="budget-dialog-title"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="modal__head">
          <h3 id="budget-dialog-title">编辑预算</h3>
          <button className="btn-icon" onClick={onClose} aria-label="关闭预算设置">
            <Icon name="x" />
          </button>
        </div>
        <div className="modal__body">
          <div className="form-group">
            <label>月度预算（USD）</label>
            <input
              type="number"
              value={limit}
              onChange={(e) => setLimit(e.target.value)}
              placeholder="0"
              min="0"
              step="0.01"
            />
            <small style={{ color: 'var(--fg-4)', fontSize: 12 }}>设为 0 表示不限制预算</small>
          </div>
          {error ? <div className="usage-dialog-error">{error}</div> : null}

          <div className="form-group">
            <label className="checkbox">
              <input
                type="checkbox"
                checked={alert80}
                onChange={(e) => setAlert80(e.target.checked)}
              />
              <span>用量阈值提醒（达 70% / 90% 时发系统通知，托盘同步显示）</span>
            </label>
          </div>

          <div className="form-group">
            <label className="checkbox">
              <input
                type="checkbox"
                checked={stop100}
                onChange={(e) => setStop100(e.target.checked)}
              />
              <span>达 100% 时自动停止新任务</span>
            </label>
            <small style={{ color: 'var(--fg-4)', fontSize: 12 }}>
              启用后，超预算将无法发送新消息
            </small>
          </div>
        </div>
        <div className="modal__foot">
          <button className="btn btn--subtle" onClick={onClose} disabled={saving}>
            取消
          </button>
          <button className="btn btn--primary" onClick={handleSave} disabled={saving}>
            {saving ? '保存中...' : '保存'}
          </button>
        </div>
      </div>
    </div>
  );
}
