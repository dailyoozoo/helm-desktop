import { describe, expect, it } from 'vitest';
import {
  connectorStatusPill,
  deriveFeaturedStates,
  FEATURED_CONNECTORS,
  filterSkillsByQuery,
  formatTestedAt,
  groupSkillsBySource,
  importResultRows,
  isCredentialKey,
  normalizeWindowsMcpLaunch,
  skillScopeNote,
  slugifySkillName,
  triggerText,
} from './extensionsViewModel';
import type { McpImportItemResult, McpServer, Skill } from './extensionsApi';

function makeSkill(overrides: Partial<Skill> & Pick<Skill, 'id' | 'source'>): Skill {
  return {
    name: overrides.id,
    description: '',
    scope: 'global',
    enabled: true,
    path: `C:\\demo\\${overrides.id}`,
    engine: 'claude-code',
    trigger: `/${overrides.id}`,
    ...overrides,
  };
}

describe('groupSkillsBySource', () => {
  it('按 内置/外部安装/自己创建 分组，空组不出现，顺序固定', () => {
    const sections = groupSkillsBySource([
      makeSkill({ id: 'review', source: 'custom' }),
      makeSkill({ id: 'plan', source: 'builtin' }),
      makeSkill({ id: 'frontend-design', source: 'market' }),
      makeSkill({ id: 'plugin-skill', source: 'plugin' }),
    ]);
    expect(sections.map((section) => section.id)).toEqual(['builtin', 'external', 'custom']);
    expect(sections[0].skills.map((skill) => skill.id)).toEqual(['plan']);
    expect(sections[1].skills.map((skill) => skill.id)).toEqual([
      'frontend-design',
      'plugin-skill',
    ]);
    expect(sections[2].skills.map((skill) => skill.id)).toEqual(['review']);
  });

  it('没有技能时返回空数组（空态由页面渲染）', () => {
    expect(groupSkillsBySource([])).toEqual([]);
  });
});

describe('triggerText', () => {
  it('Claude Code 用 / 前缀，Codex 用 $ 前缀，并归一已有前缀', () => {
    expect(triggerText('/review', 'claude-code')).toBe('/review');
    expect(triggerText('review', 'claude-code')).toBe('/review');
    expect(triggerText('/review', 'codex')).toBe('$review');
    expect(triggerText('$review', 'codex')).toBe('$review');
  });
});

describe('slugifySkillName', () => {
  it('小写归一、空白与点折叠为连字符、去掉首尾连字符', () => {
    expect(slugifySkillName('Release Check!')).toBe('release-check');
    expect(slugifySkillName('  PDF 工具 ')).toBe('pdf');
    expect(slugifySkillName('docs.lookup_v2')).toBe('docs-lookup-v2');
  });

  it('纯中文无法构成目录标识时返回空串，由调用方要求手填', () => {
    expect(slugifySkillName('发布检查')).toBe('');
  });
});

describe('skillScopeNote', () => {
  it('全局与项目、Claude 与 Codex 的落盘说明与后端 create_skill 一致', () => {
    expect(skillScopeNote('claude-code', 'global')).toContain('.claude\\skills');
    expect(skillScopeNote('codex', 'global')).toContain('.codex\\skills');
    expect(skillScopeNote('claude-code', 'project')).toContain('<工作目录>');
    expect(skillScopeNote('codex', 'project')).toContain('.codex\\skills');
  });
});

describe('filterSkillsByQuery', () => {
  it('按名称/标识/描述不区分大小写过滤，空关键词返回全部', () => {
    const skills = [
      makeSkill({ id: 'review', source: 'custom', description: '审查变更' }),
      makeSkill({ id: 'plan', source: 'builtin', description: '计划任务' }),
    ];
    expect(filterSkillsByQuery(skills, '')).toHaveLength(2);
    expect(filterSkillsByQuery(skills, 'REVIEW')).toHaveLength(1);
    expect(filterSkillsByQuery(skills, '计划')).toHaveLength(1);
    expect(filterSkillsByQuery(skills, '不存在')).toHaveLength(0);
  });
});

describe('connectorStatusPill', () => {
  it('只依据真实测试状态：connected=可用、error=连接失败、未测=未检测', () => {
    const base = { lastTestedAt: null };
    expect(connectorStatusPill({ ...base, status: 'connected' } as McpServer).label).toBe('可用');
    expect(connectorStatusPill({ ...base, status: 'error' } as McpServer).tone).toBe('error');
    expect(connectorStatusPill({ ...base, status: 'disconnected' } as McpServer).label).toBe(
      '未检测',
    );
  });
});

describe('formatTestedAt', () => {
  it('今天显示时刻、昨天显示「昨天」、更早显示日期、无记录为空', () => {
    const now = new Date(2026, 7, 22, 15, 0, 0); // 2026-08-22 15:00
    const todayNoon = Math.floor(new Date(2026, 7, 22, 12, 8, 0).getTime() / 1000);
    const yesterday = Math.floor(new Date(2026, 7, 21, 9, 0, 0).getTime() / 1000);
    const earlier = Math.floor(new Date(2026, 6, 1, 9, 0, 0).getTime() / 1000); // 2026-07-01
    expect(formatTestedAt(todayNoon, now)).toBe('检测于今天 12:08');
    expect(formatTestedAt(yesterday, now)).toBe('检测于昨天');
    expect(formatTestedAt(earlier, now)).toBe('检测于 7月1日');
    expect(formatTestedAt(null, now)).toBe('');
  });
});

describe('deriveFeaturedStates', () => {
  it('精选卡状态由真实连接器列表推导：未装=安装，已装且启用=已启用', () => {
    const servers = [
      { name: 'github', enabled: true, transport: 'stdio' },
      { name: 'playwright', enabled: false, transport: 'stdio' },
    ] as McpServer[];
    const states = deriveFeaturedStates(FEATURED_CONNECTORS, servers);
    const byName = new Map(states.map((state) => [state.template.name, state]));
    expect(byName.get('github')).toMatchObject({ installed: true, enabled: true });
    expect(byName.get('playwright')).toMatchObject({ installed: true, enabled: false });
    expect(byName.get('context7')).toMatchObject({ installed: false, enabled: false });
  });

  it('精选模板只含 stdio/http 两种接入方式，与产品边界一致', () => {
    for (const template of FEATURED_CONNECTORS) {
      expect(['stdio', 'http']).toContain(template.transport);
    }
  });

  it('stdio 模板全部经 cmd /c 启动且 npx 带 -y（Windows 直接 spawn 找不到 npx）', () => {
    for (const template of FEATURED_CONNECTORS) {
      if (template.transport !== 'stdio') continue;
      expect(template.command).toBe('cmd');
      expect(template.args?.[0]).toBe('/c');
      expect(template.args).toContain('-y');
    }
  });

  it('filesystem 模板带允许目录参数（官方必填，缺失则服务器空转）', () => {
    const fs = FEATURED_CONNECTORS.find((t) => t.name === 'filesystem');
    expect(fs?.args?.[fs.args.length - 1]).toBe('%USERPROFILE%');
  });
});

describe('isCredentialKey', () => {
  it('与后端 is_credential_key 同规则：令牌语义字段按凭证处理', () => {
    expect(isCredentialKey('GITHUB_PERSONAL_ACCESS_TOKEN')).toBe(true);
    expect(isCredentialKey('client_secret')).toBe(true);
    expect(isCredentialKey('Authorization')).toBe(true);
    expect(isCredentialKey('MODEL')).toBe(false);
  });
});

describe('importResultRows', () => {
  it('逐项映射结果；只有 failed 且带规范化定义的才允许重试', () => {
    const results: McpImportItemResult[] = [
      { name: 'ok', status: 'imported', message: null, credentialKeys: ['API_KEY'], server: null },
      {
        name: 'sse',
        status: 'skipped',
        message: '仅支持 stdio/http',
        credentialKeys: [],
        server: null,
      },
      {
        name: 'retryable',
        status: 'failed',
        message: '写入失败',
        credentialKeys: [],
        server: {
          name: 'retryable',
          command: 'npx',
          args: [],
          env: {},
          transport: 'stdio',
          enabled: true,
          status: 'disconnected',
        },
      },
      { name: 'fatal', status: 'failed', message: '缺 command', credentialKeys: [], server: null },
    ];
    const rows = importResultRows(results);
    expect(rows.map((row) => row.status)).toEqual(['imported', 'skipped', 'failed', 'failed']);
    expect(rows.map((row) => row.canRetry)).toEqual([false, false, true, false]);
    expect(rows[0].credentialKeys).toEqual(['API_KEY']);
  });
});

describe('triggerText 触发词展示', () => {
  it('插件命名空间技能去掉命名空间段', () => {
    expect(triggerText('/anthropic-agent-skills:algorithmic-art', 'claude-code')).toBe(
      '/algorithmic-art',
    );
  });

  it('普通技能与 Codex 前缀保持不变', () => {
    expect(triggerText('/plan', 'claude-code')).toBe('/plan');
    expect(triggerText('setup', 'codex')).toBe('$setup');
  });

  it('空触发词返回空串', () => {
    expect(triggerText('', 'claude-code')).toBe('');
    expect(triggerText('/', 'codex')).toBe('');
  });
});

describe('normalizeWindowsMcpLaunch Windows 启动规范化', () => {
  const base: McpServer = {
    name: 'demo',
    command: 'npx',
    args: ['-y', '@playwright/mcp@latest'],
    env: {},
    transport: 'stdio',
    enabled: true,
    status: 'disconnected',
  };

  it('裸 npx 包一层 cmd /c，参数保留', () => {
    const out = normalizeWindowsMcpLaunch(base);
    expect(out.command).toBe('cmd');
    expect(out.args).toEqual(['/c', 'npx', '-y', '@playwright/mcp@latest']);
  });

  it('npx.cmd 同样规范化，已是 cmd 的不重复包', () => {
    const wrapped = normalizeWindowsMcpLaunch({ ...base, command: 'cmd', args: ['/c', 'npx'] });
    expect(wrapped.command).toBe('cmd');
    const cmdForm = normalizeWindowsMcpLaunch({ ...base, command: 'NPX.CMD' });
    expect(cmdForm.args[0]).toBe('/c');
    expect(cmdForm.args[1]).toBe('NPX.CMD');
  });

  it('http 远程与自定义可执行不动', () => {
    const http = normalizeWindowsMcpLaunch({
      ...base,
      command: 'https://mcp.context7.com/mcp',
      transport: 'http',
    });
    expect(http.command).toBe('https://mcp.context7.com/mcp');
    const node = normalizeWindowsMcpLaunch({ ...base, command: 'node', args: ['server.js'] });
    expect(node.command).toBe('node');
  });

  it('sse 传输的本地进程形态也规范化（http 之外的进程型传输）', () => {
    const sseLocal = normalizeWindowsMcpLaunch({ ...base, transport: 'sse' });
    expect(sseLocal.command).toBe('cmd');
    expect(sseLocal.args[0]).toBe('/c');
    const sseRemote = normalizeWindowsMcpLaunch({
      ...base,
      command: 'https://example.com/sse',
      transport: 'sse',
    });
    expect(sseRemote.command).toBe('https://example.com/sse');
  });
});
