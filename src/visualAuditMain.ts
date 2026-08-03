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
  ],
};

const fixtures: Record<string, unknown> = {
  load_app_settings: {
    general: {
      workspaceName: '我的工作区',
      defaultDirectory: 'C:\\code\\helm',
      reopenLastSession: true,
      anonymousAnalytics: false,
      autoUpdateChannel: 'stable',
      updateFeedUrl: '',
      pricingAutoUpdate: true,
      pricingFeedUrls: [],
      pricingUnknownPolicy: 'warn',
      pricingMaxAgeDays: 30,
      onboardingCompleted: true,
      autoTitleSessions: true,
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
      accentColor: { base: 'oklch(55% 0.2 264)', hi: 'oklch(49% 0.21 264)' },
      uiDensity: 'comfortable',
      monospaceFont: 'JetBrains Mono',
      reduceMotion: false,
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
    request_count: 186,
    session_count: 24,
    actual_cost: 98.2,
    estimated_cost: 14.2,
    subscription_count: 2,
    unknown_count: 0,
    legacy_cost: 0,
    legacy_count: 0,
  },
  get_usage_by_model: [
    {
      model: 'claude-sonnet-4-6',
      engine: 'claude-code',
      request_count: 96,
      input_tokens: 900_000,
      output_tokens: 310_000,
      cost_usd: 58.4,
      share: 0.52,
    },
    {
      model: 'gpt-5-codex',
      engine: 'codex',
      request_count: 62,
      input_tokens: 650_000,
      output_tokens: 230_000,
      cost_usd: 38.2,
      share: 0.34,
    },
    {
      model: 'claude-haiku-4-5',
      engine: 'claude-code',
      request_count: 28,
      input_tokens: 270_000,
      output_tokens: 100_000,
      cost_usd: 15.8,
      share: 0.14,
    },
  ],
  get_usage_by_provider: [
    { provider: 'anthropic', cost_usd: 74.2, share: 0.66 },
    { provider: 'openai', cost_usd: 38.2, share: 0.34 },
  ],
  get_daily_usage: (payload: Record<string, unknown>) => {
    const days = Number(payload.days || 30);
    return Array.from({ length: days }, (_, index) => ({
      date: new Date(now - (days - 1 - index) * 86_400_000).toISOString().slice(0, 10),
      cost_usd: 1.8 + (index % 7) * 0.7,
    }));
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
};

const visualWindow = window as VisualWindow;
let callbackId = 0;
const callbacks = new Map<number, (payload: unknown) => void>();
visualWindow.__TAURI_INTERNALS__ = {
  invoke: async (command: string, payload: Record<string, unknown> = {}) => {
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
