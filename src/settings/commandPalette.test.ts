import { describe, expect, it } from 'vitest';
import {
  commandPaletteResults,
  filterCommandPaletteCommands,
  filterProviders,
  paletteCommands,
  providerToCommand,
  sessionToCommand,
} from './commandPalette';
import type { SessionSummary } from '../sessions/sessionTypes';
import type { ProviderConfig } from '../providers/api';

function makeSession(overrides: Partial<SessionSummary> = {}): SessionSummary {
  return {
    id: 's-1',
    cliSessionId: 'cli-1',
    title: '修复鉴权令牌刷新',
    engine: 'claude-code',
    model: 'claude-sonnet-4.6',
    cwd: '~/code/acme-web',
    status: 'active',
    messageCount: 12,
    inputTokens: 1000,
    outputTokens: 500,
    costUsd: 0.01,
    createdAt: 1700000000,
    updatedAt: 1700000100,
    ...overrides,
  };
}

function makeProvider(overrides: Partial<ProviderConfig> = {}): ProviderConfig {
  return {
    id: 'anthropic',
    name: 'Anthropic',
    kind: 'api',
    baseUrl: 'https://api.anthropic.com',
    keyRef: 'key-1',
    ready: true,
    lastTest: null,
    protocol: 'anthropic',
    authMethod: 'apikey',
    ...overrides,
  };
}

describe('command palette view model', () => {
  it('exposes real app commands instead of placeholder rows', () => {
    expect(paletteCommands.map((command) => command.action)).toContain('new-session');
    expect(paletteCommands.map((command) => command.action)).toContain('cycle-engine');
    expect(paletteCommands.map((command) => command.page)).toContain('providers');
  });

  it('filters commands by title, group, or shortcut hint', () => {
    expect(
      filterCommandPaletteCommands(paletteCommands, '服务商').map((command) => command.title),
    ).toEqual(['服务商与模型', '添加服务商']);
    expect(
      filterCommandPaletteCommands(paletteCommands, 'ctrl n').map((command) => command.title),
    ).toEqual(['新建会话']);
    expect(
      filterCommandPaletteCommands(paletteCommands, '扩展').map((command) => command.title),
    ).toEqual(['扩展中心', '管理 MCP 服务器', '管理技能 Skills', '管理子代理', '斜杠命令与钩子']);
  });

  it('converts a session to a command', () => {
    const command = sessionToCommand(makeSession({ id: 's-2', title: '测试会话' }));
    expect(command.type).toBe('session');
    expect(command.group).toBe('会话');
    expect(command.title).toBe('测试会话');
    expect(command.sessionId).toBe('s-2');
    expect(command.page).toBe('workspace');
    expect(command.searchText).toContain('测试会话');
    expect(command.searchText).toContain('~/code/acme-web');
    expect(command.searchText).toContain('claude-sonnet-4.6');
  });

  it('converts a provider to a command', () => {
    const command = providerToCommand(makeProvider({ id: 'openai', name: 'OpenAI' }));
    expect(command.type).toBe('provider');
    expect(command.group).toBe('服务商');
    expect(command.title).toBe('OpenAI');
    expect(command.providerId).toBe('openai');
    expect(command.page).toBe('providers');
  });

  it('filters commands by searchText', () => {
    const command = sessionToCommand(makeSession({ title: '首页重构', cwd: '/proj/hero' }));
    expect(filterCommandPaletteCommands([command], 'hero')).toHaveLength(1);
    expect(filterCommandPaletteCommands([command], '不存在')).toHaveLength(0);
  });

  it('filters providers by name', () => {
    const providers = [
      makeProvider({ id: 'anthropic', name: 'Anthropic' }),
      makeProvider({ id: 'openai', name: 'OpenAI' }),
      makeProvider({ id: 'ollama', name: 'Ollama' }),
    ];
    expect(filterProviders(providers, 'open')).toHaveLength(1);
    expect(filterProviders(providers, 'OpenAI').map((p) => p.name)).toEqual(['OpenAI']);
    expect(filterProviders(providers, '').map((p) => p.name)).toEqual([
      'Anthropic',
      'OpenAI',
      'Ollama',
    ]);
  });

  it('puts the three most recent sessions before static commands for an empty query', () => {
    const sessions = [
      makeSession({ id: 'old', title: '较早', updatedAt: 10 }),
      makeSession({ id: 'latest', title: '最新', updatedAt: 30 }),
      makeSession({ id: 'middle', title: '中间', updatedAt: 20 }),
      makeSession({ id: 'fourth', title: '更早', updatedAt: 5 }),
    ];
    const results = commandPaletteResults('', sessions, []);
    expect(results.slice(0, 3).map((item) => item.title)).toEqual(['最新', '中间', '较早']);
    expect(results.slice(0, 3).every((item) => item.group === '最近会话')).toBe(true);
    expect(results[3].id).toBe('workspace');
  });
});
