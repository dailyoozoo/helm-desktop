import { describe, expect, it } from 'vitest';
import type { SlashCommand } from '../extensions/extensionsApi';
import {
  completeSlashCommand,
  estimateTokens,
  expandSlashCommand,
  expandSlashCommandDetailed,
  filterSlashCommands,
  helmCommandAction,
  matchSlashCommand,
  resolveEnterAction,
} from './slashCommands';

function command(overrides: Partial<SlashCommand>): SlashCommand {
  return {
    id: 'review',
    trigger: '/review',
    description: '审查当前改动',
    scope: 'global',
    enabled: true,
    body: '审查 git diff。',
    engine: 'all',
    source: 'extension',
    ...overrides,
  };
}

const commands: SlashCommand[] = [
  command({ id: 'review', trigger: '/review', description: '审查当前改动' }),
  command({ id: 'test', trigger: '/test', description: '运行测试', body: '运行最相关的测试。' }),
  command({
    id: 'run-e2e',
    trigger: '/run-e2e',
    description: '端到端回归',
    body: '运行端到端测试。',
  }),
  command({
    id: 'draft',
    trigger: '/draft',
    description: '禁用命令',
    enabled: false,
    body: '不应显示。',
  }),
];

describe('filterSlashCommands', () => {
  it('filters enabled slash commands by trigger and description', () => {
    expect(filterSlashCommands(commands, '/re').map((c) => c.trigger)).toEqual(['/review']);
    expect(filterSlashCommands(commands, '/运行').map((c) => c.trigger)).toEqual(['/test']);
    expect(filterSlashCommands(commands, '/').map((c) => c.trigger)).toEqual([
      '/review',
      '/run-e2e',
      '/test',
    ]);
  });

  it('ranks trigger prefix above word-start above substring matches', () => {
    const pool = [
      command({ id: 'a', trigger: '/api', description: '包含 e2e 描述' }),
      command({ id: 'b', trigger: '/run-e2e', description: 'e2e 测试' }),
      command({ id: 'c', trigger: '/e2e-suite', description: 'x' }),
    ];
    expect(filterSlashCommands(pool, '/e2e').map((c) => c.trigger)).toEqual([
      '/e2e-suite',
      '/run-e2e',
      '/api',
    ]);
  });

  it('filters Codex skill triggers that start with $', () => {
    const skills = [command({ id: 'skill-pdf', trigger: '$pdf', description: '处理 PDF' })];
    expect(filterSlashCommands(skills, '$pd').map((item) => item.trigger)).toEqual(['$pdf']);
  });
});

describe('completeSlashCommand', () => {
  it('completes to the trigger with a trailing space instead of expanding the body', () => {
    expect(completeSlashCommand(commands[0])).toBe('/review ');
  });
});

describe('helmCommandAction', () => {
  it('extracts actions only from Helm UI commands', () => {
    expect(helmCommandAction(command({ id: '__helm_new-session', trigger: '/new' }))).toBe(
      'new-session',
    );
    expect(helmCommandAction(command({ id: 'review', trigger: '/review' }))).toBeUndefined();
  });
});

describe('matchSlashCommand', () => {
  it('matches the first token exactly against enabled commands', () => {
    expect(matchSlashCommand(commands, '/review src/')?.id).toBe('review');
    expect(matchSlashCommand(commands, '/rev')).toBeUndefined();
    expect(matchSlashCommand(commands, '/draft x')).toBeUndefined();
    expect(matchSlashCommand(commands, '普通文本')).toBeUndefined();
  });

  it('matches an exact Codex $skill trigger', () => {
    const skills = [command({ id: 'skill-pdf', trigger: '$pdf', description: '处理 PDF' })];
    expect(matchSlashCommand(skills, '$pdf report.pdf')?.id).toBe('skill-pdf');
    expect(matchSlashCommand(skills, '$pd')).toBeUndefined();
  });
});

describe('expandSlashCommand', () => {
  const extension = command({
    id: 'fix',
    trigger: '/fix',
    body: '修复 $1 中的问题：$ARGUMENTS',
  });
  const projectCmd = command({
    id: '__proj_bar',
    trigger: '/bar',
    source: 'engine-project',
    body: '项目级模板',
  });
  const builtin = command({
    id: '__proto_plan',
    trigger: '/plan',
    source: 'builtin',
    body: '请先制定计划：$ARGUMENTS',
  });
  const codexNative = command({
    id: '__codex_deploy',
    trigger: '/deploy',
    source: 'engine-user',
    engine: 'codex',
    body: '部署到 $1 环境，参数 $2',
  });

  it('passes claude-code extension/project commands through rewritten to file stems', () => {
    expect(expandSlashCommand([extension], '/fix a.ts b', 'claude-code')).toBe('/fix a.ts b');
    expect(expandSlashCommand([projectCmd], '/bar x', 'claude-code')).toBe('/bar x');
  });

  it('expands builtin commands locally even for claude-code', () => {
    expect(expandSlashCommand([builtin], '/plan 重构登录', 'claude-code')).toBe(
      '请先制定计划：重构登录',
    );
  });

  it('expands codex commands locally with $ARGUMENTS and positional words', () => {
    expect(expandSlashCommand([codexNative], '/deploy prod "east region"', 'codex')).toBe(
      '部署到 prod 环境，参数 east region',
    );
    expect(expandSlashCommand([extension], '/fix a.ts 空指针', 'codex')).toBe(
      '修复 a.ts 中的问题：a.ts 空指针',
    );
  });

  it('appends arguments when the template has no placeholder', () => {
    expect(expandSlashCommand([projectCmd], '/bar 附加参数', 'codex')).toBe(
      '项目级模板\n\n附加参数',
    );
  });

  it('returns text unchanged when no command matches', () => {
    expect(expandSlashCommand(commands, '/unknown x', 'claude-code')).toBe('/unknown x');
    expect(expandSlashCommand(commands, '普通文本', 'codex')).toBe('普通文本');
  });
});

describe('expandSlashCommandDetailed passthrough 标记（变更-08）', () => {
  const extension = command({
    id: 'fix',
    trigger: '/fix',
    source: 'extension',
    engine: 'all',
    body: '修复 $1 中的问题：$ARGUMENTS',
  });

  it('marks claude-code extension commands as passthrough', () => {
    const r = expandSlashCommandDetailed([extension], '/fix a.ts', 'claude-code');
    expect(r).toEqual({ expanded: '/fix a.ts', matched: true, passthrough: true });
  });

  it('marks codex/local expansion as non-passthrough', () => {
    const r = expandSlashCommandDetailed([extension], '/fix a.ts 空指针', 'codex');
    expect(r.matched).toBe(true);
    expect(r.passthrough).toBe(false);
  });

  it('reports no match for plain text', () => {
    const r = expandSlashCommandDetailed([extension], '普通消息', 'claude-code');
    expect(r).toEqual({ expanded: '普通消息', matched: false, passthrough: false });
  });
});

describe('resolveEnterAction（IME 与未知命令，变更-08）', () => {
  const base = {
    shiftKey: false,
    isComposing: false,
    working: false,
    menuOpen: false,
    hasMenuMatches: false,
    unknownCommand: false,
  };

  it('yields to the IME while composing (中文输入法确认候选词不发送)', () => {
    expect(resolveEnterAction({ ...base, isComposing: true })).toBe('ime');
    // 组字优先级最高，即便菜单开着/是未知命令
    expect(
      resolveEnterAction({ ...base, isComposing: true, menuOpen: true, hasMenuMatches: true }),
    ).toBe('ime');
  });

  it('inserts newline on Shift+Enter', () => {
    expect(resolveEnterAction({ ...base, shiftKey: true })).toBe('newline');
  });

  it('picks the highlighted command when the menu has matches', () => {
    expect(resolveEnterAction({ ...base, menuOpen: true, hasMenuMatches: true })).toBe('pick');
  });

  it('blocks sending an unknown /command', () => {
    expect(resolveEnterAction({ ...base, unknownCommand: true })).toBe('block');
  });

  it('queues the message while a turn is running（变更-12 排队）', () => {
    expect(resolveEnterAction({ ...base, working: true })).toBe('queue');
  });

  it('sends normal text', () => {
    expect(resolveEnterAction(base)).toBe('send');
  });
});

describe('estimateTokens（CJK 修正，变更-08）', () => {
  it('counts CJK denser than latin', () => {
    // 10 个汉字 ≈ 6 tok（/1.6），10 个 ascii ≈ 3 tok（/4）
    expect(estimateTokens('中文中文中文中文中文')).toBeGreaterThan(estimateTokens('abcdefghij'));
  });

  it('returns 0 for empty', () => {
    expect(estimateTokens('')).toBe(0);
  });
});
