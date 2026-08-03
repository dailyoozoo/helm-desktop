import { describe, expect, it } from 'vitest';
import type { SessionFolder, SessionSummary } from '../sessions/sessionTypes';
import { sessionProjectName, sidebarFolderEntries } from './SessionSidebar';

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
});
