import { useEffect, useState } from 'react';
import { Icon, type IconName } from '../shell/icons';
import { getProviderConfig } from '../providers/api';
import { listSessions } from '../sessions/api';
import { getBudget } from '../usage/api';
import { buildHomeStatus, type HomeStatus } from './homeStatus';

interface HomePageProps {
  onNavigate: (page: string) => void;
  onNewSession: () => void;
}

export function HomePage({ onNavigate, onNewSession }: HomePageProps) {
  const [status, setStatus] = useState<HomeStatus | null>(null);
  const [statusError, setStatusError] = useState<string | null>(null);

  useEffect(() => {
    let active = true;
    Promise.all([getProviderConfig(), listSessions(), getBudget()])
      .then(([config, sessions, budget]) => {
        if (!active) return;
        setStatus(buildHomeStatus(config, sessions.length, budget.current_month_cost));
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
  const screens: Array<{ id: string; icon: IconName; title: string; desc: string }> = [
    { id: 'workspace', icon: 'chat', title: '工作区', desc: '与 AI Agent 对话协作，处理开发任务' },
    {
      id: 'providers',
      icon: 'server',
      title: '服务商与模型',
      desc: '配置 API 密钥、模型与 CLI 引擎',
    },
    {
      id: 'sessions',
      icon: 'history',
      title: '会话历史',
      desc: '搜索与恢复历史会话，继续未完成的工作',
    },
    {
      id: 'extensions',
      icon: 'puzzle',
      title: '扩展中心',
      desc: '管理技能、MCP 服务器、子代理与钩子',
    },
    {
      id: 'usage',
      icon: 'chart',
      title: '用量与成本',
      desc: '跟踪 Token 消耗与花费，设置预算护栏',
    },
    { id: 'settings', icon: 'settings', title: '设置', desc: '调整主题、权限与快捷键' },
  ];

  return (
    <div className="home">
      <div className="home__in">
        <div className="hero">
          <div>
            <div className="wordmark">
              <div className="wordmark__logo">
                <Icon name="rocket" />
              </div>
              <h1>Helm</h1>
            </div>
            <p className="hero__lead">
              统一管理多个 <b>CLI Agent 引擎</b>（Claude
              Code、Codex），一个界面掌控服务商、模型、会话、扩展与成本。
            </p>
            <div className="hero__cta">
              <button className="btn btn--primary" onClick={() => onNavigate('workspace')}>
                <Icon name="chat" /> 打开工作区
              </button>
              <button className="btn" onClick={onNewSession}>
                <Icon name="plus" /> 新建会话
              </button>
            </div>
            <div className="chips">
              <span className="pill">已检测 {status?.readyEngineCount ?? '—'} 个引擎</span>
              <span className="pill">已连接 {status?.connectedProviderCount ?? '—'} 个服务商</span>
              <button className="pill mono home-pill-button" onClick={() => onNavigate('usage')}>
                {status?.monthCostText ?? '$—'} 本月
              </button>
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
              {statusError ? (
                <div className="console__error">状态读取失败：{statusError}</div>
              ) : null}
              <div className="console__foot">
                {status?.sessionCount ?? '—'} 个会话 · {status?.readyEngineCount ?? '—'} 个就绪引擎
              </div>
            </div>
          </div>
        </div>

        <section className="sec">
          <div className="sec__head">
            <div>
              <h2>快速导航</h2>
              <p>点击卡片进入对应功能</p>
            </div>
          </div>
          <div className="cards">
            {screens.map((s) => (
              <button key={s.id} className="scard" type="button" onClick={() => onNavigate(s.id)}>
                <div className="scard__ic">
                  <Icon name={s.icon} />
                </div>
                <h3>{s.title}</h3>
                <p>{s.desc}</p>
                <div className="scard__go">
                  前往 <Icon name="right" />
                </div>
              </button>
            ))}
          </div>
        </section>

        <section className="sec">
          <div className="sec__head">
            <div>
              <h2>引擎与服务商</h2>
              <p>来自本机 CLI 探测和服务商真实可达性记录。</p>
            </div>
            <button className="btn btn--subtle btn--sm" onClick={() => onNavigate('providers')}>
              管理 <Icon name="right" />
            </button>
          </div>
          <div className="statgrid">
            {status?.engines.map((engine) => (
              <div className="card estat" key={`engine-${engine.id}`}>
                <div className="estat__ic">
                  <Icon name="zap" />
                </div>
                <div className="estat__m">
                  <b>{engine.name}</b>
                  <small>{engine.detail}</small>
                </div>
                <span className={engine.ok ? 'pill pill--success' : 'pill'}>{engine.state}</span>
              </div>
            ))}
            {status?.providers.map((provider) => (
              <div className="card estat" key={`provider-${provider.id}`}>
                <div className="estat__ic">
                  <Icon name="server" />
                </div>
                <div className="estat__m">
                  <b>{provider.name}</b>
                  <small>{provider.detail}</small>
                </div>
                <span className={provider.ok ? 'pill pill--success' : 'pill'}>
                  {provider.state}
                </span>
              </div>
            ))}
            {!status ? <div className="card estat">正在加载真实状态…</div> : null}
          </div>
        </section>

        <div className="home__foot">
          <span>Helm v0.1.0 · 本地 CLI Agent 管理器</span>
          <span>© 2026</span>
        </div>
      </div>
    </div>
  );
}
