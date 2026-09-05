import { describe, expect, it } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import { PRIMARY_RAIL_ENTRIES } from './navigation';
import { Rail, RailRecentBody, RailTaskRows } from './Rail';
import { buildRailRecentGroups, type RailTaskRow } from './railViewModel';
import type { SessionSummary } from '../sessions/sessionTypes';

let seq = 0;
function session(overrides: Partial<SessionSummary> = {}): SessionSummary {
  seq += 1;
  return {
    id: 's' + seq,
    cliSessionId: null,
    title: '任务 ' + seq,
    engine: 'claude-code',
    model: 'model-x',
    cwd: 'D:/proj/helm',
    status: 'idle',
    messageCount: 1,
    inputTokens: 0,
    outputTokens: 0,
    costUsd: 0,
    createdAt: 0,
    updatedAt: seq * 10,
    ...overrides,
  };
}

const noop = () => undefined;

describe('Rail 导航协议（S0 冻结，S1 视觉承接）', () => {
  const NAV_LABELS = ['新任务', 'AI 配置', '插件', '用量'];

  it('冻结四个文字主入口，全部任务不占一级入口', () => {
    expect(PRIMARY_RAIL_ENTRIES.map((item) => item.label)).toEqual(NAV_LABELS);
    expect(PRIMARY_RAIL_ENTRIES.some((item) => item.id === 'sessions')).toBe(false);
    const markup = renderToStaticMarkup(
      <Rail active="home" onSelect={noop} onSetDefaultDirectory={noop} />,
    );
    for (const label of NAV_LABELS) {
      expect(markup).toContain('aria-label="' + label + '"');
      expect(markup).toContain('rail-nav__label');
    }
    // 「全部任务」已移入设置页承载；工具行按原型为三颗图标钮（搜索/视图选项/添加工作目录）
    expect(markup).not.toContain('rail-recent__all');
    expect(markup).toContain('aria-label="添加工作目录"');
  });

  it('设置固定在底部，当前页高亮唯一', () => {
    const markup = renderToStaticMarkup(
      <Rail active="home" onSelect={noop} onSetDefaultDirectory={noop} />,
    );
    expect(markup).toContain('aria-label="设置"');
    expect(markup).toContain('rail-footer');
    expect(markup.match(/is-active/g)).toHaveLength(1);
    expect(markup.match(/aria-current="page"/g)).toHaveLength(1);
  });

  it('工作区详情态不映射为一级导航激活项', () => {
    const markup = renderToStaticMarkup(
      <Rail active="workspace" onSelect={noop} onSetDefaultDirectory={noop} />,
    );
    expect(markup.match(/is-active/g)).toBeNull();
    expect(markup.match(/aria-current="page"/g)).toBeNull();
  });

  it('设置页激活底部设置入口', () => {
    const markup = renderToStaticMarkup(
      <Rail active="settings" onSelect={noop} onSetDefaultDirectory={noop} />,
    );
    expect(markup.match(/is-active/g)).toHaveLength(1);
  });

  it('最近任务区提供搜索与视图选项入口，空态文案不编造数据', () => {
    const markup = renderToStaticMarkup(
      <Rail active="home" onSelect={noop} onSetDefaultDirectory={noop} />,
    );
    expect(markup).toContain('aria-label="搜索任务"');
    expect(markup).toContain('aria-label="视图选项"');
    expect(markup).toContain('最近任务');
    expect(markup).toContain('正在加载最近任务…');
  });
});

describe('Rail 最近任务渲染（S1）', () => {
  const rows: RailTaskRow[] = [
    {
      session: session({ id: 't1', title: '修复鉴权令牌刷新', forkedFrom: '统一全站错误处理' }),
      timeLabel: '14:32',
    },
    {
      session: session({ id: 't2', title: '重构 ETL 聚合阶段', cwd: 'D:/proj/docs' }),
      timeLabel: '昨天',
    },
  ];

  it('任务行渲染标题 / 目录或分叉来源 / 时间 / kebab，激活行高亮', () => {
    const markup = renderToStaticMarkup(
      <RailTaskRows rows={rows} activeTaskId="t2" onOpenTask={noop} onOpenMenu={noop} />,
    );
    expect(markup).toContain('修复鉴权令牌刷新');
    expect(markup).toContain('分叉自 统一全站错误处理');
    expect(markup).toContain('rail-task__from');
    expect(markup).toContain('D:/proj/docs');
    expect(markup).toContain('14:32');
    expect(markup).toContain('aria-label="更多操作：重构 ETL 聚合阶段"');
    expect(markup.match(/is-active/g)).toHaveLength(1);
  });

  it('状态徽标按真实字段渲染：等审批/运行中替代时间位（2026-08-25 用户决策，推翻 08-23 移除决定）', () => {
    const chipRows: RailTaskRow[] = [
      {
        session: session({
          id: 'c1',
          title: '统一全站错误处理',
          pendingApproval: true,
        }),
        timeLabel: '昨天',
      },
      {
        session: session({ id: 'c2', title: '修复鉴权令牌刷新', lastTurnStatus: 'running' }),
        timeLabel: '刚刚',
      },
    ];
    const markup = renderToStaticMarkup(
      <RailTaskRows rows={chipRows} activeTaskId={null} onOpenTask={noop} onOpenMenu={noop} />,
    );
    expect(markup).toContain('rail-task__state--wait');
    expect(markup).toContain('等审批');
    expect(markup).toContain('is-wait');
    expect(markup).toContain('rail-task__state--run');
    expect(markup).toContain('运行中');
    // 徽标替代时间位：这两行不再出现时间文案
    expect(markup).not.toContain('>昨天</span>');
  });

  it('无状态的行保持时间展示，不渲染徽标', () => {
    const markup = renderToStaticMarkup(
      <RailTaskRows rows={rows} activeTaskId={null} onOpenTask={noop} onOpenMenu={noop} />,
    );
    expect(markup).not.toContain('rail-task__state');
    expect(markup).toContain('rail-task__time');
    expect(markup).toContain('14:32');
  });

  it('按工作目录分组时展示末级目录名与计数，完整 cwd 放 title', () => {
    const sessions = [
      session({ id: 'a1', cwd: 'D:/projects/helm' }),
      session({ id: 'a2', cwd: 'D:/projects/data-pipeline' }),
    ];
    const groups = buildRailRecentGroups(sessions, {
      query: '',
      grouping: 'folder',
      sort: 'recent',
    });
    const markup = renderToStaticMarkup(
      <RailRecentBody
        loading={false}
        error={null}
        groups={groups}
        hasAnySession
        activeTaskId={null}
        onRetry={noop}
        onOpenTask={noop}
        onOpenMenu={noop}
      />,
    );
    expect(markup).toContain('rail-group');
    expect(markup).toContain('data-pipeline');
    expect(markup).toContain('title="D:/projects/data-pipeline"');
  });

  it('加载 / 错误 / 空态文案可验证，错误提供重试', () => {
    const loading = renderToStaticMarkup(
      <RailRecentBody
        loading
        error={null}
        groups={[]}
        hasAnySession={false}
        activeTaskId={null}
        onRetry={noop}
        onOpenTask={noop}
        onOpenMenu={noop}
      />,
    );
    expect(loading).toContain('正在加载最近任务…');

    const failed = renderToStaticMarkup(
      <RailRecentBody
        loading={false}
        error="读取任务列表失败"
        groups={[]}
        hasAnySession={false}
        activeTaskId={null}
        onRetry={noop}
        onOpenTask={noop}
        onOpenMenu={noop}
      />,
    );
    expect(failed).toContain('role="alert"');
    expect(failed).toContain('重试');

    const empty = renderToStaticMarkup(
      <RailRecentBody
        loading={false}
        error={null}
        groups={[]}
        hasAnySession={false}
        activeTaskId={null}
        onRetry={noop}
        onOpenTask={noop}
        onOpenMenu={noop}
      />,
    );
    expect(empty).toContain('暂无最近任务');

    const noMatch = renderToStaticMarkup(
      <RailRecentBody
        loading={false}
        error={null}
        groups={[]}
        hasAnySession
        activeTaskId={null}
        onRetry={noop}
        onOpenTask={noop}
        onOpenMenu={noop}
      />,
    );
    expect(noMatch).toContain('没有匹配的任务');
  });
});

describe('每组最多 10 条、超出折叠（2026-09-04 用户规格）', () => {
  function manyRows(count: number, cwd = 'D:/proj/helm', prefix = 'm'): RailTaskRow[] {
    // updatedAt 递减：任务 0 最新，与侧栏时间倒排一致（可见的是前 10 个标题）
    return Array.from({ length: count }, (_, index) => ({
      session: session({
        id: prefix + index,
        title: '任务 ' + index,
        cwd,
        updatedAt: 1000 - index,
      }),
      timeLabel: '刚刚',
    }));
  }

  it('平铺列表超 10 条：只渲染前 10 行 + 折叠行；展开后全量可见并可收起', () => {
    const groups = buildRailRecentGroups(
      manyRows(13).map((row) => row.session),
      { query: '', grouping: 'list', sort: 'recent' },
    );
    const base = {
      loading: false,
      error: null,
      hasAnySession: true,
      activeTaskId: null,
      onRetry: noop,
      onOpenTask: noop,
      onOpenMenu: noop,
    } as const;
    const folded = renderToStaticMarkup(<RailRecentBody {...base} groups={groups} truncate />);
    for (let i = 0; i < 10; i += 1) expect(folded).toContain('任务 ' + i);
    expect(folded).not.toContain('任务 10');
    expect(folded).toContain('显示全部 13 条');
    expect(folded).not.toContain('收起');

    const expanded = renderToStaticMarkup(
      <RailRecentBody {...base} groups={groups} truncate expandedGroups={['__all__']} />,
    );
    expect(expanded).toContain('任务 12');
    expect(expanded).toContain('收起');
    expect(expanded).not.toContain('显示全部');
  });

  it('按目录分组：截断按组独立生效，目录 A 满 13 条折叠、目录 B 只有 2 条不折叠', () => {
    const sessions = [
      ...manyRows(13, 'D:/p/alpha', 'a').map((row) => row.session),
      ...manyRows(2, 'D:/p/beta', 'b').map((row) => row.session),
    ];
    const groups = buildRailRecentGroups(sessions, {
      query: '',
      grouping: 'folder',
      sort: 'recent',
    });
    const markup = renderToStaticMarkup(
      <RailRecentBody
        loading={false}
        error={null}
        groups={groups}
        hasAnySession
        activeTaskId={null}
        truncate
        onRetry={noop}
        onOpenTask={noop}
        onOpenMenu={noop}
      />,
    );
    expect(markup).toContain('显示全部 13 条');
    // beta 组只有 2 条：不出折叠行，其 2 行全部可见
    expect(markup).not.toContain('显示全部 2 条');
    // 折叠的 alpha 行不渲染第 11 条之后的内容
    expect(markup).not.toContain('任务 10');
  });

  it('搜索态（truncate=false）不截断：全部匹配行直接可见，不出折叠行', () => {
    const groups = buildRailRecentGroups(
      manyRows(13).map((row) => row.session),
      { query: '任务', grouping: 'list', sort: 'recent' },
    );
    const markup = renderToStaticMarkup(
      <RailRecentBody
        loading={false}
        error={null}
        groups={groups}
        hasAnySession
        activeTaskId={null}
        truncate={false}
        onRetry={noop}
        onOpenTask={noop}
        onOpenMenu={noop}
      />,
    );
    expect(markup).toContain('任务 12');
    expect(markup).not.toContain('显示全部');
  });
});
