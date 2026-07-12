import { useEffect, useState } from 'react';
import { Icon } from '../shell/icons';
import { showResultToast } from '../components/toast';
import { EmptyState } from '../components/EmptyState';
import { useDialogBehavior } from '../components/useDialogBehavior';
import { loadSettings, selectDirectory } from '../settings/api';
import { getProviderConfig } from '../providers/api';
import { modelCatalog } from '../providers/providerViewModel';
import {
  deleteHook,
  deleteMcpServer,
  deleteSlashCommand,
  deleteSubagent,
  listHooks,
  listMcpServers,
  listSkills,
  listSlashCommands,
  listSubagents,
  saveHook,
  saveMcpServer,
  saveSlashCommand,
  saveSubagent,
  marketInstallSkill,
  marketSearchSkills,
  testMcpConnection,
  toggleSkill,
  HOOK_EVENTS,
  type Hook,
  type MarketSkill,
  type McpServer,
  type McpTool,
  type Skill,
  type SlashCommand,
  type Subagent,
} from './extensionsApi';

type TabId = 'skills' | 'mcp' | 'subagents' | 'commands' | 'hooks';
type SkillFilter = 'all' | 'on' | 'global' | 'project';
type McpFilter = 'all' | 'on' | 'off';

interface McpDraft {
  name: string;
  transport: 'stdio' | 'sse' | 'http';
  command: string;
  args: string;
  env: string;
}

interface SubagentDraft {
  id: string;
  name: string;
  model: string;
  role: string;
  tools: string;
  auto: boolean;
  prompt: string;
  scope: 'global' | 'project';
}

interface CommandDraft {
  id: string;
  trigger: string;
  description: string;
  scope: 'global' | 'project';
  enabled: boolean;
  body: string;
  engine: 'all' | 'claude-code' | 'codex';
  argumentHint?: string;
}

interface HookDraft {
  id: string;
  event: Hook['event'];
  match: string;
  command: string;
  description: string;
  enabled: boolean;
  scope: 'global' | 'project';
}

interface PendingConfirm {
  title: string;
  body: string;
  confirmLabel: string;
  onConfirm: () => Promise<void>;
}

const EMPTY_MCP: McpDraft = {
  name: '',
  transport: 'stdio',
  command: '',
  args: '',
  env: '',
};

const EMPTY_SUBAGENT: SubagentDraft = {
  id: '',
  name: '',
  model: '',
  role: '',
  tools: 'Read,Grep,Glob',
  auto: true,
  prompt: '',
  scope: 'global',
};

const EMPTY_COMMAND: CommandDraft = {
  id: '',
  trigger: '/',
  description: '',
  scope: 'global',
  enabled: true,
  body: '',
  engine: 'all',
  argumentHint: '',
};

const EMPTY_HOOK: HookDraft = {
  id: '',
  event: 'PreToolUse',
  match: '*',
  command: '',
  description: '',
  enabled: true,
  scope: 'global',
};

export function ExtensionsPage() {
  const [tab, setTab] = useState<TabId>('skills');
  const [skills, setSkills] = useState<Skill[]>([]);
  const [mcpServers, setMcpServers] = useState<McpServer[]>([]);
  const [subagents, setSubagents] = useState<Subagent[]>([]);
  const [slashCommands, setSlashCommands] = useState<SlashCommand[]>([]);
  const [hooks, setHooks] = useState<Hook[]>([]);
  const [mcpTools, setMcpTools] = useState<Record<string, McpTool[]>>({});
  const [loading, setLoading] = useState(true);
  const [marketOpen, setMarketOpen] = useState(false);
  const [mcpDraft, setMcpDraft] = useState<McpDraft | null>(null);
  const [subagentDraft, setSubagentDraft] = useState<SubagentDraft | null>(null);
  const [commandDraft, setCommandDraft] = useState<CommandDraft | null>(null);
  const [hookDraft, setHookDraft] = useState<HookDraft | null>(null);
  const [pendingConfirm, setPendingConfirm] = useState<PendingConfirm | null>(null);
  // 加载失败与真空态区分（P2-3）：记录读取失败的来源，横幅展示并可重试
  const [loadErrors, setLoadErrors] = useState<string[]>([]);
  // 项目级作用域上下文（变更-05）：默认取设置的默认工作目录，可切换
  const [projectDir, setProjectDir] = useState('');
  const [projectDirReady, setProjectDirReady] = useState(false);
  // 子代理模型候选：来自服务商模型目录
  const [modelOptions, setModelOptions] = useState<string[]>([]);

  useEffect(() => {
    void (async () => {
      try {
        const settings = await loadSettings();
        setProjectDir(settings.general.defaultDirectory.trim());
      } catch (err) {
        console.error('读取默认目录失败:', err);
      } finally {
        setProjectDirReady(true);
      }
    })();
    void (async () => {
      try {
        const config = await getProviderConfig();
        setModelOptions(modelCatalog(config).map((model) => model.id));
      } catch (err) {
        console.error('读取模型目录失败:', err);
      }
    })();
  }, []);

  useEffect(() => {
    if (!projectDirReady) return;
    void refreshAll();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [projectDirReady, projectDir]);

  async function refreshAll() {
    setLoading(true);
    const dir = projectDir || undefined;
    const [skillsResult, mcpResult, subagentResult, commandResult, hookResult] =
      await Promise.allSettled([
        Promise.all([listSkills('claude-code', dir), listSkills('codex', dir)]).then(
          ([claudeSkills, codexSkills]) => [...claudeSkills, ...codexSkills],
        ),
        listMcpServers(),
        listSubagents(dir),
        listSlashCommands(undefined, dir),
        listHooks(dir),
      ]);
    const failures: string[] = [];
    applyResult(skillsResult, setSkills, '技能', failures);
    applyResult(mcpResult, setMcpServers, 'MCP 服务器', failures);
    applyResult(subagentResult, setSubagents, '子代理', failures);
    applyResult(commandResult, setSlashCommands, '斜杠命令', failures);
    applyResult(hookResult, setHooks, '钩子', failures);
    setLoadErrors(failures);
    setLoading(false);
  }

  async function reloadMcpServers() {
    try {
      setMcpServers(await listMcpServers());
    } catch (err) {
      console.error('加载 MCP 服务器失败:', err);
      notify(`重新加载 MCP 服务器失败：${err}`);
    }
  }

  async function reloadSubagents() {
    try {
      setSubagents(await listSubagents(projectDir || undefined));
    } catch (err) {
      console.error('加载子代理失败:', err);
      notify(`重新加载子代理失败：${err}`);
    }
  }

  async function reloadSlashCommands() {
    try {
      setSlashCommands(await listSlashCommands(undefined, projectDir || undefined));
    } catch (err) {
      console.error('加载斜杠命令失败:', err);
      notify(`重新加载斜杠命令失败：${err}`);
    }
  }

  async function reloadHooks() {
    try {
      setHooks(await listHooks(projectDir || undefined));
    } catch (err) {
      console.error('加载钩子失败:', err);
      notify(`重新加载钩子失败：${err}`);
    }
  }

  // 全局通知层（P2-2）：按文案自动区分成功/失败级别
  function notify(message: string) {
    showResultToast(message);
  }

  async function handleToggleSkill(skillId: string, enabled: boolean) {
    try {
      await toggleSkill(skillId, enabled, projectDir || undefined);
      setSkills((prev) => prev.map((s) => (s.id === skillId ? { ...s, enabled } : s)));
      notify(enabled ? '技能已启用' : '技能已停用');
    } catch (err) {
      console.error('切换技能状态失败:', err);
      notify(`切换技能状态失败：${err}`);
    }
  }

  async function handleSaveMcp(draft: McpDraft) {
    try {
      await saveMcpServer(mcpDraftToServer(draft));
      setMcpDraft(null);
      await reloadMcpServers();
      notify('MCP 服务器已保存');
    } catch (err) {
      console.error('保存 MCP 服务器失败:', err);
      notify(`保存 MCP 服务器失败：${err}`);
    }
  }

  async function handleDeleteMcp(name: string) {
    setPendingConfirm({
      title: `删除 MCP 服务器「${name}」？`,
      body: '删除后会移除该服务器配置和已加载的工具列表，当前会话后续将不能再调用这些工具。',
      confirmLabel: '删除服务器',
      onConfirm: async () => {
        try {
          await deleteMcpServer(name);
          setMcpTools((prev) => {
            const next = { ...prev };
            delete next[name];
            return next;
          });
          await reloadMcpServers();
          notify('MCP 服务器已删除');
        } catch (err) {
          console.error('删除 MCP 服务器失败:', err);
          notify(`删除 MCP 服务器失败：${err}`);
        }
      },
    });
  }

  async function handleSaveSubagent(draft: SubagentDraft) {
    try {
      await saveSubagent(draftToSubagent(draft), projectDir || undefined);
      setSubagentDraft(null);
      await reloadSubagents();
      notify('子代理已保存');
    } catch (err) {
      console.error('保存子代理失败:', err);
      notify(`保存子代理失败：${err}`);
    }
  }

  async function handleToggleSubagentAuto(subagent: Subagent, auto: boolean) {
    try {
      await saveSubagent({ ...subagent, auto }, projectDir || undefined);
      setSubagents((prev) =>
        prev.map((item) => (item.id === subagent.id ? { ...item, auto } : item)),
      );
      notify(auto ? '已设为自动委派' : '已设为手动调用');
    } catch (err) {
      console.error('更新子代理失败:', err);
      notify(`更新子代理失败：${err}`);
    }
  }

  async function handleDeleteSubagent(id: string, name: string) {
    setPendingConfirm({
      title: `删除子代理「${name}」？`,
      body: '删除后，主会话不能再自动委派或手动调用这个子代理。',
      confirmLabel: '删除子代理',
      onConfirm: async () => {
        try {
          await deleteSubagent(id, projectDir || undefined);
          await reloadSubagents();
          notify('子代理已删除');
        } catch (err) {
          console.error('删除子代理失败:', err);
          notify(`删除子代理失败：${err}`);
        }
      },
    });
  }

  async function handleSaveCommand(draft: CommandDraft) {
    try {
      await saveSlashCommand(draftToCommand(draft), projectDir || undefined);
      setCommandDraft(null);
      await reloadSlashCommands();
      notify('斜杠命令已保存');
    } catch (err) {
      console.error('保存斜杠命令失败:', err);
      notify(`保存斜杠命令失败：${err}`);
    }
  }

  async function handleToggleCommand(command: SlashCommand, enabled: boolean) {
    try {
      await saveSlashCommand({ ...command, enabled }, projectDir || undefined);
      setSlashCommands((prev) =>
        prev.map((item) => (item.id === command.id ? { ...item, enabled } : item)),
      );
      notify(enabled ? '斜杠命令已启用' : '斜杠命令已停用');
    } catch (err) {
      console.error('更新斜杠命令失败:', err);
      notify(`更新斜杠命令失败：${err}`);
    }
  }

  async function handleDeleteCommand(id: string, trigger: string) {
    setPendingConfirm({
      title: `删除斜杠命令「${trigger}」？`,
      body: '删除后，输入框的斜杠菜单不会再展示这个命令模板。',
      confirmLabel: '删除命令',
      onConfirm: async () => {
        try {
          await deleteSlashCommand(id, projectDir || undefined);
          await reloadSlashCommands();
          notify('斜杠命令已删除');
        } catch (err) {
          console.error('删除斜杠命令失败:', err);
          notify(`删除斜杠命令失败：${err}`);
        }
      },
    });
  }

  async function handleSaveHook(draft: HookDraft) {
    try {
      await saveHook(draftToHook(draft), projectDir || undefined);
      setHookDraft(null);
      await reloadHooks();
      notify('钩子已保存');
    } catch (err) {
      console.error('保存钩子失败:', err);
      notify(`保存钩子失败：${err}`);
    }
  }

  async function handleToggleHook(hook: Hook, enabled: boolean) {
    try {
      await saveHook({ ...hook, enabled }, projectDir || undefined);
      setHooks((prev) => prev.map((item) => (item.id === hook.id ? { ...item, enabled } : item)));
      notify(enabled ? '钩子已启用' : '钩子已停用');
    } catch (err) {
      console.error('更新钩子失败:', err);
      notify(`更新钩子失败：${err}`);
    }
  }

  async function handleDeleteHook(id: string, description: string) {
    setPendingConfirm({
      title: `删除钩子「${description || id}」？`,
      body: '删除后，对应事件不会再触发这条 Shell 命令。',
      confirmLabel: '删除钩子',
      onConfirm: async () => {
        try {
          await deleteHook(id, projectDir || undefined);
          await reloadHooks();
          notify('钩子已删除');
        } catch (err) {
          console.error('删除钩子失败:', err);
          notify(`删除钩子失败：${err}`);
        }
      },
    });
  }

  const enabledSkillsCount = skills.filter((s) => s.enabled).length;
  const mcpToolCount = Object.values(mcpTools).reduce((sum, tools) => sum + tools.length, 0);
  const enabledCommandCount = slashCommands.filter((command) => command.enabled).length;
  const enabledHookCount = hooks.filter((hook) => hook.enabled).length;

  return (
    <div className="page scroll">
      <div className="page__head">
        <div>
          <div className="page__title">扩展中心</div>
          <div className="page__sub">
            统一管理驱动 Agent 的技能、MCP 服务器、子代理、斜杠命令与钩子。MCP
            斜杠命令与技能按当前引擎的原生机制生效；子代理与钩子仅对 Claude Code 生效。
          </div>
          <div className="row gap-sm" style={{ marginTop: 8, alignItems: 'center' }}>
            <span className="faint" style={{ fontSize: 12 }}>
              项目级作用域：
            </span>
            <span className="mono" style={{ fontSize: 12 }}>
              {projectDir || '未设置（仅显示全局扩展）'}
            </span>
            <button
              className="btn btn--subtle btn--sm"
              onClick={() => {
                void (async () => {
                  const next = await selectDirectory();
                  if (next) setProjectDir(next);
                })();
              }}
            >
              选择项目目录
            </button>
          </div>
        </div>
        <button className="btn btn--primary" onClick={() => setMarketOpen(true)}>
          <Icon name="store" /> 浏览市场
        </button>
      </div>

      <div className="etabs tabbar">
        <TabButton active={tab === 'skills'} onClick={() => setTab('skills')}>
          技能
        </TabButton>
        <TabButton active={tab === 'mcp'} onClick={() => setTab('mcp')}>
          MCP 服务器
        </TabButton>
        <TabButton active={tab === 'subagents'} onClick={() => setTab('subagents')}>
          子代理
        </TabButton>
        <TabButton active={tab === 'commands'} onClick={() => setTab('commands')}>
          斜杠命令
        </TabButton>
        <TabButton active={tab === 'hooks'} onClick={() => setTab('hooks')}>
          钩子
        </TabButton>
      </div>

      <div className="wrap">
        {loadErrors.length > 0 ? (
          <div className="x-load-error" role="alert">
            <Icon name="alert" />
            <span>
              以下来源读取失败：{loadErrors.join('、')}
              。列表可能不完整，配置文件损坏或权限不足都会导致读取失败。
            </span>
            <button className="btn btn--subtle btn--sm" onClick={() => void refreshAll()}>
              重试
            </button>
          </div>
        ) : null}
        <div className="xsum">
          <SummaryItem icon="sparkles" value={enabledSkillsCount} label="启用技能" />
          <SummaryItem icon="plug" value={mcpToolCount || '—'} label="MCP 工具（已连接）" />
          <SummaryItem icon="bot" value={subagents.length} label="子代理" />
          <SummaryItem
            icon="code"
            value={`${enabledCommandCount} / ${enabledHookCount}`}
            label="命令 / 钩子"
          />
        </div>

        {tab === 'skills' && (
          <SkillsTab
            skills={skills}
            loading={loading}
            onToggle={handleToggleSkill}
            onBrowseMarket={() => setMarketOpen(true)}
          />
        )}
        {tab === 'mcp' && (
          <McpTab
            servers={mcpServers}
            tools={mcpTools}
            onToolsChange={setMcpTools}
            onAdd={() => setMcpDraft({ ...EMPTY_MCP })}
            onEdit={(server) => setMcpDraft(mcpServerToDraft(server))}
            onDelete={handleDeleteMcp}
            onNotify={notify}
            onTested={() => void reloadMcpServers()}
          />
        )}
        {tab === 'subagents' && (
          <SubagentsTab
            subagents={subagents}
            onAdd={() =>
              setSubagentDraft({ ...EMPTY_SUBAGENT, model: modelOptions[0] ?? 'claude-sonnet-4.6' })
            }
            onEdit={(subagent) => setSubagentDraft(subagentToDraft(subagent))}
            onDelete={handleDeleteSubagent}
            onToggleAuto={handleToggleSubagentAuto}
          />
        )}
        {tab === 'commands' && (
          <CommandsTab
            commands={slashCommands}
            onAdd={() => setCommandDraft({ ...EMPTY_COMMAND })}
            onEdit={(command) => setCommandDraft(commandToDraft(command))}
            onDelete={handleDeleteCommand}
            onToggle={handleToggleCommand}
          />
        )}
        {tab === 'hooks' && (
          <HooksTab
            hooks={hooks}
            onAdd={() => setHookDraft({ ...EMPTY_HOOK })}
            onEdit={(hook) => setHookDraft(hookToDraft(hook))}
            onDelete={handleDeleteHook}
            onToggle={handleToggleHook}
          />
        )}
      </div>

      {marketOpen && (
        <MarketplaceDialog
          projectDir={projectDir}
          onInstalled={() => {
            void refreshAll();
          }}
          onNotify={notify}
          onClose={() => setMarketOpen(false)}
          onUseMcp={(draft) => {
            setMarketOpen(false);
            setMcpDraft(draft);
            setTab('mcp');
          }}
          onUseSubagent={(draft) => {
            setMarketOpen(false);
            setSubagentDraft(draft);
            setTab('subagents');
          }}
          onUseCommand={(draft) => {
            setMarketOpen(false);
            setCommandDraft(draft);
            setTab('commands');
          }}
          onUseHook={(draft) => {
            setMarketOpen(false);
            setHookDraft(draft);
            setTab('hooks');
          }}
        />
      )}

      {mcpDraft && (
        <McpDialog
          draft={mcpDraft}
          onChange={setMcpDraft}
          onClose={() => setMcpDraft(null)}
          onSave={handleSaveMcp}
        />
      )}
      {subagentDraft && (
        <SubagentDialog
          draft={subagentDraft}
          modelOptions={modelOptions}
          projectAvailable={Boolean(projectDir)}
          onChange={setSubagentDraft}
          onClose={() => setSubagentDraft(null)}
          onSave={handleSaveSubagent}
        />
      )}
      {commandDraft && (
        <CommandDialog
          draft={commandDraft}
          onChange={setCommandDraft}
          onClose={() => setCommandDraft(null)}
          onSave={handleSaveCommand}
        />
      )}
      {hookDraft && (
        <HookDialog
          draft={hookDraft}
          projectAvailable={Boolean(projectDir)}
          onChange={setHookDraft}
          onClose={() => setHookDraft(null)}
          onSave={handleSaveHook}
        />
      )}
      {pendingConfirm ? (
        <ConfirmDialog
          confirm={pendingConfirm}
          onClose={() => setPendingConfirm(null)}
          onDone={() => setPendingConfirm(null)}
        />
      ) : null}
    </div>
  );
}

function applyResult<T>(
  result: PromiseSettledResult<T>,
  setter: (value: T) => void,
  label: string,
  failures: string[],
) {
  if (result.status === 'fulfilled') {
    setter(result.value);
  } else {
    console.error(`加载${label}失败:`, result.reason);
    failures.push(label);
  }
}

function TabButton({
  active,
  onClick,
  children,
}: {
  active: boolean;
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <button className={'tab' + (active ? ' is-active' : '')} onClick={onClick}>
      {children}
    </button>
  );
}

function SummaryItem({
  icon,
  value,
  label,
}: {
  icon: 'sparkles' | 'plug' | 'bot' | 'code';
  value: string | number;
  label: string;
}) {
  return (
    <div className="ministat">
      <span className="ic">
        <Icon name={icon} />
      </span>
      <div>
        <b className="tnum">{value}</b>
        <small>{label}</small>
      </div>
    </div>
  );
}

function SkillsTab({
  skills,
  loading,
  onToggle,
  onBrowseMarket,
}: {
  skills: Skill[];
  loading: boolean;
  onToggle: (id: string, enabled: boolean) => void;
  onBrowseMarket: () => void;
}) {
  const [search, setSearch] = useState('');
  const [filter, setFilter] = useState<SkillFilter>('all');

  const filteredSkills = skills.filter((skill) => {
    if (filter === 'on' && !skill.enabled) return false;
    if (filter === 'global' && skill.scope !== 'global') return false;
    if (filter === 'project' && skill.scope !== 'project') return false;
    const haystack = `${skill.name} ${skill.id} ${skill.description}`.toLowerCase();
    return !search || haystack.includes(search.toLowerCase());
  });

  return (
    <section>
      <p className="faint" style={{ fontSize: 12.5, margin: '-2px 0 14px' }}>
        <span className="pill">仅 Claude Code</span>{' '}
        技能是按需加载的提示词能力包，来自用户/项目目录的 skills 文件夹或市场安装。
      </p>
      <div className="toolbar">
        <SearchBox placeholder="搜索技能…" value={search} onChange={setSearch} />
        <div className="row gap-sm">
          <Chip active={filter === 'all'} onClick={() => setFilter('all')}>
            全部
          </Chip>
          <Chip active={filter === 'on'} onClick={() => setFilter('on')}>
            已启用
          </Chip>
          <Chip active={filter === 'global'} onClick={() => setFilter('global')}>
            全局
          </Chip>
          <Chip active={filter === 'project'} onClick={() => setFilter('project')}>
            项目
          </Chip>
        </div>
        <div className="grow" />
        <span className="faint" style={{ fontSize: 12.5 }}>
          {filteredSkills.length} 个技能
        </span>
      </div>
      {loading ? (
        <div className="empty">加载中...</div>
      ) : skills.length === 0 ? (
        <EmptyState
          icon="sparkles"
          title="还没有任何技能"
          hint="技能来自用户目录与项目目录的 skills 文件夹，也可以从市场安装现成的。"
          action={{ label: '浏览市场', onClick: onBrowseMarket }}
        />
      ) : filteredSkills.length === 0 ? (
        <div className="empty">没有匹配的技能，试试调整搜索或筛选条件</div>
      ) : (
        <div className="xgrid">
          {filteredSkills.map((skill) => (
            <div key={skill.id} className={'xcard' + (skill.enabled ? '' : ' is-off')}>
              <div className="xcard__top">
                <span className="xcard__ic">
                  <Icon name="sparkles" />
                </span>
                <div className="xcard__t">
                  <b>{skill.name}</b>
                  <span className="sub">{skill.id}</span>
                </div>
                <Switch
                  checked={skill.enabled}
                  disabled={skill.engine === 'codex'}
                  onChange={(enabled) => onToggle(skill.id, enabled)}
                />
              </div>
              <div className="xcard__desc">{skill.description}</div>
              <div className="xcard__foot">
                <ScopePill scope={skill.scope} />
                <span className="sep" />
                <span>{skill.engine === 'codex' ? 'Codex' : 'Claude Code'}</span>
                <span className="sep" />
                <span>
                  {skill.source === 'builtin'
                    ? '内置'
                    : skill.source === 'market'
                      ? '市场'
                      : '自定义'}
                </span>
              </div>
            </div>
          ))}
        </div>
      )}
    </section>
  );
}

function McpTab({
  servers,
  tools,
  onToolsChange,
  onAdd,
  onEdit,
  onDelete,
  onNotify,
  onTested,
}: {
  servers: McpServer[];
  tools: Record<string, McpTool[]>;
  onToolsChange: (updater: (prev: Record<string, McpTool[]>) => Record<string, McpTool[]>) => void;
  onAdd: () => void;
  onEdit: (server: McpServer) => void;
  onDelete: (name: string) => void;
  onNotify: (message: string) => void;
  onTested: () => void;
}) {
  const [testing, setTesting] = useState<string | null>(null);
  const [expanded, setExpanded] = useState<Record<string, boolean>>({});
  const [errors, setErrors] = useState<Record<string, string>>({});
  const [search, setSearch] = useState('');
  const [filter, setFilter] = useState<McpFilter>('all');

  const rows = servers.filter((server) => {
    const connected = Boolean(tools[server.name]) || server.status === 'connected';
    if (filter === 'on' && !connected) return false;
    if (filter === 'off' && connected) return false;
    const haystack = `${server.name} ${server.command} ${server.args.join(' ')}`.toLowerCase();
    return !search || haystack.includes(search.toLowerCase());
  });

  async function handleTest(server: McpServer) {
    try {
      setTesting(server.name);
      setErrors((prev) => {
        const next = { ...prev };
        delete next[server.name];
        return next;
      });
      const result = await testMcpConnection(server);
      onToolsChange((prev) => ({ ...prev, [server.name]: result }));
      onNotify(`${server.name} 连接正常 · ${result.length} 个工具已就绪`);
    } catch (err) {
      console.error('测试 MCP 连接失败:', err);
      setErrors((prev) => ({ ...prev, [server.name]: String(err) }));
      onNotify(`连接失败：${err}`);
    } finally {
      setTesting(null);
      // 持久化状态已写入 ~/.helm/mcp-status.json，刷新列表带回
      onTested();
    }
  }

  return (
    <section>
      <div className="toolbar">
        <SearchBox placeholder="搜索 MCP 服务器…" value={search} onChange={setSearch} />
        <div className="row gap-sm">
          <Chip active={filter === 'all'} onClick={() => setFilter('all')}>
            全部
          </Chip>
          <Chip active={filter === 'on'} onClick={() => setFilter('on')}>
            已连接
          </Chip>
          <Chip active={filter === 'off'} onClick={() => setFilter('off')}>
            未连接
          </Chip>
        </div>
        <div className="grow" />
        <button className="btn btn--sm" onClick={onAdd}>
          <Icon name="plus" /> 添加 MCP 服务器
        </button>
      </div>
      {servers.length === 0 ? (
        <EmptyState
          icon="plug"
          title="还没有配置 MCP 服务器"
          hint="MCP 服务器为 Agent 提供数据库、浏览器等外部工具，保存后会同步写入各引擎的配置文件。"
          action={{ label: '添加 MCP 服务器', onClick: onAdd }}
        />
      ) : rows.length === 0 ? (
        <div className="empty">没有匹配的 MCP 服务器</div>
      ) : (
        <div className="xgrid">
          {rows.map((server) => {
            const serverTools = tools[server.name];
            const connected = Boolean(serverTools) || server.status === 'connected';
            const error = errors[server.name] || (connected ? undefined : server.lastError);
            return (
              <div key={server.name} className={'xcard' + (connected ? '' : ' is-off')}>
                <div className="xcard__top">
                  <span className="xcard__ic">
                    <Icon name="plug" />
                  </span>
                  <div className="xcard__t">
                    <b>{server.name}</b>
                    <span className="sub">
                      {server.transport} · {serverTools?.length ?? server.toolCount ?? '?'} 个工具
                      {server.lastTestedAt
                        ? ` · 上次测试 ${new Date(server.lastTestedAt * 1000).toLocaleString()}`
                        : ''}
                    </span>
                  </div>
                  {error ? <span className="pill pill--danger">连接失败</span> : null}
                  {connected ? (
                    <span className="pill pill--success">
                      <span
                        className="dot dot--on"
                        style={{ width: 6, height: 6, boxShadow: 'none' }}
                      />
                      已连接
                    </span>
                  ) : null}
                </div>
                <div className="cmdurl">
                  {server.command} {server.args.join(' ')}
                </div>
                {error ? <div className="xcard__desc">{String(error)}</div> : null}
                <div className="xcard__foot xcard__actions">
                  <button
                    className="btn btn--sm"
                    onClick={() => handleTest(server)}
                    disabled={testing === server.name}
                  >
                    <Icon
                      name={testing === server.name ? 'refresh' : 'zap'}
                      className={testing === server.name ? 'spin' : undefined}
                    />
                    {testing === server.name ? '测试中...' : '测试连接'}
                  </button>
                  {serverTools ? (
                    <button
                      className="btn btn--sm btn--ghost"
                      onClick={() =>
                        setExpanded((prev) => ({ ...prev, [server.name]: !prev[server.name] }))
                      }
                    >
                      <Icon name="terminal" /> 查看工具
                    </button>
                  ) : null}
                  <button className="btn btn--sm btn--ghost" onClick={() => onEdit(server)}>
                    <Icon name="edit" /> 编辑
                  </button>
                  <button className="btn btn--sm btn--danger" onClick={() => onDelete(server.name)}>
                    <Icon name="x" /> 删除
                  </button>
                </div>
                {expanded[server.name] && serverTools ? (
                  <div className="xpanel open">
                    <div className="toolchips">
                      {serverTools.map((tool) => (
                        <span key={tool.name} className="toolchip" title={tool.description}>
                          {tool.name}
                        </span>
                      ))}
                    </div>
                  </div>
                ) : null}
              </div>
            );
          })}
        </div>
      )}
    </section>
  );
}

function SubagentsTab({
  subagents,
  onAdd,
  onEdit,
  onDelete,
  onToggleAuto,
}: {
  subagents: Subagent[];
  onAdd: () => void;
  onEdit: (subagent: Subagent) => void;
  onDelete: (id: string, name: string) => void;
  onToggleAuto: (subagent: Subagent, auto: boolean) => void;
}) {
  const [search, setSearch] = useState('');
  const [expanded, setExpanded] = useState<Record<string, boolean>>({});
  const rows = subagents.filter((subagent) => {
    const haystack =
      `${subagent.id} ${subagent.name} ${subagent.role} ${subagent.model}`.toLowerCase();
    return !search || haystack.includes(search.toLowerCase());
  });

  return (
    <section>
      <p className="faint" style={{ fontSize: 12.5, margin: '-2px 0 14px' }}>
        <span className="pill">仅 Claude Code</span> 子代理是带独立系统提示、模型与工具白名单的专职
        Agent。主 Agent 可在需要时自动委派，隔离上下文、并行处理。
      </p>
      <div className="toolbar">
        <SearchBox placeholder="搜索子代理…" value={search} onChange={setSearch} />
        <div className="grow" />
        <button className="btn btn--sm" onClick={onAdd}>
          <Icon name="plus" /> 新建子代理
        </button>
      </div>
      {subagents.length === 0 ? (
        <EmptyState
          icon="bot"
          title="还没有子代理"
          hint="子代理是带独立提示词与工具限制的专职助手，保存后写入引擎的 agents 目录。"
          action={{ label: '新建子代理', onClick: onAdd }}
        />
      ) : rows.length === 0 ? (
        <div className="empty">没有匹配的子代理</div>
      ) : (
        <div className="xgrid">
          {rows.map((subagent) => (
            <div key={subagent.id} className="xcard">
              <div className="xcard__top">
                <span className="xcard__ic">
                  <Icon name="bot" />
                </span>
                <div className="xcard__t">
                  <b>{subagent.name || subagent.id}</b>
                  <span className="sub">
                    {subagent.id} · {subagent.model || '默认模型'}
                  </span>
                </div>
                <Switch checked={subagent.auto} onChange={(auto) => onToggleAuto(subagent, auto)} />
              </div>
              <div className="xcard__desc">{subagent.role}</div>
              <div className="xcard__foot xcard__actions">
                <span className="pill">{subagent.tools || '未限制工具'}</span>
                <span className="sep" />
                <span>{subagent.auto ? '自动委派' : '手动调用'}</span>
                <div className="grow" />
                <button
                  className="btn btn--sm btn--ghost"
                  onClick={() =>
                    setExpanded((prev) => ({ ...prev, [subagent.id]: !prev[subagent.id] }))
                  }
                >
                  <Icon name="eye" /> 系统提示
                </button>
                <button className="btn btn--sm btn--ghost" onClick={() => onEdit(subagent)}>
                  <Icon name="edit" /> 编辑
                </button>
                <button
                  className="btn btn--sm btn--danger"
                  onClick={() => onDelete(subagent.id, subagent.name || subagent.id)}
                >
                  <Icon name="x" /> 删除
                </button>
              </div>
              {expanded[subagent.id] ? (
                <div className="xpanel open">
                  <div className="promptbox">{subagent.prompt}</div>
                </div>
              ) : null}
            </div>
          ))}
        </div>
      )}
    </section>
  );
}

function CommandsTab({
  commands,
  onAdd,
  onEdit,
  onDelete,
  onToggle,
}: {
  commands: SlashCommand[];
  onAdd: () => void;
  onEdit: (command: SlashCommand) => void;
  onDelete: (id: string, trigger: string) => void;
  onToggle: (command: SlashCommand, enabled: boolean) => void;
}) {
  const [search, setSearch] = useState('');
  const [expanded, setExpanded] = useState<Record<string, boolean>>({});
  const rows = commands.filter((command) => {
    const haystack = `${command.trigger} ${command.description} ${command.body}`.toLowerCase();
    return !search || haystack.includes(search.toLowerCase());
  });

  return (
    <section>
      <div className="toolbar">
        <SearchBox placeholder="搜索命令…" value={search} onChange={setSearch} />
        <div className="grow" />
        <button className="btn btn--sm" onClick={onAdd}>
          <Icon name="plus" /> 添加斜杠命令
        </button>
      </div>
      {commands.length === 0 ? (
        <EmptyState
          icon="code"
          title="还没有斜杠命令"
          hint="斜杠命令是可复用的提示词模板，在工作区输入 / 即可唤起，保存后写入引擎的 commands 目录。"
          action={{ label: '新建斜杠命令', onClick: onAdd }}
        />
      ) : rows.length === 0 ? (
        <div className="empty">没有匹配的斜杠命令</div>
      ) : (
        <div className="xlist">
          {rows.map((command) => {
            const readOnly = command.source === 'builtin' || command.source === 'engine-user';
            return (
              <div key={command.id} className={'xli' + (expanded[command.id] ? ' open' : '')}>
                <div
                  className="xli__main"
                  role="button"
                  tabIndex={0}
                  aria-expanded={Boolean(expanded[command.id])}
                  aria-label={`展开斜杠命令 ${command.trigger}`}
                  onClick={() =>
                    setExpanded((prev) => ({ ...prev, [command.id]: !prev[command.id] }))
                  }
                  onKeyDown={(event) => {
                    if (event.key === 'Enter' || event.key === ' ') {
                      event.preventDefault();
                      setExpanded((prev) => ({ ...prev, [command.id]: !prev[command.id] }));
                    }
                  }}
                >
                  <Icon name="right" className="caret" />
                  <span className="xtrig">{command.trigger}</span>
                  <span className="xli__desc">
                    {command.description}
                    {command.argumentHint ? (
                      <span className="faint mono" style={{ marginLeft: 6 }}>
                        {command.argumentHint}
                      </span>
                    ) : null}
                  </span>
                  <ScopePill scope={command.scope} />
                  <EnginePill engine={command.engine} />
                  <SourcePill source={command.source} />
                  {readOnly ? null : (
                    <>
                      <span onClick={(event) => event.stopPropagation()}>
                        <Switch
                          checked={command.enabled}
                          onChange={(enabled) => onToggle(command, enabled)}
                        />
                      </span>
                      <button
                        className="btn-icon sm"
                        title="编辑"
                        aria-label={`编辑斜杠命令 ${command.trigger}`}
                        onClick={(event) => {
                          event.stopPropagation();
                          onEdit(command);
                        }}
                      >
                        <Icon name="edit" />
                      </button>
                      <button
                        className="btn-icon sm"
                        title="删除"
                        aria-label={`删除斜杠命令 ${command.trigger}`}
                        onClick={(event) => {
                          event.stopPropagation();
                          onDelete(command.id, command.trigger);
                        }}
                      >
                        <Icon name="x" />
                      </button>
                    </>
                  )}
                </div>
                {expanded[command.id] ? (
                  <div className="xli__exp xpanel open">
                    <div className="promptbox">{command.body}</div>
                  </div>
                ) : null}
              </div>
            );
          })}
        </div>
      )}
      <p className="faint" style={{ fontSize: 12, marginTop: 12 }}>
        在工作区输入框键入 <span className="mono">/</span>{' '}
        即可唤起这些命令。命令本质是可复用的提示模板，支持参数占位。
      </p>
    </section>
  );
}

function HooksTab({
  hooks,
  onAdd,
  onEdit,
  onDelete,
  onToggle,
}: {
  hooks: Hook[];
  onAdd: () => void;
  onEdit: (hook: Hook) => void;
  onDelete: (id: string, description: string) => void;
  onToggle: (hook: Hook, enabled: boolean) => void;
}) {
  const [search, setSearch] = useState('');
  const [expanded, setExpanded] = useState<Record<string, boolean>>({});
  const rows = hooks.filter((hook) => {
    const haystack =
      `${hook.event} ${hook.match} ${hook.command} ${hook.description}`.toLowerCase();
    return !search || haystack.includes(search.toLowerCase());
  });

  return (
    <section>
      <p className="faint" style={{ fontSize: 12.5, margin: '-2px 0 14px' }}>
        <span className="pill">仅 Claude Code</span> 钩子在引擎执行工具的特定时机触发 shell
        命令——用于自动格式化、拦截危险命令、完成通知等。
      </p>
      <div className="toolbar">
        <SearchBox placeholder="搜索钩子…" value={search} onChange={setSearch} />
        <div className="grow" />
        <button className="btn btn--sm" onClick={onAdd}>
          <Icon name="plus" /> 新建钩子
        </button>
      </div>
      {hooks.length === 0 ? (
        <EmptyState
          icon="zap"
          title="还没有钩子"
          hint="钩子在工具调用等事件发生时自动执行命令（如格式化、审计），保存后写入引擎设置。"
          action={{ label: '新建钩子', onClick: onAdd }}
        />
      ) : rows.length === 0 ? (
        <div className="empty">没有匹配的钩子</div>
      ) : (
        <div className="xlist">
          {rows.map((hook) => (
            <div key={hook.id} className={'xli' + (expanded[hook.id] ? ' open' : '')}>
              <div
                className="xli__main"
                role="button"
                tabIndex={0}
                aria-expanded={Boolean(expanded[hook.id])}
                aria-label={`展开钩子 ${hook.description || hook.command}`}
                onClick={() => setExpanded((prev) => ({ ...prev, [hook.id]: !prev[hook.id] }))}
                onKeyDown={(event) => {
                  if (event.key === 'Enter' || event.key === ' ') {
                    event.preventDefault();
                    setExpanded((prev) => ({ ...prev, [hook.id]: !prev[hook.id] }));
                  }
                }}
              >
                <Icon name="right" className="caret" />
                <span className={'xev ' + eventClass(hook.event)}>{hook.event}</span>
                <span className="xli__desc">{hook.description || hook.command}</span>
                <span className="xli__match">{hook.match}</span>
                <span onClick={(event) => event.stopPropagation()}>
                  <Switch checked={hook.enabled} onChange={(enabled) => onToggle(hook, enabled)} />
                </span>
                <button
                  className="btn-icon sm"
                  title="编辑"
                  aria-label={`编辑钩子 ${hook.description || hook.command}`}
                  onClick={(event) => {
                    event.stopPropagation();
                    onEdit(hook);
                  }}
                >
                  <Icon name="edit" />
                </button>
                <button
                  className="btn-icon sm"
                  title="删除"
                  aria-label={`删除钩子 ${hook.description || hook.command}`}
                  onClick={(event) => {
                    event.stopPropagation();
                    onDelete(hook.id, hook.description);
                  }}
                >
                  <Icon name="x" />
                </button>
              </div>
              {expanded[hook.id] ? (
                <div className="xli__exp xpanel open">
                  <div className="promptbox">$ {hook.command}</div>
                </div>
              ) : null}
            </div>
          ))}
        </div>
      )}
    </section>
  );
}

function MarketplaceDialog({
  projectDir,
  onInstalled,
  onNotify,
  onClose,
  onUseMcp,
  onUseSubagent,
  onUseCommand,
  onUseHook,
}: {
  projectDir: string;
  onInstalled: () => void;
  onNotify: (message: string) => void;
  onClose: () => void;
  onUseMcp: (draft: McpDraft) => void;
  onUseSubagent: (draft: SubagentDraft) => void;
  onUseCommand: (draft: CommandDraft) => void;
  onUseHook: (draft: HookDraft) => void;
}) {
  const [query, setQuery] = useState('');
  const [results, setResults] = useState<MarketSkill[]>([]);
  const [searching, setSearching] = useState(false);
  const [searched, setSearched] = useState(false);
  const [installing, setInstalling] = useState<string | null>(null);
  const [installScope, setInstallScope] = useState<'global' | 'project'>('global');

  async function handleSearch() {
    const keyword = query.trim();
    if (!keyword) return;
    setSearching(true);
    try {
      const skills = await marketSearchSkills(keyword);
      setResults(skills.slice(0, 20));
      setSearched(true);
    } catch (err) {
      console.error('搜索技能市场失败:', err);
      onNotify(`搜索技能市场失败：${err}`);
    } finally {
      setSearching(false);
    }
  }

  async function handleInstall(skill: MarketSkill) {
    const key = `${skill.source}/${skill.skillId}`;
    setInstalling(key);
    try {
      await marketInstallSkill(
        skill.source,
        skill.skillId,
        installScope,
        installScope === 'project' ? projectDir : undefined,
      );
      onNotify(`技能「${skill.name}」已安装到${installScope === 'global' ? '全局' : '项目'}`);
      onInstalled();
    } catch (err) {
      console.error('安装技能失败:', err);
      onNotify(`安装技能失败：${err}`);
    } finally {
      setInstalling(null);
    }
  }

  const entries = [
    {
      kind: 'MCP 服务器',
      title: 'Filesystem',
      meta: 'Claude Code / Codex stdio',
      desc: '本地目录读写与检索工具。',
      action: () =>
        onUseMcp({
          name: 'filesystem',
          transport: 'stdio',
          command: 'npx',
          args: '-y\n@modelcontextprotocol/server-filesystem\nD:\\work',
          env: '',
        }),
    },
    {
      kind: 'MCP 服务器',
      title: 'GitHub',
      meta: 'Claude Code / Codex stdio',
      desc: 'Issue、PR、代码搜索与仓库操作。',
      action: () =>
        onUseMcp({
          name: 'github',
          transport: 'stdio',
          command: 'npx',
          args: '-y\n@modelcontextprotocol/server-github',
          env: 'GITHUB_PERSONAL_ACCESS_TOKEN=',
        }),
    },
    {
      kind: '子代理',
      title: 'security-reviewer',
      meta: '只读审查',
      desc: '聚焦注入、越权与密钥泄露风险。',
      action: () =>
        onUseSubagent({
          ...EMPTY_SUBAGENT,
          id: 'security-reviewer',
          name: '安全审查',
          model: 'claude-opus-4.7',
          role: '审查改动中的注入、鉴权与密钥泄露风险。',
          tools: 'Read,Grep,Glob',
          prompt: '你是安全审查代理，聚焦 OWASP Top 10。按严重程度列出风险、证据与修复建议。',
        }),
    },
    {
      kind: '斜杠命令',
      title: '/review',
      meta: '全局命令',
      desc: '审查当前改动并按严重程度输出问题。',
      action: () =>
        onUseCommand({
          ...EMPTY_COMMAND,
          id: 'review',
          trigger: '/review',
          description: '审查当前改动并按严重程度给出修复建议',
          body: '审查 git diff 中的全部改动：\n1. 按 严重 / 一般 / 提示 三档列出问题\n2. 每条给出文件:行号与可执行的修复\n3. 标注潜在的安全与性能隐患',
        }),
    },
    {
      kind: '钩子',
      title: 'format-before-write',
      meta: 'PreToolUse',
      desc: '写入文件前运行格式化命令。',
      action: () =>
        onUseHook({
          ...EMPTY_HOOK,
          id: 'format-before-write',
          event: 'PreToolUse',
          match: 'Edit|Write',
          description: '写入文件前自动格式化',
          command: 'prettier -w "$FILE"',
        }),
    },
  ];

  return (
    <Dialog title="扩展市场" onClose={onClose}>
      <div className="market-search" style={{ marginBottom: 14 }}>
        <div className="row gap-sm" style={{ alignItems: 'center' }}>
          <div className="search grow">
            <Icon name="search" />
            <input
              placeholder="搜索 skills.sh 技能市场（如 review、docs、testing）…"
              value={query}
              onChange={(event) => setQuery(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === 'Enter') void handleSearch();
              }}
            />
          </div>
          <select
            className="select"
            style={{ width: 120 }}
            value={installScope}
            onChange={(event) => setInstallScope(event.target.value as 'global' | 'project')}
          >
            <option value="global">装到全局</option>
            <option value="project" disabled={!projectDir}>
              装到项目
            </option>
          </select>
          <button className="btn btn--sm" onClick={() => void handleSearch()} disabled={searching}>
            {searching ? '搜索中…' : '搜索'}
          </button>
        </div>
        {results.length > 0 ? (
          <div className="market-list" style={{ marginTop: 10 }}>
            {results.map((skill) => {
              const key = `${skill.source}/${skill.skillId}`;
              return (
                <div key={key} className="market-row">
                  <span className="xcard__ic">
                    <Icon name="sparkles" />
                  </span>
                  <div className="market-row__main">
                    <div className="row gap-sm">
                      <b>{skill.name}</b>
                      <span className="pill">技能</span>
                      <span className="faint" style={{ fontSize: 12 }}>
                        {skill.source} · {skill.installs.toLocaleString()} 次安装
                      </span>
                    </div>
                  </div>
                  <button
                    className="btn btn--sm"
                    onClick={() => void handleInstall(skill)}
                    disabled={installing === key}
                  >
                    <Icon name="plus" /> {installing === key ? '安装中…' : '安装'}
                  </button>
                </div>
              );
            })}
          </div>
        ) : searched && !searching ? (
          <p className="faint" style={{ fontSize: 12, marginTop: 8 }}>
            没有匹配的技能，换个关键词试试。
          </p>
        ) : null}
      </div>
      <p className="faint" style={{ fontSize: 12, margin: '0 0 8px' }}>
        精选模板
      </p>
      <div className="market-list">
        {entries.map((entry) => (
          <div key={`${entry.kind}-${entry.title}`} className="market-row">
            <span className="xcard__ic">
              <Icon
                name={
                  entry.kind === 'MCP 服务器'
                    ? 'plug'
                    : entry.kind === '子代理'
                      ? 'bot'
                      : entry.kind === '斜杠命令'
                        ? 'code'
                        : 'terminal'
                }
              />
            </span>
            <div className="market-row__main">
              <div className="row gap-sm">
                <b>{entry.title}</b>
                <span className="pill">{entry.kind}</span>
                <span className="faint" style={{ fontSize: 12 }}>
                  {entry.meta}
                </span>
              </div>
              <p>{entry.desc}</p>
            </div>
            <button className="btn btn--sm" onClick={entry.action}>
              <Icon name="plus" /> 添加
            </button>
          </div>
        ))}
      </div>
    </Dialog>
  );
}

function McpDialog({
  draft,
  onChange,
  onClose,
  onSave,
}: {
  draft: McpDraft;
  onChange: (draft: McpDraft) => void;
  onClose: () => void;
  onSave: (draft: McpDraft) => void;
}) {
  return (
    <Dialog
      title="MCP 服务器配置"
      onClose={onClose}
      footer={
        <>
          <button className="btn btn--ghost" onClick={onClose}>
            取消
          </button>
          <button className="btn btn--primary" onClick={() => onSave(draft)}>
            保存
          </button>
        </>
      }
    >
      <div className="form-grid">
        <Field label="名称">
          <input
            className="input"
            value={draft.name}
            onChange={(e) => onChange({ ...draft, name: e.target.value })}
            placeholder="filesystem"
          />
        </Field>
        <Field label="传输方式">
          <div className="seg">
            <button
              className={draft.transport === 'stdio' ? 'is-active' : ''}
              onClick={() => onChange({ ...draft, transport: 'stdio' })}
            >
              stdio
            </button>
            <button
              className={draft.transport === 'sse' ? 'is-active' : ''}
              onClick={() => onChange({ ...draft, transport: 'sse', args: '', env: '' })}
            >
              SSE
            </button>
            <button
              className={draft.transport === 'http' ? 'is-active' : ''}
              onClick={() => onChange({ ...draft, transport: 'http', args: '', env: '' })}
            >
              HTTP
            </button>
          </div>
        </Field>
        <Field
          label={draft.transport === 'stdio' ? '命令' : `${draft.transport.toUpperCase()} URL`}
        >
          <input
            className="input"
            value={draft.command}
            onChange={(e) => onChange({ ...draft, command: e.target.value })}
            placeholder={
              draft.transport === 'stdio'
                ? 'npx'
                : draft.transport === 'sse'
                  ? 'http://127.0.0.1:3000/sse'
                  : 'https://example.com/mcp'
            }
          />
        </Field>
        {draft.transport === 'stdio' ? (
          <>
            <Field label="参数（每行一个）">
              <textarea
                className="textarea mono"
                rows={5}
                value={draft.args}
                onChange={(e) => onChange({ ...draft, args: e.target.value })}
              />
            </Field>
            <Field label="环境变量（KEY=value，每行一个）">
              <textarea
                className="textarea mono"
                rows={4}
                value={draft.env}
                onChange={(e) => onChange({ ...draft, env: e.target.value })}
              />
            </Field>
          </>
        ) : null}
      </div>
    </Dialog>
  );
}

function SubagentDialog({
  draft,
  modelOptions,
  projectAvailable,
  onChange,
  onClose,
  onSave,
}: {
  draft: SubagentDraft;
  modelOptions: string[];
  projectAvailable: boolean;
  onChange: (draft: SubagentDraft) => void;
  onClose: () => void;
  onSave: (draft: SubagentDraft) => void;
}) {
  return (
    <Dialog
      title="子代理配置"
      onClose={onClose}
      footer={
        <>
          <button className="btn btn--ghost" onClick={onClose}>
            取消
          </button>
          <button className="btn btn--primary" onClick={() => onSave(draft)}>
            保存
          </button>
        </>
      }
    >
      <div className="form-grid two">
        <Field label="ID">
          <input
            className="input mono"
            value={draft.id}
            onChange={(e) => onChange({ ...draft, id: e.target.value })}
            placeholder="security-reviewer"
          />
        </Field>
        <Field label="名称">
          <input
            className="input"
            value={draft.name}
            onChange={(e) => onChange({ ...draft, name: e.target.value })}
            placeholder="安全审查"
          />
        </Field>
        <Field label="模型">
          <>
            <input
              className="input mono"
              list="subagent-model-options"
              value={draft.model}
              onChange={(e) => onChange({ ...draft, model: e.target.value })}
              placeholder="从模型目录选择或输入官方模型 ID"
            />
            <datalist id="subagent-model-options">
              {modelOptions.map((model) => (
                <option key={model} value={model} />
              ))}
            </datalist>
          </>
        </Field>
        <Field label="作用域">
          <select
            className="select"
            value={draft.scope}
            onChange={(e) => onChange({ ...draft, scope: e.target.value as 'global' | 'project' })}
          >
            <option value="global">全局（~/.claude/agents）</option>
            <option value="project" disabled={!projectAvailable}>
              项目（.claude/agents）
            </option>
          </select>
        </Field>
        <Field label="工具白名单">
          <input
            className="input mono"
            value={draft.tools}
            onChange={(e) => onChange({ ...draft, tools: e.target.value })}
          />
        </Field>
        <label className="check-row">
          <input
            type="checkbox"
            checked={draft.auto}
            onChange={(e) => onChange({ ...draft, auto: e.target.checked })}
          />
          <span>允许自动委派</span>
        </label>
        <Field label="职责">
          <textarea
            className="textarea"
            rows={3}
            value={draft.role}
            onChange={(e) => onChange({ ...draft, role: e.target.value })}
          />
        </Field>
        <Field label="系统提示">
          <textarea
            className="textarea mono"
            rows={8}
            value={draft.prompt}
            onChange={(e) => onChange({ ...draft, prompt: e.target.value })}
          />
        </Field>
      </div>
    </Dialog>
  );
}

function CommandDialog({
  draft,
  onChange,
  onClose,
  onSave,
}: {
  draft: CommandDraft;
  onChange: (draft: CommandDraft) => void;
  onClose: () => void;
  onSave: (draft: CommandDraft) => void;
}) {
  return (
    <Dialog
      title="斜杠命令配置"
      onClose={onClose}
      footer={
        <>
          <button className="btn btn--ghost" onClick={onClose}>
            取消
          </button>
          <button className="btn btn--primary" onClick={() => onSave(draft)}>
            保存
          </button>
        </>
      }
    >
      <div className="form-grid two">
        <Field label="ID">
          <input
            className="input mono"
            value={draft.id}
            onChange={(e) => onChange({ ...draft, id: e.target.value })}
            placeholder="review"
          />
        </Field>
        <Field label="触发器">
          <input
            className="input mono"
            value={draft.trigger}
            onChange={(e) => onChange({ ...draft, trigger: e.target.value })}
            placeholder="/review"
          />
        </Field>
        <Field label="作用域">
          <select
            className="select"
            value={draft.scope}
            onChange={(e) => onChange({ ...draft, scope: e.target.value as 'global' | 'project' })}
          >
            <option value="global">全局</option>
            <option value="project">项目</option>
          </select>
        </Field>
        <Field label="适用引擎">
          <select
            className="select"
            value={draft.engine}
            onChange={(e) =>
              onChange({
                ...draft,
                engine: e.target.value as 'all' | 'claude-code' | 'codex',
              })
            }
          >
            <option value="all">全部引擎</option>
            <option value="claude-code">Claude Code</option>
            <option value="codex">Codex</option>
          </select>
        </Field>
        <Field label="参数提示">
          <input
            className="input mono"
            value={draft.argumentHint ?? ''}
            onChange={(e) => onChange({ ...draft, argumentHint: e.target.value })}
            placeholder="[文件或目录]"
          />
        </Field>
        <label className="check-row">
          <input
            type="checkbox"
            checked={draft.enabled}
            onChange={(e) => onChange({ ...draft, enabled: e.target.checked })}
          />
          <span>启用</span>
        </label>
        <Field label="说明">
          <input
            className="input"
            value={draft.description}
            onChange={(e) => onChange({ ...draft, description: e.target.value })}
          />
        </Field>
        <Field label="模板">
          <textarea
            className="textarea mono"
            rows={8}
            value={draft.body}
            onChange={(e) => onChange({ ...draft, body: e.target.value })}
          />
        </Field>
      </div>
    </Dialog>
  );
}

function HookDialog({
  draft,
  projectAvailable,
  onChange,
  onClose,
  onSave,
}: {
  draft: HookDraft;
  projectAvailable: boolean;
  onChange: (draft: HookDraft) => void;
  onClose: () => void;
  onSave: (draft: HookDraft) => void;
}) {
  return (
    <Dialog
      title="钩子配置"
      onClose={onClose}
      footer={
        <>
          <button className="btn btn--ghost" onClick={onClose}>
            取消
          </button>
          <button className="btn btn--primary" onClick={() => onSave(draft)}>
            保存
          </button>
        </>
      }
    >
      <div className="form-grid two">
        <Field label="ID">
          <input
            className="input mono"
            value={draft.id}
            onChange={(e) => onChange({ ...draft, id: e.target.value })}
            placeholder="format-before-write"
          />
        </Field>
        <Field label="事件">
          <select
            className="select"
            value={draft.event}
            onChange={(e) => onChange({ ...draft, event: e.target.value as Hook['event'] })}
          >
            {HOOK_EVENTS.map((event) => (
              <option key={event} value={event}>
                {event}
              </option>
            ))}
          </select>
        </Field>
        <Field label="作用域">
          <select
            className="select"
            value={draft.scope}
            onChange={(e) => onChange({ ...draft, scope: e.target.value as 'global' | 'project' })}
          >
            <option value="global">全局（~/.claude/settings.json）</option>
            <option value="project" disabled={!projectAvailable}>
              项目（.claude/settings.json）
            </option>
          </select>
        </Field>
        <Field label="匹配规则">
          <input
            className="input mono"
            value={draft.match}
            onChange={(e) => onChange({ ...draft, match: e.target.value })}
          />
        </Field>
        <label className="check-row">
          <input
            type="checkbox"
            checked={draft.enabled}
            onChange={(e) => onChange({ ...draft, enabled: e.target.checked })}
          />
          <span>启用</span>
        </label>
        <Field label="说明">
          <input
            className="input"
            value={draft.description}
            onChange={(e) => onChange({ ...draft, description: e.target.value })}
          />
        </Field>
        <Field label="Shell 命令">
          <textarea
            className="textarea mono"
            rows={6}
            value={draft.command}
            onChange={(e) => onChange({ ...draft, command: e.target.value })}
          />
        </Field>
      </div>
    </Dialog>
  );
}

function ConfirmDialog({
  confirm,
  onClose,
  onDone,
}: {
  confirm: PendingConfirm;
  onClose: () => void;
  onDone: () => void;
}) {
  const [busy, setBusy] = useState(false);
  return (
    <Dialog
      title={confirm.title}
      onClose={busy ? () => undefined : onClose}
      footer={
        <>
          <button className="btn btn--ghost" onClick={onClose} disabled={busy}>
            取消
          </button>
          <button
            className="btn btn--danger"
            disabled={busy}
            onClick={() => {
              setBusy(true);
              void confirm.onConfirm().finally(() => {
                setBusy(false);
                onDone();
              });
            }}
          >
            {busy ? '处理中...' : confirm.confirmLabel}
          </button>
        </>
      }
    >
      <p className="modal-copy">{confirm.body}</p>
    </Dialog>
  );
}

function Dialog({
  title,
  children,
  footer,
  onClose,
}: {
  title: string;
  children: React.ReactNode;
  footer?: React.ReactNode;
  onClose: () => void;
}) {
  const dialogRef = useDialogBehavior(onClose);
  return (
    <div className="modal-overlay" role="dialog" aria-modal="true" aria-label={title}>
      <div className="modal-panel" ref={dialogRef} tabIndex={-1}>
        <div className="modal-panel__head">
          <b>{title}</b>
          <button className="btn-icon sm" onClick={onClose} aria-label="关闭">
            <Icon name="x" />
          </button>
        </div>
        <div className="modal-panel__body">{children}</div>
        {footer ? <div className="modal-panel__foot">{footer}</div> : null}
      </div>
    </div>
  );
}

function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <label className="field">
      <span>{label}</span>
      {children}
    </label>
  );
}

function SearchBox({
  placeholder,
  value,
  onChange,
}: {
  placeholder: string;
  value: string;
  onChange: (value: string) => void;
}) {
  return (
    <div className="search" style={{ width: 260 }}>
      <Icon name="search" />
      <input
        placeholder={placeholder}
        value={value}
        onChange={(event) => onChange(event.target.value)}
      />
    </div>
  );
}

function Chip({
  active,
  onClick,
  children,
}: {
  active: boolean;
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <button className={'chip' + (active ? ' is-active' : '')} onClick={onClick}>
      {children}
    </button>
  );
}

function Switch({
  checked,
  disabled = false,
  onChange,
}: {
  checked: boolean;
  disabled?: boolean;
  onChange: (checked: boolean) => void;
}) {
  return (
    <label className="switch">
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

function ScopePill({ scope }: { scope: 'global' | 'project' }) {
  return (
    <span className={scope === 'global' ? 'pill pill--accent' : 'pill'}>
      {scope === 'global' ? '全局' : '项目'}
    </span>
  );
}

function EnginePill({ engine }: { engine: SlashCommand['engine'] }) {
  const label =
    engine === 'claude-code' ? 'Claude Code' : engine === 'codex' ? 'Codex' : '全部引擎';
  return <span className="pill">{label}</span>;
}

function SourcePill({ source }: { source: SlashCommand['source'] }) {
  const label =
    source === 'extension'
      ? '扩展中心'
      : source === 'engine-project'
        ? '项目'
        : source === 'engine-user'
          ? '引擎'
          : '内置';
  return (
    <span className="pill" title={source === 'extension' ? undefined : '引擎原生/内置命令，只读'}>
      {label}
    </span>
  );
}

function eventClass(event: Hook['event']) {
  if (event === 'PreToolUse') return 'xev--pre';
  if (event === 'PostToolUse') return 'xev--post';
  return 'xev--other';
}

function splitLines(value: string) {
  return value
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter(Boolean);
}

function parseEnv(value: string) {
  return splitLines(value).reduce<Record<string, string>>((env, line) => {
    const index = line.indexOf('=');
    if (index <= 0) return env;
    env[line.slice(0, index).trim()] = line.slice(index + 1).trim();
    return env;
  }, {});
}

function envToLines(env: Record<string, string>) {
  return Object.entries(env)
    .map(([key, value]) => `${key}=${value}`)
    .join('\n');
}

function mcpDraftToServer(draft: McpDraft): McpServer {
  return {
    name: draft.name.trim(),
    command: draft.command.trim(),
    args: draft.transport === 'stdio' ? splitLines(draft.args) : [],
    env: draft.transport === 'stdio' ? parseEnv(draft.env) : {},
    transport: draft.transport,
    enabled: true,
    status: 'disconnected',
  };
}

function mcpServerToDraft(server: McpServer): McpDraft {
  return {
    name: server.name,
    transport: server.transport,
    command: server.command,
    args: server.args.join('\n'),
    env: envToLines(server.env),
  };
}

function draftToSubagent(draft: SubagentDraft): Subagent {
  return { ...draft, id: draft.id.trim(), name: draft.name.trim() || draft.id.trim() };
}

function subagentToDraft(subagent: Subagent): SubagentDraft {
  return { ...subagent };
}

function draftToCommand(draft: CommandDraft): SlashCommand {
  return {
    ...draft,
    id: draft.id.trim(),
    trigger: draft.trigger.trim().startsWith('/')
      ? draft.trigger.trim()
      : `/${draft.trigger.trim()}`,
    source: 'extension',
    argumentHint: draft.argumentHint?.trim() || null,
  };
}

function commandToDraft(command: SlashCommand): CommandDraft {
  return { ...command, argumentHint: command.argumentHint ?? '' };
}

function draftToHook(draft: HookDraft): Hook {
  return { ...draft, id: draft.id.trim() };
}

function hookToDraft(hook: Hook): HookDraft {
  return { ...hook };
}
