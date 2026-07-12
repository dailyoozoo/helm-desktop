import type { ProviderConfig } from '../providers/api';
import type { SessionSummary } from '../sessions/sessionTypes';
import type { PageId } from '../shell/Rail';
import type { IconName } from '../shell/icons';
import type { AppShortcutAction } from './shortcuts';

export type CommandPaletteItemType = 'command' | 'session' | 'provider';

export interface CommandPaletteCommand {
  id: string;
  type: CommandPaletteItemType;
  group: string;
  title: string;
  icon: IconName;
  hint?: string;
  page?: PageId;
  action?: AppShortcutAction;
  sessionId?: string;
  providerId?: string;
  searchText?: string;
}

export const paletteCommands: CommandPaletteCommand[] = [
  {
    id: 'workspace',
    type: 'command',
    group: '导航',
    title: '打开工作区',
    icon: 'chat',
    hint: 'G W',
    page: 'workspace',
  },
  {
    id: 'sessions',
    type: 'command',
    group: '导航',
    title: '会话历史',
    icon: 'history',
    hint: 'G S',
    page: 'sessions',
  },
  {
    id: 'providers',
    type: 'command',
    group: '导航',
    title: '服务商与模型',
    icon: 'server',
    hint: 'G P',
    page: 'providers',
  },
  {
    id: 'extensions',
    type: 'command',
    group: '导航',
    title: '扩展中心',
    icon: 'puzzle',
    hint: 'G X',
    page: 'extensions',
  },
  {
    id: 'usage',
    type: 'command',
    group: '导航',
    title: '用量与成本',
    icon: 'chart',
    hint: 'G U',
    page: 'usage',
  },
  {
    id: 'settings',
    type: 'command',
    group: '导航',
    title: '设置',
    icon: 'settings',
    hint: 'G ,',
    page: 'settings',
  },
  {
    id: 'home',
    type: 'command',
    group: '导航',
    title: '总览',
    icon: 'home',
    hint: 'G H',
    page: 'home',
  },
  {
    id: 'new-session',
    type: 'command',
    group: '操作',
    title: '新建会话',
    icon: 'plus',
    hint: 'Ctrl N',
    page: 'workspace',
    action: 'new-session',
  },
  {
    id: 'toggle-context',
    type: 'command',
    group: '操作',
    title: '切换上下文面板',
    icon: 'panelright',
    hint: 'Ctrl .',
    page: 'workspace',
    action: 'toggle-context',
  },
  {
    id: 'cycle-engine',
    type: 'command',
    group: '操作',
    title: '切换引擎',
    icon: 'cpu',
    hint: 'Ctrl E',
    page: 'workspace',
    action: 'cycle-engine',
  },
  {
    id: 'add-provider',
    type: 'command',
    group: '操作',
    title: '添加服务商',
    icon: 'key',
    page: 'providers',
  },
  {
    id: 'mcp',
    type: 'command',
    group: '扩展',
    title: '管理 MCP 服务器',
    icon: 'plug',
    page: 'extensions',
  },
  {
    id: 'skills',
    type: 'command',
    group: '扩展',
    title: '管理技能 Skills',
    icon: 'sparkles',
    page: 'extensions',
  },
  {
    id: 'subagents',
    type: 'command',
    group: '扩展',
    title: '管理子代理',
    icon: 'bot',
    page: 'extensions',
  },
  {
    id: 'slash-hooks',
    type: 'command',
    group: '扩展',
    title: '斜杠命令与钩子',
    icon: 'terminal',
    page: 'extensions',
  },
];

export function sessionToCommand(session: SessionSummary): CommandPaletteCommand {
  return {
    id: `session:${session.id}`,
    type: 'session',
    group: '会话',
    title: session.title || '未命名会话',
    icon: 'history',
    hint: session.model,
    page: 'workspace',
    sessionId: session.id,
    searchText: `${session.title} ${session.cwd} ${session.model}`,
  };
}

export function providerToCommand(provider: ProviderConfig): CommandPaletteCommand {
  return {
    id: `provider:${provider.id}`,
    type: 'provider',
    group: '服务商',
    title: provider.name,
    icon: 'server',
    hint: provider.protocol,
    page: 'providers',
    providerId: provider.id,
    searchText: `${provider.name} ${provider.protocol}`,
  };
}

export function filterCommandPaletteCommands(
  commands: CommandPaletteCommand[],
  query: string,
): CommandPaletteCommand[] {
  const q = normalize(query);
  if (!q) return commands;
  return commands.filter((command) =>
    [command.title, command.group, command.hint ?? '', command.searchText ?? ''].some((part) =>
      normalize(part).includes(q),
    ),
  );
}

export function filterProviders(providers: ProviderConfig[], query: string): ProviderConfig[] {
  const q = normalize(query);
  if (!q) return providers;
  return providers.filter((provider) => normalize(provider.name).includes(q));
}

function normalize(value: string): string {
  return value.toLowerCase().replace(/\s+/g, ' ').trim();
}
