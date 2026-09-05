// 插件页（S7）：技能 / 连接器 双 Tab，对齐 prototype/extensions.html 与
// docs/插件页原型-技能与连接器方案.md。子代理/斜杠命令/钩子不再作为本页入口。
// 所有列表与状态来自真实 Rust 命令；精选目录只是安装模板，卡片状态由真实服务器列表推导。
import { useCallback, useEffect, useMemo, useState, type ReactNode } from 'react';
import { Icon } from '../shell/icons';
import { EngineBrand } from '../shell/EngineBrand';
import brandGithub from '../assets/brands/github.svg';
import brandPlaywright from '../assets/brands/playwright.svg';
import brandChrome from '../assets/brands/chrome.svg';
import brandContext7 from '../assets/brands/context7.ico';
import { showResultToast } from '../components/toast';
import { EmptyState } from '../components/EmptyState';
import { ConfirmDialog } from '../components/ConfirmDialog';
import { Dialog } from '../components/Dialog';
import { openPathInSystem } from '../engine/transport';
import { loadSettings } from '../settings/api';
import {
  createSkill,
  deleteMcpServer,
  deleteSkill,
  importMcpServers,
  listMcpServers,
  listSkills,
  marketInstallSkill,
  marketSearchSkills,
  readSkillSource,
  saveMcpServer,
  setMcpServerEnabled,
  testMcpConnection,
  type CreateSkillRequest,
  type MarketSkill,
  type McpImportItemResult,
  type McpServer,
  type McpTool,
  type Skill,
  type SkillSourceFile,
} from './extensionsApi';
import {
  connectorStatusPill,
  deriveFeaturedStates,
  FEATURED_CONNECTORS,
  filterFeaturedStates,
  filterSkillsByQuery,
  formatTestedAt,
  groupSkillsBySource,
  importResultRows,
  isCredentialKey,
  marketRowIcon,
  skillCardIcon,
  skillScopeNote,
  slugifySkillName,
  transportLabel,
  triggerText,
  type FeaturedCardState,
  type FeaturedConnectorTemplate,
  type ImportResultRow,
  type SkillEngine,
} from './extensionsViewModel';
import './extensions.css';

type TabId = 'skills' | 'mcp';

interface EnvRow {
  key: string;
  value: string;
}

interface McpDraft {
  name: string;
  transport: 'stdio' | 'http';
  /** stdio 启动命令 / http 服务地址 */
  command: string;
  /** stdio 参数，每行一个 */
  args: string;
  envRows: EnvRow[];
  headerRows: EnvRow[];
}

interface PendingConfirm {
  title: string;
  body: string;
  confirmLabel: string;
  onConfirm: () => Promise<void>;
}

const EMPTY_DRAFT: McpDraft = {
  name: '',
  transport: 'stdio',
  command: '',
  args: '',
  envRows: [{ key: '', value: '' }],
  headerRows: [{ key: '', value: '' }],
};

/** 精选卡品牌图（与 prototype/assets/brands 同源）；无品牌图的模板回落通用图标。 */
const FEATURED_BRANDS: Record<string, string> = {
  github: brandGithub,
  playwright: brandPlaywright,
  'chrome-devtools': brandChrome,
  context7: brandContext7,
};

function engineLabel(engine: SkillEngine): string {
  return engine === 'codex' ? 'Codex' : 'Claude Code';
}

export function ExtensionsPage() {
  const [tab, setTab] = useState<TabId>('skills');
  // 项目级作用域上下文：取设置里的默认工作目录，页内不再提供切换（原型无此入口）
  const [projectDir, setProjectDir] = useState('');
  // ===== 技能 =====
  const [engine, setEngine] = useState<SkillEngine>('claude-code');
  const [skills, setSkills] = useState<Skill[]>([]);
  const [skillsLoading, setSkillsLoading] = useState(true);
  const [skillsError, setSkillsError] = useState('');
  const [skillSearch, setSkillSearch] = useState('');
  const [createOpen, setCreateOpen] = useState(false);
  const [marketOpen, setMarketOpen] = useState(false);
  const [drawerSkill, setDrawerSkill] = useState<Skill | null>(null);
  const [skillSource, setSkillSource] = useState<{
    state: 'loading' | 'ready' | 'error';
    file?: SkillSourceFile;
    error?: string;
  }>({ state: 'loading' });
  const [sourceView, setSourceView] = useState<'preview' | 'source'>('preview');
  // ===== 连接器 =====
  const [servers, setServers] = useState<McpServer[]>([]);
  const [serversLoading, setServersLoading] = useState(true);
  const [serversError, setServersError] = useState('');
  const [mcpSearch, setMcpSearch] = useState('');
  // 工具列表只保留本次会话真实检测过的结果；跨重启事实走 servers 的 lastTestedAt/toolCount
  const [testedTools, setTestedTools] = useState<Record<string, McpTool[]>>({});
  const [testing, setTesting] = useState<string | null>(null);
  const [mcpDraft, setMcpDraft] = useState<McpDraft | null>(null);
  const [importOpen, setImportOpen] = useState(false);
  const [drawerServerName, setDrawerServerName] = useState<string | null>(null);
  const [drawerFeatured, setDrawerFeatured] = useState<FeaturedConnectorTemplate | null>(null);
  const [installingFeatured, setInstallingFeatured] = useState<string | null>(null);
  const [pendingConfirm, setPendingConfirm] = useState<PendingConfirm | null>(null);

  useEffect(() => {
    void (async () => {
      try {
        const settings = await loadSettings();
        setProjectDir(settings.general.defaultDirectory.trim());
      } catch (err) {
        console.error('读取默认目录失败:', err);
      }
    })();
  }, []);

  const refreshSkills = useCallback(async () => {
    setSkillsLoading(true);
    try {
      setSkills(await listSkills(engine, projectDir || undefined));
      setSkillsError('');
    } catch (err) {
      console.error('加载技能失败:', err);
      setSkillsError(String(err));
    } finally {
      setSkillsLoading(false);
    }
  }, [engine, projectDir]);

  const refreshServers = useCallback(async () => {
    setServersLoading(true);
    try {
      setServers(await listMcpServers());
      setServersError('');
    } catch (err) {
      console.error('加载连接器失败:', err);
      setServersError(String(err));
    } finally {
      setServersLoading(false);
    }
  }, []);

  useEffect(() => {
    void refreshSkills();
  }, [refreshSkills]);

  useEffect(() => {
    void refreshServers();
  }, [refreshServers]);

  function notify(message: string) {
    showResultToast(message);
  }

  /** 精选一键安装：真实写入双引擎配置并自动检测（零输入模板免表单）。 */
  async function installFeaturedDirect(template: FeaturedConnectorTemplate) {
    setInstallingFeatured(template.name);
    try {
      setDrawerFeatured(null);
      await handleSaveDraft(templateToDraft(template.name), true);
    } finally {
      setInstallingFeatured(null);
    }
  }

  /** 静默检测：抽屉打开时自动拉取真实工具列表，不弹 toast 打断。 */
  async function silentTest(server: McpServer) {
    setTesting(server.name);
    try {
      const tools = await testMcpConnection(server);
      setTestedTools((prev) => ({ ...prev, [server.name]: tools }));
    } catch (err) {
      console.error('自动检测连接器失败:', err);
    } finally {
      setTesting(null);
    }
  }

  // ===== 技能动作 =====
  function openSkillDrawer(skill: Skill) {
    setDrawerSkill(skill);
    setSourceView('preview');
    setSkillSource({ state: 'loading' });
    void (async () => {
      try {
        const file = await readSkillSource(skill.id, engine, projectDir || undefined);
        setSkillSource({ state: 'ready', file });
      } catch (err) {
        console.error('读取技能源码失败:', err);
        setSkillSource({ state: 'error', error: String(err) });
      }
    })();
  }

  async function handleRevealSkill(skill: Skill, sourcePath?: string) {
    const dir = directoryOf(sourcePath || skill.path);
    if (!dir) {
      notify('无法定位技能目录');
      return;
    }
    try {
      await openPathInSystem(dir);
    } catch (err) {
      console.error('打开技能目录失败:', err);
      notify(`打开技能目录失败：${err}`);
    }
  }

  function confirmUninstallSkill(skill: Skill) {
    setPendingConfirm({
      title: `卸载技能「${skill.name}」？`,
      body: `会从 ${engineLabel(engine)} 的技能目录删除该技能文件夹，引擎将不再加载它。`,
      confirmLabel: '卸载技能',
      onConfirm: async () => {
        try {
          await deleteSkill(skill.id, engine, projectDir || undefined);
          setDrawerSkill(null);
          await refreshSkills();
          notify('技能已卸载');
        } catch (err) {
          console.error('卸载技能失败:', err);
          notify(`卸载技能失败：${err}`);
        }
      },
    });
  }

  async function handleCreateSkill(request: CreateSkillRequest) {
    try {
      await createSkill(request, request.scope === 'project' ? projectDir : undefined);
      setCreateOpen(false);
      await refreshSkills();
      notify(`技能已创建到 ${engineLabel(engine)}`);
    } catch (err) {
      console.error('创建技能失败:', err);
      notify(`创建技能失败：${err}`);
    }
  }

  // ===== 连接器动作 =====
  async function handleTest(server: McpServer) {
    setTesting(server.name);
    try {
      const tools = await testMcpConnection(server);
      setTestedTools((prev) => ({ ...prev, [server.name]: tools }));
      notify(`${server.name} 连接正常 · ${tools.length} 个工具`);
    } catch (err) {
      console.error('测试连接器失败:', err);
      notify(`连接失败：${err}`);
    } finally {
      setTesting(null);
      // 最近一次检测结果由后端持久化，刷新带回真实状态
      void refreshServers();
    }
  }

  async function handleToggleServer(server: McpServer, enabled: boolean) {
    try {
      await setMcpServerEnabled(server.name, enabled);
      await refreshServers();
      notify(enabled ? '连接器已启用，配置写回双引擎' : '连接器已停用，定义保留但不注入引擎');
    } catch (err) {
      console.error('切换连接器状态失败:', err);
      notify(`切换连接器状态失败：${err}`);
    }
  }

  function confirmDeleteServer(server: McpServer) {
    setPendingConfirm({
      title: `卸载连接器「${server.name}」？`,
      body: '会从 Claude Code 与 Codex 配置中删除该定义，并清理系统钥匙串中的相关凭证。',
      confirmLabel: '卸载连接器',
      onConfirm: async () => {
        try {
          await deleteMcpServer(server.name);
          setTestedTools((prev) => {
            const next = { ...prev };
            delete next[server.name];
            return next;
          });
          setDrawerServerName(null);
          await refreshServers();
          notify('连接器已卸载');
        } catch (err) {
          console.error('卸载连接器失败:', err);
          notify(`卸载连接器失败：${err}`);
        }
      },
    });
  }

  async function handleSaveDraft(draft: McpDraft, autoTest: boolean) {
    const server = draftToServer(draft);
    try {
      await saveMcpServer(server);
    } catch (err) {
      console.error('保存连接器失败:', err);
      notify(`保存连接器失败：${err}`);
      return;
    }
    setMcpDraft(null);
    await refreshServers();
    if (!autoTest) {
      notify('连接器已保存到双引擎');
      return;
    }
    await handleTest(server);
  }

  const drawerServer = servers.find((server) => server.name === drawerServerName) ?? null;
  const filteredFeatured = useMemo(
    () => filterFeaturedStates(deriveFeaturedStates(FEATURED_CONNECTORS, servers), mcpSearch),
    [servers, mcpSearch],
  );

  return (
    <div className="page scroll ex-root">
      <div className="cm-tabs-wrapper">
        <div className="cm-tabs" role="tablist" aria-label="插件页导航">
          <button
            role="tab"
            aria-selected={tab === 'skills'}
            className={tab === 'skills' ? 'is-active' : ''}
            onClick={() => setTab('skills')}
          >
            技能
          </button>
          <button
            role="tab"
            aria-selected={tab === 'mcp'}
            className={tab === 'mcp' ? 'is-active' : ''}
            onClick={() => setTab('mcp')}
          >
            连接器
          </button>
        </div>
      </div>

      <div className="cm-pagebody cm-pagebody--scroll">
        {tab === 'skills' ? (
          <SkillsTab
            engine={engine}
            onEngineChange={setEngine}
            skills={skills}
            loading={skillsLoading}
            error={skillsError}
            search={skillSearch}
            onSearchChange={setSkillSearch}
            onRefresh={() => void refreshSkills()}
            onOpenSkill={openSkillDrawer}
            onCreate={() => setCreateOpen(true)}
            onMarket={() => setMarketOpen(true)}
          />
        ) : (
          <ConnectorsTab
            servers={servers}
            featured={filteredFeatured}
            loading={serversLoading}
            error={serversError}
            search={mcpSearch}
            onSearchChange={setMcpSearch}
            testedTools={testedTools}
            onRefresh={() => void refreshServers()}
            onToggle={handleToggleServer}
            onAdd={() => setMcpDraft({ ...EMPTY_DRAFT })}
            onAddTemplate={(name) => setMcpDraft(templateToDraft(name))}
            onImport={() => setImportOpen(true)}
            onOpenDrawer={(name) => {
              setDrawerServerName(name);
              const server = servers.find((item) => item.name === name);
              if (server && server.enabled && !testedTools[name]) void silentTest(server);
            }}
            onOpenFeatured={(template) => {
              setDrawerServerName(null);
              setDrawerFeatured(template);
            }}
            onInstallFeatured={installFeaturedDirect}
            installingFeatured={installingFeatured}
          />
        )}
      </div>

      {drawerSkill ? (
        <SkillDrawer
          skill={drawerSkill}
          engine={engine}
          source={skillSource}
          view={sourceView}
          onViewChange={setSourceView}
          onClose={() => setDrawerSkill(null)}
          onReveal={() => void handleRevealSkill(drawerSkill, skillSource.file?.path)}
          onUninstall={() => confirmUninstallSkill(drawerSkill)}
        />
      ) : null}

      {drawerFeatured ? (
        <ConnectorDrawer
          featured={drawerFeatured}
          installingName={installingFeatured}
          onClose={() => setDrawerFeatured(null)}
          onInstall={() => void installFeaturedDirect(drawerFeatured)}
          onConfigure={() => {
            setDrawerFeatured(null);
            setMcpDraft(templateToDraft(drawerFeatured.name));
          }}
        />
      ) : drawerServer ? (
        <ConnectorDrawer
          server={drawerServer}
          tools={testedTools[drawerServer.name]}
          testing={testing === drawerServer.name}
          onClose={() => setDrawerServerName(null)}
          onTest={() => void handleTest(drawerServer)}
          onDelete={() => confirmDeleteServer(drawerServer)}
        />
      ) : null}

      {createOpen ? (
        <CreateSkillDialog
          engine={engine}
          projectAvailable={Boolean(projectDir)}
          onClose={() => setCreateOpen(false)}
          onSubmit={handleCreateSkill}
        />
      ) : null}

      {marketOpen ? (
        <MarketDialog
          engine={engine}
          installedIds={new Set(skills.map((skill) => skill.id.replace(/^proj:/, '')))}
          projectDir={projectDir}
          projectAvailable={Boolean(projectDir)}
          onClose={() => setMarketOpen(false)}
          onInstalled={() => void refreshSkills()}
          onNotify={notify}
          onViewInstalled={(skillId) => {
            setMarketOpen(false);
            const skill = skills.find((item) => item.id.replace(/^proj:/, '') === skillId);
            if (skill) openSkillDrawer(skill);
          }}
        />
      ) : null}

      {mcpDraft ? (
        <ConnectorDialog
          draft={mcpDraft}
          onChange={setMcpDraft}
          onClose={() => setMcpDraft(null)}
          onSave={handleSaveDraft}
        />
      ) : null}

      {importOpen ? (
        <ImportDialog
          onClose={() => setImportOpen(false)}
          onNotify={notify}
          onChanged={() => void refreshServers()}
        />
      ) : null}

      {pendingConfirm ? (
        <ConfirmDialog
          title={pendingConfirm.title}
          body={pendingConfirm.body}
          confirmLabel={pendingConfirm.confirmLabel}
          onCancel={() => setPendingConfirm(null)}
          onConfirm={() => pendingConfirm.onConfirm().finally(() => setPendingConfirm(null))}
        />
      ) : null}
    </div>
  );
}

// ===== 草稿与映射 =====

function directoryOf(path: string): string | null {
  const normalized = path.replace(/\\/g, '/');
  const index = normalized.lastIndexOf('/');
  if (index <= 0) return null;
  return normalized.slice(0, index);
}

function rowsToMap(rows: EnvRow[]): Record<string, string> {
  const result: Record<string, string> = {};
  for (const row of rows) {
    const key = row.key.trim();
    if (!key) continue;
    result[key] = row.value;
  }
  return result;
}

function draftToServer(draft: McpDraft): McpServer {
  if (draft.transport === 'http') {
    return {
      name: draft.name.trim(),
      command: draft.command.trim(),
      args: [],
      env: {},
      headers: rowsToMap(draft.headerRows),
      transport: 'http',
      enabled: true,
      status: 'disconnected',
    };
  }
  return {
    name: draft.name.trim(),
    command: draft.command.trim(),
    args: draft.args
      .split('\n')
      .map((line) => line.trim())
      .filter(Boolean),
    env: rowsToMap(draft.envRows),
    headers: {},
    transport: 'stdio',
    enabled: true,
    status: 'disconnected',
  };
}

function templateToDraft(templateName: string): McpDraft {
  const template = FEATURED_CONNECTORS.find(
    (item) => item.name.toLowerCase() === templateName.toLowerCase(),
  );
  if (!template) return { ...EMPTY_DRAFT };
  return {
    name: template.name,
    transport: template.transport,
    command: template.transport === 'http' ? (template.url ?? '') : (template.command ?? ''),
    args: (template.args ?? []).join('\n'),
    envRows: (template.envKeys ?? []).map((entry) => ({ key: entry.key, value: '' })),
    headerRows: [{ key: '', value: '' }],
  };
}

// ===== 共用小组件 =====

function LoadErrorBanner({ error, onRetry }: { error: string; onRetry: () => void }) {
  return (
    <div className="ex-loaderror" role="alert">
      <Icon name="alert" />
      <span>加载失败：{error}</span>
      <button className="cm-action" type="button" onClick={onRetry}>
        <Icon name="refresh" /> 重试
      </button>
    </div>
  );
}

function SearchBox({
  placeholder,
  value,
  onChange,
  wide,
}: {
  placeholder: string;
  value: string;
  onChange: (value: string) => void;
  wide?: boolean;
}) {
  return (
    <label className={'cm-search' + (wide ? ' ex-search--wide' : '')}>
      <Icon name="search" />
      <input
        value={value}
        placeholder={placeholder}
        aria-label={placeholder}
        onChange={(event) => onChange(event.target.value)}
      />
    </label>
  );
}

function Switch({
  checked,
  disabled,
  title,
  onChange,
}: {
  checked: boolean;
  disabled?: boolean;
  title?: string;
  onChange: (checked: boolean) => void;
}) {
  return (
    <label className="cm-switch" title={title} onClick={(event) => event.stopPropagation()}>
      <input
        type="checkbox"
        checked={checked}
        disabled={disabled}
        onChange={(event) => onChange(event.target.checked)}
      />
      <i />
    </label>
  );
}

/** 卡片来源小标签用原型的短文案；market 与 plugin 统一显示「外部」（2026-08-27 决议）。 */
function sourceLabel(skill: Skill): string {
  if (skill.source === 'builtin') return '内置';
  if (skill.source === 'market' || skill.source === 'plugin') return '外部';
  return '自己创建';
}

function canUninstall(skill: Skill): boolean {
  return skill.source !== 'builtin' && skill.source !== 'plugin';
}

/** 连接器状态胶囊：视图模型 tone → 共享库 cm-status-pill 状态类。 */
function statusPillClass(tone: 'ok' | 'error' | 'muted'): string {
  if (tone === 'ok') return ' is-ready';
  if (tone === 'error') return ' is-danger';
  return '';
}

// ===== 技能 Tab =====

function SkillsTab({
  engine,
  onEngineChange,
  skills,
  loading,
  error,
  search,
  onSearchChange,
  onRefresh,
  onOpenSkill,
  onCreate,
  onMarket,
}: {
  engine: SkillEngine;
  onEngineChange: (engine: SkillEngine) => void;
  skills: Skill[];
  loading: boolean;
  error: string;
  search: string;
  onSearchChange: (value: string) => void;
  onRefresh: () => void;
  onOpenSkill: (skill: Skill) => void;
  onCreate: () => void;
  onMarket: () => void;
}) {
  const filtered = filterSkillsByQuery(skills, search);
  const sections = groupSkillsBySource(filtered);

  return (
    <section aria-label="技能">
      <div className="cm-toolbar">
        <div className="cm-toolbar__left">
          <div className="cm-segment ex-engine-seg" role="tablist" aria-label="技能引擎">
            <button
              type="button"
              role="tab"
              aria-selected={engine === 'claude-code'}
              className={engine === 'claude-code' ? 'is-active' : ''}
              onClick={() => onEngineChange('claude-code')}
            >
              <EngineBrand engine="claude-code" size={14} /> Claude Code
            </button>
            <button
              type="button"
              role="tab"
              aria-selected={engine === 'codex'}
              className={engine === 'codex' ? 'is-active' : ''}
              onClick={() => onEngineChange('codex')}
            >
              <EngineBrand engine="codex" size={14} /> Codex
            </button>
          </div>
          <SearchBox placeholder="搜索技能" value={search} onChange={onSearchChange} wide />
        </div>
        <div className="cm-toolbar__right">
          <button className="cm-action cm-action--primary" type="button" onClick={onMarket}>
            <Icon name="upright" /> 从外部安装
          </button>
          <button className="cm-action" type="button" onClick={onCreate}>
            <Icon name="plus" /> 创建技能
          </button>
        </div>
      </div>

      {loading ? (
        <div className="empty">正在读取{engineLabel(engine)}技能…</div>
      ) : error ? (
        <LoadErrorBanner error={error} onRetry={onRefresh} />
      ) : skills.length === 0 ? (
        <EmptyState
          icon="sparkles"
          title={engineLabel(engine) + ' 还没有技能'}
          hint="技能来自用户目录与项目目录的 skills 文件夹，也可以从市场安装现成的。"
          action={{ label: '从外部安装', onClick: onMarket }}
        />
      ) : sections.length === 0 ? (
        <div className="empty">没有匹配的技能，试试调整搜索条件</div>
      ) : (
        sections.map((section) => (
          <section key={section.id} className="cm-section">
            <div className="cm-section__head">
              <div>
                <h2>{section.title}</h2>
                <p>{section.hint}</p>
              </div>
              <span className="cm-source-label">{section.skills.length}</span>
            </div>
            <div className="cm-skill-grid">
              {section.skills.map((skill) => (
                <SkillCard key={skill.id} skill={skill} engine={engine} onOpen={onOpenSkill} />
              ))}
            </div>
          </section>
        ))
      )}
    </section>
  );
}

function SkillCard({
  skill,
  engine,
  onOpen,
}: {
  skill: Skill;
  engine: SkillEngine;
  onOpen: (skill: Skill) => void;
}) {
  return (
    // 卡片头对齐原型（品牌图标 + 来源标签）；启停入口已按 2026-08-27 反馈从卡片移除
    <article
      className={'cm-skill-card' + (skill.enabled ? '' : ' is-off')}
      onClick={() => onOpen(skill)}
    >
      <div className="cm-skill-card__head">
        <span className="cm-brand cm-brand--icon">
          <Icon name={skillCardIcon(skill)} />
        </span>
        <span className="cm-source-label">{sourceLabel(skill)}</span>
      </div>
      <h3>{skill.name}</h3>
      <p>{skill.description || '（无描述）'}</p>
      <div className="cm-skill-card__foot">
        <span className="mono">{triggerText(skill.trigger, engine) || '—'}</span>
        <span>{skill.scope === 'project' ? '项目' : '全局'}</span>
      </div>
    </article>
  );
}

function SkillDrawer({
  skill,
  engine,
  source,
  view,
  onViewChange,
  onClose,
  onReveal,
  onUninstall,
}: {
  skill: Skill;
  engine: SkillEngine;
  source: { state: 'loading' | 'ready' | 'error'; file?: SkillSourceFile; error?: string };
  view: 'preview' | 'source';
  onViewChange: (view: 'preview' | 'source') => void;
  onClose: () => void;
  onReveal: () => void;
  onUninstall: () => void;
}) {
  return (
    // 原型 #skillDrawer（extensions.html L749-L755）：cm-drawer 结构，React 条件挂载补 is-open
    <div className="cm-drawer-backdrop is-open ex-drawer-backdrop" onClick={onClose}>
      <aside
        className="cm-drawer ex-drawer"
        role="dialog"
        aria-modal="true"
        aria-label={skill.name}
        onClick={(event) => event.stopPropagation()}
      >
        <div className="cm-drawer__head">
          <div>
            <h2>{skill.name}</h2>
            <p>
              {sourceLabel(skill)} · {engineLabel(engine)} ·{' '}
              {skill.scope === 'project' ? '项目' : '全局'}
            </p>
          </div>
          <button className="btn-icon" type="button" aria-label="关闭" onClick={onClose}>
            <Icon name="x" />
          </button>
        </div>

        <div className="cm-drawer__body">
          <div className="cm-subtle-grid">
            <div className="cm-subtle-stat">
              <small>触发方式</small>
              <b className="mono">{triggerText(skill.trigger, engine) || '—'}</b>
            </div>
            <div className="cm-subtle-stat">
              <small>范围</small>
              <b>{skill.scope === 'project' ? '项目' : '全局'}</b>
            </div>
            <div className="cm-subtle-stat">
              <small>状态</small>
              <b>
                {skill.source === 'builtin' ? '内置 · 只读' : skill.enabled ? '已安装' : '已停用'}
              </b>
            </div>
          </div>

          <section className="cm-section">
            <div className="cm-section__head ex-md-head">
              <div>
                <h2>SKILL.md</h2>
              </div>
              <div className="cm-segment ex-view-seg">
                <button
                  type="button"
                  aria-pressed={view === 'preview'}
                  className={view === 'preview' ? 'is-active' : ''}
                  onClick={() => onViewChange('preview')}
                >
                  预览
                </button>
                <button
                  type="button"
                  aria-pressed={view === 'source'}
                  className={view === 'source' ? 'is-active' : ''}
                  onClick={() => onViewChange('source')}
                >
                  源码
                </button>
              </div>
            </div>
            {source.state === 'loading' ? (
              <div className="empty">正在读取技能文件…</div>
            ) : source.state === 'error' ? (
              <div className="empty">读取失败：{source.error}</div>
            ) : source.file?.truncated ? (
              <div className="empty">
                文件超过 256 KiB，为避免卡顿未在软件内展示。可用「打开所在位置」查看原文。
              </div>
            ) : view === 'source' ? (
              <div className="ex-code-source">
                <pre>{source.file?.content}</pre>
              </div>
            ) : (
              <MarkdownPreview content={source.file?.content ?? ''} />
            )}
          </section>
        </div>

        <div className="cm-panel__foot">
          <span className="ex-foot-path mono" title={source.file?.path ?? skill.path}>
            {source.file?.path ?? skill.path}
          </span>
          <button className="cm-action" type="button" onClick={onReveal}>
            <Icon name="folderopen" /> 打开所在位置
          </button>
          {canUninstall(skill) ? (
            <button className="cm-action" type="button" onClick={onUninstall}>
              <Icon name="trash" /> 卸载
            </button>
          ) : null}
        </div>
      </aside>
    </div>
  );
}

/** 轻量 Markdown 预览：只做行级结构（标题/列表/段落），不注入 HTML，避免脚本风险。 */
function MarkdownPreview({ content }: { content: string }) {
  const blocks: ReactNode[] = [];
  const lines = content.split('\n');
  let list: string[] = [];
  let ordered = false;

  const flushList = () => {
    if (list.length === 0) return;
    const items = list.map((item, index) => <li key={index}>{item}</li>);
    blocks.push(
      ordered ? <ol key={blocks.length}>{items}</ol> : <ul key={blocks.length}>{items}</ul>,
    );
    list = [];
  };

  for (const raw of lines) {
    const line = raw.trimEnd();
    const heading = /^(#{1,4})\s+(.*)$/.exec(line);
    const bullet = /^[-*]\s+(.*)$/.exec(line);
    const numbered = /^\d+[.)]\s+(.*)$/.exec(line);
    if (heading) {
      flushList();
      const level = heading[1].length;
      const text = heading[2];
      blocks.push(
        level <= 1 ? (
          <h3 key={blocks.length}>{text}</h3>
        ) : level === 2 ? (
          <h4 key={blocks.length}>{text}</h4>
        ) : (
          <b key={blocks.length} className="ex-preview-minor">
            {text}
          </b>
        ),
      );
    } else if (bullet || numbered) {
      const isOrdered = Boolean(numbered);
      if (list.length > 0 && ordered !== isOrdered) flushList();
      ordered = isOrdered;
      list.push((numbered ?? bullet)![1]);
    } else if (line.trim() === '') {
      flushList();
    } else {
      flushList();
      blocks.push(<p key={blocks.length}>{line}</p>);
    }
  }
  flushList();

  if (blocks.length === 0) {
    return <div className="ex-skill-preview faint">（文件为空）</div>;
  }
  return <div className="ex-skill-preview">{blocks}</div>;
}

// ===== 创建技能 / 从外部安装 =====

function CreateSkillDialog({
  engine,
  projectAvailable,
  onClose,
  onSubmit,
}: {
  engine: SkillEngine;
  projectAvailable: boolean;
  onClose: () => void;
  onSubmit: (request: CreateSkillRequest) => Promise<void>;
}) {
  const [name, setName] = useState('');
  const [slug, setSlug] = useState('');
  // 标识跟随名称自动生成；用户改过之后不再覆盖
  const [slugTouched, setSlugTouched] = useState(false);
  const [scope, setScope] = useState<'global' | 'project'>('global');
  const [description, setDescription] = useState('');
  const [instructions, setInstructions] = useState('');
  const [busy, setBusy] = useState(false);

  const effectiveSlug = slugTouched ? slugifySkillName(slug) : slugifySkillName(name || slug);
  const trigger = effectiveSlug
    ? triggerText('/' + effectiveSlug, engine)
    : engine === 'codex'
      ? '$标识'
      : '/标识';

  return (
    <Dialog
      title="创建技能"
      onClose={onClose}
      footer={
        <div className="ex-form-actions">
          <button className="cm-action" type="button" onClick={onClose}>
            取消
          </button>
          <button
            className="cm-action cm-action--primary"
            type="button"
            disabled={busy || !name.trim() || !effectiveSlug}
            onClick={() => {
              setBusy(true);
              void onSubmit({
                engine,
                scope,
                id: effectiveSlug,
                name: name.trim(),
                description: description.trim(),
                instructions,
              }).finally(() => setBusy(false));
            }}
          >
            {busy ? '创建中…' : '创建技能'}
          </button>
        </div>
      }
    >
      {/* 原型 #createSkill（L759）：desc + 触发词预览 跟在标题后 */}
      <p className="cm-pagehead__desc">保存为当前引擎的标准 SKILL.md。</p>
      <p className="ex-trigger-preview">
        {engineLabel(engine)} · 触发 {trigger}
      </p>
      <div className="cm-form ex-modal-form">
        <div className="cm-field">
          <label>名称</label>
          <input
            className="cm-input"
            value={name}
            placeholder="例如：发布检查"
            onChange={(event) => setName(event.target.value)}
          />
        </div>
        <div className="cm-field">
          <label>标识</label>
          <input
            className="cm-input mono"
            value={slugTouched ? slug : effectiveSlug}
            placeholder="release-check"
            onChange={(event) => {
              setSlugTouched(true);
              setSlug(event.target.value);
            }}
          />
          <small>只能用小写字母、数字和连字符；会作为目录名，一般不用改。</small>
        </div>
        <div className="cm-field">
          <label>范围</label>
          <div className="cm-segment ex-scope-seg">
            <button
              type="button"
              aria-pressed={scope === 'global'}
              className={scope === 'global' ? 'is-active' : ''}
              onClick={() => setScope('global')}
            >
              全局
            </button>
            <button
              type="button"
              aria-pressed={scope === 'project'}
              disabled={!projectAvailable}
              title={projectAvailable ? undefined : '先在设置里配置默认工作目录'}
              className={scope === 'project' ? 'is-active' : ''}
              onClick={() => setScope('project')}
            >
              当前项目
            </button>
          </div>
          <small className="ex-scope-note">{skillScopeNote(engine, scope)}</small>
        </div>
        <div className="cm-field">
          <label>一句话说明</label>
          <textarea
            className="cm-textarea"
            rows={2}
            value={description}
            placeholder="执行项目发布前的版本、构建和产物核对。"
            onChange={(event) => setDescription(event.target.value)}
          />
          <small>会出现在技能卡片和工作区的触发联想里。</small>
        </div>
        <div className="cm-field">
          <label>技能指令</label>
          <textarea
            className="cm-textarea mono ex-skill-textarea"
            rows={6}
            value={instructions}
            placeholder={
              '用 Markdown 写给 Agent 的步骤。\n例如：\n1. 核对版本号与 changelog 是否一致\n2. 跑构建，确认产物可发布'
            }
            onChange={(event) => setInstructions(event.target.value)}
          />
        </div>
      </div>
    </Dialog>
  );
}

function MarketDialog({
  engine,
  installedIds,
  projectDir,
  projectAvailable,
  onClose,
  onInstalled,
  onNotify,
  onViewInstalled,
}: {
  engine: SkillEngine;
  installedIds: Set<string>;
  projectDir: string;
  projectAvailable: boolean;
  onClose: () => void;
  onInstalled: () => void | Promise<void>;
  onNotify: (message: string) => void;
  onViewInstalled: (skillId: string) => void;
}) {
  const [query, setQuery] = useState('');
  const [results, setResults] = useState<MarketSkill[] | null>(null);
  const [searching, setSearching] = useState(false);
  const [hotMode, setHotMode] = useState(true);
  const [installing, setInstalling] = useState<string | null>(null);
  const [scope, setScope] = useState<'global' | 'project'>('global');

  async function runSearch(keyword: string, hot = false) {
    if (!keyword.trim()) {
      onNotify('请输入搜索关键词，数据来自 skills.sh 公开目录');
      return;
    }
    setSearching(true);
    try {
      setResults(await marketSearchSkills(keyword.trim()));
      setHotMode(hot);
    } catch (err) {
      console.error('搜索技能市场失败:', err);
      if (!hot) onNotify(`搜索技能市场失败：${err}`);
    } finally {
      setSearching(false);
    }
  }

  // 打开即拉取 skills.sh 实时热门（真实安装量数据，非本地伪造），按安装量排序展示。
  useEffect(() => {
    void runSearch('skill', true);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const shown = results ? [...results].sort((a, b) => b.installs - a.installs) : null;

  async function install(skill: MarketSkill) {
    setInstalling(skill.skillId);
    try {
      await marketInstallSkill(
        skill.source,
        skill.skillId,
        scope,
        scope === 'project' ? projectDir || undefined : undefined,
      );
      onNotify(`已安装 ${skill.name} 到 ${engineLabel(engine)}`);
      await onInstalled();
    } catch (err) {
      console.error('安装技能失败:', err);
      onNotify(`安装技能失败：${err}`);
    } finally {
      setInstalling(null);
    }
  }

  return (
    <Dialog title="从外部安装" onClose={onClose}>
      {/* 原型 #skillMarket（L757）：desc / 宽搜索 / 安装到范围 / 热门列表 */}
      <p className="cm-pagehead__desc">安装到 {engineLabel(engine)} · 数据来自 skills.sh</p>
      <div className="ex-market-bar">
        <SearchBox
          placeholder="搜索技能，例如 review、docs、testing"
          value={query}
          onChange={setQuery}
          wide
        />
        {/* 搜索按钮为实现适配：skills.sh 需显式查询（2026-08-27 决议：保留） */}
        <button
          className="cm-action"
          type="button"
          disabled={searching}
          onClick={() => void runSearch(query)}
        >
          搜索
        </button>
      </div>
      <div className="cm-field">
        <label>安装到</label>
        <div className="cm-segment ex-scope-seg">
          <button
            type="button"
            aria-pressed={scope === 'global'}
            className={scope === 'global' ? 'is-active' : ''}
            onClick={() => setScope('global')}
          >
            全局
          </button>
          <button
            type="button"
            aria-pressed={scope === 'project'}
            disabled={!projectAvailable}
            title={projectAvailable ? undefined : '先在设置里配置默认工作目录'}
            className={scope === 'project' ? 'is-active' : ''}
            onClick={() => setScope('project')}
          >
            当前项目
          </button>
        </div>
        <small className="ex-scope-note">
          只影响本次安装；当前项目写入工作目录的 .claude\skills（Codex 为 .codex\skills）。
        </small>
      </div>

      <div className="cm-list">
        {searching ? (
          <div className="empty">正在搜索 skills.sh…</div>
        ) : shown === null ? (
          <p className="faint">
            输入关键词搜索 skills.sh 的公开技能目录；安装会把 SKILL.md 写入本机技能文件夹。
          </p>
        ) : (
          <>
            <div className="cm-section__head ex-md-head">
              <div>
                <h2>{hotMode ? '全站热门' : '搜索结果'}</h2>
                <p>
                  {hotMode
                    ? '按安装量排序 · 结果来自外部市场，不表示当前引擎专属兼容。'
                    : '结果来自外部市场，不表示当前引擎专属兼容。'}
                </p>
              </div>
            </div>
            {shown.length === 0 ? (
              <div className="empty">没有匹配的技能，换个关键词试试。</div>
            ) : (
              shown.map((skill) => {
                const installed = installedIds.has(skill.skillId);
                return (
                  <div key={`${skill.source}/${skill.skillId}`} className="cm-list__row">
                    <span className="cm-brand cm-brand--icon">
                      <Icon name={marketRowIcon(skill.name)} />
                    </span>
                    <div className="cm-list__main">
                      <b>{skill.name}</b>
                      <small>
                        {skill.source} · {skill.installs.toLocaleString()} 次安装
                      </small>
                      {skill.description ? (
                        <p className="ex-market-desc">{skill.description}</p>
                      ) : null}
                    </div>
                    {installed ? (
                      <button
                        type="button"
                        className="cm-action"
                        onClick={() => onViewInstalled(skill.skillId)}
                      >
                        已安装 · 查看
                      </button>
                    ) : (
                      <button
                        className="cm-action cm-action--primary"
                        type="button"
                        disabled={installing === skill.skillId}
                        onClick={() => void install(skill)}
                      >
                        {installing === skill.skillId ? '安装中…' : '安装'}
                      </button>
                    )}
                  </div>
                );
              })
            )}
          </>
        )}
      </div>
    </Dialog>
  );
}

// ===== 连接器 Tab =====

function ConnectorsTab({
  servers,
  featured,
  loading,
  error,
  search,
  onSearchChange,
  testedTools,
  onRefresh,
  onToggle,
  onAdd,
  onAddTemplate,
  onImport,
  onOpenDrawer,
  onOpenFeatured,
  onInstallFeatured,
  installingFeatured,
}: {
  servers: McpServer[];
  featured: FeaturedCardState[];
  loading: boolean;
  error: string;
  search: string;
  onSearchChange: (value: string) => void;
  testedTools: Record<string, McpTool[]>;
  onRefresh: () => void;
  onToggle: (server: McpServer, enabled: boolean) => Promise<void>;
  onAdd: () => void;
  onAddTemplate: (name: string) => void;
  onImport: () => void;
  onOpenDrawer: (name: string) => void;
  onOpenFeatured: (template: FeaturedConnectorTemplate) => void;
  onInstallFeatured: (template: FeaturedConnectorTemplate) => Promise<void>;
  installingFeatured: string | null;
}) {
  const keyword = search.trim().toLowerCase();
  const filteredServers = keyword
    ? servers.filter((server) =>
        `${server.name} ${transportLabel(server.transport)} ${server.command}`
          .toLowerCase()
          .includes(keyword),
      )
    : servers;

  return (
    <section aria-label="连接器">
      <div className="cm-toolbar">
        <div className="cm-toolbar__left">
          <SearchBox placeholder="搜索连接器" value={search} onChange={onSearchChange} wide />
        </div>
        <div className="cm-toolbar__right">
          <button className="cm-action cm-action--quiet" type="button" onClick={onImport}>
            <Icon name="code" /> 导入 JSON
          </button>
          <button className="cm-action cm-action--primary" type="button" onClick={onAdd}>
            <Icon name="plus" /> 添加连接器
          </button>
        </div>
      </div>

      {loading ? (
        <div className="empty">正在读取连接器…</div>
      ) : error ? (
        <LoadErrorBanner error={error} onRetry={onRefresh} />
      ) : (
        <>
          {featured.length > 0 ? (
            <section className="cm-section">
              <div className="cm-section__head">
                <div>
                  <h2>精选</h2>
                  {/* 分区说明保留实现文案（2026-08-27 决议：②以实现为准） */}
                  <p>常见连接器的安装模板；安装后会真实写入双引擎配置并检测连接。</p>
                </div>
                <span className="cm-source-label">{featured.length}</span>
              </div>
              <div className="cm-skill-grid">
                {featured.map((state) => {
                  const brandSrc = FEATURED_BRANDS[state.template.name.toLowerCase()];
                  return (
                    <article
                      key={state.template.name}
                      className="cm-market-card"
                      onClick={() =>
                        state.installed
                          ? onOpenDrawer(state.template.name)
                          : onOpenFeatured(state.template)
                      }
                    >
                      <span className="cm-brand cm-brand--light">
                        {brandSrc ? (
                          <img src={brandSrc} alt="" />
                        ) : (
                          <Icon name={state.template.transport === 'http' ? 'shield' : 'plug'} />
                        )}
                      </span>
                      <h3>{state.template.displayName ?? state.template.name}</h3>
                      <p>{state.template.description}</p>
                      <div className="cm-market-card__foot">
                        <span className="cm-market-card__meta">
                          {transportLabel(state.template.transport)}
                        </span>
                        {state.installed && state.enabled ? (
                          <button
                            className="cm-action"
                            type="button"
                            onClick={(event) => {
                              event.stopPropagation();
                              onOpenDrawer(state.template.name);
                            }}
                          >
                            已启用
                          </button>
                        ) : state.installed ? (
                          <button
                            className="cm-action"
                            type="button"
                            onClick={(event) => {
                              event.stopPropagation();
                              onOpenDrawer(state.template.name);
                            }}
                          >
                            已停用
                          </button>
                        ) : (state.template.envKeys?.length ?? 0) > 0 ? (
                          <button
                            className="cm-action cm-action--primary"
                            type="button"
                            onClick={(event) => {
                              event.stopPropagation();
                              onAddTemplate(state.template.name);
                            }}
                          >
                            安装并配置
                          </button>
                        ) : (
                          <button
                            className="cm-action cm-action--primary"
                            type="button"
                            disabled={installingFeatured === state.template.name}
                            onClick={(event) => {
                              event.stopPropagation();
                              void onInstallFeatured(state.template);
                            }}
                          >
                            {installingFeatured === state.template.name ? '安装中…' : '安装'}
                          </button>
                        )}
                      </div>
                    </article>
                  );
                })}
              </div>
            </section>
          ) : null}

          {filteredServers.length > 0 || !keyword ? (
            <section className="cm-section">
              <div className="cm-section__head">
                <div>
                  <h2>已安装</h2>
                  {/* 分区说明保留实现文案（2026-08-27 决议：②以实现为准） */}
                  <p>开关 = 配置保留但不注入引擎；卸载在抽屉里。状态来自最近一次真实检测。</p>
                </div>
                <span className="cm-source-label">{filteredServers.length}</span>
              </div>
              {filteredServers.length === 0 ? (
                <EmptyState
                  icon="plug"
                  title="还没有连接器"
                  hint="从精选模板安装，或添加自定义 stdio / http 连接器；也可以粘贴 mcpServers JSON 批量导入。"
                  action={{ label: '添加连接器', onClick: onAdd }}
                />
              ) : (
                <div className="ex-mcp-card-grid">
                  {filteredServers.map((server) => {
                    const pill = server.enabled
                      ? connectorStatusPill(server)
                      : { label: '已停用', tone: 'muted' as const };
                    const tools = testedTools[server.name];
                    return (
                      <article
                        key={server.name}
                        className={'ex-mcp-card' + (server.enabled ? '' : ' is-off')}
                        onClick={() => onOpenDrawer(server.name)}
                      >
                        <div className="ex-mcp-card__top">
                          <span className="cm-brand cm-brand--light">
                            {FEATURED_BRANDS[server.name.toLowerCase()] ? (
                              <img src={FEATURED_BRANDS[server.name.toLowerCase()]} alt="" />
                            ) : (
                              <Icon name={server.transport === 'http' ? 'shield' : 'terminal'} />
                            )}
                          </span>
                          <div className="ex-mcp-card__main">
                            <b>{server.name}</b>
                            <span>
                              {transportLabel(server.transport)} ·{' '}
                              {tools?.length ?? server.toolCount ?? '?'} 个工具 ·{' '}
                              {formatTestedAt(server.lastTestedAt) || '未检测'}
                            </span>
                          </div>
                          <span className={'cm-status-pill' + statusPillClass(pill.tone)}>
                            {pill.label}
                          </span>
                          <Switch
                            checked={server.enabled}
                            onChange={(enabled) => void onToggle(server, enabled)}
                          />
                        </div>
                      </article>
                    );
                  })}
                </div>
              )}
            </section>
          ) : null}
        </>
      )}
    </section>
  );
}

function ConnectorDrawer({
  server,
  featured,
  tools,
  testing,
  onClose,
  onTest,
  onDelete,
  onInstall,
  onConfigure,
  installingName,
}: {
  server?: McpServer;
  featured?: FeaturedConnectorTemplate;
  tools?: McpTool[];
  testing?: boolean;
  onClose: () => void;
  onTest?: () => void;
  onDelete?: () => void;
  onInstall?: () => void;
  onConfigure?: () => void;
  installingName?: string | null;
}) {
  if (featured) {
    const isHttp = featured.transport === 'http';
    const needsConfig = (featured.envKeys?.length ?? 0) > 0;
    const busy = installingName === featured.name;
    return (
      // 原型 #mcpDrawer（extensions.html L761）：精选态只有主操作按钮
      <div className="cm-drawer-backdrop is-open ex-drawer-backdrop" onClick={onClose}>
        <aside
          className="cm-drawer ex-drawer"
          role="dialog"
          aria-modal="true"
          aria-label={featured.displayName ?? featured.name}
          onClick={(event) => event.stopPropagation()}
        >
          <div className="cm-drawer__head">
            <div>
              <h2>{featured.displayName ?? featured.name}</h2>
              <p>精选 · {transportLabel(featured.transport)}</p>
            </div>
            <button className="btn-icon" type="button" aria-label="关闭" onClick={onClose}>
              <Icon name="x" />
            </button>
          </div>

          <div className="cm-drawer__body">
            <div className="cm-note">
              <Icon name={isHttp ? 'shield' : 'terminal'} />
              {isHttp ? (
                <span>通过远程地址连接；请求头中的密钥进系统钥匙串，写操作仍走审批。</span>
              ) : (
                <span>在本机启动本地进程，不经过账号授权；进程能力仍由当前任务的权限控制。</span>
              )}
            </div>

            <section className="cm-section">
              <div className="cm-section__head">
                <div>
                  <h2>同步状态</h2>
                  {/* 分区说明保留实现文案（2026-08-27 决议：②以实现为准） */}
                  <p>Helm 一个定义同时写入双引擎；安装后默认启用并自动检测。</p>
                </div>
              </div>
              <div className="cm-subtle-grid">
                <div className="cm-subtle-stat">
                  <small>定义范围</small>
                  <b>Claude Code + Codex</b>
                </div>
                <div className="cm-subtle-stat">
                  <small>注入状态</small>
                  <b>安装后启用</b>
                </div>
                <div className="cm-subtle-stat">
                  <small>最近检测</small>
                  <b>未检测</b>
                </div>
              </div>
            </section>

            <section className="cm-section">
              <div className="cm-section__head">
                <div>
                  <h2>工具列表</h2>
                </div>
              </div>
              <p className="faint">安装并授权后可见。</p>
            </section>
          </div>

          <div className="cm-panel__foot">
            <span className="grow" />
            <button
              className="cm-action cm-action--primary"
              type="button"
              disabled={busy}
              onClick={needsConfig ? onConfigure : onInstall}
            >
              {busy ? '安装中…' : needsConfig ? '安装并配置' : '安装并检测'}
            </button>
          </div>
        </aside>
      </div>
    );
  }
  if (!server) return null;
  const pill = connectorStatusPill(server);
  return (
    // 原型 #mcpDrawer（extensions.html L761）：已安装态 卸载/检测 + 状态胶囊；
    // 「编辑」按钮按 2026-08-27 决议移除。
    <div className="cm-drawer-backdrop is-open ex-drawer-backdrop" onClick={onClose}>
      <aside
        className="cm-drawer ex-drawer"
        role="dialog"
        aria-modal="true"
        aria-label={server.name}
        onClick={(event) => event.stopPropagation()}
      >
        <div className="cm-drawer__head">
          <div>
            <h2>{server.name}</h2>
            <p>{transportLabel(server.transport)}</p>
          </div>
          <button className="btn-icon" type="button" aria-label="关闭" onClick={onClose}>
            <Icon name="x" />
          </button>
        </div>

        <div className="cm-drawer__body">
          <div className="cm-note">
            <Icon name={server.transport === 'http' ? 'shield' : 'terminal'} />
            {server.transport === 'http' ? (
              <span>
                远程连接：配置同时保存到 Claude Code（含请求头）与 Codex（仅 URL，Codex
                不支持自定义请求头）。
              </span>
            ) : (
              <span>本地进程：连接时在本机启动命令行程序；写操作仍走会话审批。</span>
            )}
          </div>

          <section className="cm-section">
            <div className="cm-section__head">
              <div>
                <h2>同步状态</h2>
                {/* 分区说明保留实现文案（2026-08-27 决议：②以实现为准） */}
                <p>启用状态与最近一次检测分别核对。</p>
              </div>
            </div>
            <div className="cm-subtle-grid">
              <div className="cm-subtle-stat">
                <small>定义范围</small>
                <b>Claude Code + Codex</b>
              </div>
              <div className="cm-subtle-stat">
                <small>注入状态</small>
                <b>{server.enabled ? '已启用' : '已停用（配置保留）'}</b>
              </div>
              <div className="cm-subtle-stat">
                <small>最近检测</small>
                <b>{formatTestedAt(server.lastTestedAt) || '未检测'}</b>
              </div>
            </div>
            {server.lastError ? (
              <p className="ex-error-line">上次错误：{server.lastError}</p>
            ) : null}
          </section>

          <section className="cm-section">
            <div className="cm-section__head">
              <div>
                <h2>工具列表</h2>
              </div>
            </div>
            {testing ? (
              <p className="faint">正在获取工具列表…</p>
            ) : tools ? (
              tools.length === 0 ? (
                <p className="faint">该连接器没有暴露任何工具。</p>
              ) : (
                <div className="ex-tool-list">
                  {tools.map((tool) => (
                    <div key={tool.name} className="ex-tool-row">
                      <span className="ex-tool-dot" />
                      <div className="ex-tool-main">
                        <b className="mono">{tool.name}</b>
                        {tool.description ? (
                          <small title={tool.description}>{tool.description}</small>
                        ) : null}
                      </div>
                    </div>
                  ))}
                </div>
              )
            ) : (
              <p className="faint">
                {server.toolCount
                  ? '上次检测到 ' + server.toolCount + ' 个工具；点「检测」获取本次会话的工具列表。'
                  : '还没有本次会话内的检测结果；点「检测」获取真实工具列表。'}
              </p>
            )}
          </section>
        </div>

        <div className="cm-panel__foot">
          <span className={'cm-status-pill' + statusPillClass(pill.tone)}>{pill.label}</span>
          <span className="grow" />
          <button
            className="cm-action"
            type="button"
            disabled={testing || !server.enabled}
            title={server.enabled ? undefined : '已停用的连接器先启用再检测'}
            onClick={onTest}
          >
            <Icon name="refresh" /> {testing ? '检测中…' : '检测'}
          </button>
          <button className="cm-action cm-action--danger" type="button" onClick={onDelete}>
            <Icon name="trash" /> 卸载
          </button>
        </div>
      </aside>
    </div>
  );
}

// ===== 添加 / 编辑连接器 =====

function ConnectorDialog({
  draft,
  onChange,
  onClose,
  onSave,
}: {
  draft: McpDraft;
  onChange: (draft: McpDraft) => void;
  onClose: () => void;
  onSave: (draft: McpDraft, autoTest: boolean) => Promise<void>;
}) {
  const [busy, setBusy] = useState(false);
  const isHttp = draft.transport === 'http';

  function updateTransport(transport: 'stdio' | 'http') {
    onChange({ ...draft, transport });
  }

  return (
    <Dialog
      title="添加连接器"
      onClose={onClose}
      footer={
        <div className="ex-form-actions">
          <button className="cm-action" type="button" onClick={onClose}>
            取消
          </button>
          <button
            className="cm-action cm-action--primary"
            type="button"
            disabled={busy || !draft.name.trim() || !draft.command.trim()}
            onClick={() => {
              setBusy(true);
              void onSave(draft, true).finally(() => setBusy(false));
            }}
          >
            {busy ? '保存中…' : '保存并检测'}
          </button>
        </div>
      }
    >
      {/* 原型 #customMcp（L763）：接入方式双卡 + 按传输切换字段 + 键值行分组表 */}
      <p className="cm-pagehead__desc">保存后分别写入 Claude Code 与 Codex，默认双引擎同步。</p>
      <div className="cm-form ex-modal-form">
        <div className="cm-field">
          <label>名称</label>
          <input
            className="cm-input mono"
            value={draft.name}
            placeholder="github"
            onChange={(event) => onChange({ ...draft, name: event.target.value })}
          />
        </div>

        <div className="cm-field">
          <label>接入方式</label>
          <div className="ex-type-grid">
            <button
              type="button"
              aria-pressed={!isHttp}
              className={'ex-type-card' + (!isHttp ? ' is-active' : '')}
              onClick={() => updateTransport('stdio')}
            >
              <b>
                <Icon name="terminal" />
                本地进程
              </b>
              <small>
                在本机启动一个命令行程序（对应 STDIO），适合 npx / node 启动的开源服务。
              </small>
            </button>
            <button
              type="button"
              aria-pressed={isHttp}
              className={'ex-type-card' + (isHttp ? ' is-active' : '')}
              onClick={() => updateTransport('http')}
            >
              <b>
                <Icon name="upright" />
                远程地址
              </b>
              <small>连接一个 HTTPS 服务（对应 Streamable HTTP），无需本机安装依赖。</small>
            </button>
          </div>
        </div>

        {isHttp ? (
          <div className="cm-field">
            <label>服务地址</label>
            <input
              className="cm-input mono"
              value={draft.command}
              placeholder="https://example.com/mcp"
              onChange={(event) => onChange({ ...draft, command: event.target.value })}
            />
            {draft.command.trim() &&
            !draft.command.trim().startsWith('http://') &&
            !draft.command.trim().startsWith('https://') ? (
              <small className="ex-error-line">远程地址必须以 http:// 或 https:// 开头</small>
            ) : null}
          </div>
        ) : (
          <div className="cm-field">
            <label>启动命令</label>
            <input
              className="cm-input mono"
              value={draft.command}
              placeholder="npx"
              onChange={(event) => onChange({ ...draft, command: event.target.value })}
            />
            <small>只填可执行文件本身；参数写在下面。</small>
          </div>
        )}

        {!isHttp ? (
          <div className="cm-field">
            <label>参数（每行一个）</label>
            <textarea
              className="cm-textarea mono"
              rows={3}
              value={draft.args}
              placeholder={'-y\n@modelcontextprotocol/server-github'}
              onChange={(event) => onChange({ ...draft, args: event.target.value })}
            />
          </div>
        ) : null}

        {!isHttp ? (
          <EnvRowsField
            label="环境变量（可选）"
            rows={draft.envRows}
            onRowsChange={(envRows) => onChange({ ...draft, envRows })}
            keyPlaceholder="API_KEY"
            valuePlaceholder="值 · 存入系统钥匙串"
            addLabel="添加环境变量"
          />
        ) : (
          <EnvRowsField
            label="请求头（可选）"
            rows={draft.headerRows}
            onRowsChange={(headerRows) => onChange({ ...draft, headerRows })}
            keyPlaceholder="Authorization"
            valuePlaceholder="Bearer … · 存入系统钥匙串"
            addLabel="添加请求头"
            hint="凭证进系统钥匙串；远程连接器不需要环境变量。"
          />
        )}

        <p className="ex-keynote">
          <Icon name="key" />
          凭证类字段（TOKEN / KEY / SECRET
          等）的值会写入系统钥匙串用于回填与清理；同时按引擎原生格式同步到本机引擎配置文件供 CLI
          启动连接器进程。不进入 Helm 数据库、日志或导出包。
        </p>
      </div>
    </Dialog>
  );
}

function EnvRowsField({
  label,
  rows,
  onRowsChange,
  keyPlaceholder = '变量名，如 API_TOKEN',
  valuePlaceholder = '值',
  addLabel = '添加一行',
  hint,
}: {
  label: string;
  rows: EnvRow[];
  onRowsChange: (rows: EnvRow[]) => void;
  keyPlaceholder?: string;
  valuePlaceholder?: string;
  addLabel?: string;
  hint?: string;
}) {
  function updateRow(index: number, patch: Partial<EnvRow>) {
    onRowsChange(rows.map((row, i) => (i === index ? { ...row, ...patch } : row)));
  }

  return (
    <div className="cm-field">
      <label>{label}</label>
      <div className="ex-env">
        {rows.map((row, index) => {
          const credential = isCredentialKey(row.key);
          return (
            <div key={index} className="ex-env__row">
              <input
                className="cm-input mono"
                value={row.key}
                placeholder={keyPlaceholder}
                aria-label="名称"
                onChange={(event) => updateRow(index, { key: event.target.value })}
              />
              <SecretInput
                value={row.value}
                credential={credential}
                placeholder={valuePlaceholder}
                onChange={(value) => updateRow(index, { value })}
              />
              <button
                type="button"
                className="btn-icon sm"
                aria-label="删除此行"
                onClick={() =>
                  onRowsChange(
                    rows.length === 1
                      ? [{ key: '', value: '' }]
                      : rows.filter((_, i) => i !== index),
                  )
                }
              >
                <Icon name="x" />
              </button>
            </div>
          );
        })}
        <button
          type="button"
          className="ex-env__add"
          onClick={() => onRowsChange([...rows, { key: '', value: '' }])}
        >
          <Icon name="plus" /> {addLabel}
        </button>
      </div>
      {hint ? <small>{hint}</small> : null}
    </div>
  );
}

function SecretInput({
  value,
  credential,
  placeholder,
  onChange,
}: {
  value: string;
  credential: boolean;
  placeholder?: string;
  onChange: (value: string) => void;
}) {
  const [revealed, setRevealed] = useState(false);
  return (
    <span className="ex-secret">
      <input
        type={revealed ? 'text' : 'password'}
        value={value}
        placeholder={placeholder ?? (credential ? '值 · 存入系统钥匙串' : '值')}
        aria-label="值"
        autoComplete="off"
        onChange={(event) => onChange(event.target.value)}
      />
      <button
        type="button"
        className="btn-icon"
        aria-label={revealed ? '隐藏值' : '显示值'}
        onClick={() => setRevealed((prev) => !prev)}
      >
        <Icon name={revealed ? 'eyeoff' : 'eye'} />
      </button>
    </span>
  );
}

// ===== JSON 导入 =====

function ImportDialog({
  onClose,
  onNotify,
  onChanged,
}: {
  onClose: () => void;
  onNotify: (message: string) => void;
  onChanged: () => void | Promise<void>;
}) {
  const [json, setJson] = useState('');
  const [results, setResults] = useState<McpImportItemResult[] | null>(null);
  const [busy, setBusy] = useState(false);
  const [retrying, setRetrying] = useState<string | null>(null);
  const rows: ImportResultRow[] = results ? importResultRows(results) : [];

  async function runImport() {
    if (!json.trim()) {
      onNotify('请先粘贴 mcpServers JSON');
      return;
    }
    setBusy(true);
    try {
      setResults(await importMcpServers(json));
      await onChanged();
    } catch (err) {
      console.error('导入连接器失败:', err);
      onNotify(`导入失败：${err}`);
    } finally {
      setBusy(false);
    }
  }

  async function retry(row: ImportResultRow) {
    const original = results?.find(
      (item) => item.name === row.name && item.status === 'failed' && item.server,
    );
    if (!original?.server) return;
    setRetrying(row.name);
    try {
      await saveMcpServer(original.server);
      setResults((prev) =>
        (prev ?? []).map((item) =>
          item.name === row.name ? { ...item, status: 'imported', message: null } : item,
        ),
      );
      onNotify(`${row.name} 已重试写入`);
      await onChanged();
    } catch (err) {
      console.error('重试导入失败:', err);
      onNotify(`重试失败：${err}`);
    } finally {
      setRetrying(null);
    }
  }

  return (
    <Dialog
      title="从 JSON 导入连接器"
      onClose={onClose}
      footer={
        <div className="ex-form-actions">
          <button className="cm-action" type="button" onClick={onClose}>
            取消
          </button>
          <button
            className="cm-action cm-action--primary"
            type="button"
            disabled={busy}
            onClick={() => void runImport()}
          >
            {busy ? '导入中…' : '导入并检测'}
          </button>
        </div>
      }
    >
      {/* 原型 #importMcp（L764）：说明 + mono 大文本域 */}
      <p className="cm-pagehead__desc">
        粘贴 Claude Code 的 mcpServers 配置；Helm 会逐项写入双引擎并转换成 Codex
        TOML。一次可导入多个；type=sse 会跳过并在结果里说明。
      </p>
      <textarea
        className="cm-textarea mono ex-import-textarea"
        rows={10}
        value={json}
        spellCheck={false}
        placeholder={
          '{\n  "mcpServers": {\n    "context7": { "type": "http", "url": "https://mcp.context7.com/mcp" }\n  }\n}'
        }
        onChange={(event) => setJson(event.target.value)}
      />

      {results ? (
        <div className="ex-result-list" aria-live="polite">
          {rows.length === 0 ? (
            <div className="empty">没有解析到任何条目。</div>
          ) : (
            rows.map((row) => (
              <div
                key={row.name || row.message}
                className={`ex-result-row ex-result-row--${row.status}`}
              >
                <Icon
                  name={
                    row.status === 'imported'
                      ? 'checkc'
                      : row.status === 'skipped'
                        ? 'info'
                        : 'alert'
                  }
                />
                <div className="ex-result-row__main">
                  <b>{row.name}</b>
                  <span>
                    {row.status === 'imported' && row.credentialKeys.length > 0
                      ? `已写入双引擎 · 凭证 ${row.credentialKeys.join('、')} 已存入系统钥匙串`
                      : row.status === 'imported'
                        ? '已写入双引擎'
                        : row.message}
                  </span>
                </div>
                {row.canRetry ? (
                  <button
                    className="cm-action"
                    type="button"
                    disabled={retrying === row.name}
                    onClick={() => void retry(row)}
                  >
                    {retrying === row.name ? '重试中…' : '重试'}
                  </button>
                ) : null}
              </div>
            ))
          )}
        </div>
      ) : null}
    </Dialog>
  );
}
