import { mountApp } from './renderApp';

const now = Date.now();
const sessions = Array.from({ length: 4 }, (_, index) => ({
  id: `session-${index + 1}`,
  cliSessionId: `cli-${index + 1}`,
  title: ['修复鉴权令牌刷新', '在设置中添加深色模式', '重构 ETL 聚合阶段', '更新 README 快速开始'][
    index
  ],
  engine: index === 2 ? 'codex' : 'claude-code',
  model: index === 2 ? 'gpt-5-codex' : 'claude-sonnet-4-6',
  cwd: index === 2 ? 'data-pipeline' : 'acme-web',
  status: index < 2 ? 'active' : 'done',
  messageCount: 8 + index,
  inputTokens: 1000,
  outputTokens: 500,
  costUsd: 0.5,
  createdAt: now - (index + 1) * 3_600_000,
  updatedAt: now - (index + 1) * 3_600_000,
  folderId: index === 2 ? 'folder-data' : index === 3 ? 'folder-default' : 'folder-acme',
}));

const activeSession = {
  ...sessions[0],
  fork: {
    id: 'fork-visual-audit',
    handoffId: 'handoff-visual-audit',
    sourceSessionId: 'session-2',
    sourceTitleSnapshot: '在设置中添加深色模式',
    sourceEngine: 'codex',
    targetEngine: 'claude-code',
    boundaryTurnId: 'turn-source-3',
    boundaryTurnEpoch: 3,
    createdAt: now - 24_000,
  },
  messages: [
    { role: 'user', text: '修复令牌刷新并运行相关测试', ts: now - 20_000, turnId: 'turn-1' },
    {
      role: 'assistant',
      text: '我先核对刷新逻辑与现有测试，再提交最小修改。',
      ts: now - 17_000,
      turnId: 'turn-1',
    },
    {
      role: 'assistant',
      text: '刷新重试已修复，相关测试全部通过。',
      ts: now - 8_000,
      turnId: 'turn-1',
    },
  ],
  toolCalls: [
    {
      id: 'read-1',
      name: 'Read',
      input: { path: 'src/auth/token.ts' },
      status: 'success',
      output: '86 lines',
      ts: now - 19_000,
      endedAt: now - 18_500,
      turnId: 'turn-1',
    },
    {
      id: 'grep-1',
      name: 'Grep',
      input: { pattern: 'refreshToken' },
      status: 'success',
      output: '3 matches',
      ts: now - 18_400,
      endedAt: now - 18_000,
      turnId: 'turn-1',
    },
    {
      id: 'edit-1',
      name: 'Edit',
      input: { path: 'src/auth/token.ts' },
      status: 'success',
      output: 'updated',
      diff: {
        path: 'src/auth/token.ts',
        hunks: [
          {
            oldStart: 42,
            newStart: 42,
            lines: [
              { kind: 'del', text: 'return refresh(token)' },
              { kind: 'add', text: 'return withRetry(() => refresh(token))' },
            ],
          },
        ],
      },
      ts: now - 16_000,
      endedAt: now - 14_000,
      turnId: 'turn-1',
    },
    {
      id: 'bash-1',
      name: 'Bash',
      input: { command: 'npm test -- auth' },
      status: 'success',
      output: 'PASS 3 tests',
      ts: now - 13_000,
      endedAt: now - 9_000,
      turnId: 'turn-1',
    },
    {
      // S9 §5 待办：失败工具的就地呈现锚点——独立失败轮（终态失败 → 过程区默认展开），
      // 非子代理失败工具由 ToolBlock 提成 [data-kind="fail"] > .failc 就地展开卡，
      // 供视觉矩阵断言「工具就地折叠（turn-1 收起）+ 失败工具就地展开（turn-2）」。
      id: 'grep-2',
      name: 'Grep',
      input: { pattern: 'withRetry', path: 'docs' },
      status: 'error',
      output: 'EACCES: permission denied, open docs/README.md',
      ts: now - 7_000,
      endedAt: now - 6_500,
      turnId: 'turn-2',
    },
  ],
  checkpoints: [],
  approvals: [],
  turns: [
    {
      id: 'turn-1',
      epoch: 1,
      mode: 'build',
      permissionProfile: 'standard',
      status: 'succeeded',
      startedAt: now - 20_000,
      endedAt: now - 8_000,
    },
    {
      id: 'turn-2',
      epoch: 2,
      mode: 'build',
      permissionProfile: 'standard',
      status: 'failed',
      startedAt: now - 7_500,
      endedAt: now - 6_000,
    },
  ],
};

const fixtures: Record<string, unknown> = {
  load_app_settings: {
    general: {
      defaultDirectory: 'C:\\code\\helm',
      reopenLastSession: true,
      autoUpdateChannel: 'stable',
      updateFeedUrl: '',
      pricingAutoUpdate: true,
      pricingFeedUrls: [],
      pricingUnknownPolicy: 'warn',
      pricingMaxAgeDays: 30,
      autoTitleSessions: true,
      generativeUi: false,
    },
    engines: {
      defaultEngine: 'claude-code',
      claudeCode: {
        executablePath: 'claude',
        version: '2.1.206',
        detected: true,
        permissionMode: 'ask',
      },
      codex: { executablePath: 'codex', version: '0.144.1', detected: true },
    },
    permissions: {},
    appearance: {
      theme: 'light',
      accentColor: { base: 'oklch(52% 0.12 230)', hi: 'oklch(46% 0.13 230)' },
    },
    shortcuts: {
      commandPalette: 'Ctrl+K',
      newSession: 'Ctrl+N',
      toggleContext: 'Ctrl+.',
      cycleEngine: 'Ctrl+E',
      navigationPrefix: 'G',
      home: 'H',
      workspace: 'W',
      providers: 'P',
      sessions: 'S',
      extensions: 'E',
      usage: 'U',
      settings: ',',
    },
  },
  // S2 新任务页就绪契约（ReadinessReport / WorkspaceDeps）——矩阵需真实渲染该页
  get_readiness_report: {
    claudeCode: {
      installed: true,
      path: 'C:\\Users\\demo\\AppData\\Roaming\\npm\\claude.cmd',
      version: '2.1.206',
      error: null,
      login: { state: 'ok', detail: '订阅登录有效' },
    },
    codex: {
      installed: true,
      path: 'C:\\Users\\demo\\AppData\\Roaming\\npm\\codex.cmd',
      version: '0.144.1',
      error: null,
      login: { state: 'ok', detail: '订阅登录有效' },
    },
    hasProvider: true,
    hasReadyProvider: true,
    defaultEngine: 'claude-code',
    boundEngines: ['claude-code', 'codex'],
    cwd: { configured: true, exists: true, path: 'C:\\code\\helm' },
  },
  detect_workspace_deps: {
    node: { available: true, version: 'v22.14.0' },
    npm: { available: true, version: '10.9.2' },
    git: { available: true, version: 'git version 2.47.1.windows.1' },
  },
  // —— 新任务页状态矩阵（docs/可靠性检查-新任务页-状态矩阵-2026-08-23.md）：此前
  // 缺失的六个命令在视觉入口会兜底返回 null，@文件中心会因此拿到 null 崩溃。
  search_workspace_files: (payload: Record<string, unknown>) => {
    const query = String(payload.query || '')
      .trim()
      .toLocaleLowerCase('zh-CN');
    const tree = [
      'src/App.tsx',
      'src/home/NewTaskPage.tsx',
      'src/workspace/',
      'src/workspace/Composer.tsx',
      'docs/PRD.md',
      'docs/技术方案.md',
      'package.json',
    ];
    return query ? tree.filter((item) => item.toLocaleLowerCase('zh-CN').includes(query)) : tree;
  },
  select_directory: 'C:\\code\\demo-project',
  install_node: async () => ({
    path: 'C:\\Program Files\\nodejs\\node.exe',
    version: 'v22.14.0',
    restartRequired: false,
  }),
  install_git: async () => ({
    path: 'C:\\Program Files\\Git\\cmd\\git.exe',
    version: 'git version 2.47.1.windows.1',
    restartRequired: false,
  }),
  install_cli_engine: async () => ({
    path: 'C:\\Users\\demo\\AppData\\Roaming\\npm\\claude.cmd',
    version: '2.1.206',
    output: 'added 1 package (fixture)',
  }),
  get_reasoning_effort_capability: {
    support: 'supported',
    options: ['auto', 'low', 'medium', 'high'],
    source: 'builtin-catalog',
  },
  get_provider_config: {
    providers: [
      {
        id: 'anthropic',
        name: 'Anthropic',
        kind: 'api',
        baseUrl: 'https://api.anthropic.com',
        ready: true,
        protocol: 'anthropic',
        authMethod: 'apikey',
        lastTest: { result: 'ok', at: 1 },
      },
      {
        id: 'openai',
        name: 'OpenAI',
        kind: 'api',
        baseUrl: 'https://api.openai.com',
        ready: true,
        protocol: 'openai-responses',
        authMethod: 'apikey',
        lastTest: { result: 'ok', at: 1 },
      },
      {
        id: 'openrouter',
        name: 'OpenRouter',
        kind: 'api',
        baseUrl: 'https://openrouter.ai/api',
        ready: true,
        protocol: 'openai-chat',
        authMethod: 'apikey',
        lastTest: { result: 'ok', at: 1 },
      },
    ],
    models: [
      {
        id: 'claude-sonnet-4-6',
        providerId: 'anthropic',
        displayName: 'Claude Sonnet 4.6',
        inputPricePerMtok: 3,
        outputPricePerMtok: 15,
        priceSource: 'builtin',
        enabled: true,
      },
      {
        id: 'claude-haiku-4-5',
        providerId: 'anthropic',
        displayName: 'Claude Haiku 4.5',
        inputPricePerMtok: 1,
        outputPricePerMtok: 5,
        priceSource: 'builtin',
        enabled: true,
      },
      {
        id: 'gpt-5-codex',
        providerId: 'openai',
        displayName: 'GPT-5 Codex',
        inputPricePerMtok: 2,
        outputPricePerMtok: 10,
        priceSource: 'builtin',
        enabled: true,
      },
      {
        id: 'gpt-5-mini',
        providerId: 'openai',
        displayName: 'GPT-5 mini',
        inputPricePerMtok: 0.5,
        outputPricePerMtok: 2,
        priceSource: 'builtin',
        enabled: true,
      },
    ],
    engines: [
      {
        id: 'claude-code',
        name: 'Claude Code',
        bin: 'claude',
        defaultModel: 'claude-sonnet-4-6',
        status: 'ready',
        version: '2.1.206',
      },
      {
        id: 'codex',
        name: 'Codex',
        bin: 'codex',
        defaultModel: 'gpt-5-codex',
        status: 'ready',
        version: '0.144.1',
      },
    ],
    bindings: [
      {
        engineId: 'claude-code',
        providerId: 'anthropic',
        primaryModel: 'claude-sonnet-4-6',
        fastModel: 'claude-haiku-4-5',
        reasoningEffort: 'auto',
      },
      {
        engineId: 'codex',
        providerId: 'openai',
        primaryModel: 'gpt-5-codex',
        fastModel: 'gpt-5-mini',
        reasoningEffort: 'auto',
      },
    ],
    defaultEngine: 'claude-code',
    defaultModel: 'claude-sonnet-4-6',
  },
  list_sessions: sessions,
  list_folders: [
    {
      id: 'folder-default',
      name: '默认',
      sortOrder: 0,
      collapsed: false,
      locked: true,
      createdAt: 1,
    },
    {
      id: 'folder-acme',
      name: 'acme-web',
      sortOrder: 1,
      collapsed: false,
      locked: false,
      createdAt: 2,
    },
    {
      id: 'folder-data',
      name: '数据管线',
      sortOrder: 2,
      collapsed: false,
      locked: false,
      createdAt: 3,
    },
  ],
  get_active_session: activeSession,
  get_session_history: activeSession,
  list_session_contexts: [
    {
      id: 'context-guide',
      kind: 'file',
      sourcePath: 'C:\\code\\helm\\docs\\architecture-and-runtime-context-verification-guide.md',
      canonicalPath: 'C:\\code\\helm\\docs\\architecture-and-runtime-context-verification-guide.md',
      displayName: 'architecture-and-runtime-context-verification-guide.md',
      status: 'ready',
      statusDetail: null,
      createdAt: now - 60_000,
      updatedAt: now - 60_000,
    },
    {
      id: 'context-missing',
      kind: 'directory',
      sourcePath: 'C:\\code\\helm\\examples\\removed',
      canonicalPath: 'C:\\code\\helm\\examples\\removed',
      displayName: 'removed',
      status: 'missing',
      statusDetail: '会话上下文路径不存在或已被移动',
      createdAt: now - 30_000,
      updatedAt: now - 10_000,
    },
  ],
  add_session_context: null,
  remove_session_context: null,
  resume_session: 'visual-handle',
  get_budget: {
    monthly_limit: 200,
    alert_at_80: true,
    stop_at_100: true,
    current_month_cost: 112.4,
    percentage: 56.2,
  },
  get_usage_stats: {
    total_cost: 112.4,
    total_tokens: 2_460_000,
    input_tokens: 1_820_000,
    output_tokens: 640_000,
    cached_input_tokens: 830_000,
    cache_write_input_tokens: 120_000,
    request_count: 186,
    session_count: 24,
    actual_cost: 98.2,
    estimated_cost: 14.2,
    subscription_count: 2,
    unknown_count: 0,
    legacy_cost: 0,
    legacy_count: 0,
  },
  // S4 冻结契约：统一 get_usage_breakdown(days, dimension)，行内含缓存分子/分母与成本类型
  get_usage_breakdown: (payload: Record<string, unknown>) => {
    const kinds = { actual: 60, estimated: 20, subscription: 2, unknown: 0, legacy: 0 };
    if (String(payload.dimension || 'model') === 'engine') {
      return [
        {
          key: 'claude-code',
          engine: 'claude-code',
          request_count: 124,
          input_tokens: 1_170_000,
          output_tokens: 410_000,
          cached_input_tokens: 520_000,
          cache_write_input_tokens: 90_000,
          cost_usd: 74.2,
          share: 0.66,
          cost_kinds: kinds,
        },
        {
          key: 'codex',
          engine: 'codex',
          request_count: 62,
          input_tokens: 650_000,
          output_tokens: 230_000,
          cached_input_tokens: 310_000,
          cache_write_input_tokens: 30_000,
          cost_usd: 38.2,
          share: 0.34,
          cost_kinds: kinds,
        },
      ];
    }
    if (String(payload.dimension || 'model') === 'provider') {
      return [
        {
          key: 'anthropic',
          engine: 'claude-code',
          request_count: 124,
          input_tokens: 1_170_000,
          output_tokens: 410_000,
          cached_input_tokens: 520_000,
          cache_write_input_tokens: 90_000,
          cost_usd: 74.2,
          share: 0.66,
          cost_kinds: kinds,
        },
        {
          key: 'openai',
          engine: 'codex',
          request_count: 62,
          input_tokens: 650_000,
          output_tokens: 230_000,
          cached_input_tokens: 310_000,
          cache_write_input_tokens: 30_000,
          cost_usd: 38.2,
          share: 0.34,
          cost_kinds: kinds,
        },
      ];
    }
    return [
      {
        key: 'claude-sonnet-4-6',
        engine: 'claude-code',
        request_count: 96,
        input_tokens: 900_000,
        output_tokens: 310_000,
        cached_input_tokens: 400_000,
        cache_write_input_tokens: 70_000,
        cost_usd: 58.4,
        share: 0.52,
        cost_kinds: kinds,
      },
      {
        key: 'gpt-5-codex',
        engine: 'codex',
        request_count: 62,
        input_tokens: 650_000,
        output_tokens: 230_000,
        cached_input_tokens: 310_000,
        cache_write_input_tokens: 30_000,
        cost_usd: 38.2,
        share: 0.34,
        cost_kinds: kinds,
      },
      {
        key: 'claude-haiku-4-5',
        engine: 'claude-code',
        request_count: 28,
        input_tokens: 270_000,
        output_tokens: 100_000,
        cached_input_tokens: null,
        cache_write_input_tokens: null,
        cost_usd: 15.8,
        share: 0.14,
        cost_kinds: { ...kinds, actual: 0, legacy: 28 },
      },
    ];
  },
  get_daily_usage: (payload: Record<string, unknown>) => {
    const days = Number(payload.days || 30);
    return Array.from({ length: days }, (_, index) => {
      const legacyOnly = index % 11 === 5;
      return {
        date: new Date(now - (days - 1 - index) * 86_400_000).toISOString().slice(0, 10),
        cost_usd: 1.8 + (index % 7) * 0.7,
        request_count: legacyOnly ? 3 : 12 + (index % 5),
        input_tokens: legacyOnly ? null : 40_000 + (index % 9) * 6_000,
        output_tokens: legacyOnly ? null : 12_000 + (index % 4) * 2_000,
        cached_input_tokens: legacyOnly ? null : 18_000 + (index % 6) * 3_000,
        cache_write_input_tokens: legacyOnly ? null : 4_000,
      };
    });
  },
  get_top_sessions: sessions.slice(0, 3).map((session, index) => ({
    id: session.id,
    title: session.title,
    model: session.model,
    engine: session.engine,
    cost_usd: 12.4 - index * 2.1,
    total_tokens: 180_000 - index * 30_000,
  })),
  list_skills: (payload: Record<string, unknown>) =>
    [
      {
        id: 'frontend-skill',
        name: 'Frontend Skill',
        description: '构建并审查产品界面。',
        scope: 'global',
        source: 'custom',
        enabled: true,
        path: 'C:\\skills\\frontend',
        engine: 'claude-code',
        trigger: '/frontend-skill',
      },
      {
        id: 'review',
        name: 'Code Review',
        description: '执行结构化代码审查。',
        scope: 'project',
        source: 'custom',
        enabled: true,
        path: 'C:\\code\\helm\\.codex\\skills\\review',
        engine: 'codex',
        trigger: '$review',
      },
    ].filter((skill) => skill.engine === payload.engine),
  list_mcp_servers: [
    {
      name: 'filesystem',
      command: 'npx',
      args: ['-y', '@modelcontextprotocol/server-filesystem'],
      env: {},
      transport: 'stdio',
      enabled: true,
      status: 'connected',
      toolCount: 8,
      lastTestedAt: Math.floor(now / 1000),
    },
  ],
  // —— 插件页状态矩阵（docs/可靠性检查-插件页-状态矩阵-2026-08-24.md）：
  // 扩展页动作命令的真实形状；状态变体见下方 extFixtureVariants。
  toggle_skill: null,
  create_skill: (payload: Record<string, unknown>) => {
    const request = payload.request as Record<string, unknown>;
    const id = String(request.id);
    return {
      id,
      name: String(request.name),
      description: String(request.description ?? ''),
      scope: request.scope,
      source: 'custom',
      enabled: true,
      path: `C:\\skills\\${id}`,
      engine: request.engine,
      trigger: request.engine === 'codex' ? `$${id}` : `/${id}`,
    };
  },
  read_skill_source: (payload: Record<string, unknown>) => ({
    path: `C:\\skills\\${String(payload.skillId)}\\SKILL.md`,
    content: [
      '# Fixture 技能',
      '',
      '用于视觉探针的示例 SKILL.md。',
      '',
      '- 步骤一：核对输入',
      '- 步骤二：执行检查',
      '',
      '## 小节',
      '',
      '正文段落，验证预览行级渲染。',
    ].join('\n'),
    truncated: false,
  }),
  delete_skill: null,
  market_search_skills: (payload: Record<string, unknown>) => {
    const query = String(payload.query || '')
      .trim()
      .toLowerCase();
    const catalog = [
      {
        skillId: 'frontend-design',
        name: 'frontend-design',
        source: 'anthropics/skills',
        installs: 124000,
      },
      {
        skillId: 'document-skills',
        name: 'document-skills',
        source: 'anthropics/skills',
        installs: 81000,
      },
      {
        skillId: 'database-design',
        name: 'database-design',
        source: 'community/skills',
        installs: 32000,
      },
    ];
    return query
      ? catalog.filter((item) => `${item.name} ${item.source}`.toLowerCase().includes(query))
      : catalog;
  },
  market_install_skill: null,
  test_mcp_connection: [
    { name: 'get_file_contents', description: '读取仓库文件内容' },
    { name: 'list_pull_requests', description: '列出 Pull Request' },
    { name: 'create_issue', description: '' },
  ],
  save_mcp_server: (payload: Record<string, unknown>) => {
    // 记录保存调用供探针断言真实写入路径（仅视觉入口环境）。
    const w = window as unknown as { __savedServers?: unknown[] };
    w.__savedServers = [...(w.__savedServers ?? []), payload.server];
    return null;
  },
  set_mcp_server_enabled: null,
  delete_mcp_server: null,
  open_path_in_system: null,
  import_mcp_servers: [
    {
      name: 'notion',
      status: 'imported',
      message: null,
      credentialKeys: ['NOTION_API_KEY'],
      server: null,
    },
    {
      name: 'legacy',
      status: 'skipped',
      message: '不支持 type=sse，已跳过（产品面仅 stdio / http）',
      credentialKeys: [],
      server: null,
    },
    {
      name: 'broken',
      status: 'failed',
      message: '缺少 command 或 url',
      credentialKeys: [],
      server: {
        name: 'broken',
        command: 'npx',
        args: ['-y', '@demo/broken'],
        env: {},
        transport: 'stdio',
        enabled: true,
        status: 'disconnected',
      },
    },
  ],
  list_subagents: [
    {
      id: 'reviewer',
      name: '审查代理',
      model: 'claude-sonnet-4-6',
      role: '检查风险与回归',
      tools: 'Read,Grep',
      auto: true,
      prompt: '审查当前变更。',
      scope: 'project',
    },
  ],
  list_slash_commands: [
    {
      id: 'status',
      trigger: '/status',
      description: '查看当前会话状态',
      scope: 'global',
      enabled: true,
      body: '',
      engine: 'all',
      source: 'builtin',
    },
  ],
  list_hooks: [
    {
      id: 'format',
      event: 'PostToolUse',
      match: 'Write',
      command: 'npm run format',
      description: '写入后格式化',
      enabled: true,
      scope: 'project',
    },
  ],
  get_pricing_catalog_status: {
    source: 'builtin',
    catalogVersion: '2026-07-17',
    sequence: 1,
    publishedAt: '2026-07-17',
    lastCheckedAt: now,
    lastError: null,
    stale: false,
  },
  list_model_price_overrides: [],
  read_engine_config_file: (payload: Record<string, unknown>) => ({
    path:
      payload.engineId === 'codex'
        ? 'C:\\Users\\demo\\.codex\\config.toml'
        : 'C:\\Users\\demo\\.claude\\settings.json',
    content:
      payload.engineId === 'codex' ? 'model = "gpt-5-codex"' : '{"model":"claude-sonnet-4-6"}',
  }),
  get_equivalent_env: [],
  get_permission_rules: [],
  save_app_settings: null,
  get_update_status: { currentVersion: '0.1.0', channel: 'stable', canCheck: false, message: '' },
};

type VisualWindow = Window & {
  __TAURI_INTERNALS__?: Record<string, unknown>;
  __TAURI_EVENT_PLUGIN_INTERNALS__?: Record<string, unknown>;
  /** 状态矩阵探针通道：运行时覆盖单个命令的 fixture（值可为 '__reject' | '__pending' | 函数）。 */
  __setFixture?: (command: string, value: unknown) => void;
};

// —— 新任务页状态矩阵 fixture 变体（docs/可靠性检查-新任务页-状态矩阵-2026-08-23.md §1）——
// URL ?fixture=<name> 挂载即应用预设；探针运行时用 window.__setFixture 动态驱动
// 安装挂起/失败、复检翻面等时序态。仅带查询参数的会话受影响，默认矩阵不变。
const fixtureOverrides: Record<string, unknown> = {};
const vaEngineReadiness = (installed: boolean, npmName: 'claude' | 'codex') => ({
  installed,
  path: installed ? `C:\\Users\\demo\\AppData\\Roaming\\npm\\${npmName}.cmd` : null,
  version: installed ? (npmName === 'claude' ? '2.1.206' : '0.144.1') : null,
  error: null,
  login: installed
    ? { state: 'ok', detail: '订阅登录有效' }
    : { state: 'missing', detail: '未检测到 CLI' },
});
const vaDeps = (gitAvailable: boolean) => ({
  node: { available: true, version: 'v22.14.0' },
  npm: { available: true, version: '10.9.2' },
  git: { available: gitAvailable, version: gitAvailable ? 'git version 2.47.1.windows.1' : null },
});
const homeFixtureVariants: Record<string, Record<string, unknown>> = {
  'home-pending': {
    get_readiness_report: '__pending',
    detect_workspace_deps: '__pending',
    get_provider_config: '__pending',
  },
  'home-reject-report': { get_readiness_report: '__reject' },
  'home-reject-config': { get_provider_config: '__reject' },
  'home-reject-skills': { list_skills: '__reject', list_slash_commands: '__reject' },
  'home-effort-fail': { get_reasoning_effort_capability: '__reject' },
  // R-2：三项全缺（CLI/Git 缺、无绑定、目录未配置）
  'home-r2': {
    get_readiness_report: {
      claudeCode: vaEngineReadiness(false, 'claude'),
      codex: vaEngineReadiness(false, 'codex'),
      hasProvider: false,
      hasReadyProvider: false,
      defaultEngine: 'claude-code',
      boundEngines: [],
      cwd: { configured: false, exists: false, path: '' },
    },
    detect_workspace_deps: vaDeps(false),
  },
  // 用户决议（2026-09）：设置无默认目录 + 就绪报告无 cwd 的「未选工作目录」页——
  // @/菜单直开文件中心（单一弹框），框内提供选择工作目录入口。探针 V 段 D-10 断言用。
  'home-no-dir': {
    // 注意：这里不能引用后面才声明的 vaSettingsWithoutProjectDir（TDZ），
    // 直接从 fixtures 基线克隆并清空默认目录。
    load_app_settings: {
      ...(fixtures.load_app_settings as object),
      general: {
        ...(fixtures.load_app_settings as { general: object }).general,
        defaultDirectory: '',
      },
    },
    get_readiness_report: {
      claudeCode: vaEngineReadiness(true, 'claude'),
      codex: vaEngineReadiness(true, 'codex'),
      hasProvider: true,
      hasReadyProvider: true,
      defaultEngine: 'claude-code',
      boundEngines: ['claude-code', 'codex'],
      cwd: { configured: false, exists: false, path: '' },
    },
  },
  // R-3：仅缺 CLI（Git/服务商/目录就绪）
  'home-r3': {
    get_readiness_report: {
      claudeCode: vaEngineReadiness(false, 'claude'),
      codex: vaEngineReadiness(true, 'codex'),
      hasProvider: true,
      hasReadyProvider: true,
      defaultEngine: 'claude-code',
      boundEngines: ['claude-code', 'codex'],
      cwd: { configured: true, exists: true, path: 'C:\\code\\helm' },
    },
    detect_workspace_deps: vaDeps(true),
  },
  // R-4：仅缺 Git（CLI 就绪、服务商缺失、目录就绪）
  'home-r4': {
    get_readiness_report: {
      claudeCode: vaEngineReadiness(true, 'claude'),
      codex: vaEngineReadiness(true, 'codex'),
      hasProvider: true,
      hasReadyProvider: true,
      defaultEngine: 'claude-code',
      boundEngines: [],
      cwd: { configured: true, exists: true, path: 'C:\\code\\helm' },
    },
    detect_workspace_deps: vaDeps(false),
  },
  // R-6：仅服务商缺失
  'home-r6': {
    get_readiness_report: {
      claudeCode: vaEngineReadiness(true, 'claude'),
      codex: vaEngineReadiness(true, 'codex'),
      hasProvider: true,
      hasReadyProvider: true,
      defaultEngine: 'claude-code',
      boundEngines: [],
      cwd: { configured: true, exists: true, path: 'C:\\code\\helm' },
    },
    detect_workspace_deps: vaDeps(true),
  },
  // R-7：目录未配置
  'home-r7': {
    get_readiness_report: {
      claudeCode: vaEngineReadiness(true, 'claude'),
      codex: vaEngineReadiness(true, 'codex'),
      hasProvider: true,
      hasReadyProvider: true,
      defaultEngine: 'claude-code',
      boundEngines: ['claude-code', 'codex'],
      cwd: { configured: false, exists: false, path: '' },
    },
    detect_workspace_deps: vaDeps(true),
  },
  // R-8：目录不存在
  'home-r8': {
    get_readiness_report: {
      claudeCode: vaEngineReadiness(true, 'claude'),
      codex: vaEngineReadiness(true, 'codex'),
      hasProvider: true,
      hasReadyProvider: true,
      defaultEngine: 'claude-code',
      boundEngines: ['claude-code', 'codex'],
      cwd: { configured: true, exists: false, path: 'C:\\code\\gone-project' },
    },
    detect_workspace_deps: vaDeps(true),
  },
  // M-6：当前引擎无任何服务商/模型 → 模型菜单空态 + 强度仅自动
  'home-models-empty': {
    get_provider_config: {
      providers: [],
      models: [],
      engines: [
        {
          id: 'claude-code',
          name: 'Claude Code',
          bin: 'claude',
          defaultModel: '',
          status: 'ready',
          version: '2.1.206',
        },
        {
          id: 'codex',
          name: 'Codex',
          bin: 'codex',
          defaultModel: '',
          status: 'ready',
          version: '0.144.1',
        },
      ],
      bindings: [],
      defaultEngine: 'claude-code',
      defaultModel: '',
    },
  },
  // 双端实机比对（scripts/home-live-compare.mjs）：全就绪 + 双引擎绑定 +
  // 带上下文窗口的模型目录，对齐原型 index.html 的 mock 数据形状。
  'home-live': {
    get_readiness_report: {
      claudeCode: vaEngineReadiness(true, 'claude'),
      codex: vaEngineReadiness(true, 'codex'),
      hasProvider: true,
      hasReadyProvider: true,
      defaultEngine: 'claude-code',
      boundEngines: ['claude-code', 'codex'],
      cwd: { configured: true, exists: true, path: 'C:\\code\\helm' },
    },
    detect_workspace_deps: vaDeps(true),
    // 文件中心：对齐原型 mock 的行形状（相对路径，目录带尾斜杠）
    search_workspace_files: ['src/workspace/Composer.tsx', 'src/workspace/', 'docs/PRD.md'],
    get_reasoning_effort_capability: {
      support: 'supported',
      options: ['auto', 'low', 'medium', 'high', 'xhigh', 'max'],
      source: 'builtin-catalog',
    },
    get_provider_config: {
      providers: [
        {
          id: 'anthropic-subscription',
          name: 'Claude 订阅',
          kind: 'subscription',
          baseUrl: '',
          keyRef: null,
          ready: true,
          lastTest: null,
          protocol: 'anthropic-messages',
          authMethod: 'oauth',
        },
        {
          id: 'openai-subscription',
          name: 'Codex 订阅',
          kind: 'subscription',
          baseUrl: '',
          keyRef: null,
          ready: true,
          lastTest: null,
          protocol: 'openai-responses',
          authMethod: 'oauth',
        },
      ],
      models: [
        {
          id: 'claude-sonnet-4.6',
          providerId: 'anthropic-subscription',
          displayName: 'Claude Sonnet 4.6',
          inputPricePerMtok: 0,
          outputPricePerMtok: 0,
          enabled: true,
          contextWindow: 200000,
        },
        {
          id: 'claude-opus-4.7',
          providerId: 'anthropic-subscription',
          displayName: 'Claude Opus 4.7',
          inputPricePerMtok: 0,
          outputPricePerMtok: 0,
          enabled: true,
          contextWindow: 200000,
        },
        {
          id: 'claude-haiku-4.5',
          providerId: 'anthropic-subscription',
          displayName: 'Claude Haiku 4.5',
          inputPricePerMtok: 0,
          outputPricePerMtok: 0,
          enabled: true,
          contextWindow: 200000,
        },
        {
          id: 'gpt-5-codex',
          providerId: 'openai-subscription',
          displayName: 'GPT-5 Codex',
          inputPricePerMtok: 0,
          outputPricePerMtok: 0,
          enabled: true,
          contextWindow: 272000,
        },
        {
          id: 'gpt-5',
          providerId: 'openai-subscription',
          displayName: 'GPT-5',
          inputPricePerMtok: 0,
          outputPricePerMtok: 0,
          enabled: true,
          contextWindow: 272000,
        },
      ],
      engines: [
        {
          id: 'claude-code',
          name: 'Claude Code',
          bin: 'claude',
          defaultModel: 'claude-sonnet-4.6',
          status: 'ready',
          version: '2.1.206',
        },
        {
          id: 'codex',
          name: 'Codex',
          bin: 'codex',
          defaultModel: 'gpt-5-codex',
          status: 'ready',
          version: '0.144.1',
        },
      ],
      bindings: [
        {
          engineId: 'claude-code',
          providerId: 'anthropic-subscription',
          primaryModel: 'claude-sonnet-4.6',
        },
        { engineId: 'codex', providerId: 'openai-subscription', primaryModel: 'gpt-5-codex' },
      ],
      defaultEngine: 'claude-code',
      defaultModel: 'claude-sonnet-4.6',
    },
  },
};

// —— 插件页状态矩阵 fixture 变体（docs/可靠性检查-插件页-状态矩阵-2026-08-24.md §1）——
const vaSkill = (patch: Record<string, unknown>) => ({
  id: 'skill',
  name: '技能',
  description: '示例描述。',
  scope: 'global',
  source: 'custom',
  enabled: true,
  path: 'C:\\skills\\skill',
  engine: 'claude-code',
  trigger: '/skill',
  ...patch,
});
const vaSettingsWithoutProjectDir = (() => {
  const clone = JSON.parse(JSON.stringify(fixtures.load_app_settings)) as {
    general: { defaultDirectory: string };
  };
  clone.general.defaultDirectory = '';
  return clone;
})();
const extFixtureVariants: Record<string, Record<string, unknown>> = {
  // A-01/A-02：加载中
  'ext-pending-skills': { list_skills: '__pending' },
  'ext-pending-servers': { list_mcp_servers: '__pending' },
  // A-03/A-04：加载失败
  'ext-reject-skills': { list_skills: '__reject' },
  'ext-reject-servers': { list_mcp_servers: '__reject' },
  // C-01/G-01：空列表
  'ext-empty-skills': { list_skills: () => [] },
  'ext-empty-servers': { list_mcp_servers: [] },
  // A-06：默认工作目录未配置
  'ext-noproject': { load_app_settings: vaSettingsWithoutProjectDir },
  // C 组：来源四类 + 项目范围 + 停用卡 + 无描述 + 超长文本
  'ext-skill-matrix': {
    list_skills: (payload: Record<string, unknown>) =>
      payload.engine === 'codex'
        ? [
            vaSkill({
              id: 'review',
              name: 'Code Review',
              engine: 'codex',
              trigger: '$review',
              scope: 'project',
              path: 'C:\\code\\helm\\.codex\\skills\\review',
            }),
          ]
        : [
            vaSkill({
              id: 'plan',
              name: '计划任务',
              source: 'builtin',
              description: '将复杂工作拆解成可追踪的步骤，并随执行更新状态。',
            }),
            vaSkill({
              id: 'pdf',
              name: 'PDF 工具',
              source: 'market',
              description: '读取、生成并按页面渲染校验 PDF 文档。',
            }),
            vaSkill({
              id: 'legacy-hook',
              name: '旧插件技能',
              source: 'plugin',
              description: '历史插件安装来源。',
            }),
            vaSkill({
              id: 'release-check',
              name: '发布检查',
              scope: 'project',
              description: '执行项目发布前的版本、构建和产物核对。',
            }),
            vaSkill({ id: 'off-skill', name: '已停用技能', enabled: false }),
            vaSkill({ id: 'no-desc', name: '无描述技能', description: '' }),
            vaSkill({
              id: 'long-skill',
              name: '超长名称超长名称超长名称超长名称超长名称超长名称技能',
              description: '这是一段用于验证两行截断行为的超长描述文本。'.repeat(12),
            }),
          ],
  },
  // D-03/D-04/D-05：抽屉源码读取三态
  'ext-source-reject': { read_skill_source: '__reject' },
  'ext-source-truncated': {
    read_skill_source: () => ({ path: 'C:\\skills\\big\\SKILL.md', content: '', truncated: true }),
  },
  'ext-source-empty': {
    read_skill_source: () => ({
      path: 'C:\\skills\\empty\\SKILL.md',
      content: '',
      truncated: false,
    }),
  },
  // C-09：技能启停失败回滚
  'ext-toggle-reject': { toggle_skill: '__reject' },
  // F 组：市场搜索态；installed 变体验证「已安装 · 查看」
  'ext-market-pending': { market_search_skills: '__pending' },
  'ext-market-reject': { market_search_skills: '__reject' },
  'ext-market-empty': { market_search_skills: () => [] },
  'ext-market-installed': {
    list_skills: (payload: Record<string, unknown>) =>
      payload.engine === 'claude-code'
        ? [vaSkill({ id: 'frontend-design', name: 'frontend-design', source: 'market' })]
        : [],
  },
  'ext-install-pending': { market_install_skill: '__pending' },
  'ext-install-reject': { market_install_skill: '__reject' },
  // E-07：创建挂起
  'ext-create-pending': { create_skill: '__pending' },
  // G 组：连接器 pill/时间/工具数 全组合
  'ext-server-states': {
    list_mcp_servers: [
      {
        name: 'github-ok',
        command: 'https://api.githubcopilot.com/mcp',
        args: [],
        env: {},
        headers: {},
        transport: 'http',
        enabled: true,
        status: 'connected',
        toolCount: 12,
        lastTestedAt: Math.floor(now / 1000) - 3600,
      },
      {
        name: 'context7-untested',
        command: 'https://mcp.context7.com/mcp',
        args: [],
        env: {},
        transport: 'http',
        enabled: true,
        status: 'disconnected',
        toolCount: null,
        lastTestedAt: null,
      },
      {
        name: 'playwright-error',
        command: 'npx',
        args: ['-y', '@playwright/mcp@latest'],
        env: {},
        transport: 'stdio',
        enabled: true,
        status: 'error',
        toolCount: 3,
        lastTestedAt: Math.floor(now / 1000) - 86400,
        lastError: 'connect ECONNREFUSED 127.0.0.1:9222',
      },
      {
        name: 'sentry-disabled',
        command: 'npx',
        args: ['-y', '@sentry/mcp'],
        env: {},
        transport: 'http',
        enabled: false,
        status: 'connected',
        toolCount: 5,
        lastTestedAt: Math.floor(now / 1000) - 86400 * 20,
      },
    ],
  },
  // G-10：检测挂起/失败
  'ext-test-pending': { test_mcp_connection: '__pending' },
  'ext-test-reject': { test_mcp_connection: '__reject' },
  // G-08：停用开关写入失败
  'ext-server-toggle-reject': { set_mcp_server_enabled: '__reject' },
  // G-11：极端数量
  'ext-many-servers': {
    list_mcp_servers: Array.from({ length: 11 }, (_, index) => ({
      name: 'server-' + index,
      command: 'npx',
      args: ['-y', '@demo/server-' + index],
      env: {},
      transport: index % 2 ? 'http' : 'stdio',
      enabled: true,
      status: 'connected',
      toolCount: 3 + index,
      lastTestedAt: Math.floor(now / 1000) - index * 3600,
    })),
  },
  // H/I/K：保存与卸载挂起
  'ext-save-pending': { save_mcp_server: '__pending', test_mcp_connection: '__pending' },
  'ext-delete-pending': { delete_skill: '__pending', delete_mcp_server: '__pending' },
  // J：导入空结果 / 命令失败
  'ext-import-empty': { import_mcp_servers: () => [] },
  'ext-import-reject': { import_mcp_servers: '__reject' },
};

// —— 工作区（对话页）状态矩阵 fixture 变体（docs/可靠性检查-工作区对话页-状态矩阵-2026-08-25.md §1）——
// 追加式：仅新增 wsFixtureVariants 与一处 URL 分发；默认视觉矩阵与生产行为不变。
// 线程内容经 get_active_session / get_session_history 的 SessionDetail 恢复链路注入
// （useSession resume_handle → itemsFromHistory）；thinking/plan/compact 为 live-only
// 事件种类，历史恢复不含，属矩阵登记的复现缺口而非遗漏。
type WsDetail = Record<string, unknown>;
const vaWsSettings = (patch: { reopenLastSession?: boolean; defaultDirectory?: string }) => {
  const clone = JSON.parse(JSON.stringify(fixtures.load_app_settings)) as {
    general: { reopenLastSession: boolean; defaultDirectory: string };
  };
  if (patch.reopenLastSession != null) clone.general.reopenLastSession = patch.reopenLastSession;
  if (patch.defaultDirectory != null) clone.general.defaultDirectory = patch.defaultDirectory;
  return clone;
};
const vaWsDetail = (patch: WsDetail): WsDetail => ({
  ...(JSON.parse(JSON.stringify(activeSession)) as WsDetail),
  ...patch,
});
const wsMsg = (role: 'user' | 'assistant', text: string, ts: number, turnId?: string) => ({
  role,
  text,
  ts,
  ...(turnId ? { turnId } : {}),
});
const wsTool = (patch: Record<string, unknown>): Record<string, unknown> => ({
  id: 'ws-tool',
  name: 'Read',
  input: { path: 'src/auth/token.ts' },
  status: 'success',
  output: '86 lines',
  ts: now - 12_000,
  endedAt: now - 11_000,
  turnId: 'turn-1',
  ...patch,
});
const wsTurnRow = (patch: Record<string, unknown>): Record<string, unknown> => ({
  id: 'turn-1',
  epoch: 1,
  mode: 'build',
  permissionProfile: 'standard',
  status: 'succeeded',
  startedAt: now - 60_000,
  endedAt: now - 8_000,
  ...patch,
});
const wsApprovalsDetail = vaWsDetail({
  messages: [wsMsg('user', '帮我把这次改动发布到 npm，命令由你来跑。', now - 30_000, 'turn-1')],
  toolCalls: [],
  approvals: [
    {
      id: 'appr-pending',
      action: '运行命令',
      detail: 'npm publish --access public',
      status: 'pending',
      ts: now - 20_000,
      turnId: 'turn-1',
    },
    {
      id: 'appr-done',
      action: '编辑文件',
      detail: 'src/version.ts',
      status: 'resolved',
      decision: 'allow',
      resolvedAt: now - 15_000,
      ts: now - 18_000,
      turnId: 'turn-1',
    },
  ],
  checkpoints: [],
  turns: [wsTurnRow({ status: 'running', endedAt: null })],
});
const wsCheckpointsDetail = vaWsDetail({
  messages: [
    wsMsg('user', '重构 token 刷新逻辑并保留可回退点。', now - 40_000, 'turn-1'),
    wsMsg('assistant', '已完成重构；两个检查点已记录，可随时回溯。', now - 9_000, 'turn-1'),
  ],
  toolCalls: [
    wsTool({
      id: 'ws-write-1',
      name: 'Write',
      input: { path: 'src/auth/token.ts' },
      output: 'written',
    }),
  ],
  checkpoints: [
    {
      id: 'cp-ok',
      label: '写入 auth.ts 前',
      ts: now - 13_000,
      restorable: true,
      fileCount: 2,
      turnId: 'turn-1',
    },
    {
      id: 'cp-ro',
      label: '工作区外目标，仅记录',
      ts: now - 12_500,
      restorable: false,
      reason: '路径越界',
      turnId: 'turn-1',
    },
  ],
});
const wsSubagentsDetail = vaWsDetail({
  messages: [
    wsMsg('user', '并行调研 + 起草发布说明，最后汇总。', now - 60_000, 'turn-1'),
    wsMsg(
      'assistant',
      '三个子代理已派出：检索完成、测试失败、起草仍在进行；后台 dev 服务已挂起。',
      now - 6_000,
      'turn-1',
    ),
  ],
  toolCalls: [
    wsTool({
      id: 'ws-agent-scout',
      name: 'Task',
      input: { description: '检索鉴权中间件的三处用法', name: 'scout' },
      output: '命中 src/auth/* 共 3 处',
      startedAt: now - 50_000,
      endedAt: now - 42_000,
    }),
    wsTool({
      id: 'ws-agent-runner',
      name: 'Task',
      input: { description: '运行集成测试矩阵', name: 'runner' },
      status: 'error',
      output: 'exit code 1 · 2 failed',
      startedAt: now - 48_000,
      endedAt: now - 40_000,
    }),
    wsTool({
      id: 'ws-agent-writer',
      name: 'Task',
      input: { description: '起草 v0.4 发布说明', name: 'writer' },
      status: 'pending',
      startedAt: now - 30_000,
    }),
    wsTool({
      id: 'ws-bg-dev',
      name: 'Bash',
      input: { command: 'npm run dev', timeout: 900_000 },
      status: 'success',
      output: 'running on :1420',
      startedAt: now - 35_000,
    }),
  ],
});
const wsFailedTurnDetail = vaWsDetail({
  messages: [
    wsMsg('user', '跑一遍全量测试然后总结结果。', now - 45_000, 'turn-1'),
    wsMsg('assistant', '测试执行到一半失败了：', now - 10_000, 'turn-1'),
  ],
  toolCalls: [
    wsTool({
      id: 'ws-bash-test',
      name: 'Bash',
      input: { command: 'npm test' },
      status: 'error',
      output: 'Command failed with exit code 1',
      retryable: true,
      startedAt: now - 40_000,
      endedAt: now - 11_000,
    }),
  ],
  turns: [wsTurnRow({ status: 'failed', endedAt: now - 10_000 })],
});
const wsWindowedMessages: Array<Record<string, unknown>> = [];
const wsWindowedTurns: Array<Record<string, unknown>> = [];
for (let i = 0; i < 110; i += 1) {
  const ts = now - (400 - i) * 60_000;
  wsWindowedMessages.push(
    wsMsg(
      'user',
      '第 ' + (i + 1) + ' 轮提问：请继续完善模块 ' + (i + 1) + ' 的边界处理。',
      ts,
      'turn-' + (i + 1),
    ),
  );
  wsWindowedMessages.push(
    wsMsg(
      'assistant',
      '第 ' + (i + 1) + ' 轮结论：模块 ' + (i + 1) + ' 已按约定完成并通过回归。',
      ts + 30_000,
      'turn-' + (i + 1),
    ),
  );
  wsWindowedTurns.push(
    wsTurnRow({ id: 'turn-' + (i + 1), epoch: i + 1, startedAt: ts, endedAt: ts + 30_000 }),
  );
}
const wsWindowedDetail = vaWsDetail({
  messages: wsWindowedMessages,
  toolCalls: [],
  approvals: [],
  checkpoints: [],
  turns: wsWindowedTurns,
});
const wsNoDiffDetail = vaWsDetail({
  toolCalls: [
    wsTool({ id: 'ws-read-only', name: 'Read', input: { path: 'README.md' } }),
    wsTool({
      id: 'ws-grep-only',
      name: 'Grep',
      input: { pattern: 'refreshToken' },
      output: '3 matches',
      ts: now - 11_500,
    }),
  ],
});
const wsCompactBase = (lastContextTokens: number): WsDetail =>
  vaWsDetail({
    inputTokens: 41_200,
    cachedInputTokens: 2_100_000,
    cacheWriteInputTokens: 118_600,
    outputTokens: 38_200,
    costUsd: 0.42,
    lastContextTokens,
    lastContextWindow: 200_000,
    createdAt: Math.floor((now - 3 * 3_600_000) / 1000),
  });
const wsArchivedSessions = () => [
  ...sessions,
  {
    ...JSON.parse(JSON.stringify(sessions[0])),
    id: 'session-arch',
    title: '旧迁移脚本清理与归档说明补全',
    archived: true,
    folderId: 'folder-acme',
    updatedAt: now - 72 * 3_600_000,
  },
];
const wsManySessions = () => {
  const rows: Array<Record<string, unknown>> = [];
  for (let i = 0; i < 40; i += 1) {
    rows.push({
      ...JSON.parse(JSON.stringify(sessions[i % 4])),
      id: 'ws-many-' + (i + 1),
      cliSessionId: 'ws-cli-' + (i + 1),
      title: '批量任务 ' + (i + 1) + '：修复构建告警与文档链接',
      pinned: i % 7 === 0,
      archived: i % 11 === 5,
      folderId: ['folder-acme', 'folder-data', 'folder-default'][i % 3],
      updatedAt: now - i * 17 * 60_000,
    });
  }
  return rows;
};
const wsModelsEmptyConfig: Record<string, unknown> = {
  providers: [
    {
      id: 'anthropic-subscription',
      name: 'Claude 订阅',
      kind: 'subscription',
      baseUrl: '',
      keyRef: null,
      ready: true,
      lastTest: null,
      protocol: 'anthropic-messages',
      authMethod: 'oauth',
    },
  ],
  models: [],
  engines: [
    {
      id: 'claude-code',
      name: 'Claude Code',
      bin: 'claude',
      defaultModel: '',
      status: 'ready',
      version: '2.1.206',
    },
    {
      id: 'codex',
      name: 'Codex',
      bin: 'codex',
      defaultModel: '',
      status: 'ready',
      version: '0.144.1',
    },
  ],
  bindings: [{ engineId: 'claude-code', providerId: 'anthropic-subscription', primaryModel: '' }],
  defaultEngine: 'claude-code',
  defaultModel: '',
};
const wsLongUserText =
  '请审阅这份超长说明：' + '这是一段用于验证换行与滚动行为的中文长文本。'.repeat(60);
const wsLongAssistantText =
  '结论先行：可以合入。\n\n' +
  '段落内容重复用于撑起纵向滚动与两端对齐的排版验证。'.repeat(120) +
  '\n\n无空格超长标记：' +
  'token'.repeat(160);
const wsLongTextDetail = vaWsDetail({
  messages: [
    wsMsg('user', wsLongUserText, now - 40_000, 'turn-1'),
    wsMsg('assistant', wsLongAssistantText, now - 9_000, 'turn-1'),
  ],
});
const wsSwchTurns: Array<Record<string, unknown>> = [
  wsTurnRow({
    id: 'turn-1',
    model: 'claude-sonnet-4-6',
    requestedModelId: 'claude-sonnet-4-6',
    routedModelId: 'claude-sonnet-4-6',
    startedAt: now - 90_000,
    endedAt: now - 70_000,
  }),
  wsTurnRow({
    id: 'turn-2',
    model: 'claude-opus-4-7',
    requestedModelId: 'claude-opus-4-7',
    routedModelId: 'claude-opus-4-7',
    startedAt: now - 60_000,
    endedAt: now - 40_000,
  }),
  wsTurnRow({
    id: 'turn-3',
    model: 'claude-opus-4-7',
    requestedModelId: 'claude-opus-4-7',
    routedModelId: 'claude-opus-4-7',
    startedAt: now - 35_000,
    endedAt: now - 20_000,
  }),
  wsTurnRow({
    id: 'turn-4',
    model: 'claude-haiku-4-5',
    requestedModelId: 'claude-haiku-4-5',
    routedModelId: 'claude-haiku-4-5',
    startedAt: now - 18_000,
    endedAt: now - 9_000,
  }),
];
const wsSwchMessages: Array<Record<string, unknown>> = [
  wsMsg('user', '第一轮：用 Sonnet 快速过一遍。', now - 90_000, 'turn-1'),
  wsMsg('assistant', '第一轮完成。', now - 70_000, 'turn-1'),
  wsMsg('user', '第二轮：换 Opus 深入分析。', now - 60_000, 'turn-2'),
  wsMsg('assistant', '第二轮深入分析完成。', now - 40_000, 'turn-2'),
  wsMsg('user', '第三轮：继续用 Opus 补测试。', now - 35_000, 'turn-3'),
  wsMsg('assistant', '第三轮测试补齐。', now - 20_000, 'turn-3'),
  wsMsg('user', '第四轮：用 Haiku 收尾文档。', now - 18_000, 'turn-4'),
  wsMsg('assistant', '第四轮文档收尾完成。', now - 9_000, 'turn-4'),
];
const wsSwchDetail = vaWsDetail({
  messages: wsSwchMessages,
  toolCalls: [],
  approvals: [],
  checkpoints: [],
  turns: wsSwchTurns,
});
const wsFixtureVariants: Record<string, Record<string, unknown>> = {
  // A-3/A-7/B-6：全新会话与空列表族
  'ws-no-cwd': {
    load_app_settings: vaWsSettings({ reopenLastSession: false, defaultDirectory: '' }),
    get_active_session: null,
    get_session_history: null,
  },
  'ws-no-reopen': {
    load_app_settings: vaWsSettings({ reopenLastSession: false }),
    get_active_session: null,
    get_session_history: null,
  },
  'ws-empty-all': {
    load_app_settings: vaWsSettings({ reopenLastSession: false }),
    list_sessions: [],
    list_folders: [{ id: 'folder-default', name: '默认项目', cwd: null, collapsed: false }],
    get_active_session: null,
    get_session_history: null,
  },
  // 启动首落页（2026-09-03 决议）验证变体：
  // 指针缺失但历史有任务 → App 兜底 pendingSessionId 打开最近会话；
  // 一个任务都没有（reopenLastSession 保持默认开启）→ App 落新任务页。
  'ws-landing-recent': { get_active_session: null },
  'ws-landing-empty': {
    list_sessions: [],
    list_folders: [{ id: 'folder-default', name: '默认项目', cwd: null, collapsed: false }],
    get_active_session: null,
    get_session_history: null,
  },
  // B-3/B-17：归档隔离与计数
  'ws-archived-sessions': { list_sessions: wsArchivedSessions() },
  // B-18：恢复挂起（seq 守卫观察窗口）
  'ws-resume-pending': { resume_session: '__pending' },
  // C 组线程头
  'ws-models-empty': { get_provider_config: wsModelsEmptyConfig },
  'ws-effort-fail': { get_reasoning_effort_capability: '__reject' },
  // D 组线程内容（恢复链路注入）
  'ws-approvals-pending': {
    get_active_session: wsApprovalsDetail,
    get_session_history: wsApprovalsDetail,
  },
  'ws-checkpoints': {
    get_active_session: wsCheckpointsDetail,
    get_session_history: wsCheckpointsDetail,
  },
  'ws-subagents': {
    get_active_session: wsSubagentsDetail,
    get_session_history: wsSubagentsDetail,
  },
  'ws-failed-turn': {
    get_active_session: wsFailedTurnDetail,
    get_session_history: wsFailedTurnDetail,
  },
  'ws-swch': {
    get_active_session: wsSwchDetail,
    get_session_history: wsSwchDetail,
  },
  'ws-windowed': {
    get_active_session: wsWindowedDetail,
    get_session_history: wsWindowedDetail,
  },
  'ws-long-text': {
    get_active_session: wsLongTextDetail,
    get_session_history: wsLongTextDetail,
  },
  // F/G/H 组：占用口径与提醒
  'ws-compact-80': {
    get_active_session: wsCompactBase(164_000),
    get_session_history: wsCompactBase(164_000),
  },
  'ws-compact-95': {
    get_active_session: wsCompactBase(192_000),
    get_session_history: wsCompactBase(192_000),
  },
  // I 组右栏
  'ws-no-diff': {
    get_active_session: wsNoDiffDetail,
    get_session_history: wsNoDiffDetail,
  },
  'ws-files-empty': { search_workspace_files: () => [] },
  'ws-files-error': { search_workspace_files: '__reject' },
  'ws-mcp-error': { list_mcp_servers: '__reject' },
  'ws-skills-error': { list_skills: '__reject' },
  // J 组发送阻断
  'ws-blocked-providers': {
    load_app_settings: vaWsSettings({ reopenLastSession: false }),
    get_provider_config: {
      providers: [],
      models: [],
      engines: [
        {
          id: 'claude-code',
          name: 'Claude Code',
          bin: 'claude',
          defaultModel: '',
          status: 'ready',
          version: '2.1.206',
        },
        {
          id: 'codex',
          name: 'Codex',
          bin: 'codex',
          defaultModel: '',
          status: 'ready',
          version: '0.144.1',
        },
      ],
      bindings: [],
      defaultEngine: 'claude-code',
      defaultModel: '',
    },
    get_active_session: null,
    get_session_history: null,
  },
  'ws-deps-missing': {
    load_app_settings: vaWsSettings({ reopenLastSession: false }),
    detect_workspace_deps: vaDeps(false),
    detect_cli_engine: '__reject',
    get_active_session: null,
    get_session_history: null,
  },
};
void wsManySessions;
const requestedFixtureVariant = new URLSearchParams(window.location.search).get('fixture');
if (requestedFixtureVariant) {
  if (requestedFixtureVariant in homeFixtureVariants) {
    Object.assign(fixtureOverrides, homeFixtureVariants[requestedFixtureVariant]);
  }
  if (requestedFixtureVariant in extFixtureVariants) {
    Object.assign(fixtureOverrides, extFixtureVariants[requestedFixtureVariant]);
  }
  if (requestedFixtureVariant in wsFixtureVariants) {
    Object.assign(fixtureOverrides, wsFixtureVariants[requestedFixtureVariant]);
  }
}

const visualWindow = window as VisualWindow;
visualWindow.__setFixture = (command: string, value: unknown) => {
  if (value === null) delete fixtureOverrides[command];
  else fixtureOverrides[command] = value;
};
let callbackId = 0;
const callbacks = new Map<number, (payload: unknown) => void>();
visualWindow.__TAURI_INTERNALS__ = {
  invoke: async (command: string, payload: Record<string, unknown> = {}) => {
    if (command in fixtureOverrides) {
      const override = fixtureOverrides[command];
      if (override === '__reject') throw new Error(`fixture 已注入拒绝：${command}`);
      if (override === '__pending') return new Promise<never>(() => undefined);
      return typeof override === 'function'
        ? (override as (input: Record<string, unknown>) => unknown)(payload)
        : override;
    }
    const fixture = fixtures[command];
    return typeof fixture === 'function'
      ? (fixture as (input: Record<string, unknown>) => unknown)(payload)
      : (fixture ?? null);
  },
  transformCallback: (callback: (payload: unknown) => void) => {
    const id = ++callbackId;
    callbacks.set(id, callback);
    return id;
  },
  unregisterCallback: (id: number) => callbacks.delete(id),
  runCallback: (id: number, payload: unknown) => callbacks.get(id)?.(payload),
  callbacks,
  metadata: { currentWindow: { label: 'main' }, windows: [{ label: 'main' }] },
  convertFileSrc: (file: string) => file,
};
visualWindow.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: () => undefined };
document.body.dataset.visualBoot = 'mocked';
mountApp();
document.body.dataset.visualBoot = 'mounted';

// —— 探针专用：?probeOpen=<state> 挂载后自动驱动新任务页到指定交互态 ——
// 供无 CDP 通道（headless --screenshot）的截图验证使用；focus() 刻意复现真实
// 鼠标点击的 :focus-within 链路（el.click() 不聚焦，曾让浮层缺陷漏检四轮）。
// 仅带该查询参数的会话受影响，默认视觉矩阵与生产行为不变。
const probeOpenState = new URLSearchParams(window.location.search).get('probeOpen');
if (probeOpenState) {
  // 截图验证可选主题（?theme=dark）：仅 probeOpen 会话生效，默认矩阵不变。
  // 设置加载是异步的，会晚于挂载覆盖 dataset.theme——头 5 秒内持续强制回写。
  const probeTheme = new URLSearchParams(window.location.search).get('theme');
  if (probeTheme === 'dark' || probeTheme === 'light') {
    const enforceTheme = () => {
      document.documentElement.dataset.theme = probeTheme;
    };
    enforceTheme();
    const themeTimer = window.setInterval(enforceTheme, 100);
    window.setTimeout(() => window.clearInterval(themeTimer), 5000);
  }
  const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
  const clickAt = (selector: string) => {
    const el = document.querySelector<HTMLElement>(selector);
    if (!el) throw new Error('probeOpen: missing ' + selector);
    el.focus();
    el.click();
  };
  const clickMenuItemByText = (scopeSelector: string, text: string) => {
    const scope = document.querySelector(scopeSelector);
    if (!scope) throw new Error('probeOpen: missing scope ' + scopeSelector);
    for (const item of scope.querySelectorAll<HTMLElement>('button')) {
      if (item.textContent?.includes(text)) {
        item.focus();
        item.click();
        return;
      }
    }
    throw new Error('probeOpen: menu item not found: ' + text);
  };
  void (async () => {
    await wait(400);
    document.querySelector<HTMLButtonElement>('button[aria-label="新任务"]')?.click();
    for (let i = 0; i < 120; i++) {
      if (document.querySelector('.cm-composer')) break;
      await wait(50);
    }
    await wait(600); // 就绪报告 / Provider 配置等 fixture 数据与渲染落定
    const capTrigger = '.cap-anchor > .cm-tool';
    try {
      switch (probeOpenState) {
        case 'base':
          break;
        case 'capmenu':
          clickAt(capTrigger);
          break;
        case 'cap-file':
          clickAt(capTrigger);
          await wait(200);
          clickMenuItemByText('.cm-menu--above', '文件与目录');
          await wait(500);
          break;
        case 'cap-file-nodir':
          // 未选工作目录：点菜单也直开文件中心（单一弹框，用户决议 2026-09）
          clickAt(capTrigger);
          await wait(200);
          clickMenuItemByText('.cm-menu--above', '文件与目录');
          await wait(500);
          break;
        case 'cap-cmd':
          clickAt(capTrigger);
          await wait(200);
          clickMenuItemByText('.cm-menu--above', '命令与技能');
          await wait(500);
          break;
        case 'mode':
          clickAt('button[title="任务模式"]');
          break;
        case 'permission':
          clickAt('button[title="权限"]');
          break;
        case 'model':
          clickAt('button[title="模型"]');
          break;
        case 'effort':
          clickAt('button[aria-label="选择推理强度"]');
          break;
        case 'engine':
          clickAt('button[title="更换 Agent"]');
          break;
        case 'dirmode':
          clickAt('button[title="更换工作目录"]');
          await wait(400);
          break;
        default:
          throw new Error('probeOpen: unknown state ' + probeOpenState);
      }
      document.body.dataset.probeOpenDone = '1';
    } catch (error) {
      document.body.dataset.probeOpenError = String(error);
    }
  })();
}
