// 插件页视图模型（S7）：技能分组/触发词/标识推导、连接器状态药丸与精选目录状态推导。
// 纯函数层：不做 IO，不持有 mock 状态；精选目录只是真实安装模板，安装走 save_mcp_server。
import type { IconName } from '../shell/icons';
import type { McpImportItemResult, McpServer, Skill } from './extensionsApi';

export type SkillEngine = Skill['engine'];
export type SkillScopeValue = Skill['scope'];

// ==================== 技能 ====================

export type SkillSectionId = 'builtin' | 'external' | 'custom';

/** 技能卡品牌块图标：所有技能统一按名称语义映射同一套线性图标（2026-08-27 反馈：风格要统一、
 *  但不要所有技能一个图标）；名称无关键词命中时再按来源兜底。 */
export function skillCardIcon(skill: Pick<Skill, 'source' | 'name'>): IconName {
  const semantic = semanticSkillIcon(skill.name);
  if (semantic) return semantic;
  if (skill.source === 'builtin') return 'zap';
  if (skill.source === 'market') return 'store';
  if (skill.source === 'plugin') return 'puzzle';
  return 'packagecheck';
}

function semanticSkillIcon(name: string): IconName | null {
  const n = name.toLowerCase();
  if (/plan|todo|任务|计划/.test(n)) return 'listtodo';
  if (/review|审查|审计/.test(n)) return 'filetext';
  if (/doc|pdf|文档|写作/.test(n)) return 'book';
  if (/search|搜索|查找|检索/.test(n)) return 'search';
  if (/test|测试|验证/.test(n)) return 'zap';
  if (/data|数据|数据库|sql/.test(n)) return 'database';
  if (/art|设计|design|界面|ui|前端|frontend/.test(n)) return 'layouttemplate';
  if (/api|接口|请求|http/.test(n)) return 'plug';
  if (/sec|安全|加密|密|auth|鉴权/.test(n)) return 'shield';
  if (/deploy|发布|构建|build|ci/.test(n)) return 'packagecheck';
  if (/git|commit|分支/.test(n)) return 'gitbranch';
  if (/doc|文档/.test(n)) return 'book';
  if (/write|写作|文案|blog|文章/.test(n)) return 'filetext';
  if (/code|编码|重构|refactor/.test(n)) return 'terminal';
  return null;
}

/** 市场行图标：按技能名关键词映射（对齐原型 skillMarket 行图形）。 */
export function marketRowIcon(name: string): IconName {
  const n = name.toLowerCase();
  if (/front|design|layout|ui|界面|设计/.test(n)) return 'layouttemplate';
  if (/doc|pdf|document|文档/.test(n)) return 'filetext';
  if (/database|data|sql|数据库/.test(n)) return 'database';
  if (/lib|库|框架/.test(n)) return 'library';
  return 'store';
}

export interface SkillSection {
  id: SkillSectionId;
  title: string;
  hint: string;
  skills: Skill[];
}

/** 按来源分组：内置只读在前，其次外部安装（市场/插件），最后自己创建。 */
export function groupSkillsBySource(skills: Skill[]): SkillSection[] {
  const sections: SkillSection[] = [
    { id: 'builtin', title: '内置', hint: '由当前执行引擎提供，只读。', skills: [] },
    { id: 'external', title: '外部安装', hint: '从外部来源安装到当前执行引擎。', skills: [] },
    { id: 'custom', title: '自己创建', hint: '保存在当前执行引擎的技能目录。', skills: [] },
  ];
  for (const skill of skills) {
    if (skill.source === 'builtin') sections[0].skills.push(skill);
    else if (skill.source === 'market' || skill.source === 'plugin') sections[1].skills.push(skill);
    else sections[2].skills.push(skill);
  }
  return sections.filter((section) => section.skills.length > 0);
}

/** 触发词跟随引擎：Claude Code 用 /name，Codex 用 $name。 */
/** 触发词展示：Claude Code 用 /name，Codex 用 $name。
 *  插件市场技能的原始触发带命名空间（/anthropic-agent-skills:algorithmic-art），
 *  展示按 2026-08-27 反馈去掉命名空间段（/algorithmic-art）；无触发词返回空串。 */
export function triggerText(rawTrigger: string, engine: SkillEngine): string {
  const body = rawTrigger.replace(/^[/$]/, '').split(':').pop()!.trim();
  if (!body) return '';
  return engine === 'codex' ? `$${body}` : `/${body}`;
}

/** 名称转目录标识：小写字母/数字/短横线，其余折叠为连字符；纯中文等无法成名的输入返回空串。 */
export function slugifySkillName(value: string): string {
  return value
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+|-+$/g, '');
}

/** 范围说明文案：与后端 create_skill 的落盘位置一致（括注另一引擎路径，对齐原型）。 */
export function skillScopeNote(engine: SkillEngine, scope: SkillScopeValue): string {
  const home = '%USERPROFILE%';
  if (scope === 'project') {
    return engine === 'codex'
      ? '当前项目写入 <工作目录>\\.codex\\skills'
      : '当前项目写入 <工作目录>\\.claude\\skills（Codex 为 .codex\\skills）';
  }
  return engine === 'codex'
    ? `全局写入 ${home}\\.codex\\skills`
    : `全局写入 ${home}\\.claude\\skills（Codex 为 ${home}\\.codex\\skills）`;
}

export function filterSkillsByQuery(skills: Skill[], query: string): Skill[] {
  const keyword = query.trim().toLowerCase();
  if (!keyword) return skills;
  return skills.filter((skill) =>
    `${skill.name} ${skill.id} ${skill.description}`.toLowerCase().includes(keyword),
  );
}

// ==================== 连接器 ====================

export interface ConnectorStatusPill {
  label: string;
  tone: 'ok' | 'error' | 'muted';
}

/** Windows 启动规范化：npx/npm/pnpm 等在 Windows 上是 .cmd 脚本，后端直接 spawn
 *  只认 .exe（报 program not found）；统一包一层 cmd /c（对齐 Claude Code 官方
 *  Windows 指引）。http/sse 远程连接器不经 spawn，不受影响。
 *  保存与检测共用，保证旧配置里存的裸 npx 也能测通。 */
export function normalizeWindowsMcpLaunch<T extends McpServer>(server: T): T {
  const command = server.command.trim();
  // 本地进程型传输（stdio/sse 走 Command::new 拉起进程）才需要包装；
  // sse 常见是远程 URL，command 形如 http(s):// 自然不匹配正则
  if (server.transport !== 'http' && /^(npx|npm|pnpm|yarn|bun|deno)(\.cmd)?$/i.test(command)) {
    return { ...server, command: 'cmd', args: ['/c', command, ...server.args] };
  }
  return server;
}

/** 状态药丸只用真实事实：最近一次测试结果与持久化状态，不推测。 */
export function connectorStatusPill(
  server: Pick<McpServer, 'status' | 'lastTestedAt'>,
): ConnectorStatusPill {
  if (server.status === 'connected') return { label: '可用', tone: 'ok' };
  if (server.status === 'error') return { label: '连接失败', tone: 'error' };
  return { label: '未检测', tone: 'muted' };
}

export function transportLabel(transport: McpServer['transport']): string {
  if (transport === 'stdio') return '本地进程';
  if (transport === 'http') return '远程地址';
  return '远程地址（旧 SSE）';
}

/** 最近检测时间：今天显示时刻，昨天显示「昨天」，更早显示日期；无记录返回空串。 */
export function formatTestedAt(
  testedAt: number | null | undefined,
  now: Date = new Date(),
): string {
  if (!testedAt) return '';
  const date = new Date(testedAt * 1000);
  const startOfDay = (value: Date) =>
    new Date(value.getFullYear(), value.getMonth(), value.getDate()).getTime();
  const dayDiff = Math.round((startOfDay(now) - startOfDay(date)) / 86_400_000);
  const hhmm = `${String(date.getHours()).padStart(2, '0')}:${String(date.getMinutes()).padStart(2, '0')}`;
  if (dayDiff <= 0) return `检测于今天 ${hhmm}`;
  if (dayDiff === 1) return '检测于昨天';
  return `检测于 ${date.getMonth() + 1}月${date.getDate()}日`;
}

export interface FeaturedConnectorTemplate {
  name: string;
  /** 卡片/抽屉展示名（2026-08-27 决议对齐原型品牌名）；缺省回落 name。匹配真实服务器一律用 name。 */
  displayName?: string;
  transport: 'stdio' | 'http';
  /** 一句话说明（静态内容；运行状态一律来自真实服务器列表） */
  description: string;
  /** stdio 启动命令（可执行文件本身） */
  command?: string;
  args?: string[];
  /** http 服务地址 */
  url?: string;
  /** 安装时需要用户补值的环境变量名；credential=true 的值进系统钥匙串 */
  envKeys?: { key: string; credential: boolean }[];
}

/**
 * 精选目录：官方/知名连接器的真实安装模板。
 * 卡片上的「已启用」等状态永远由已保存的真实连接器列表推导，这里不含任何运行时状态。
 */
/** 顺序按 2026-03 真实流行度（mcp.directory 热度 + mcpfind 星标），见差异清单 §7-R2。 */
export const FEATURED_CONNECTORS: FeaturedConnectorTemplate[] = [
  {
    name: 'context7',
    displayName: 'Context7',
    transport: 'http',
    description: '按版本检索库与框架的最新官方文档。',
    url: 'https://mcp.context7.com/mcp',
  },
  {
    name: 'playwright',
    displayName: 'Playwright',
    transport: 'stdio',
    description: '在真实浏览器中运行网页检查和交互测试。',
    command: 'cmd',
    args: ['/c', 'npx', '-y', '@playwright/mcp@latest'],
  },
  {
    name: 'sequential-thinking',
    displayName: 'Sequential Thinking',
    transport: 'stdio',
    description: '让模型逐步拆解复杂问题，输出结构化推理过程。',
    command: 'cmd',
    args: ['/c', 'npx', '-y', '@modelcontextprotocol/server-sequential-thinking'],
  },
  {
    name: 'github',
    displayName: 'GitHub',
    transport: 'stdio',
    description: '读取仓库、Issue 和 Pull Request，并执行经过授权的协作操作。',
    command: 'cmd',
    args: ['/c', 'npx', '-y', '@modelcontextprotocol/server-github'],
    envKeys: [{ key: 'GITHUB_PERSONAL_ACCESS_TOKEN', credential: true }],
  },
  {
    name: 'chrome-devtools',
    displayName: 'Chrome DevTools',
    transport: 'stdio',
    description: '调试页面性能、网络请求和运行时状态。',
    command: 'cmd',
    args: ['/c', 'npx', '-y', 'chrome-devtools-mcp@latest'],
  },
  {
    name: 'filesystem',
    displayName: 'Filesystem',
    transport: 'stdio',
    description: '在授权目录内读写与检索本地文件。',
    command: 'cmd',
    // 官方要求至少一个允许目录参数（缺失时服务器空转等待 roots 协议）；
    // %USERPROFILE% 由 cmd 展开为用户主目录（2026-08-27 实测握手 14 个工具通过）
    args: ['/c', 'npx', '-y', '@modelcontextprotocol/server-filesystem', '%USERPROFILE%'],
  },
];

export interface FeaturedCardState {
  template: FeaturedConnectorTemplate;
  installed: boolean;
  enabled: boolean;
}

/** 精选卡状态：按名字匹配真实连接器列表；找不到就是未安装。 */
export function deriveFeaturedStates(
  templates: FeaturedConnectorTemplate[],
  servers: McpServer[],
): FeaturedCardState[] {
  const byName = new Map(servers.map((server) => [server.name.toLowerCase(), server]));
  return templates.map((template) => {
    const existing = byName.get(template.name.toLowerCase());
    return {
      template,
      installed: Boolean(existing),
      enabled: Boolean(existing?.enabled),
    };
  });
}

/** 精选卡搜索：按模板名或说明过滤（状态仍来自真实服务器列表）。 */
export function filterFeaturedStates(
  states: FeaturedCardState[],
  query: string,
): FeaturedCardState[] {
  const keyword = query.trim().toLowerCase();
  if (!keyword) return states;
  return states.filter((state) =>
    `${state.template.name} ${state.template.displayName ?? ''} ${state.template.description}`
      .toLowerCase()
      .includes(keyword),
  );
}

/** 凭证类字段名判定（与后端 is_credential_key 同规则）：用于把值输入渲染为密码框。 */
export function isCredentialKey(key: string): boolean {
  const upper = key.toUpperCase();
  const markers = [
    'TOKEN',
    'SECRET',
    'PASSWORD',
    'PASSWD',
    'API_KEY',
    'APIKEY',
    'API-KEY',
    'CREDENTIAL',
    'AUTHORIZATION',
  ];
  return markers.some((marker) => upper.includes(marker));
}

export interface ImportResultRow {
  name: string;
  status: McpImportItemResult['status'];
  message: string;
  credentialKeys: string[];
  canRetry: boolean;
}

/** 导入逐项结果行：failed 且带规范化定义的才允许重试（确定性错误重试没有意义）。 */
export function importResultRows(results: McpImportItemResult[]): ImportResultRow[] {
  return results.map((result) => ({
    name: result.name,
    status: result.status,
    message: result.message ?? '',
    credentialKeys: result.credentialKeys ?? [],
    canRetry: result.status === 'failed' && Boolean(result.server),
  }));
}
