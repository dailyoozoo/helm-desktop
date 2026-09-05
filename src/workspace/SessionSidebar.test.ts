import { describe, expect, it } from 'vitest';
import type { SessionFolder, SessionSummary } from '../sessions/sessionTypes';
import { sessionProjectName, sidebarFolderEntries, sidebarStatusCounts } from './SessionSidebar';

const folders: SessionFolder[] = [
  {
    id: 'folder-default',
    name: '默认',
    sortOrder: 0,
    collapsed: false,
    locked: true,
    createdAt: 1,
  },
  {
    id: 'folder-helm',
    name: 'Helm 项目',
    sortOrder: 1,
    collapsed: false,
    locked: false,
    createdAt: 2,
  },
];

const session = (id: string, folderId: string): SessionSummary => ({
  id,
  cliSessionId: null,
  title: id,
  engine: 'codex',
  model: 'gpt-5',
  cwd: `D:\\work\\${id}`,
  status: 'done',
  messageCount: 1,
  inputTokens: 0,
  outputTokens: 0,
  costUsd: 0,
  createdAt: 1,
  updatedAt: 1,
  folderId,
});

describe('SessionSidebar view model', () => {
  it('从 cwd 显示项目名', () => {
    expect(sessionProjectName('D:\\work\\Helm\\')).toBe('Helm');
  });

  it('文件夹名命中时展示该文件夹及其全部会话', () => {
    const sessions = [session('a', 'folder-default'), session('b', 'folder-helm')];
    const result = sidebarFolderEntries(folders, sessions, [], 'helm 项目');

    expect(result).toHaveLength(1);
    expect(result[0].folder.id).toBe('folder-helm');
    expect(result[0].items.map((item) => item.id)).toEqual(['b']);
  });

  it('新安装时不展示空的默认文件夹', () => {
    const result = sidebarFolderEntries([folders[0]], [], [], '');

    expect(result).toEqual([]);
  });

  // ---- 切片C · P1-01 工作区侧栏状态计数（F1） ----

  it('状态计数按真实字段派生，归档只计入「已归档」不计入「全部」', () => {
    const sessions: SessionSummary[] = [
      // 普通完成 → all
      session('a', 'folder-default'),
      // 运行中（active + currentTool）→ all + running
      {
        ...session('b', 'folder-default'),
        status: 'active',
        currentTool: 'Write',
        currentTarget: 'auth.ts',
      },
      // 待审批 → all + waiting_approval
      { ...session('c', 'folder-default'), pendingApproval: true },
      // 失败 → all + failed
      { ...session('d', 'folder-default'), lastTurnFailed: true },
      // 归档 → archived only
      { ...session('e', 'folder-default'), archived: true },
    ];
    const counts = sidebarStatusCounts(sessions);
    expect(counts.all).toBe(4);
    expect(counts.waiting_approval).toBe(1);
    expect(counts.running).toBe(1);
    expect(counts.failed).toBe(1);
    expect(counts.archived).toBe(1);
  });

  it('没有会话时全部计数为 0', () => {
    const counts = sidebarStatusCounts([]);
    expect(counts).toEqual({ all: 0, waiting_approval: 0, running: 0, failed: 0, archived: 0 });
  });

  it('状态为 waiting_approval 的会话即使没有 pendingApproval 也计入等审批', () => {
    const sessions: SessionSummary[] = [
      { ...session('a', 'folder-default'), status: 'waiting_approval' },
    ];
    expect(sidebarStatusCounts(sessions).waiting_approval).toBe(1);
  });

  it('active 但无 currentTool 不计入运行中', () => {
    const sessions: SessionSummary[] = [{ ...session('a', 'folder-default'), status: 'active' }];
    expect(sidebarStatusCounts(sessions).running).toBe(0);
  });
});
