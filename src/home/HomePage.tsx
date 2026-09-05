import { useEffect, useState } from 'react';
import { Icon, type IconName } from '../shell/icons';
import { EngineBrand } from '../shell/EngineBrand';
import { getProviderConfig, detectCliLogin } from '../providers/api';
import { listSessions } from '../sessions/api';
import type { SessionSummary } from '../sessions/sessionTypes';
import { getBudget, getUsageStats, type Budget, type UsageStats } from '../usage/api';
import { projectedMonthEndCost } from '../usage/metrics';
import { buildHomeStatus, type HomeStatus } from './homeStatus';

interface HomePageProps {
  onNavigate: (page: string) => void;
  onNewSession: () => void;
}

interface HomeData {
  status: HomeStatus;
  sessions: SessionSummary[];
  budget: Budget;
  usageStats: UsageStats;
}

async function loadHomeData(): Promise<HomeData> {
  const [config, sessions, budget, usageStats] = await Promise.all([
    getProviderConfig(),
    listSessions(),
    getBudget(),
    getUsageStats(30),
  ]);
  const loginEntries = await Promise.all(
    config.providers
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
  const loginByProvider = Object.fromEntries(loginEntries);
  const providers = config.providers.map((provider) => ({
    ...provider,
    login: loginByProvider[provider.id] ?? null,
  }));
  return {
    status: buildHomeStatus({ ...config, providers }, sessions.length, budget.current_month_cost),
    sessions: [...sessions].sort((a, b) => b.updatedAt - a.updatedAt).slice(0, 4),
    budget,
    usageStats,
  };
}

const screens: Array<{ id: string; icon: IconName; title: string; desc: string }> = [
  {
    id: 'workspace',
    icon: 'chat',
    title: '工作区',
    desc: '在项目里驱动 Agent，对话、工具调用、代码差异与实时文件上下文。',
  },
  {
    id: 'sessions',
    icon: 'history',
    title: '全部任务',
    desc: '跨项目、引擎与模型，搜索并恢复任意一段任务。',
  },
  {
    id: 'providers',
    icon: 'server',
    title: 'AI 配置',
    desc: '配置执行引擎、服务商路由和工作台可用模型。',
  },
  {
    id: 'extensions',
    icon: 'puzzle',
    title: '插件',
    desc: '管理技能与连接器。',
  },
  {
    id: 'usage',
    icon: 'chart',
    title: '用量',
    desc: '按模型、引擎与服务商统计 Token 和预估费用。',
  },
  { id: 'settings', icon: 'settings', title: '设置', desc: '通用、主题、快捷键与关于。' },
];

function formatRelativeTime(timestamp: number): string {
  const elapsed = Math.max(0, Date.now() - timestamp);
  const minute = 60_000;
  const hour = 60 * minute;
  const day = 24 * hour;
  if (elapsed < minute) return '刚刚';
  if (elapsed < hour) return `${Math.floor(elapsed / minute)} 分钟前`;
  if (elapsed < day) return `${Math.floor(elapsed / hour)} 小时前`;
  if (elapsed < 7 * day) return `${Math.floor(elapsed / day)} 天前`;
  return new Intl.DateTimeFormat('zh-CN', { month: 'numeric', day: 'numeric' }).format(timestamp);
}

function sessionStatus(session: SessionSummary): { label: string; className: string } {
  if (session.status === 'active') return { label: '运行中', className: 'pill pill--success' };
  if (session.status === 'waiting_approval')
    return { label: '等待审批', className: 'pill pill--warn' };
  if (session.status === 'done') return { label: '已完成', className: 'pill pill--ghost' };
  return { label: '可继续', className: 'pill' };
}

export function HomePage({ onNavigate, onNewSession }: HomePageProps) {
  const [data, setData] = useState<HomeData | null>(null);
  const [statusError, setStatusError] = useState<string | null>(null);
  const [retesting, setRetesting] = useState(false);

  const fetchStatus = () => {
    loadHomeData()
      .then((next) => {
        setData(next);
        setStatusError(null);
      })
      .catch((error: unknown) => {
        setStatusError(error instanceof Error ? error.message : '无法读取运行状态');
      });
  };

  useEffect(() => {
    let active = true;
    loadHomeData()
      .then((next) => {
        if (!active) return;
        setData(next);
        setStatusError(null);
      })
      .catch((error: unknown) => {
        if (!active) return;
        setStatusError(error instanceof Error ? error.message : '无法读取运行状态');
      });
    return () => {
      active = false;
    };
  }, []);

  const handleRetest = async () => {
    setRetesting(true);
    try {
      // 并行检测所有引擎
      const engines = data?.status.engines ?? [];
      await Promise.allSettled(
        engines.map(async (engine) => {
          try {
            await detectCliLogin(engine.id as 'claude-code' | 'codex');
          } catch {
            // 忽略单个引擎检测失败
          }
        }),
      );
      // 重新获取状态
      fetchStatus();
    } finally {
      setRetesting(false);
    }
  };

  const status = data?.status ?? null;
  const statusFallback = !data ? (statusError ? '检测失败' : '正在检测…') : null;
  const costFallback = !data ? (statusError ? '—' : '正在统计…') : null;
  const openSession = (sessionId: string) => {
    window.dispatchEvent(new CustomEvent('helm:open-session', { detail: { sessionId } }));
  };

  return (
    <div className="home">
      <div className="home__in">
        <section className="hero">
          <div>
            <div className="eyebrow">CLI 驱动的 Agent 控制台 · 桌面端</div>
            <div className="wordmark">
              <div className="wordmark__logo">
                <Icon name="rocket" />
              </div>
              <h1>Helm</h1>
            </div>
            <p className="hero__lead">
              把你已经在用的编码 Agent 收进一个桌面主场。<b>Claude Code</b> 与 <b>Codex</b>
              在终端里干活，Helm 负责管好它们周边的<b>服务商、模型、会话与文件</b>。
            </p>
            <div className="hero__cta">
              <button className="btn btn--primary btn--lg" onClick={() => onNavigate('workspace')}>
                <Icon name="chat" /> 打开工作区
              </button>
              <button className="btn btn--lg" onClick={onNewSession}>
                <Icon name="plus" /> 新建会话
              </button>
            </div>
            <div className="chips">
              <span className="pill">
                <span className="dot dot--on" /> 已检测到{' '}
                {status?.readyEngineCount ?? statusFallback ?? '—'} 个引擎
              </span>
              <span className="pill">
                <span className="dot dot--accent" /> 已就绪{' '}
                {status?.readyProviderCount ?? statusFallback ?? '—'} 个服务商
              </span>
              <button className="pill mono home-pill-button" onClick={() => onNavigate('usage')}>
                {status?.monthCostText ?? costFallback ?? '$—'} 本月
              </button>
              <span className="pill mono">Ctrl K 搜索</span>
            </div>
            <div className="sec-band">
              <span className="pill">
                <Icon name="shield" /> 审批记录可追溯
              </span>
              <span className="pill">
                <Icon name="checkc" /> 审批链 · 精确指纹授权
              </span>
              <span className="pill">
                <Icon name="key" /> 密钥只进系统钥匙串
              </span>
            </div>
          </div>

          <div className="console" aria-label="引擎状态">
            <div className="console__bar">
              <span className="t">
                <i />
                <i />
                <i />
              </span>
              <span>helm — 引擎状态</span>
            </div>
            <div className="console__body">
              <div className="cline">
                <span className="pr">$</span> <span className="cmd">helm status</span>
              </div>
              {status?.engines.map((engine) => (
                <div className="cline" key={engine.id}>
                  <span className="nm">● {engine.name}</span>
                  <span className="ver">{engine.detail}</span>
                  <span className={engine.ok ? 'ok' : 'warn'}>{engine.state}</span>
                </div>
              )) ?? <div className="console__loading">正在读取真实 CLI 状态…</div>}
              {status?.consoleProviders.map((provider) => (
                <div className="cline" key={`console-provider-${provider.id}`}>
                  <span className="nm">● {provider.name}</span>
                  <span className="ver">{provider.access}</span>
                  <span className={provider.ok ? 'conn' : 'warn'}>{provider.state}</span>
                </div>
              ))}
              {statusError ? (
                <div className="console__error">状态读取失败：{statusError}</div>
              ) : null}
              <div className="console__foot">
                <button
                  className="btn btn--subtle btn--sm"
                  onClick={() => void handleRetest()}
                  disabled={retesting}
                >
                  <Icon name="refresh" className="h-3.5 w-3.5" style={{ width: 13, height: 13 }} />
                  {retesting ? '重新检测中…' : '重新检测'}
                </button>
                <span>
                  {status?.sessionCount ?? '—'} 个会话 · {status?.readyEngineCount ?? '—'}{' '}
                  个就绪引擎
                </span>
              </div>
            </div>
          </div>
        </section>

        <section className="sec">
          <div className="sec__head">
            <div>
              <h2>快速进入</h2>
              <p>桌面客户端的每个界面，一切从这里出发。</p>
            </div>
          </div>
          <div className="cards">
            {screens.map((screen) => (
              <button
                key={screen.id}
                className={`scard${screen.id === 'workspace' ? ' feat' : ''}`}
                type="button"
                onClick={() => onNavigate(screen.id)}
              >
                <div className="scard__ic">
                  <Icon name={screen.icon} />
                </div>
                <h3>{screen.title}</h3>
                <p>{screen.desc}</p>
                <div className="scard__go">
                  打开 <Icon name="right" />
                </div>
              </button>
            ))}
          </div>
        </section>

        <section className="sec">
          <div className="sec__head">
            <div>
              <h2>引擎与服务商</h2>
              <p>Helm 驱动这台机器上已安装的 CLI。</p>
            </div>
            <button className="btn btn--subtle btn--sm" onClick={() => onNavigate('providers')}>
              管理 <Icon name="right" />
            </button>
          </div>
          <div className="statgrid">
            {status?.engines.map((engine) => (
              <div className="card estat" key={`engine-${engine.id}`}>
                <div className="estat__ic">
                  <EngineBrand engine={engine.id} size={16} />
                </div>
                <div className="estat__m">
                  <b>
                    {engine.name}{' '}
                    <span className={engine.ok ? 'pill pill--success' : 'pill'}>
                      {engine.state}
                    </span>
                  </b>
                  <small className="mono">{engine.detail}</small>
                </div>
              </div>
            ))}
            {status?.providers.map((provider) => (
              <div className="card estat" key={`provider-${provider.id}`}>
                <div className="estat__ic">
                  <Icon name="server" />
                </div>
                <div className="estat__m">
                  <b>
                    {provider.name}{' '}
                    <span className={provider.ok ? 'pill pill--accent' : 'pill'}>
                      {provider.state}
                    </span>
                  </b>
                  <small>{provider.detail}</small>
                </div>
                <span className={provider.ok ? 'dot dot--on' : 'dot'} />
              </div>
            ))}
            {!status ? <div className="card estat">正在加载真实状态…</div> : null}
          </div>
        </section>

        <section className="sec">
          <div className="sec__head">
            <div>
              <h2>继续工作</h2>
              <p>最近的会话与本月用量，从上次停下的地方继续。</p>
            </div>
            <button className="btn btn--subtle btn--sm" onClick={() => onNavigate('sessions')}>
              全部会话 <Icon name="right" />
            </button>
          </div>
          <div className="cont">
            <div className="card recent-sessions">
              {data?.sessions.map((session) => {
                const state = sessionStatus(session);
                return (
                  <button className="rrow" key={session.id} onClick={() => openSession(session.id)}>
                    <span className="rrow__ic">
                      <EngineBrand engine={session.engine} size={14} />
                    </span>
                    <span className="rrow__m">
                      <b>{session.title || '未命名会话'}</b>
                      <small>
                        {session.cwd} · {session.model} · {formatRelativeTime(session.updatedAt)}
                      </small>
                    </span>
                    <span className={state.className}>{state.label}</span>
                    <span className="rrow__go">
                      <Icon name="right" />
                    </span>
                  </button>
                );
              })}
              {data && data.sessions.length === 0 ? (
                <div className="home-empty">还没有会话，创建一个会话后会显示在这里。</div>
              ) : null}
              {!data && !statusError ? <div className="home-empty">正在读取最近会话…</div> : null}
            </div>

            <div className="card card--pad umini">
              <div className="row row--between umini__head">
                <span className="eyebrow">本月用量</span>
                <button className="btn btn--ghost btn--sm" onClick={() => onNavigate('usage')}>
                  详情 <Icon name="right" />
                </button>
              </div>
              <div className="umini__big">
                {status?.monthCostText ?? '$—'}{' '}
                <small>
                  /{' '}
                  {data?.budget.monthly_limit
                    ? `$${data.budget.monthly_limit.toFixed(0)} 预算`
                    : '未设预算'}
                </small>
              </div>
              <div className={`meter${(data?.budget.percentage ?? 0) >= 80 ? ' meter--warn' : ''}`}>
                <i
                  style={{ width: `${Math.min(100, Math.max(0, data?.budget.percentage ?? 0))}%` }}
                />
              </div>
              <div className="umini__row">
                <span>实际计费</span>
                <span className="mono">${(data?.usageStats.actual_cost ?? 0).toFixed(2)}</span>
              </div>
              <div className="umini__row">
                <span>估算（无回执）</span>
                <span className="mono">${(data?.usageStats.estimated_cost ?? 0).toFixed(2)}</span>
              </div>
              <div className="umini__row">
                <span>订阅内（不计费）</span>
                <span className="mono">{data?.usageStats.subscription_count ?? 0} 次</span>
              </div>
              <div className="umini__row umini__muted">
                <span>预计月末</span>
                <span className="mono">
                  {data
                    ? `≈ $${projectedMonthEndCost(data.budget.current_month_cost).toFixed(0)}`
                    : '—'}
                </span>
              </div>
            </div>
          </div>
        </section>

        <div className="home__foot">
          <span>Helm · 桌面版 v0.1.0</span>
          <span className="row gap-lg">
            <span className="mono">Ctrl K</span> 命令面板 · <span className="mono">G 然后 W</span>{' '}
            打开工作区
          </span>
        </div>
      </div>
    </div>
  );
}
