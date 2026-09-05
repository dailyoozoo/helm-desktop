import { describe, expect, it } from 'vitest';
import {
  activeRailTaskId,
  applyManualOrder,
  buildRailRecentGroups,
  directoryOptions,
  filterRecentSessions,
  folderNameByCwd,
  RAIL_VISIBLE_ROW_LIMIT,
  railTaskChip,
  reorderVisibleIds,
  splitRailRows,
} from './railViewModel';
import type { SessionFolder, SessionSummary } from '../sessions/sessionTypes';

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

function folder(cwd: string | null, name: string): SessionFolder {
  return { id: name, name, sortOrder: 0, collapsed: false, locked: false, createdAt: 0, cwd };
}

describe('任务行状态徽标（2026-08-25 用户规格）', () => {
  it('等审批优先级最高：pendingApproval 或 waiting_approval 状态都亮等审批', () => {
    expect(railTaskChip(session({ pendingApproval: true }))).toBe('waiting_approval');
    expect(
      railTaskChip(session({ status: 'waiting_approval', lastTurnStatus: 'waiting_approval' })),
    ).toBe('waiting_approval');
  });

  it('运行中来自最新轮次真实状态；旧数据缺字段时退回活跃+未完成工具的保守判定', () => {
    expect(railTaskChip(session({ lastTurnStatus: 'running' }))).toBe('running');
    expect(
      railTaskChip(session({ status: 'active', currentTool: 'Edit', lastTurnStatus: null })),
    ).toBe('running');
    // 活跃但无未完成工具（思考空档）且轮次快照不在跑：不亮灯，避免假运行中
    expect(railTaskChip(session({ status: 'active', currentTool: null }))).toBeNull();
  });

  it('处理完成 = 终态 + 本机看过时间早于最近活动；成功/失败/中断一视同仁', () => {
    const seenAt = 1000;
    for (const status of ['succeeded', 'failed', 'interrupted']) {
      expect(railTaskChip(session({ lastTurnStatus: status, updatedAt: 2 }), { seenAt })).toBe(
        'done_unseen',
      );
    }
  });

  it('点开看过就不再展示：seenAt 晚于最近活动、或该行正打开在工作台时都不标', () => {
    const base = { lastTurnStatus: 'succeeded', updatedAt: 2 } as const;
    expect(railTaskChip(session({ ...base }), { seenAt: 2000 })).toBeNull();
    expect(railTaskChip(session({ ...base }), { seenAt: 1000, isActive: true })).toBeNull();
  });

  it('看过后又跑完新一轮：updatedAt 越过 seenAt 重新算未看', () => {
    const result = railTaskChip(session({ lastTurnStatus: 'succeeded', updatedAt: 9000 }), {
      seenAt: 5000,
    });
    expect(result).toBe('done_unseen');
  });

  it('从未在本机打开过（无记录）不标处理完成，避免首启全列表刷徽标', () => {
    expect(railTaskChip(session({ lastTurnStatus: 'succeeded' }))).toBeNull();
    expect(railTaskChip(session({ lastTurnStatus: 'succeeded' }), { seenAt: null })).toBeNull();
  });

  it('非终态（stalled 等）与归档会话不出徽标', () => {
    expect(railTaskChip(session({ lastTurnStatus: 'stalled' }), { seenAt: 1 })).toBeNull();
    expect(
      railTaskChip(session({ lastTurnStatus: 'failed', archived: true }), { seenAt: 1 }),
    ).toBeNull();
  });
});

describe('最近任务过滤（S1）', () => {
  it('归档会话退出最近任务', () => {
    const keep = session();
    const archived = session({ archived: true });
    const result = filterRecentSessions([keep, archived], '');
    expect(result.map((item) => item.id)).toEqual([keep.id]);
  });

  it('搜索按标题与 canonical cwd 子串匹配，大小写不敏感', () => {
    const auth = session({ title: '修复鉴权令牌', cwd: 'D:/proj/helm' });
    const etl = session({ title: '重构 ETL 聚合阶段', cwd: 'D:/proj/data-pipeline' });
    const readme = session({ title: '更新 README', cwd: 'D:/proj/docs', archived: true });
    expect(filterRecentSessions([auth, etl, readme], 'etl').map((i) => i.id)).toEqual([etl.id]);
    // 已归档会话即使 cwd 命中也不回归最近任务
    expect(filterRecentSessions([auth, etl, readme], 'docs')).toEqual([]);
    expect(filterRecentSessions([auth, etl, readme], 'HELM').map((i) => i.id)).toEqual([auth.id]);
  });
});

describe('最近任务排序与分组（五次反馈规格）', () => {
  it('按列表 + 最近更新：时间倒排，置顶始终最前（默认视图）', () => {
    const old = session({ updatedAt: 100 });
    const newest = session({ updatedAt: 300 });
    const pinned = session({ updatedAt: 200, pinned: true });
    const groups = buildRailRecentGroups([old, newest, pinned], {
      query: '',
      grouping: 'list',
      sort: 'recent',
    });
    expect(groups).toHaveLength(1);
    expect(groups[0].rows.map((row) => row.session.id)).toEqual([pinned.id, newest.id, old.id]);
  });

  it('按目录分组（默认）：组序跟随最近活跃首次出现，组内按时间倒排', () => {
    const helm = session({ cwd: 'D:/projects/helm' });
    const helmSub = session({ cwd: 'D:/projects/helm/sub', updatedAt: 5 });
    const data = session({ cwd: 'D:/projects/data-pipeline', engine: 'codex' });
    const groups = buildRailRecentGroups([helm, helmSub, data], {
      query: '',
      grouping: 'folder',
      sort: 'recent',
    });
    // S0 冻结语义：组序跟随排序后的首次出现（最近更新时即活跃优先）
    expect(groups.map((group) => group.cwd)).toEqual([
      'D:/projects/data-pipeline',
      'D:/projects/helm',
      'D:/projects/helm/sub',
    ]);
    expect(groups.map((group) => group.label)).toEqual(['data-pipeline', 'helm', 'sub']);
    expect(groups[1].rows).toHaveLength(1);
  });

  it('手动排序：manualOrder 决定先后，置顶仍最前，未登记者按最近活跃垫底', () => {
    const a = session({ updatedAt: 100 });
    const b = session({ updatedAt: 200 });
    const c = session({ updatedAt: 300 });
    const pinned = session({ updatedAt: 150, pinned: true });
    const fresh = session({ updatedAt: 999 }); // 不在 manualOrder 中
    const ordered = applyManualOrder([a, b, c, pinned, fresh], [c.id, a.id, b.id]);
    expect(ordered.map((item) => item.id)).toEqual([pinned.id, c.id, a.id, b.id, fresh.id]);
  });

  it('手动排序 + 按目录：组间顺序跟随首次出现，组内保持手动顺序', () => {
    const h1 = session({ cwd: 'D:/p/helm', updatedAt: 10 });
    const h2 = session({ cwd: 'D:/p/helm', updatedAt: 20 });
    const d1 = session({ cwd: 'D:/p/data', updatedAt: 30 });
    const groups = buildRailRecentGroups(
      [h1, h2, d1],
      { query: '', grouping: 'folder', sort: 'manual', manualOrder: [h2.id, h1.id, d1.id] },
      [],
    );
    expect(groups.map((group) => group.cwd)).toEqual(['D:/p/helm', 'D:/p/data']);
    expect(groups[0].rows.map((row) => row.session.id)).toEqual([h2.id, h1.id]);
  });

  it('reorderVisibleIds 把 drag 移到 over 位置；未知 id 原样返回', () => {
    expect(reorderVisibleIds(['a', 'b', 'c'], 'a', 'c')).toEqual(['b', 'c', 'a']);
    expect(reorderVisibleIds(['a', 'b', 'c'], 'c', 'a')).toEqual(['c', 'a', 'b']);
    expect(reorderVisibleIds(['a', 'b'], 'x', 'b')).toEqual(['a', 'b']);
    expect(reorderVisibleIds(['a', 'b'], 'a', 'a')).toEqual(['a', 'b']);
  });

  it('自动 Folder 的命名优先作为分组展示名，canonical cwd 仍是分组真值', () => {
    const target = session({ cwd: 'D:/work/helm-app' });
    const groups = buildRailRecentGroups(
      [target],
      { query: '', grouping: 'folder', sort: 'recent' },
      [folder('D:/work/helm-app', 'Helm 工作区'), folder(null, '默认 Folder')],
    );
    expect(groups[0].label).toBe('Helm 工作区');
    expect(groups[0].rows[0].session.cwd).toBe('D:/work/helm-app');
  });

  it('folderNameByCwd 忽略无 cwd 的 Folder 且不去重覆盖先到者', () => {
    const map = folderNameByCwd([folder(null, '默认'), folder('D:/a', 'A1'), folder('D:/a', 'A2')]);
    expect(map.get('D:/a')).toBe('A1');
    expect(map.size).toBe(1);
  });
});

describe('行模型（S1 · 2026-08-23 像素对齐修订）', () => {
  it('行模型只携带会话与相对时间，不再派生状态 chip', () => {
    const busy = session({ status: 'active', currentTool: 'Edit' });
    const groups = buildRailRecentGroups([busy], { query: '', grouping: 'list', sort: 'recent' });
    const row = groups[0].rows[0];
    expect(row.session.id).toBe(busy.id);
    expect(typeof row.timeLabel).toBe('string');
    expect('statusKey' in row).toBe(false);
  });

  it('分叉会话携带 forkedFrom 来源标题，普通会话为空', () => {
    const fork = session({ forkedFrom: '修复鉴权令牌刷新' });
    const plain = session();
    const groups = buildRailRecentGroups([fork, plain], {
      query: '',
      grouping: 'list',
      sort: 'recent',
    });
    expect(groups[0].rows.map((row) => row.session.forkedFrom ?? null)).toEqual([
      null,
      '修复鉴权令牌刷新',
    ]);
  });

  it('目录候选：真实会话 cwd 去重按最近活跃排序，自动 Folder 命名优先', () => {
    const a = session({ cwd: 'D:/work/alpha', updatedAt: 100 });
    const b = session({ cwd: 'D:/work/beta', updatedAt: 300 });
    const b2 = session({ cwd: 'D:/work/beta', updatedAt: 200 });
    const rows = directoryOptions([a, b, b2], [folder('D:/work/alpha', 'Alpha 工作区')], '');
    expect(rows.map((row) => row.cwd)).toEqual(['D:/work/beta', 'D:/work/alpha']);
    expect(rows[1].label).toBe('Alpha 工作区');
  });

  it('目录候选：query 同时匹配展示名与完整 cwd 子串', () => {
    const a = session({ cwd: 'D:/work/data-pipeline' });
    expect(directoryOptions([a], [], 'pipeline')).toHaveLength(1);
    expect(directoryOptions([a], [], 'data')).toHaveLength(1);
    expect(directoryOptions([a], [], '不匹配')).toHaveLength(0);
  });
});

describe('每组最多 10 条、超出折叠（2026-09-04 用户规格）', () => {
  it('不超过上限全部可见；超出时前 10 条最新可见、其余进折叠余量', () => {
    const few = [session(), session(), session()].map((s) => ({ session: s, timeLabel: '刚刚' }));
    expect(splitRailRows(few)).toEqual({ visible: few, hidden: [] });

    // updatedAt 递减：index 0 最新，与侧栏时间倒排一致
    const many = Array.from({ length: RAIL_VISIBLE_ROW_LIMIT + 5 }, (_, index) => ({
      session: session({ updatedAt: 1000 - index }),
      timeLabel: '刚刚',
    }));
    const { visible, hidden } = splitRailRows(many);
    expect(visible).toHaveLength(RAIL_VISIBLE_ROW_LIMIT);
    expect(hidden).toHaveLength(5);
    // 行序保持原排序（最新在前）：可见的是前 10 条
    expect(visible.map((row) => row.session.id)).toEqual(
      many.slice(0, RAIL_VISIBLE_ROW_LIMIT).map((row) => row.session.id),
    );
    expect(hidden.map((row) => row.session.id)).toEqual(
      many.slice(RAIL_VISIBLE_ROW_LIMIT).map((row) => row.session.id),
    );
  });

  it('恰好 10 条时不出现折叠行', () => {
    const exact = Array.from({ length: RAIL_VISIBLE_ROW_LIMIT }, (_, index) => ({
      session: session({ updatedAt: 1000 - index }),
      timeLabel: '刚刚',
    }));
    const { visible, hidden } = splitRailRows(exact);
    expect(visible).toHaveLength(RAIL_VISIBLE_ROW_LIMIT);
    expect(hidden).toHaveLength(0);
  });

  it('分组结果本身不变：截断是渲染层行为，buildRailRecentGroups 仍返回全量行', () => {
    const many = Array.from({ length: RAIL_VISIBLE_ROW_LIMIT + 3 }, () =>
      session({ cwd: 'D:/p/helm' }),
    );
    const groups = buildRailRecentGroups(many, { query: '', grouping: 'folder', sort: 'recent' });
    expect(groups[0].rows).toHaveLength(RAIL_VISIBLE_ROW_LIMIT + 3);
  });
});

describe('主侧栏选中行 = 工作区当前会话（2026-09-04 用户报告「新对话了左边还选中老对话」）', () => {
  const a = session({ id: 'old', title: '老对话' });
  const b = session({ id: 'new', title: '新对话', cliSessionId: 'cli-new' });
  const list = [a, b];

  it('上报空身份（新建未落库的会话）时不选中任何行', () => {
    expect(activeRailTaskId(list, null)).toBeNull();
    expect(
      activeRailTaskId(list, { historyId: null, handleId: null, cliSessionId: null }),
    ).toBeNull();
  });

  it('historyId / handleId 命中即选中，工作区切走后不再滞留在老对话', () => {
    expect(activeRailTaskId(list, { historyId: 'new' })).toBe('new');
    expect(activeRailTaskId(list, { handleId: 'new' })).toBe('new');
    expect(activeRailTaskId(list, { historyId: 'old' })).toBe('old');
  });

  it('只有 CLI 会话 id 时按 cliSessionId 反查 Helm 会话 id', () => {
    expect(activeRailTaskId(list, { cliSessionId: 'cli-new' })).toBe('new');
    expect(activeRailTaskId(list, { cliSessionId: 'cli-gone' })).toBeNull();
  });

  it('会话尚未进入列表（刚创建、列表未刷新）返回 null，交给调用方回退乐观选中', () => {
    expect(activeRailTaskId([a], { historyId: 'new' })).toBeNull();
  });
});
