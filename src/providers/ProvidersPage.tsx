import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { listen } from '@tauri-apps/api/event';
import { open as openFileDialog } from '@tauri-apps/plugin-dialog';
import { Icon } from '../shell/icons';
import { EngineBrand } from '../shell/EngineBrand';
import { ProviderBrand } from './ProviderBrand';
import { getReadinessReport } from '../settings/api';
import { loadSettings, saveSettings } from '../settings/api';
import { engineEffortTiers } from '../home/newTaskViewModel';

/** 定价目录模糊匹配（用户裁决）：忽略大小写；逐级去厂商前缀（a/b/c → b/c → c）；
 *  再展开常见协议后缀（m2/glm-5.2-openai → glm-5.2）；任一候选与目录 ID 相等即命中。 */
function fuzzyPriceMatch(
  entries: PricingCatalogEntry[],
  modelId: string,
): PricingCatalogEntry | null {
  const target = modelId.toLowerCase();
  const forms = new Set<string>([target]);
  const parts = target.split('/');
  for (let index = 1; index < parts.length; index += 1) {
    forms.add(parts.slice(index).join('/'));
  }
  for (const form of [...forms]) {
    for (const suffix of ['-openai', '-responses', '-chat', '-completions']) {
      if (form.endsWith(suffix)) forms.add(form.slice(0, -suffix.length));
    }
  }
  for (const entry of entries) {
    if (forms.has(entry.modelId.toLowerCase())) return entry;
  }
  return null;
}

/** 手动添加模型：按模糊匹配预填目录价（未命中则未计价），返回新模型条目。 */
function buildManualModel(
  providerId: string,
  id: string,
  match: PricingCatalogEntry | null,
): ModelConfig {
  return {
    id,
    providerId,
    displayName: id,
    inputPricePerMtok: match?.input ?? 0,
    cachedInputPricePerMtok: match?.cachedInput ?? undefined,
    outputPricePerMtok: match?.output ?? 0,
    priceSource: match ? 'builtin' : 'manual',
    enabled: true,
  };
}

/** 目录价签文案：$输入/$缓存/$输出（对齐原型 priceText 三段格式）。 */
function catalogPriceText(entry: PricingCatalogEntry): string {
  return `$${Number(entry.input)}/$${Number(entry.cachedInput ?? 0)}/$${Number(entry.output)}`;
}

/** 模型 ID 组合框（原型 roleCombo）：自由填写 + 下拉候选（输入即筛选、候选行附价签）。
 *  候选只来自「同步模型」拉取的远端列表（同步前为空、纯手输）；
 *  失焦/回车确认：确认后由父级做模糊匹配带出目录价；空值保留该行。
 *
 *  三条硬约束（改动前必读，都是踩过的坑）：
 *  1. 菜单必须 portal 到最近的 [role=dialog]，不能 portal 到 body。Radix Dialog 打开时给
 *     body 设 pointer-events:none，并把 dialog 子树外的 pointerdown 判为 outside 直接关弹窗；
 *     挂到 body 的菜单「看得见点不到」，点一下还会顺带关掉整个添加流程。
 *  2. 筛选只在用户真正敲字后生效（filtering）。若直接拿输入框内容当查询词，已经填了 ID 的
 *     行展开时只筛得出它自己，等于换不了模型——只能新增一行才看得到候选。
 *  3. 定位先渲染再实测校正。用固定估算高度会让矮菜单在向上翻转时悬在半空。 */
function ModelIdCombo(props: {
  value: string;
  options: { id: string; priceText: string }[];
  /** 已被同一表单其它行占用的 ID：候选里标注「已添加」，避免用户选了才被拒 */
  taken?: string[];
  ghost?: boolean;
  placeholder?: string;
  autoOpen?: boolean;
  emptyHint?: string;
  onCommit: (id: string) => void;
}) {
  const [text, setText] = useState(props.value);
  const [open, setOpen] = useState(Boolean(props.autoOpen));
  const [filtering, setFiltering] = useState(false);
  const [activeIndex, setActiveIndex] = useState(0);
  const [coords, setCoords] = useState<{
    top: number;
    left: number;
    width: number;
    maxHeight: number;
  } | null>(null);
  const [host, setHost] = useState<HTMLElement | null>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const measuredRef = useRef(0);

  useEffect(() => {
    setText(props.value);
    setFiltering(false);
  }, [props.value]);
  useEffect(() => {
    if (props.autoOpen) setOpen(true);
  }, [props.autoOpen]);
  // 挂载点：Radix 弹窗内必须挂进 dialog，否则既点不到也会触发 outside 关闭
  useEffect(() => {
    setHost((inputRef.current?.closest('[role="dialog"]') as HTMLElement | null) ?? document.body);
  }, []);

  const place = useCallback(() => {
    const el = inputRef.current;
    if (!el) return;
    const rect = el.getBoundingClientRect();
    const rendered = menuRef.current?.offsetHeight ?? 0;
    const menuH = rendered || measuredRef.current || 200;
    const gap = 4;
    const spaceBelow = window.innerHeight - rect.bottom - gap - 8;
    const spaceAbove = rect.top - gap - 8;
    const flip = menuH > spaceBelow && spaceAbove > spaceBelow;
    const top = flip ? Math.max(8, rect.top - menuH - gap) : rect.bottom + gap;
    const maxHeight = Math.max(120, Math.min(300, flip ? spaceAbove : spaceBelow));
    const width = Math.min(Math.max(rect.width, 280), window.innerWidth - 16);
    const left = Math.max(8, Math.min(rect.left, window.innerWidth - width - 8));
    setCoords({ top, left, width, maxHeight });
  }, []);

  useEffect(() => {
    if (!open) {
      setCoords(null);
      measuredRef.current = 0;
      return;
    }
    place();
    const onScroll = () => place();
    window.addEventListener('scroll', onScroll, true);
    window.addEventListener('resize', onScroll);
    return () => {
      window.removeEventListener('scroll', onScroll, true);
      window.removeEventListener('resize', onScroll);
    };
  }, [open, place]);

  // 菜单真实高度只有渲染后才知道，据此再校正一次（有阈值保护，收敛后不再触发）
  useLayoutEffect(() => {
    if (!open || !coords) return;
    const rendered = menuRef.current?.offsetHeight ?? 0;
    if (rendered > 0 && Math.abs(rendered - measuredRef.current) > 1) {
      measuredRef.current = rendered;
      place();
    }
  });

  useEffect(() => {
    if (!open) return;
    const onDown = (event: MouseEvent) => {
      const target = event.target as Node;
      if (inputRef.current && inputRef.current.contains(target)) return;
      if (menuRef.current && menuRef.current.contains(target)) return;
      setOpen(false);
      setFiltering(false);
    };
    document.addEventListener('mousedown', onDown);
    return () => document.removeEventListener('mousedown', onDown);
  }, [open]);

  const query = filtering ? text.trim().toLowerCase() : '';
  const filtered = useMemo(() => {
    const seen = new Set<string>();
    const list: { id: string; priceText: string }[] = [];
    for (const option of props.options) {
      if (!option.id) continue;
      const key = option.id.toLowerCase();
      if (seen.has(key)) continue;
      seen.add(key);
      list.push(option);
    }
    return query ? list.filter((option) => option.id.toLowerCase().includes(query)) : list;
  }, [props.options, query]);
  useEffect(() => {
    setActiveIndex((prev) => (filtered.length ? Math.min(prev, filtered.length - 1) : 0));
  }, [filtered.length]);
  const commit = (id: string) => {
    setOpen(false);
    setFiltering(false);
    setText(id);
    props.onCommit(id);
  };
  const cancel = () => {
    setOpen(false);
    setFiltering(false);
    setText(props.value);
  };
  const menu =
    open && coords && host
      ? createPortal(
          <div
            ref={menuRef}
            className="pv-combo__menu"
            style={{
              position: 'fixed',
              top: coords.top,
              left: coords.left,
              width: coords.width,
              maxHeight: coords.maxHeight,
              zIndex: 1000,
            }}
          >
            {filtered.map((option, index) => {
              const taken = option.id !== props.value && (props.taken ?? []).includes(option.id);
              return (
                <button
                  key={option.id}
                  type="button"
                  className={
                    'pv-combo__opt' +
                    (index === activeIndex ? ' is-active' : '') +
                    (taken ? ' is-taken' : '')
                  }
                  onMouseDown={(event) => event.preventDefault()}
                  onMouseEnter={() => setActiveIndex(index)}
                  onClick={() => commit(option.id)}
                >
                  <span className="pv-combo__opt-id">{option.id}</span>
                  <span className="pv-combo__opt-price">{taken ? '已添加' : option.priceText}</span>
                </button>
              );
            })}
            {props.options.length === 0 ? (
              <div className="pv-combo__empty">
                {props.emptyHint ?? '还没有候选 · 点「同步模型」获取，或直接手动输入'}
              </div>
            ) : filtered.length === 0 ? (
              <div className="pv-combo__empty">未找到匹配的模型</div>
            ) : null}
          </div>,
          host,
        )
      : null;
  return (
    <span className={'pv-combo' + (props.ghost === false ? '' : ' pv-combo--ghost')}>
      <input
        ref={inputRef}
        className="cm-model-input mono"
        value={text}
        placeholder={props.placeholder ?? '输入或从候选选择'}
        onChange={(event) => {
          setText(event.target.value);
          setFiltering(true);
          setActiveIndex(0);
          setOpen(true);
        }}
        onFocus={() => setOpen(true)}
        onBlur={() => commit(text.trim())}
        onKeyDown={(event) => {
          if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
            event.preventDefault();
            if (!open) {
              setOpen(true);
              return;
            }
            if (!filtered.length) return;
            const step = event.key === 'ArrowDown' ? 1 : -1;
            setActiveIndex((prev) => (prev + step + filtered.length) % filtered.length);
            return;
          }
          if (event.key === 'Enter') {
            event.preventDefault();
            if (open && filtered[activeIndex]) commit(filtered[activeIndex].id);
            else commit(text.trim());
            return;
          }
          if (event.key === 'Escape') {
            if (!open) return;
            // 阻止冒泡：否则会连带关掉抽屉或 Radix 弹窗
            event.stopPropagation();
            cancel();
          }
        }}
      />
      <button
        className="btn-icon pv-combo__toggle"
        type="button"
        aria-label="展开候选模型"
        onMouseDown={(event) => event.preventDefault()}
        onClick={() => {
          setFiltering(false);
          setOpen((prev) => !prev);
        }}
      >
        <Icon name="chevrondown" />
      </button>
      {menu}
    </span>
  );
}
import { showResultToast } from '../components/toast';
import { ConfirmDialog } from '../components/ConfirmDialog';
import {
  Dialog as ShadcnDialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogDescription,
  DialogFooter,
} from '@/components/ui/dialog';
import { Button } from '@/components/ui/button';
import type { ReasoningEffort } from '@helm/protocol';
import { reasoningEffortLabel } from '../reasoning';
import {
  deleteProviderConfig,
  detectCliLogin,
  getEquivalentEnv,
  listModelPriceOverrides,
  getProviderConfig,
  loginCliAccount,
  logoutCliAccount,
  readEngineConfigFile,
  revealProviderSecret,
  renameProviderModel,
  saveBindingConfig,
  saveEngineConfig,
  saveModelConfig,
  openExternalUrl,
  saveProviderModelSelection,
  saveProviderModelsConfig,
  saveModelPriceOverride,
  getPricingCatalogEntries,
  getPricingCatalogStatus,
  importPricingCatalog,
  refreshPricingCatalog,
  saveProviderConfig,
  listProviderModels,
  syncProviderModels,
  testProviderDraft,
  testEngineConfig,
  writeEngineConfigFile,
  type AppConfig,
  type BindingConfig,
  type CliLoginState,
  type EngineConfig,
  type EngineConfigFile,
  type ModelConfig,
  type PricingCatalogEntry,
  type PricingCatalogStatus,
  type ProviderConfig,
  type ProviderProtocol,
  type ProviderRoleKey,
} from './api';
import {
  PROVIDER_ACCESS_GROUPS,
  PROVIDER_ROLE_ROWS,
  PROVIDER_TEMPLATES,
  accessGroupHint,
  bindingForEngine,
  canBindProvider,
  compatibleProvidersForEngine,
  createProviderDraft,
  envPairsToText,
  isRelayProvider,
  lastSyncTimeText,
  modelCalibrationLabel,
  priceChipFor,
  loginStateLabel,
  matchingSubscriptionProvider,
  modelAccessGroups,
  bindingModelOptions,
  modelCatalogForProvider,
  modelGroupPriceSummary,
  normalizeBindingDraft,
  providerAccessGroup,
  providerAccessGroupLabel,
  enabledModelCount,
  providerBrandKey,
  providerCardStatus,
  providerDeleteConfirmation,
  providerDeleteBlockedReason,
  providerCanDelete,
  protocolLabel,
  providerModelMode,
  providerRoleModelId,
  readinessText,
  templatesForAccessGroup,
  withRoleModel,
  commitProviderModelRow,
  type ProviderAccessGroup,
  type ProviderTemplate,
  type ProviderTemplateId,
} from './providerViewModel';
import './providers.css';

type Tab = 'bindings' | 'providers' | 'models';

// Bedrock/Vertex 暂不支持（后端 engine_accepts 不允许绑定，等价环境变量也未实现），
// 在表单里隐藏，避免用户配置出永远无法生效的服务商（可靠性检查 G2）。

function emptyConfig(): AppConfig {
  return {
    providers: [],
    models: [],
    engines: [],
    bindings: [],
    defaultEngine: 'claude-code',
    defaultModel: '',
  };
}

function priceText(pricePerMtok: number) {
  return pricePerMtok > 0 ? `$${pricePerMtok.toFixed(2)}` : '待配置';
}

function errorMessage(err: unknown, fallback: string) {
  if (err instanceof Error) return err.message;
  if (typeof err === 'string') return err;
  return fallback;
}

export function ProvidersPage() {
  const [tab, setTab] = useState<Tab>('bindings');
  const [config, setConfig] = useState<AppConfig>(emptyConfig);
  const [loading, setLoading] = useState(true);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [addProviderOpen, setAddProviderOpen] = useState(false);
  // S6：详情抽屉。创建/点击卡片进入；关闭仅丢弃未保存草稿，不影响已保存配置。
  const [drawerProviderId, setDrawerProviderId] = useState<string | null>(null);
  // 决策 B-5a：抽屉「去绑定」跨 Tab 联动——切到执行引擎并自动打开兼容引擎卡的绑定弹窗
  const [pendingBindingEngineId, setPendingBindingEngineId] = useState<string | null>(null);

  // 全局通知层（P2-2）：保留 notify(string | null) 签名，null 表示无事发生（旧实现用于清除）
  const notify = useCallback((message: string | null) => {
    if (message) showResultToast(message);
  }, []);

  const loadConfig = useCallback(() => {
    setLoading(true);
    setLoadError(null);
    getProviderConfig()
      .then((loaded) => {
        setConfig(loaded);
      })
      .catch((err: unknown) => {
        const message = errorMessage(err, '读取服务商配置失败');
        setLoadError(message);
        notify(message);
      })
      .finally(() => setLoading(false));
  }, [notify]);

  useEffect(() => {
    loadConfig();
  }, [loadConfig]);

  // 价格目录刷新成功后后端会 emit("helm-pricing-catalog-updated")。
  // 重拉服务商配置，让模型列表价格随之更新，无需用户手动「同步模型」或重启。
  useEffect(() => {
    let active = true;
    let unlisten: (() => void) | null = null;
    void listen('helm-pricing-catalog-updated', () => {
      if (!active) return;
      void getProviderConfig()
        .then((next) => {
          if (active) setConfig(next);
        })
        .catch(() => {
          // 静默失败：浏览器预览无 Tauri 事件桥，或可选重拉失败不阻断刷新成功
        });
    })
      .then((stop) => {
        if (active) unlisten = stop;
        else stop();
      })
      .catch(() => {
        // 浏览器预览没有 Tauri 事件桥，忽略
      });
    return () => {
      active = false;
      unlisten?.();
    };
  }, []);

  const drawerProvider = useMemo(
    () => config.providers.find((provider) => provider.id === drawerProviderId),
    [drawerProviderId, config.providers],
  );

  return (
    <main className="main">
      <div className="page scroll">
        <div className="cm-tabs-wrapper">
          <div className="cm-tabs">
            {[
              ['bindings', '执行引擎'],
              ['providers', '服务商'],
              ['models', '模型'],
            ].map(([id, label]) => (
              <button
                key={id}
                className={tab === id ? 'is-active' : ''}
                onClick={() => setTab(id as Tab)}
              >
                {label}
              </button>
            ))}
          </div>
        </div>

        <div className="cm-pagebody cm-pagebody--scroll">
          {loading ? <div className="providers-empty">正在读取配置…</div> : null}
          {!loading && loadError ? (
            <div className="providers-empty providers-load-error" role="alert">
              <b>服务商配置读取失败</b>
              <span>{loadError}</span>
              <button className="btn btn--subtle btn--sm" type="button" onClick={loadConfig}>
                重试
              </button>
            </div>
          ) : null}
          {!loading && !loadError && tab === 'bindings' ? (
            <BindingsPanel
              config={config}
              onConfig={setConfig}
              onNotice={notify}
              autoOpenEngineId={pendingBindingEngineId}
              onAutoOpenConsumed={() => setPendingBindingEngineId(null)}
              onOpenAddFlow={() => {
                setTab('providers');
                setAddProviderOpen(true);
              }}
            />
          ) : null}
          {!loading && !loadError && tab === 'providers' ? (
            config.providers.length > 0 ? (
              <ProvidersGrid
                config={config}
                onOpen={(id) => {
                  setDrawerProviderId(id);
                }}
                onAdd={() => setAddProviderOpen(true)}
              />
            ) : (
              <EmptyProvidersPrompt onAdd={() => setAddProviderOpen(true)} />
            )
          ) : null}
          {!loading && !loadError && tab === 'models' ? (
            <ModelsPanel config={config} onConfig={setConfig} onNotice={notify} />
          ) : null}
        </div>
        {addProviderOpen ? (
          <AddProviderModal
            providers={config.providers}
            onConfig={setConfig}
            onNotice={notify}
            onClose={() => setAddProviderOpen(false)}
            onOpenExisting={(id) => {
              setAddProviderOpen(false);
              setDrawerProviderId(id);
            }}
          />
        ) : null}
        {drawerProvider ? (
          <ProviderDrawer
            key={drawerProvider.id}
            config={config}
            activeProvider={drawerProvider}
            onConfig={setConfig}
            onNotice={notify}
            onClose={() => setDrawerProviderId(null)}
            onUnbindJump={(engineId) => {
              // 决策 B-5a：切到执行引擎 Tab 并自动打开该引擎的绑定弹窗
              setPendingBindingEngineId(engineId);
              setTab('bindings');
              setDrawerProviderId(null);
            }}
          />
        ) : null}
      </div>
    </main>
  );
}

/** 添加流程出现的分段：仅列出有预设模板的接入类型（本地服务保留入口，不丢真实能力） */
// 用户裁决：添加流程只保留四类（本地服务不再提供新增入口，历史数据仍正常渲染）
const ADD_FLOW_GROUPS: ProviderTemplate['accessGroup'][] = [
  'subscription',
  'official',
  'plan',
  'relay',
];

function AddProviderModal({
  providers,
  onConfig,
  onNotice,
  onClose,
  onOpenExisting,
}: {
  providers: ProviderConfig[];
  onConfig: (config: AppConfig) => void;
  onNotice: (message: string | null) => void;
  onClose: () => void;
  onOpenExisting: (providerId: string) => void;
}) {
  // 对齐原型 §7.5.1-§7.5.5：选类型 → 选预设卡（已添加直接进详情）→ 表单 → 同步出模型阶段一起保存。
  const [step, setStep] = useState<'pick' | 'form' | 'login' | 'models'>('pick');
  const [group, setGroup] = useState<ProviderTemplate['accessGroup']>('subscription');
  const [selected, setSelected] = useState<ProviderTemplateId>(PROVIDER_TEMPLATES[0].id);
  const [name, setName] = useState(PROVIDER_TEMPLATES[0].name);
  const [baseUrl, setBaseUrl] = useState(PROVIDER_TEMPLATES[0].baseUrl);
  const [apiKey, setApiKey] = useState('');
  const [creating, setCreating] = useState(false);
  const [created, setCreated] = useState<ProviderConfig | null>(null);
  const [modelDrafts, setModelDrafts] = useState<ModelConfig[]>([]);
  const [roleDrafts, setRoleDrafts] = useState<Partial<Record<ProviderRoleKey, string>>>({});
  // 定价目录候选：模型组合框下拉与模糊匹配价签共用（读取失败按空目录处理，仍可手填）
  const [catalog, setCatalog] = useState<PricingCatalogEntry[]>([]);
  useEffect(() => {
    let active = true;
    getPricingCatalogEntries()
      .then((entries) => {
        if (active) setCatalog(entries);
      })
      .catch(() => {
        if (active) setCatalog([]);
      });
    return () => {
      active = false;
    };
  }, []);
  // 订阅卡「已登录」态：与列表页同一真实 CLI 检测源
  const [logins, setLogins] = useState<Partial<Record<'claude-code' | 'codex', CliLoginState>>>({});
  useEffect(() => {
    let active = true;
    for (const engine of ['claude-code', 'codex'] as const) {
      detectCliLogin(engine)
        .then((state) => {
          if (active) setLogins((prev) => ({ ...prev, [engine]: state }));
        })
        .catch(() => undefined);
    }
    return () => {
      active = false;
    };
  }, []);
  const [testing, setTesting] = useState(false);
  const [syncBusy, setSyncBusy] = useState(false);
  const [syncedAt, setSyncedAt] = useState<number | null>(null);
  const [syncedCount, setSyncedCount] = useState(0);
  // 同步候选缓存（原型 card.models）：只喂组合框下拉，不生成模型行
  const [syncOptions, setSyncOptions] = useState<{ id: string; priceText: string }[]>([]);
  const template = PROVIDER_TEMPLATES.find((item) => item.id === selected) ?? PROVIDER_TEMPLATES[0];
  const needsApiKey = template.authMethod === 'apikey';
  const testable = Boolean(baseUrl.trim()) && (!needsApiKey || Boolean(apiKey.trim()));
  const existingSubscription =
    template.kind === 'subscription'
      ? matchingSubscriptionProvider(providers, template.protocol)
      : undefined;
  const readyToCreate =
    Boolean(name.trim()) &&
    !existingSubscription &&
    (template.kind === 'subscription' || Boolean(baseUrl.trim())) &&
    (!needsApiKey || Boolean(apiKey.trim()));
  // 模型区形态只由当前模板协议决定，不能读 created——换模板时 created 可能是上一家服务商的残留
  const mode = providerModelMode({ protocol: template.protocol, kind: template.kind });
  const relay = template.accessGroup === 'relay';
  const [editingPrice, setEditingPrice] = useState<ModelConfig | null>(null);
  const compatLabel = protocolLabel(template.protocol);

  /** 丢弃上一轮创建成果。选了别的模板必须调用，否则「添加模型」会沿用旧服务商，
   *  模型区按旧协议渲染（openai → anthropic → openai 会看到 anthropic 的角色行）。 */
  const resetCreatedState = () => {
    setCreated(null);
    setModelDrafts([]);
    setRoleDrafts({});
    setSyncOptions([]);
    setSyncedAt(null);
    setSyncedCount(0);
  };

  const openFormFor = (item: (typeof PROVIDER_TEMPLATES)[number]) => {
    const sameTemplate = item.id === selected;
    setSelected(item.id);
    setName(item.name);
    setBaseUrl(item.baseUrl);
    setApiKey('');
    if (!sameTemplate) resetCreatedState();
    setStep('form');
  };
  /** 进入模型步骤：新建时带出该服务商已落库模型（保住二次进入的草稿），角色回显已有配置 */
  const enterModelsStep = (saved: ProviderConfig, config: AppConfig) => {
    setCreated(saved);
    setModelDrafts(modelCatalogForProvider(config, saved.id));
    setRoleDrafts({ ...(saved.roleModels ?? {}) });
    setSyncOptions([]);
    setSyncedAt(null);
    setSyncedCount(0);
    setStep('models');
  };
  const createProvider = (
    templateId: ProviderTemplateId = selected,
  ): Promise<{ saved: ProviderConfig; config: AppConfig }> => {
    const tpl = PROVIDER_TEMPLATES.find((item) => item.id === templateId) ?? template;
    const draft = createProviderDraft(templateId);
    const payload: ProviderConfig = {
      ...draft,
      // 只有在表单里编辑过当前模板时才用表单值；跨模板调用一律用预设值
      name: (templateId === selected ? name.trim() : tpl.name) || tpl.name,
      baseUrl:
        tpl.kind === 'subscription'
          ? ''
          : (templateId === selected ? baseUrl.trim() : tpl.baseUrl) || tpl.baseUrl,
    };
    setCreating(true);
    return saveProviderConfig(
      payload,
      templateId === selected ? apiKey.trim() || undefined : undefined,
    ).then((next) => {
      onConfig(next);
      const saved = next.providers.find((item) => item.id === payload.id);
      if (!saved) throw new Error('创建服务商失败：保存结果缺失');
      return { saved, config: next };
    });
  };
  /** 订阅卡点击即登录：显式传入模板，避免 setState 尚未生效时创建出别的服务商 */
  const startLoginFor = (item: (typeof PROVIDER_TEMPLATES)[number]) => {
    setCreating(true);
    loginCliAccount(item.protocol === 'anthropic' ? 'claude-code' : 'codex')
      .then(() => createProvider(item.id))
      .then(({ saved }) => {
        onNotice('登录完成，正在打开服务商详情');
        onOpenExisting(saved.id);
      })
      .catch((err: unknown) => onNotice(errorMessage(err, '登录未完成')))
      .finally(() => setCreating(false));
  };
  const finish = () => {
    if (!created) return;
    const rows = modelDrafts.filter((model) => model.id.trim() !== '');
    const payload: ProviderConfig = {
      ...created,
      name: name.trim(),
      baseUrl: template.kind === 'subscription' ? '' : baseUrl.trim(),
      ...(mode === 'roles-anthropic' ? { roleModels: roleDrafts } : {}),
    };
    void saveProviderConfig(payload, apiKey.trim() || undefined)
      .then((next) => {
        if (mode !== 'list-openai') return next;
        // 目录与启用集分两条命令落库：save_models_for_provider 会沿用旧 enabled，
        // 勾选必须再经 save_provider_model_selection 才生效（与服务商详情抽屉同口径）。
        return saveProviderModelsConfig(created.id, rows).then((afterModels) =>
          saveProviderModelSelection(
            created.id,
            rows.filter((model) => model.enabled).map((model) => model.id),
          ).then(() => afterModels),
        );
      })
      .then((next) => {
        onConfig(next);
        onNotice('服务商配置已保存');
        onClose();
      })
      .catch((err: unknown) => onNotice(errorMessage(err, '保存服务商失败')));
  };
  // 角色下拉候选：同步缓存优先（新添加流程 modelDrafts 为空）
  const modelById = new Map(modelDrafts.map((model) => [model.id, model]));
  const priceTextOf = (id: string) => {
    const saved = modelById.get(id);
    if (saved) return priceChipFor(saved).text;
    const match = fuzzyPriceMatch(catalog, id);
    return match ? catalogPriceText(match) : '未计价';
  };
  const roleRows = PROVIDER_ROLE_ROWS.anthropic;
  // 组合框确认（选中/失焦/回车）：空值保留该行；重复 ID 提示但不删行；其余模糊匹配带出目录价
  const commitModelRow = (providerId: string, index: number, id: string) => {
    const decision = commitProviderModelRow(
      modelDrafts.map((item) => item.id),
      index,
      id,
    );
    if (decision.action === 'keep-empty') return;
    if (decision.action === 'duplicate') {
      onNotice('该模型已在目录中');
      return;
    }
    const match = fuzzyPriceMatch(catalog, decision.id);
    setModelDrafts((prev) =>
      prev.map((item, at) =>
        at === index
          ? { ...buildManualModel(providerId, decision.id, match), enabled: item.enabled }
          : item,
      ),
    );
    if (match) onNotice(`已匹配定价目录：${match.modelId}`);
  };
  // 「测试连接」：草稿端点真实探活（GET /models 口径），服务商尚未创建、不落测试记录
  const testConnection = () => {
    setTesting(true);
    void testProviderDraft(baseUrl.trim(), apiKey.trim(), template.protocol)
      .then((result) => {
        onNotice(
          result.ok ? `连接正常 · ${result.latencyMs}ms` : `测试连接失败：${result.message}`,
        );
      })
      .catch((err: unknown) => onNotice(errorMessage(err, '测试连接失败')))
      .finally(() => setTesting(false));
  };
  // 模型步骤「同步模型」：拉取远端 /models 只作组合框候选（不铺到模型行）；
  // 已手动添加的行原样保留，后续「添加模型」从候选中挑选或继续手输。
  const runSync = () => {
    if (!created) return;
    setSyncBusy(true);
    void listProviderModels(created.id, { baseUrl, apiKey })
      .then((listing) => {
        onConfig(listing.config);
        setSyncOptions(
          listing.modelIds.map((id) => {
            const match = fuzzyPriceMatch(catalog, id);
            return { id, priceText: match ? catalogPriceText(match) : '未计价' };
          }),
        );
        setSyncedAt(Date.now());
        setSyncedCount(listing.modelIds.length);
        onNotice(`已同步 ${listing.modelIds.length} 个候选模型`);
      })
      .catch((err: unknown) => onNotice(errorMessage(err, '同步模型失败')))
      .finally(() => setSyncBusy(false));
  };

  return (
    <ShadcnDialog
      open
      onOpenChange={(open) => {
        if (!open) onClose();
      }}
    >
      <DialogContent className="max-w-2xl">
        <DialogHeader>
          <DialogTitle>
            {step === 'pick' ? '添加服务商' : step === 'login' ? template.name : template.name}
          </DialogTitle>
          <DialogDescription>
            {step === 'pick'
              ? '先选择接入类型，再选择具体服务商。'
              : step === 'login'
                ? '订阅账号通过隔离登录接入 Helm。'
                : step === 'form'
                  ? `接入类型：${providerAccessGroupLabel(template.accessGroup)}`
                  : `${compatLabel} · ${syncedAt ? `已同步 ${syncedCount} 个候选` : '模型手动添加或点「同步模型」获取候选'}`}
          </DialogDescription>
        </DialogHeader>

        {step === 'pick' ? (
          <div className="pv-modal__bd">
            <div className="pv-pick-seg" role="tablist" aria-label="接入类型">
              {ADD_FLOW_GROUPS.map((id) => {
                const definition = PROVIDER_ACCESS_GROUPS.find((item) => item.id === id);
                return (
                  <button
                    key={id}
                    role="tab"
                    aria-selected={group === id}
                    className={group === id ? 'is-active' : undefined}
                    onClick={() => setGroup(id)}
                    type="button"
                  >
                    {definition?.label ?? id}
                  </button>
                );
              })}
            </div>
            <p className="pv-pick-hint">{accessGroupHint(group)}</p>
            <div className="pv-add pv-pick">
              {templatesForAccessGroup(group).map((item) => {
                const added =
                  item.kind === 'subscription'
                    ? matchingSubscriptionProvider(providers, item.protocol)
                    : undefined;
                const loginOk =
                  item.kind === 'subscription' &&
                  (item.protocol === 'anthropic'
                    ? logins['claude-code']?.state === 'ok'
                    : logins['codex']?.state === 'ok');
                const subSettled = Boolean(added) || loginOk;
                const body = (
                  <>
                    <span className="pv-add__ic">
                      {item.icon ? (
                        <Icon name={item.icon} />
                      ) : (
                        <ProviderBrand providerId={item.brand} />
                      )}
                    </span>
                    <span className="pv-add__main">
                      <b>{item.title}</b>
                      <small>
                        {added
                          ? `同协议订阅「${added.name}」已存在，点击查看详情。`
                          : item.description}
                      </small>
                    </span>
                    {item.kind === 'subscription' ? (
                      added || loginOk ? (
                        <span className="pill pill--success">{added ? '已添加' : '已登录'}</span>
                      ) : null
                    ) : null}
                  </>
                );
                // 订阅卡整卡可点：已添加/已登录进详情；未添加未登录才给登录引导（HTML 不允许嵌套 button，用 div）
                return item.kind === 'subscription' ? (
                  <div
                    key={item.id}
                    className="pv-add__opt pv-add__opt--static"
                    role={subSettled ? 'button' : undefined}
                    tabIndex={subSettled ? 0 : undefined}
                    onClick={() => {
                      if (added) onOpenExisting(added.id);
                      else openFormFor(item);
                    }}
                  >
                    {body}
                  </div>
                ) : (
                  <button
                    key={item.id}
                    className="pv-add__opt"
                    onClick={() => openFormFor(item)}
                    type="button"
                  >
                    {body}
                  </button>
                );
              })}
            </div>
          </div>
        ) : null}

        {step === 'form' && template.kind === 'subscription' ? (
          <div className="pv-login">
            <span className="pv-login__brand">
              <ProviderBrand
                providerId={template.protocol === 'anthropic' ? 'anthropic' : 'openai'}
              />
            </span>
            <h3>{template.name}</h3>
            <p>{template.description}</p>
            <ol>
              <li>
                <i>1</i>在浏览器完成 OAuth 授权，登录态只写入 Helm 隔离 Profile
              </li>
              <li>
                <i>2</i>自动同步订阅官方模型目录，按订阅折算计费
              </li>
              <li>
                <i>3</i>回到「执行引擎」绑定对应引擎即可使用
              </li>
            </ol>
            <div className="cm-form-actions pv-login__actions">
              <button
                className="cm-action cm-action--primary"
                type="button"
                disabled={creating}
                onClick={() => startLoginFor(template)}
              >
                <Icon name="key" /> {creating ? '等待浏览器授权…' : '前往登录'}
              </button>
            </div>
            <small className="pv-login__note">
              无需 Base URL 与 API Key；退出登录只影响 Helm 隔离登录态。
            </small>
          </div>
        ) : null}

        {step === 'form' && template.kind !== 'subscription' ? (
          <div className="pv-modal__bd pv-add-form">
            <button
              className="cm-action cm-action--quiet pv-pick__back"
              onClick={() => setStep('pick')}
              type="button"
            >
              <Icon name="left" /> 返回上一步
            </button>
            <section className="cm-detail-card">
              <div className="cm-detail-card__head">
                <div>
                  <h3>{template.name}</h3>
                  <small>
                    {protocolLabel(template.protocol)}，
                    {template.accessGroup === 'relay'
                      ? '填入中转端点与密钥后，在下一步逐个添加模型。'
                      : '预设端点已内置；填入密钥后，在下一步逐个添加模型。'}
                  </small>
                </div>
                <span className="cm-status-pill is-ready">未配置</span>
              </div>
              <div className="cm-form">
                <div className="cm-field">
                  <label>服务商名称</label>
                  <input
                    className="cm-input"
                    value={name}
                    readOnly={
                      template.accessGroup === 'official' || template.accessGroup === 'plan'
                    }
                    onChange={(event) => setName(event.target.value)}
                  />
                  {template.accessGroup === 'official' || template.accessGroup === 'plan' ? (
                    <small>预设名称，保存后可在服务商详情调整。</small>
                  ) : null}
                </div>
                <div className="cm-field">
                  <label>Base URL</label>
                  <input
                    className="cm-input mono"
                    value={baseUrl}
                    readOnly={
                      template.accessGroup === 'official' || template.accessGroup === 'plan'
                    }
                    onChange={(event) => setBaseUrl(event.target.value)}
                  />
                  {template.accessGroup === 'official' || template.accessGroup === 'plan' ? (
                    <small>预设端点，只读。</small>
                  ) : null}
                </div>
                {needsApiKey ? (
                  <div className="cm-field">
                    <label>{template.accessGroup === 'plan' ? 'API Key / Token' : 'API Key'}</label>
                    <input
                      className="cm-input mono"
                      type="password"
                      value={apiKey}
                      onChange={(event) => setApiKey(event.target.value)}
                      placeholder={template.accessGroup === 'plan' ? '套餐令牌' : 'sk-...'}
                    />
                    <small>只进系统钥匙串，不写入配置文件与日志。</small>
                  </div>
                ) : null}
              </div>
            </section>
          </div>
        ) : null}

        {step === 'models' && created ? (
          <div className="pv-modal__bd pv-add-form">
            <button
              className="cm-action cm-action--quiet pv-pick__back"
              onClick={() => setStep('form')}
              type="button"
            >
              <Icon name="left" /> 返回上一步
            </button>
            {mode === 'list-openai' ? (
              <section className="cm-detail-card">
                <div className="cm-detail-card__head">
                  <div>
                    <h3>模型配置</h3>
                    <small>
                      {syncedAt
                        ? `已同步 ${syncedCount} 个候选 · 刚刚 · ${compatLabel}`
                        : '尚未同步 · 可直接手动填写，或点右上角「同步模型」获取候选'}
                    </small>
                  </div>
                  <span className="pv-headright">
                    <span className={'cm-status-pill' + (syncedAt ? ' is-ready' : ' is-warn')}>
                      {syncedAt ? '已同步' : '未同步'}
                    </span>
                    <button
                      className="cm-action"
                      type="button"
                      disabled={syncBusy}
                      onClick={runSync}
                    >
                      <Icon name="refresh" className={syncBusy ? 'spin' : undefined} /> 同步模型
                    </button>
                  </span>
                </div>
                {modelDrafts.length === 0 ? (
                  <div className="pv-empty">
                    {syncedAt
                      ? '还没有添加模型 · 点下方「添加模型」从候选中选择'
                      : '还没有添加模型 · 建议先「同步模型」获取候选，也可直接手动填写'}
                  </div>
                ) : (
                  <div className="pv-openai-list">
                    {modelDrafts.map((model, index) => {
                      const chip = priceChipFor(model);
                      return (
                        <div key={index} className="pv-synclist__row" data-row-id={model.id}>
                          <label className="cm-switch">
                            <input
                              type="checkbox"
                              checked={model.enabled}
                              onChange={() =>
                                setModelDrafts(
                                  modelDrafts.map((item, at) =>
                                    at === index ? { ...item, enabled: !item.enabled } : item,
                                  ),
                                )
                              }
                            />
                            <i />
                          </label>
                          <ModelIdCombo
                            value={model.id}
                            options={syncOptions}
                            taken={modelDrafts.map((item) => item.id.trim()).filter(Boolean)}
                            autoOpen={!model.id.trim() && syncOptions.length > 0}
                            onCommit={(id) => commitModelRow(created.id, index, id)}
                          />
                          <span className="pv-synclist__end">
                            <span
                              className={'pv-pricechip' + (chip.priced ? '' : ' is-none')}
                              title={
                                chip.priced ? '输入 / 缓存读取 / 输出，单位 $/MTok' : undefined
                              }
                            >
                              {chip.text}
                            </span>
                            {relay ? (
                              <button
                                className="cm-action pv-priceedit"
                                type="button"
                                title="修改此模型的定价（输入 / 缓存 / 输出）"
                                onClick={() => setEditingPrice(model)}
                              >
                                改价
                              </button>
                            ) : null}
                            <button
                              className="btn-icon"
                              type="button"
                              aria-label="移除模型"
                              onClick={() =>
                                setModelDrafts(modelDrafts.filter((_, at) => at !== index))
                              }
                            >
                              <Icon name="trash" />
                            </button>
                          </span>
                        </div>
                      );
                    })}
                  </div>
                )}
                <div className="pv-synclist__add pv-synclist__add--solo">
                  <button
                    className="cm-action"
                    type="button"
                    onClick={() =>
                      setModelDrafts([...modelDrafts, buildManualModel(created.id, '', null)])
                    }
                  >
                    <Icon name="plus" /> 添加模型
                  </button>
                </div>
              </section>
            ) : (
              <section className="cm-detail-card">
                <div className="cm-detail-card__head">
                  <div>
                    <h3>模型配置</h3>
                    <small>
                      {syncedAt
                        ? `已同步 ${syncedCount} 个候选 · 刚刚 · ${compatLabel}`
                        : '尚未同步 · 可直接手动填写，或点右上角「同步模型」获取候选'}
                    </small>
                  </div>
                  <span className="pv-headright">
                    <span className={'cm-status-pill' + (syncedAt ? ' is-ready' : ' is-warn')}>
                      {syncedAt ? '已同步' : '未同步'}
                    </span>
                    <button
                      className="cm-action"
                      type="button"
                      disabled={syncBusy}
                      onClick={runSync}
                    >
                      <Icon name="refresh" className={syncBusy ? 'spin' : undefined} /> 同步模型
                    </button>
                  </span>
                </div>
                <div className="pv-selwrap">
                  {roleRows.map((role) => {
                    const currentId = roleDrafts[role.key] ?? '';
                    return (
                      <div key={role.key} className="pv-selrow">
                        <b>{role.label}</b>
                        <ModelIdCombo
                          value={currentId}
                          options={syncOptions.map((option) => ({
                            id: option.id,
                            priceText: priceTextOf(option.id),
                          }))}
                          ghost={false}
                          placeholder={syncedAt ? '' : '等待同步或手动填写'}
                          onCommit={(id) => setRoleDrafts({ ...roleDrafts, [role.key]: id.trim() })}
                        />
                        <span className="pv-selrow__end">
                          <span
                            className={
                              'pv-pricechip' +
                              (currentId && priceTextOf(currentId) !== '未计价' ? '' : ' is-none')
                            }
                          >
                            {currentId ? priceTextOf(currentId) : '未计价'}
                          </span>
                          {relay && currentId ? (
                            <button
                              className="cm-action pv-priceedit"
                              type="button"
                              title="修改此模型的定价（输入 / 缓存 / 输出）"
                              onClick={() =>
                                setEditingPrice(
                                  modelById.get(currentId) ??
                                    buildManualModel(
                                      created.id,
                                      currentId,
                                      fuzzyPriceMatch(catalog, currentId),
                                    ),
                                )
                              }
                            >
                              改价
                            </button>
                          ) : null}
                        </span>
                      </div>
                    );
                  })}
                </div>
              </section>
            )}
          </div>
        ) : null}

        <div className="cm-form-actions">
          {step === 'form' && template.kind !== 'subscription' ? (
            <button
              className="cm-action"
              disabled={!testable || testing}
              onClick={testConnection}
              type="button"
            >
              {testing ? '正在测试…' : '测试连接'}
            </button>
          ) : null}
          {step === 'form' && template.kind !== 'subscription' ? (
            <button
              className="cm-action cm-action--primary"
              disabled={!readyToCreate || creating}
              onClick={() => {
                setCreating(true);
                // created 只在同一模板的往返中复用；换过模板或协议不符时必须真正新建，
                // 否则会把上一家服务商当成本次成果，模型区沿用旧协议形态。
                const reusable = created && created.protocol === template.protocol;
                const work = (
                  reusable ? Promise.resolve({ saved: created, config: null }) : createProvider()
                ).then(({ saved, config }) => {
                  if (config) enterModelsStep(saved, config);
                  else {
                    setCreated(saved);
                    setStep('models');
                  }
                });
                void work
                  .catch((err: unknown) => onNotice(errorMessage(err, '创建服务商失败')))
                  .finally(() => setCreating(false));
              }}
              type="button"
            >
              {creating ? '正在创建…' : '添加模型'}
            </button>
          ) : null}
          {step === 'models' ? (
            <button className="cm-action cm-action--primary" onClick={finish} type="button">
              保存
            </button>
          ) : null}
        </div>
        {editingPrice ? (
          <ModelPriceModal
            model={editingPrice}
            onClose={() => setEditingPrice(null)}
            onSaved={async (message) => {
              // 手动价按服务商+模型落库；刷新全局配置并回写当前草稿行，让价签立即反映
              try {
                onConfig(await getProviderConfig());
                const overrides = await listModelPriceOverrides();
                const existing = overrides.find(
                  (item) => item.providerId === created?.id && item.modelId === editingPrice.id,
                );
                const band = existing?.tiers.standard?.bands[0];
                if (band) {
                  setModelDrafts((prev) =>
                    prev.map((item) =>
                      item.id === editingPrice.id
                        ? {
                            ...item,
                            inputPricePerMtok: band.input,
                            cachedInputPricePerMtok: band.cachedInput ?? undefined,
                            outputPricePerMtok: band.output,
                            priceSource: 'manual',
                          }
                        : item,
                    ),
                  );
                }
              } catch {
                // 覆盖价回读失败不影响保存结果提示
              }
              onNotice(message);
              setEditingPrice(null);
            }}
          />
        ) : null}
      </DialogContent>
    </ShadcnDialog>
  );
}
function EmptyProvidersPrompt({ onAdd }: { onAdd: () => void }) {
  return (
    <section className="card card--pad providers-empty-card">
      <span className="provider-detail__big">
        <Icon name="server" />
      </span>
      <div>
        <h2>还没有服务商</h2>
        <p>添加 Claude / ChatGPT 订阅，或接入一个 API 服务商、本地模型服务。</p>
      </div>
      <button className="btn btn--primary" onClick={onAdd} type="button">
        <Icon name="plus" /> 添加服务商
      </button>
    </section>
  );
}

function BindingsPanel({
  config,
  onConfig,
  onNotice,
  autoOpenEngineId,
  onAutoOpenConsumed,
  onOpenAddFlow,
}: {
  config: AppConfig;
  onConfig: (config: AppConfig) => void;
  onNotice: (message: string | null) => void;
  autoOpenEngineId?: string | null;
  onAutoOpenConsumed?: () => void;
  onOpenAddFlow?: () => void;
}) {
  const [editingEngine, setEditingEngine] = useState<EngineConfig | null>(null);
  const [selectedEngineId, setSelectedEngineId] = useState<string>(config.engines[0]?.id ?? '');
  useEffect(() => {
    if (!autoOpenEngineId) return;
    const engine = config.engines.find((item) => item.id === autoOpenEngineId);
    if (engine) setEditingEngine(engine);
    onAutoOpenConsumed?.();
    // 决策 B-5a：抽屉「去绑定」跳转后仅触发一次
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [autoOpenEngineId]);

  const selectedEngine =
    config.engines.find((item) => item.id === selectedEngineId) ?? config.engines[0] ?? null;

  return (
    <>
      <div className="cm-grid cm-grid--2">
        {config.engines.map((engine) => (
          <EngineBindingCard
            key={engine.id}
            config={config}
            engine={engine}
            selected={selectedEngine?.id === engine.id}
            onSelect={() => setSelectedEngineId(engine.id)}
            onEdit={() => setEditingEngine(engine)}
          />
        ))}
      </div>
      {selectedEngine ? (
        <EngineDetailPanel
          key={selectedEngine.id}
          config={config}
          engine={selectedEngine}
          onConfig={onConfig}
          onNotice={onNotice}
        />
      ) : null}
      {editingEngine ? (
        <BindingModal
          config={config}
          engine={editingEngine}
          onClose={() => setEditingEngine(null)}
          onConfig={onConfig}
          onNotice={onNotice}
          onOpenAddFlow={onOpenAddFlow}
        />
      ) : null}
    </>
  );
}

function EngineBindingCard({
  config,
  engine,
  selected,
  onSelect,
  onEdit,
}: {
  config: AppConfig;
  engine: EngineConfig;
  selected: boolean;
  onSelect: () => void;
  onEdit: () => void;
}) {
  const binding = bindingForEngine(config, engine);
  const providerNameText = binding
    ? (config.providers.find((item) => item.id === binding.providerId)?.name ?? '未绑定服务商')
    : '未绑定服务商';

  return (
    <section
      className={'cm-engine-card' + (selected ? ' is-selected' : '')}
      onClick={onSelect}
      role="button"
      tabIndex={0}
      onKeyDown={(event) => {
        if (event.key === 'Enter' || event.key === ' ') onSelect();
      }}
    >
      <div className="cm-engine-card__top">
        <span className="cm-brand cm-brand--light">
          <EngineBrand engine={engine.id} size={18} />
        </span>
        <div className="cm-engine-card__id">
          <h3>{engine.name}</h3>
          <span
            className={
              'cm-status-pill' +
              (binding && engine.status === 'ready'
                ? ' is-ready'
                : !binding
                  ? ' is-warn'
                  : ' is-warn')
            }
          >
            {binding && engine.status === 'ready' ? '已就绪' : !binding ? '未绑定' : '未就绪'}
          </span>
        </div>
        <button
          className="cm-action cm-engine-card__bind"
          onClick={(event) => {
            event.stopPropagation();
            onEdit();
          }}
        >
          <Icon name="gitbranch" /> {binding ? '更改绑定' : '绑定服务商'}
        </button>
      </div>
      <p className="cm-engine-card__desc">{ENGINE_DESCRIPTIONS[engine.id] ?? ''}</p>
      <div className="cm-engine-card__meta">
        <span>CLI v{engine.version || '未检测'}</span>
        <span>绑定 {providerNameText}</span>
        <span>默认 {binding?.primaryModel || '—'}</span>
      </div>
    </section>
  );
}

const ENGINE_DESCRIPTIONS: Record<string, string> = {
  'claude-code': 'Anthropic 的本地编码 Agent，适合长任务、复杂代码理解和多步执行。',
  codex: 'OpenAI 的本地编码 Agent，支持原生线程续接、沙箱和结构化工具调用。',
};

function EngineDetailPanel({
  config,
  engine,
  onConfig,
  onNotice,
}: {
  config: AppConfig;
  engine: EngineConfig;
  onConfig: (config: AppConfig) => void;
  onNotice: (message: string | null) => void;
}) {
  const binding = bindingForEngine(config, engine);
  const isClaude = engine.id === 'claude-code';
  const [readiness, setReadiness] = useState<{
    installed: boolean;
    path: string | null;
    version: string | null;
  } | null>(null);
  const [checking, setChecking] = useState(false);
  const [lastCheckText, setLastCheckText] = useState('尚未检测');
  const [envText, setEnvText] = useState(binding ? '正在读取等价配置…' : '尚未配置绑定');
  const [configFile, setConfigFile] = useState<EngineConfigFile | null>(null);
  const [fileDraft, setFileDraft] = useState('');
  const [editingFile, setEditingFile] = useState(false);
  const [fileBusy, setFileBusy] = useState(false);
  type EnvRow = { name: string; value: string };
  const [envRows, setEnvRows] = useState<EnvRow[]>([]);
  const providerId = binding?.providerId ?? '';
  const models = providerId ? bindingModelOptions(config, providerId) : [];
  const [fastModel, setFastModel] = useState<string>(binding?.fastModel ?? '');
  const [reasoningEffort, setReasoningEffort] = useState<ReasoningEffort>(
    binding?.reasoningEffort ?? 'auto',
  );
  const [thinkingEnabled, setThinkingEnabled] = useState<boolean>(
    binding?.thinkingEnabled ?? false,
  );
  const [context1m, setContext1m] = useState<boolean>(binding?.context1m ?? false);

  useEffect(() => {
    let live = true;
    getReadinessReport()
      .then((report) => {
        if (!live) return;
        const item = engine.id === 'claude-code' ? report.claudeCode : report.codex;
        setReadiness({ installed: item.installed, path: item.path, version: item.version });
      })
      .catch(() => {
        if (live) setReadiness(null);
      });
    return () => {
      live = false;
    };
  }, [engine.id]);

  useEffect(() => {
    let live = true;
    if (!binding) {
      setEnvText('尚未配置绑定');
      return;
    }
    getEquivalentEnv(binding)
      .then((pairs) => {
        if (live) setEnvText(envPairsToText(pairs));
      })
      .catch((err: unknown) => {
        if (live) setEnvText(errorMessage(err, '读取等价配置失败'));
      });
    return () => {
      live = false;
    };
  }, [binding]);

  useEffect(() => {
    let live = true;
    readEngineConfigFile(engine.id)
      .then((file) => {
        if (!live) return;
        setConfigFile(file);
        setFileDraft(file.content);
      })
      .catch(() => undefined);
    return () => {
      live = false;
    };
  }, [engine.id]);

  // 环境变量：引擎级覆盖，独立于 Helm 配置文件存储（EngineConfig.envVars）
  useEffect(() => {
    setEnvRows(
      (engine.envVars ?? []).map((item) => ({
        name: item.name,
        value: item.value ?? '',
      })),
    );
  }, [engine.id, engine.envVars]);

  const persistEnv = (rows: EnvRow[]) => {
    const clean = rows.filter((row) => row.name.trim());
    void saveEngineConfig({
      ...engine,
      envVars: clean.map((row) => ({ name: row.name.trim(), value: row.value })),
    })
      .then((next) => {
        onConfig(next);
        onNotice('环境变量已保存到当前执行引擎');
      })
      .catch((err: unknown) => onNotice(errorMessage(err, '保存环境变量失败')));
  };

  useEffect(() => {
    if (!binding) return;
    setFastModel(binding.fastModel ?? '');
    setReasoningEffort(binding.reasoningEffort ?? 'auto');
    setThinkingEnabled(binding.thinkingEnabled ?? false);
    setContext1m(binding.context1m ?? false);
  }, [binding]);

  const persistPref = (patch: {
    fastModel?: string;
    reasoningEffort?: ReasoningEffort;
    thinkingEnabled?: boolean;
    context1m?: boolean;
  }) => {
    if (!binding) {
      onNotice('请先绑定服务商后再设置引擎偏好');
      return;
    }
    const draft = normalizeBindingDraft(config, {
      engineId: engine.id,
      providerId: binding.providerId,
      primaryModel: binding.primaryModel,
      fastModel: patch.fastModel ?? fastModel,
      reasoningEffort: patch.reasoningEffort ?? reasoningEffort,
    });
    const next: BindingConfig = {
      ...draft,
      thinkingEnabled: patch.thinkingEnabled ?? thinkingEnabled,
      context1m: patch.context1m ?? context1m,
    };
    void saveBindingConfig(next)
      .then((saved) => {
        onConfig(saved);
        onNotice('引擎偏好已保存');
      })
      .catch((err: unknown) => onNotice(errorMessage(err, '保存引擎偏好失败')));
  };

  const rerunCheck = () => {
    setChecking(true);
    void testEngineConfig(engine.bin)
      .then((result) => {
        if (result.ok) setLastCheckText('刚刚');
        onNotice(result.ok ? null : result.message);
        return getReadinessReport().then((report) => {
          const item = engine.id === 'claude-code' ? report.claudeCode : report.codex;
          setReadiness({ installed: item.installed, path: item.path, version: item.version });
        });
      })
      .catch((err: unknown) => onNotice(errorMessage(err, '检测引擎失败')))
      .finally(() => setChecking(false));
  };

  const releaseUrl =
    engine.id === 'claude-code'
      ? 'https://github.com/anthropics/claude-code/releases'
      : 'https://github.com/openai/codex/releases';
  const prefsDisabled = !binding;

  return (
    <div className="cm-engine-detail">
      <div className="cm-detail-header">
        <div>
          <h2>{engine.name} 配置</h2>
          <p>
            {isClaude
              ? '保存后影响新任务和已有任务的下一次发送，运行中的 Turn 不受影响。'
              : '绑定服务商后即可解析可用模型与计费口径；保存后影响下一次发送。'}
          </p>
        </div>
        <span
          className={
            'cm-status-pill' + (binding && engine.status === 'ready' ? ' is-ready' : ' is-warn')
          }
        >
          {binding && engine.status === 'ready' ? '配置有效' : '未绑定服务商'}
        </span>
      </div>
      {!binding ? (
        <div className="pv-bind-needed">
          <Icon name="alert" />
          <span>
            {engine.name}{' '}
            尚未绑定服务商，无法解析可用模型与计费口径。在引擎卡上点击「绑定服务商」完成绑定。
          </span>
        </div>
      ) : null}
      <div className="cm-detail-card pv-panel">
        <section className="pv-sec">
          <div className="pv-sec__head">
            <div>
              <h3>运行环境</h3>
              <small>上次检测：{checking ? '正在检测…' : lastCheckText}</small>
            </div>
            <button
              className="btn btn--subtle btn--sm"
              disabled={checking}
              onClick={rerunCheck}
              type="button"
            >
              <Icon name="refresh" className={checking ? 'spin' : undefined} />
              重新检测 {engine.name} CLI
            </button>
          </div>
          <div className="cm-setting-grid">
            <div className="cm-option-row">
              <div className="cm-option-row__main">
                <b>CLI 路径</b>
                <small className="mono">{readiness?.path || engine.bin}</small>
              </div>
              <span className={'cm-status-pill' + (readiness?.installed ? ' is-ready' : '')}>
                {readiness?.installed ? '可用' : '未检测'}
              </span>
            </div>
            <div className="cm-option-row">
              <div className="cm-option-row__main">
                <b>CLI 版本</b>
                <small className="mono">{readiness?.version || engine.version || '未检测'}</small>
              </div>
              <a
                className="cm-inline-link"
                href={releaseUrl}
                target="_blank"
                rel="noreferrer"
                onClick={(event) => {
                  event.preventDefault();
                  void openExternalUrl(releaseUrl).catch((err: unknown) =>
                    onNotice(errorMessage(err, '打开链接失败')),
                  );
                }}
              >
                查看发布说明
              </a>
            </div>
          </div>
        </section>
        <section className="pv-sec">
          <div className="pv-sec__head">
            <div>
              <h3>引擎偏好</h3>
              <small>存储于 Helm Engine Profile，不影响终端原生配置</small>
            </div>
          </div>
          <div className="cm-setting-grid">
            <div className="cm-option-row">
              <div className="cm-option-row__main">
                <b>默认推理强度</b>
                <small>
                  {isClaude
                    ? '新任务默认带出，工作台仍可在 Turn 之间调整。'
                    : '按 Codex 真实支持范围提供：低、中、高、超高与自动。'}
                </small>
              </div>
              <select
                className="cm-select w-130"
                disabled={prefsDisabled}
                value={reasoningEffort}
                onChange={(event) => {
                  const next = event.target.value as ReasoningEffort;
                  setReasoningEffort(next);
                  persistPref({ reasoningEffort: next });
                }}
              >
                {engineEffortTiers(engine.id).map((effort) => (
                  <option key={effort} value={effort}>
                    {reasoningEffortLabel(effort)}
                  </option>
                ))}
              </select>
            </div>
            {isClaude ? (
              <div className="cm-option-row">
                <div className="cm-option-row__main">
                  <b>默认开启思考</b>
                  <small>独立于推理强度，使用 Claude Code 的原生能力。</small>
                </div>
                <label className="cm-switch">
                  <input
                    type="checkbox"
                    checked={thinkingEnabled}
                    disabled={prefsDisabled}
                    onChange={(event) => {
                      const next = event.target.checked;
                      setThinkingEnabled(next);
                      persistPref({ thinkingEnabled: next });
                    }}
                  />
                  <i />
                </label>
              </div>
            ) : null}
            {isClaude ? (
              <div className="cm-option-row">
                <div className="cm-option-row__main">
                  <b>1M 上下文</b>
                  <small>开启时先校验当前服务商与模型能力。</small>
                </div>
                <label className="cm-switch">
                  <input
                    type="checkbox"
                    checked={context1m}
                    disabled={prefsDisabled}
                    onChange={(event) => {
                      const next = event.target.checked;
                      setContext1m(next);
                      persistPref({ context1m: next });
                    }}
                  />
                  <i />
                </label>
              </div>
            ) : null}
            <div className="cm-option-row">
              <div className="cm-option-row__main">
                <b>快速模型</b>
                <small>用于自动标题等轻量后台任务，不进入工作台模型列表。</small>
              </div>
              <select
                className="cm-select w-160"
                disabled={prefsDisabled}
                value={fastModel}
                onChange={(event) => {
                  const next = event.target.value;
                  setFastModel(next);
                  persistPref({ fastModel: next });
                }}
              >
                <option value="">未设置</option>
                {(() => {
                  const boundProvider = config.providers.find((item) => item.id === providerId);
                  if (boundProvider && providerModelMode(boundProvider) === 'roles-anthropic') {
                    return PROVIDER_ROLE_ROWS.anthropic.map((role) => (
                      <option key={role.key} value={`role:${role.key}`}>
                        {role.label.toUpperCase()}
                      </option>
                    ));
                  }
                  return models.map((model) => (
                    <option key={model.id} value={model.id}>
                      {model.displayName && model.displayName !== model.id
                        ? `${model.displayName} · ${model.id}`
                        : model.id}
                    </option>
                  ));
                })()}
              </select>
            </div>
          </div>
        </section>
        <details className="pv-advanced">
          <summary>
            <span className="pv-model__chev">
              <Icon name="right" />
            </span>
            <span>高级设置</span>
            <span className="pv-advanced__hint">环境变量 · Helm Engine Profile</span>
          </summary>
          <div className="pv-advanced__body">
            <div className="providers-warning" role="note">
              这里只调整引擎自身的行为；服务商、模型、认证、权限等由 Helm
              在对应页面统一配置，不在此重复设置。
            </div>
            <div className="pv-advsec">
              <div className="pv-advsec__head">
                <h3>环境变量</h3>
                <button
                  className="btn btn--subtle btn--sm"
                  type="button"
                  onClick={() => setEnvRows([...envRows, { name: '', value: '' }])}
                >
                  <Icon name="plus" /> 添加变量
                </button>
              </div>
              <p className="pv-advsec__note">
                秘密值存入系统钥匙串，预览、SQLite 与日志不回显明文。
              </p>
              {envRows.length ? (
                <div className="pv-env">
                  {envRows.map((row, index) => (
                    <div key={index} className="pv-env__row">
                      <input
                        className="input mono"
                        placeholder="变量名"
                        aria-label="变量名"
                        value={row.name}
                        onChange={(event) => {
                          const next = [...envRows];
                          next[index] = { ...row, name: event.target.value };
                          setEnvRows(next);
                        }}
                        onBlur={() => persistEnv(envRows)}
                      />
                      <div className="pv-env__value">
                        <input
                          className="input mono"
                          placeholder="变量值"
                          aria-label="变量值"
                          value={row.value}
                          onChange={(event) => {
                            const next = [...envRows];
                            next[index] = { ...row, value: event.target.value };
                            setEnvRows(next);
                          }}
                          onBlur={() => persistEnv(envRows)}
                        />
                      </div>
                      <button
                        className="btn-icon sm"
                        type="button"
                        title="移除变量"
                        aria-label="移除变量"
                        onClick={() => {
                          const next = envRows.filter((_, k) => k !== index);
                          setEnvRows(next);
                          persistEnv(next);
                        }}
                      >
                        <Icon name="trash" />
                      </button>
                    </div>
                  ))}
                </div>
              ) : (
                <p className="pv-models-hint">暂无自定义环境变量。</p>
              )}
            </div>
            <div className="pv-advsec">
              <div className="pv-advsec__head">
                <h3>
                  Helm 配置 <span className="cm-source-label">{isClaude ? 'JSON' : 'TOML'}</span>
                </h3>
              </div>
              <p className="pv-advsec__note">
                受控 Profile，不修改全局配置；与上方「引擎偏好」联动，保存时自动汇总。
              </p>
              <div className="engine-file">
                <div className="engine-file__actions">
                  <button
                    className="btn btn--subtle btn--sm"
                    onClick={() => setEditingFile((value) => !value)}
                    type="button"
                  >
                    <Icon name="edit" /> 高级编辑
                  </button>
                  {editingFile ? (
                    <button
                      className="btn btn--primary btn--sm"
                      disabled={fileBusy}
                      type="button"
                      onClick={() => {
                        setFileBusy(true);
                        void writeEngineConfigFile(engine.id, fileDraft)
                          .then((file) => {
                            setConfigFile(file);
                            setFileDraft(file.content);
                            setEditingFile(false);
                            onNotice('已写入真实配置文件');
                          })
                          .catch((err: unknown) => onNotice(errorMessage(err, '保存配置文件失败')))
                          .finally(() => setFileBusy(false));
                      }}
                    >
                      保存
                    </button>
                  ) : null}
                </div>
                {editingFile ? (
                  <textarea
                    className="engine-file__editor mono"
                    value={fileDraft}
                    onChange={(event) => setFileDraft(event.target.value)}
                  />
                ) : (
                  <pre className="engine-file__preview mono">
                    {configFile?.content || '真实配置文件为空或尚未创建'}
                  </pre>
                )}
              </div>
            </div>
            <div className="pv-advsec">
              <div className="pv-advsec__head">
                <h3>最终启动配置</h3>
                <span className="faint">只读</span>
              </div>
              <p className="pv-advsec__note">
                由真实启动解析器生成，反映当前绑定、模型与引擎偏好。
              </p>
              <pre className="env">{envText}</pre>
            </div>
          </div>
        </details>
      </div>
    </div>
  );
}

function BindingModal({
  config,
  engine,
  onClose,
  onConfig,
  onNotice,
  onOpenAddFlow,
}: {
  config: AppConfig;
  engine: EngineConfig;
  onClose: () => void;
  onConfig: (config: AppConfig) => void;
  onNotice: (message: string | null) => void;
  onOpenAddFlow?: () => void;
}) {
  const providers = compatibleProvidersForEngine(config, engine.id);
  const current = bindingForEngine(config, engine);
  const initialProviderId = current?.providerId ?? providers[0]?.id ?? '';
  const initialBinding = normalizeBindingDraft(config, {
    engineId: engine.id,
    providerId: initialProviderId,
    primaryModel: current?.primaryModel ?? '',
    fastModel: current?.fastModel ?? null,
    reasoningEffort: current?.reasoningEffort ?? 'auto',
  });
  const [providerId, setProviderId] = useState(initialBinding.providerId);
  const models = bindingModelOptions(config, providerId);
  const [primaryModel, setPrimaryModel] = useState(initialBinding.primaryModel);
  const [fastModel, setFastModel] = useState(initialBinding.fastModel ?? '');
  const [reasoningEffort] = useState<ReasoningEffort>(initialBinding.reasoningEffort ?? 'auto');
  const [, setSyncingSubscriptionModels] = useState(false);
  const [subscriptionModelsRefreshed, setSubscriptionModelsRefreshed] = useState(false);
  const selectedProvider = config.providers.find((provider) => provider.id === providerId);
  const selectedProviderId = selectedProvider?.id;
  const selectedProviderKind = selectedProvider?.kind;
  const [selectedLogin, setSelectedLogin] = useState<CliLoginState | null>(null);
  const cancelBindingChange = useCallback(() => {
    if (subscriptionModelsRefreshed) {
      onNotice('已取消绑定更改；账号模型刷新结果仍已保存，当前生效绑定没有改变');
    }
    onClose();
  }, [onClose, onNotice, subscriptionModelsRefreshed]);

  useEffect(() => {
    const normalized = normalizeBindingDraft(config, {
      engineId: engine.id,
      providerId,
      primaryModel,
      fastModel: fastModel || null,
      reasoningEffort,
    });
    if (normalized.primaryModel !== primaryModel) {
      setPrimaryModel(normalized.primaryModel);
    }
    if ((normalized.fastModel ?? '') !== fastModel) {
      setFastModel(normalized.fastModel ?? '');
    }
  }, [config, engine.id, fastModel, primaryModel, providerId, reasoningEffort]);

  useEffect(() => {
    let live = true;
    if (selectedProviderKind !== 'subscription') {
      setSelectedLogin(null);
      return;
    }
    setSelectedLogin(null);
    detectCliLogin(engine.id)
      .then(async (state) => {
        if (live) setSelectedLogin(state);
        if (
          live &&
          state.state === 'ok' &&
          state.authMethod === 'subscription' &&
          selectedProviderId
        ) {
          setSyncingSubscriptionModels(true);
          try {
            const next = await syncProviderModels(selectedProviderId);
            if (live) {
              onConfig(next);
              setSubscriptionModelsRefreshed(true);
            }
          } catch (err: unknown) {
            if (live) onNotice(errorMessage(err, '读取订阅账号模型失败'));
          } finally {
            if (live) setSyncingSubscriptionModels(false);
          }
        }
      })
      .catch((err: unknown) => {
        if (live) {
          setSelectedLogin({
            state: 'unknown',
            authMethod: 'unknown',
            detail: errorMessage(err, '检测登录态失败'),
          });
        }
      });
    return () => {
      live = false;
    };
  }, [engine.id, onConfig, onNotice, selectedProviderId, selectedProviderKind]);

  return (
    <ShadcnDialog
      open
      onOpenChange={(open) => {
        if (!open) cancelBindingChange();
      }}
    >
      <DialogContent className="max-w-[620px]">
        <DialogHeader>
          <DialogTitle>更改 {engine.name} 绑定</DialogTitle>
          <DialogDescription>
            新绑定从下一次真正发送时生效，不修改正在运行的 Turn。
          </DialogDescription>
        </DialogHeader>
        <div className="pv-modal__bd">
          <label className="field">
            <span>服务商</span>
            <select
              className="cm-select"
              value={providerId}
              onChange={(event) => {
                const nextProviderId = event.target.value;
                if (nextProviderId === '__add__') {
                  onOpenAddFlow?.();
                  onClose();
                  return;
                }
                const normalized = normalizeBindingDraft(config, {
                  engineId: engine.id,
                  providerId: nextProviderId,
                  primaryModel,
                  fastModel: fastModel || null,
                  reasoningEffort,
                });
                setProviderId(nextProviderId);
                setPrimaryModel(normalized.primaryModel);
                setFastModel(normalized.fastModel ?? '');
              }}
            >
              {providers.map((provider) => (
                <option key={provider.id} value={provider.id}>
                  {provider.name}
                </option>
              ))}
              <option value="__add__">添加新服务商…</option>
            </select>
          </label>
          <label className="field">
            <span>默认模型</span>
            <select
              className="cm-select"
              value={primaryModel}
              onChange={(event) => setPrimaryModel(event.target.value)}
            >
              {selectedProvider && providerModelMode(selectedProvider) === 'roles-anthropic'
                ? PROVIDER_ROLE_ROWS.anthropic.map((role) => (
                    <option key={role.key} value={`role:${role.key}`}>
                      {role.label.toUpperCase()}
                    </option>
                  ))
                : models.map((model) => (
                    <option key={`${model.providerId}:${model.id}`} value={model.id}>
                      {model.displayName && model.displayName !== model.id
                        ? `${model.displayName} · ${model.id}`
                        : model.id}
                    </option>
                  ))}
            </select>
          </label>
          <div className="cm-note">
            <Icon name="info" />
            <span>已有任务后续发送也会使用新的服务商；模型候选将同步刷新。</span>
          </div>
        </div>
        <DialogFooter>
          <Button
            variant="primary"
            disabled={
              !selectedProvider ||
              !providerId ||
              !primaryModel ||
              !canBindProvider(selectedProvider, selectedLogin, models.length)
            }
            onClick={() => {
              const binding = normalizeBindingDraft(config, {
                engineId: engine.id,
                providerId,
                primaryModel,
                fastModel: fastModel || null,
                reasoningEffort,
              });
              void saveBindingConfig(binding)
                .then((next) => {
                  onConfig(next);
                  onNotice(
                    selectedProvider?.kind === 'subscription'
                      ? `${engine.name} 的订阅绑定已保存`
                      : selectedProvider?.lastTest?.result === 'ok'
                        ? `${engine.name} 的绑定已保存`
                        : `${engine.name} 的绑定已保存；建议先测试可达性`,
                  );
                  onClose();
                })
                .catch((err: unknown) => onNotice(errorMessage(err, '保存绑定失败')));
            }}
          >
            确认绑定
          </Button>
        </DialogFooter>
      </DialogContent>
    </ShadcnDialog>
  );
}

/** S6：服务商页 = 按接入类型分组的卡片网格；点击卡片进入详情抽屉。状态 pill 只消费真实数据。 */
function ProvidersGrid({
  config,
  onOpen,
  onAdd,
}: {
  config: AppConfig;
  onOpen: (id: string) => void;
  onAdd: () => void;
}) {
  const [query, setQuery] = useState('');
  // 订阅卡的「已登录」态来自真实 CLI 登录检测（按协议各测一次）
  const [logins, setLogins] = useState<Partial<Record<'claude-code' | 'codex', CliLoginState>>>({});
  useEffect(() => {
    let active = true;
    for (const engine of ['claude-code', 'codex'] as const) {
      detectCliLogin(engine)
        .then((state) => {
          if (active) setLogins((prev) => ({ ...prev, [engine]: state }));
        })
        .catch(() => {
          if (active) setLogins((prev) => ({ ...prev, [engine]: prev[engine] ?? null }));
        });
    }
    return () => {
      active = false;
    };
  }, []);

  const groups = useMemo(() => {
    const keyword = query.trim().toLowerCase();
    const list: { group: ProviderAccessGroup; providers: ProviderConfig[] }[] = [];
    for (const definition of PROVIDER_ACCESS_GROUPS) {
      const providers = config.providers.filter(
        (provider) =>
          providerAccessGroup(provider) === definition.id &&
          (!keyword ||
            provider.name.toLowerCase().includes(keyword) ||
            provider.protocol.includes(keyword)),
      );
      if (providers.length) list.push({ group: definition.id, providers });
    }
    return list;
  }, [config.providers, query]);

  return (
    <>
      <div className="pv-head">
        <div>
          <h2 className="pv-head__title">服务商</h2>
          <p className="pv-head__sub">
            授权登录、官方 API、兼容套餐与第三方中转，使用同一套连接管理。
          </p>
        </div>
        <button className="btn btn--primary" onClick={onAdd} type="button">
          <Icon name="plus" /> 添加服务商
        </button>
      </div>
      <div className="pv-toolbar">
        <label className="cm-search">
          <Icon name="search" />
          <input
            placeholder="搜索服务商"
            aria-label="搜索服务商"
            value={query}
            onChange={(event) => setQuery(event.target.value)}
          />
        </label>
      </div>
      {groups.map(({ group, providers }) => (
        <section className="pv-group" key={group}>
          <div className="pv-group__label">{providerAccessGroupLabel(group)}</div>
          <div className="pv-grid">
            {providers.map((provider) => {
              const login =
                provider.kind === 'subscription'
                  ? provider.protocol === 'anthropic'
                    ? (logins['claude-code'] ?? null)
                    : (logins['codex'] ?? null)
                  : null;
              const modelCount = enabledModelCount(config, provider.id);
              const status = providerCardStatus(provider, login, modelCount);
              const boundEngines = [
                ...new Set(
                  config.bindings
                    .filter((binding) => binding.providerId === provider.id)
                    .map(
                      (binding) =>
                        config.engines.find((engine) => engine.id === binding.engineId)?.name ??
                        binding.engineId,
                    ),
                ),
              ];
              return (
                <button
                  key={provider.id}
                  type="button"
                  className="pv-card"
                  onClick={() => onOpen(provider.id)}
                >
                  <span className="pv-card__top">
                    <span className="pv-card__brand">
                      <ProviderBrand providerId={providerBrandKey(provider)} />
                    </span>
                    <span className="pv-card__id">
                      <b>{provider.name}</b>
                      <span className="pv-card__type">{providerAccessGroupLabel(group)}</span>
                    </span>
                    <span className={'pill pv-status--' + status.tone}>{status.label}</span>
                  </span>
                  <span className="pv-card__meta">
                    <span>
                      {boundEngines.length ? `绑定 ${boundEngines.join('、')}` : '未绑定'}
                    </span>
                    <span>{modelCount} 个模型</span>
                    <span>
                      {provider.kind === 'subscription' && login?.state !== 'ok'
                        ? loginStateLabel(login)
                        : lastSyncTimeText(provider, modelCount > 0)}
                    </span>
                  </span>
                </button>
              );
            })}
          </div>
        </section>
      ))}
      {groups.length === 0 ? <div className="providers-empty">没有匹配的服务商。</div> : null}
    </>
  );
}

/** S6：服务商详情抽屉。承载既有 CRUD/探活/同步全部能力；关闭只丢弃未保存草稿。 */
function ProviderDrawer({
  config,
  activeProvider,
  onConfig,
  onNotice,
  onClose,
  onUnbindJump,
}: {
  config: AppConfig;
  activeProvider: ProviderConfig;
  onConfig: (config: AppConfig) => void;
  onNotice: (message: string | null) => void;
  onClose: () => void;
  onUnbindJump: (engineId: string) => void;
}) {
  // Esc 关闭抽屉（焦点在输入框内时同样生效）
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') onClose();
    };
    document.addEventListener('keydown', onKeyDown);
    return () => document.removeEventListener('keydown', onKeyDown);
  }, [onClose]);

  const [draft, setDraft] = useState(activeProvider);
  const [apiKey, setApiKey] = useState('');
  const [showKey, setShowKey] = useState(false);
  const [editingKey, setEditingKey] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [modelDrafts, setModelDrafts] = useState(() =>
    modelCatalogForProvider(config, activeProvider.id),
  );
  // 每行对应的「上次落库 ID」（与 modelDrafts 按下标对齐；null = 本行是未落库的新增行）。
  // 保存时据此识别「模型改名」——尤其绑定正在使用的模型——先走后端改名命令级联
  // binding 与会话偏好，再落目录/勾选，避免「删旧建新导致绑定指向不存在的 ID」。
  const [rowOrigins, setRowOrigins] = useState<string[]>(() =>
    modelCatalogForProvider(config, activeProvider.id).map((model) => model.id),
  );
  /** 目录快照重置：切换服务商 / 同步 / 保存成功后，行与落库 ID 一并回填。 */
  const resetRowsFromCatalog = (next: AppConfig, providerId: string) => {
    const catalog = modelCatalogForProvider(next, providerId);
    setModelDrafts(catalog);
    setRowOrigins(catalog.map((model) => model.id));
  };
  const [login, setLogin] = useState<CliLoginState | null>(null);
  const [loginBusy, setLoginBusy] = useState<null | 'refresh' | 'logout' | 'login'>(null);
  const [syncBusy, setSyncBusy] = useState(false);
  const [syncedAt, setSyncedAt] = useState<number | null>(null);
  // 同步候选缓存：只喂组合框/角色下拉，不生成模型行（订阅除外——官方目录是持久来源）
  const [syncOptions, setSyncOptions] = useState<{ id: string; priceText: string }[]>([]);
  // 定价目录候选：模型组合框下拉与模糊匹配价签共用（读取失败按空目录处理，仍可手填）
  const [catalog, setCatalog] = useState<PricingCatalogEntry[]>([]);
  useEffect(() => {
    let active = true;
    getPricingCatalogEntries()
      .then((entries) => {
        if (active) setCatalog(entries);
      })
      .catch(() => {
        if (active) setCatalog([]);
      });
    return () => {
      active = false;
    };
  }, []);
  const [editingPrice, setEditingPrice] = useState<ModelConfig | null>(null);

  useEffect(() => {
    setDraft(activeProvider);
    setApiKey('');
    setShowKey(false);
    setEditingKey(false);
    setModelDrafts(modelCatalogForProvider(config, activeProvider.id));
    setRowOrigins(modelCatalogForProvider(config, activeProvider.id).map((model) => model.id));
    setSyncOptions([]);
    setSyncedAt(null);
    // 仅在切换服务商时重置；依赖对象本身会让测试结果被随后的 config 刷新立即抹掉
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [activeProvider.id]);

  const engineId = draft.protocol === 'anthropic' ? 'claude-code' : 'codex';
  const isSub = draft.kind === 'subscription';
  useEffect(() => {
    if (!isSub) return;
    let active = true;
    detectCliLogin(engineId)
      .then((state) => {
        if (active) setLogin(state);
      })
      .catch(() => {
        if (active)
          setLogin(
            (prev) => prev ?? { state: 'unknown', authMethod: 'unknown', detail: '无法检测登录态' },
          );
      });
    return () => {
      active = false;
    };
  }, [isSub, engineId, draft.id]);

  const keyValue = apiKey || (!editingKey && draft.keyRef ? '••••••••••••••••••••' : '');
  const mode = providerModelMode(draft);
  const relay = isRelayProvider(draft);
  const group = providerAccessGroup(draft);
  // 「同步模型」：拉取远端 /models 只作组合框/角色下拉候选，不铺到模型行；
  // 已添加的模型行原样保留。订阅服务商走落库同步（官方目录是角色行的持久来源）。
  const runSync = () => {
    if (isSub) {
      setSyncBusy(true);
      void syncProviderModels(draft.id)
        .then((next) => {
          onConfig(next);
          resetRowsFromCatalog(next, draft.id);
          setSyncedAt(Date.now());
          onNotice('订阅模型目录已同步');
        })
        .catch((err: unknown) => onNotice(errorMessage(err, '同步模型失败')))
        .finally(() => setSyncBusy(false));
      return;
    }
    setSyncBusy(true);
    void listProviderModels(draft.id, {
      baseUrl: draft.baseUrl,
      apiKey,
    })
      .then((listing) => {
        onConfig(listing.config);
        const saved = listing.config.providers.find((item) => item.id === draft.id);
        if (saved) setDraft((prev) => ({ ...prev, lastSyncAt: saved.lastSyncAt }));
        setSyncOptions(
          listing.modelIds.map((id) => {
            const match = fuzzyPriceMatch(catalog, id);
            return { id, priceText: match ? catalogPriceText(match) : '未计价' };
          }),
        );
        setSyncedAt(Date.now());
        onNotice(`已同步 ${listing.modelIds.length} 个候选模型`);
      })
      .catch((err: unknown) => onNotice(errorMessage(err, '同步模型失败')))
      .finally(() => setSyncBusy(false));
  };
  const markSecretMissing = () => {
    setDraft({ ...draft, keyRef: null });
    setApiKey('');
    setShowKey(true);
    setEditingKey(true);
  };
  // 组合框确认（选中/失焦/回车）：空值保留该行；重复 ID 提示但不删行；其余模糊匹配带出目录价
  const commitDraftModelRow = (index: number, id: string) => {
    const decision = commitProviderModelRow(
      modelDrafts.map((item) => item.id),
      index,
      id,
    );
    if (decision.action === 'keep-empty') return;
    if (decision.action === 'duplicate') {
      onNotice('该模型已在目录中');
      return;
    }
    const match = fuzzyPriceMatch(catalog, decision.id);
    setModelDrafts((prev) =>
      prev.map((item, at) =>
        at === index
          ? { ...buildManualModel(draft.id, decision.id, match), enabled: item.enabled }
          : item,
      ),
    );
    if (match) onNotice(`已匹配定价目录：${match.modelId}`);
  };
  const requireApiKeyWhenEditing = () => {
    if (draft.authMethod !== 'apikey') return false;
    if (!editingKey || apiKey.trim()) return false;
    onNotice('请先粘贴新的 API 密钥，再保存或测试');
    return true;
  };
  const runLoginAction = (action: 'refresh' | 'logout' | 'login') => {
    setLoginBusy(action);
    const work =
      action === 'refresh'
        ? detectCliLogin(engineId)
        : action === 'logout'
          ? logoutCliAccount(engineId)
          : loginCliAccount(engineId);
    void work
      .then((state) => {
        setLogin(state);
        if (action === 'logout') onNotice('已退出 Helm 隔离登录');
        else if (action === 'login') onNotice('登录完成，正在同步订阅模型目录');
        else onNotice('登录状态已刷新');
        return syncProviderModels(draft.id)
          .then((next) => {
            onConfig(next);
            resetRowsFromCatalog(next, draft.id);
            if (action === 'refresh') onNotice('登录状态已刷新，订阅模型目录已同步');
          })
          .catch(() => undefined);
      })
      .catch((err: unknown) => onNotice(errorMessage(err, '登录操作失败')))
      .finally(() => setLoginBusy(null));
  };
  const saveProvider = () => {
    if (requireApiKeyWhenEditing()) return;
    // 改名检测：行的落库来源 ID 与当前 ID 不同 → 该行是「改名」而非「删旧建新」。
    // 在用（被绑定引用）的模型也允许改名，但必须先走后端 rename_provider_model：
    // 它会级联同服务商 binding 的 primary/fast/assistant 与全库会话 preferred_model，
    // 否则 save_models_for_provider 删掉旧条目后绑定会指向不存在的 ID，下轮发送即报错。
    // 同一旧 ID 出现在多行属异常草稿态，只取第一行，其余按普通行处理。
    const renamedRows: { oldId: string; index: number }[] = [];
    const seenOrigins = new Set<string>();
    modelDrafts.forEach((model, index) => {
      const origin = rowOrigins[index] ?? null;
      const currentId = model.id.trim();
      if (!origin || !currentId || origin === currentId || seenOrigins.has(origin)) return;
      seenOrigins.add(origin);
      renamedRows.push({ oldId: origin, index });
    });
    const runModelSaves = (): Promise<AppConfig> => {
      if (mode !== 'list-openai') return Promise.reject(new Error('该模式下无模型目录操作'));
      // 1) 先逐个改名（顺序执行，避免后端目录并发写竞争）
      const runRenames = async (): Promise<void> => {
        for (const renamed of renamedRows) {
          const target = modelDrafts[renamed.index];
          await renameProviderModel(draft.id, renamed.oldId, target.id.trim());
        }
      };
      // 2) 再落完整目录与勾选集（两条命令，与服务商添加流程同口径）
      return runRenames().then(() => {
        const savedDrafts = modelDrafts.filter((model) => model.id.trim() !== '');
        // 注意：renameProviderModel 返回的 config 已含改名结果，但这里统一以
        // save_provider_models_config 传入的草稿为准（含新行/删除行/价格编辑）。
        return saveProviderModelsConfig(draft.id, savedDrafts).then((afterModels) =>
          saveProviderModelSelection(
            draft.id,
            savedDrafts.filter((model) => model.enabled).map((model) => model.id),
          ).then(() => afterModels),
        );
      });
    };
    void saveProviderConfig(draft, apiKey)
      .then((next) => {
        onConfig(next);
        if (mode === 'list-openai') {
          // 模型目录与启用集分两条命令持久化：save_models_for_provider 会保留旧 enabled，
          // 勾选状态必须经 save_provider_model_selection 落库（否则开关永远不生效）。
          return runModelSaves();
        }
        return next;
      })
      .then((next) => {
        onConfig(next);
        resetRowsFromCatalog(next, draft.id);
        onNotice(
          renamedRows.length > 0
            ? '服务商配置已保存；模型已改名，相关绑定与会话偏好已同步'
            : '服务商配置已保存；当前引擎绑定未改变',
        );
        setApiKey('');
        setShowKey(false);
        setEditingKey(false);
      })
      .catch((err: unknown) => {
        onNotice(errorMessage(err, '保存服务商失败'));
      });
  };
  const providerModelCount = config.models.filter((model) => model.providerId === draft.id).length;
  const providerBindingCount = config.bindings.filter(
    (binding) => binding.providerId === draft.id,
  ).length;
  const deleteBlockedReason = providerDeleteBlockedReason(providerBindingCount);
  const deleteCopy = providerDeleteConfirmation(draft, providerModelCount, providerBindingCount);
  const statusText = isSub ? loginStateLabel(login) : readinessText(draft);
  const roleRows = PROVIDER_ROLE_ROWS.anthropic;
  // 角色下拉候选：同步缓存优先，已保存模型兜底
  const poolIds = [
    ...new Set([
      ...syncOptions.map((option) => option.id),
      ...modelDrafts.map((model) => model.id),
    ]),
  ];
  const modelById = new Map(modelDrafts.map((model) => [model.id, model]));
  // 候选价签：同步缓存里的 ID 直接查价（候选行未落库，不能走 modelById）
  const syncPriceText = (id: string) => {
    const hit = syncOptions.find((option) => option.id === id);
    if (hit) return hit.priceText;
    const match = fuzzyPriceMatch(catalog, id);
    return match ? catalogPriceText(match) : '未计价';
  };

  return (
    <div className="pv-drawer-layer">
      <button
        type="button"
        className="pv-drawer__backdrop"
        aria-label="关闭详情"
        onClick={onClose}
      />
      <aside
        className="cm-drawer"
        role="dialog"
        aria-modal="true"
        aria-labelledby="pv-drawer-title"
      >
        <div className="cm-drawer__head">
          <div className="grow">
            <h2 id="pv-drawer-title">{draft.name}</h2>
            <p>
              {providerAccessGroupLabel(group)} · {statusText}
            </p>
          </div>
          <button className="btn-icon" type="button" aria-label="关闭" onClick={onClose}>
            <Icon name="x" />
          </button>
        </div>
        <div className="cm-drawer__body">
          <section className="cm-detail-card">
            <div className="cm-detail-card__head">
              <div>
                <h3>连接信息</h3>
                <small>
                  {isSub
                    ? `订阅通过 ${engineId === 'claude-code' ? 'Claude Code' : 'Codex'} 隔离登录接入，Helm 不持有凭证。`
                    : '同步在「模型配置」卡片发起；密钥只进系统钥匙串。'}
                </small>
              </div>
              <span
                className={
                  'cm-status-pill ' +
                  (statusText === '已登录' || statusText === '配置就绪' ? 'is-ready' : 'is-warn')
                }
              >
                {statusText}
              </span>
            </div>
            <div className="cm-form">
              <div className="cm-field">
                <label>服务商名称</label>
                <input
                  className="cm-input"
                  value={draft.name}
                  onChange={(event) => setDraft({ ...draft, name: event.target.value })}
                />
              </div>
              {isSub ? (
                <>
                  <div className="cm-field">
                    <label>登录账号</label>
                    <input
                      className="cm-input"
                      value={login?.accountLabel || login?.plan || (login ? '订阅账号' : '')}
                      readOnly
                    />
                    <small>由 CLI 隔离登录态提供，不可编辑。</small>
                  </div>
                  <div className="cm-field">
                    <label>登录状态</label>
                    <input
                      className="cm-input"
                      value={
                        login
                          ? `${loginStateLabel(login)}${login.plan ? ` · ${login.plan}` : ''}`
                          : '检测中…'
                      }
                      readOnly
                    />
                  </div>
                  <div className="cm-note">
                    <Icon name="info" />
                    <span>
                      订阅不使用 Base URL 与 API Key；退出登录只影响 Helm 隔离登录态，不改全局 CLI
                      登录。
                    </span>
                  </div>
                  <div className="cm-form-actions">
                    <button
                      className="cm-action"
                      type="button"
                      disabled={loginBusy !== null}
                      onClick={() => runLoginAction('refresh')}
                    >
                      <Icon name="refresh" /> 刷新登录状态
                    </button>
                    {login?.state === 'ok' ? (
                      <button
                        className="cm-action"
                        type="button"
                        disabled={loginBusy !== null}
                        onClick={() => runLoginAction('logout')}
                      >
                        {loginBusy === 'logout' ? '正在退出…' : '退出登录'}
                      </button>
                    ) : (
                      <button
                        className="cm-action cm-action--primary"
                        type="button"
                        disabled={loginBusy !== null}
                        onClick={() => runLoginAction('login')}
                      >
                        {loginBusy === 'login' ? '等待浏览器授权…' : '前往登录'}
                      </button>
                    )}
                  </div>
                </>
              ) : (
                <>
                  <div className="cm-field">
                    <label>Base URL</label>
                    <input
                      data-provider-field="base-url"
                      className="cm-input mono"
                      value={draft.baseUrl}
                      readOnly={group === 'plan'}
                      onChange={(event) => setDraft({ ...draft, baseUrl: event.target.value })}
                    />
                    {group === 'plan' ? (
                      <small>套餐预设端点，只读；自定义地址走第三方中转。</small>
                    ) : null}
                  </div>
                  <AuthFields
                    draft={draft}
                    keyValue={keyValue}
                    apiKey={apiKey}
                    showKey={showKey}
                    editingKey={editingKey}
                    onDraft={setDraft}
                    onApiKey={setApiKey}
                    onShowKey={setShowKey}
                    onEditingKey={setEditingKey}
                    onSecretMissing={markSecretMissing}
                    onConfig={onConfig}
                    onNotice={onNotice}
                  />
                </>
              )}
            </div>
          </section>
          <section className="cm-detail-card">
            <div className="cm-detail-card__head">
              <div>
                <h3>模型配置</h3>
                <small>
                  {isSub
                    ? `来自 ${engineId === 'claude-code' ? 'Claude Code' : 'Codex'} 订阅官方目录，按订阅折算计费`
                    : `${lastSyncTimeText(draft, modelDrafts.length > 0)} · ${modelCalibrationLabel(draft)}`}
                </small>
              </div>
              <span className="pv-headright">
                <span
                  className={'cm-status-pill ' + (modelDrafts.length > 0 ? 'is-ready' : 'is-warn')}
                >
                  {modelDrafts.length > 0 ? `${modelDrafts.length} 个模型` : '未同步'}
                </span>
                {!isSub ? (
                  <button className="cm-action" type="button" disabled={syncBusy} onClick={runSync}>
                    <Icon name="refresh" className={syncBusy ? 'spin' : undefined} /> 同步模型
                  </button>
                ) : null}
              </span>
            </div>
            {mode === 'list-openai' ? (
              <>
                {modelDrafts.length === 0 ? (
                  <div className="pv-empty">
                    {syncedAt
                      ? '还没有添加模型 · 点下方「添加模型」从候选中选择'
                      : '还没有添加模型 · 建议先「同步模型」获取候选，也可直接手动填写'}
                  </div>
                ) : (
                  <div>
                    {modelDrafts.map((model, index) => (
                      <div key={index} className="pv-mline pv-mline--list" data-row-id={model.id}>
                        <label className="cm-switch">
                          <input
                            type="checkbox"
                            checked={model.enabled}
                            onChange={() =>
                              setModelDrafts(
                                modelDrafts.map((item, at) =>
                                  at === index ? { ...item, enabled: !item.enabled } : item,
                                ),
                              )
                            }
                          />
                          <i />
                        </label>
                        <ModelIdCombo
                          value={model.id}
                          options={syncOptions}
                          taken={modelDrafts.map((item) => item.id.trim()).filter(Boolean)}
                          autoOpen={!model.id.trim() && syncOptions.length > 0}
                          onCommit={(id) => commitDraftModelRow(index, id)}
                        />
                        <span className="pv-mline__end">
                          <span
                            className={
                              'pv-pricechip' + (priceChipFor(model).priced ? '' : ' is-none')
                            }
                            title={
                              priceChipFor(model).priced
                                ? '输入 / 缓存读取 / 输出，单位 $/MTok'
                                : undefined
                            }
                          >
                            {priceChipFor(model).text}
                          </span>
                          {relay ? (
                            <button
                              className="cm-action pv-priceedit"
                              type="button"
                              title="修改此模型的定价（输入 / 缓存 / 输出）"
                              onClick={() => setEditingPrice(model)}
                            >
                              改价
                            </button>
                          ) : null}
                          <button
                            className="btn-icon"
                            type="button"
                            aria-label="移除模型"
                            title="移除模型"
                            onClick={() => {
                              setModelDrafts(modelDrafts.filter((_, at) => at !== index));
                              setRowOrigins(rowOrigins.filter((_, at) => at !== index));
                            }}
                          >
                            <Icon name="trash" />
                          </button>
                        </span>
                      </div>
                    ))}
                  </div>
                )}
                <div className="pv-synclist__add pv-synclist__add--solo">
                  <button
                    className="cm-action"
                    type="button"
                    onClick={() => {
                      setModelDrafts([...modelDrafts, buildManualModel(draft.id, '', null)]);
                      setRowOrigins([...rowOrigins, '']);
                    }}
                  >
                    <Icon name="plus" /> 添加模型
                  </button>
                </div>
              </>
            ) : (
              <>
                <div className="pv-mlist">
                  {roleRows.map((role) => {
                    const currentId = providerRoleModelId(draft, role.key);
                    const chip = modelById.get(currentId);
                    // 价签：已落库模型走 priceChipFor；同步候选（未落库）按目录价显示
                    const chipText = chip ? priceChipFor(chip).text : syncPriceText(currentId);
                    const chipPriced = chip ? priceChipFor(chip).priced : chipText !== '未计价';
                    return (
                      <div key={role.key} className="pv-mline pv-mline--role">
                        <b>{role.label}</b>
                        <ModelIdCombo
                          value={currentId}
                          options={poolIds.map((id) => ({
                            id,
                            priceText: modelById.get(id)
                              ? priceChipFor(modelById.get(id)!).text
                              : syncPriceText(id),
                          }))}
                          ghost={false}
                          placeholder="选择或填写模型 ID"
                          onCommit={(id) => setDraft(withRoleModel(draft, role.key, id.trim()))}
                        />
                        <span className="pv-mline__end">
                          <span
                            className={'pv-pricechip' + (chipPriced ? '' : ' is-none')}
                            title={chipPriced ? '输入 / 缓存读取 / 输出，单位 $/MTok' : undefined}
                          >
                            {chipText}
                          </span>
                          {relay ? (
                            <button
                              className="cm-action pv-priceedit"
                              type="button"
                              title="修改此模型的定价（输入 / 缓存 / 输出）"
                              onClick={() =>
                                setEditingPrice(
                                  chip ?? {
                                    id: currentId,
                                    providerId: draft.id,
                                    displayName: currentId,
                                    inputPricePerMtok: 0,
                                    outputPricePerMtok: 0,
                                    priceSource: 'manual',
                                    enabled: true,
                                  },
                                )
                              }
                            >
                              改价
                            </button>
                          ) : null}
                        </span>
                      </div>
                    );
                  })}
                </div>
              </>
            )}
          </section>
          <div className="pv-drawer__foot cm-panel__foot">
            <span className="pv-foot__delete">
              <button
                className={
                  'cm-action' +
                  (providerCanDelete(providerBindingCount) ? ' cm-action--danger' : ' is-blocked')
                }
                type="button"
                onClick={() => {
                  if (!providerCanDelete(providerBindingCount)) {
                    onNotice(deleteBlockedReason);
                    return;
                  }
                  setConfirmDelete(true);
                }}
                title={deleteBlockedReason ?? undefined}
              >
                <Icon name="trash" /> 删除服务商
              </button>
              {deleteBlockedReason ? <small>{deleteBlockedReason}</small> : null}
            </span>
            <span className="pv-detail__foot-main">
              <button
                className="cm-action"
                type="button"
                onClick={() =>
                  onUnbindJump(draft.protocol === 'anthropic' ? 'claude-code' : 'codex')
                }
              >
                去绑定
              </button>
              <button className="cm-action cm-action--primary" type="button" onClick={saveProvider}>
                保存修改
              </button>
            </span>
          </div>
          {confirmDelete ? (
            <ConfirmDialog
              title={deleteCopy.title}
              body={deleteCopy.body}
              confirmLabel={deleteCopy.confirmLabel}
              danger
              onCancel={() => setConfirmDelete(false)}
              onConfirm={() => {
                void deleteProviderConfig(draft.id)
                  .then((next) => {
                    onConfig(next);
                    onNotice('服务商已删除，关联模型也已移除');
                    setConfirmDelete(false);
                    onClose(); // 该服务商已不存在，直接收起抽屉
                  })
                  .catch((err: unknown) => {
                    onNotice(errorMessage(err, '删除服务商失败'));
                  });
              }}
            />
          ) : null}
          {editingPrice ? (
            <ModelPriceModal
              model={editingPrice}
              onClose={() => setEditingPrice(null)}
              onSaved={async (message) => {
                onConfig(await getProviderConfig());
                onNotice(message);
                setEditingPrice(null);
              }}
            />
          ) : null}
        </div>
      </aside>
    </div>
  );
}
function ModelPriceModal({
  model,
  onClose,
  onSaved,
}: {
  model: ModelConfig;
  onClose: () => void;
  onSaved: (message: string) => Promise<void>;
}) {
  const [input, setInput] = useState(model.inputPricePerMtok || 0);
  const [cachedInput, setCachedInput] = useState(0);
  const [cacheWrite, setCacheWrite] = useState(0);
  const [output, setOutput] = useState(model.outputPricePerMtok || 0);
  const [saving, setSaving] = useState(false);
  const [loadingOverride, setLoadingOverride] = useState(true);
  const [saveError, setSaveError] = useState<string | null>(null);

  useEffect(() => {
    let active = true;
    listModelPriceOverrides()
      .then((overrides) => {
        if (!active) return;
        const existing = overrides.find(
          (item) => item.providerId === model.providerId && item.modelId === model.id,
        );
        const band = existing?.tiers.standard?.bands[0];
        if (band) {
          setInput(band.input);
          setCachedInput(band.cachedInput ?? 0);
          setCacheWrite(band.cacheWrite ?? 0);
          setOutput(band.output);
        }
      })
      .catch((err: unknown) => {
        if (active) setSaveError(errorMessage(err, '读取手动价格失败'));
      })
      .finally(() => {
        if (active) setLoadingOverride(false);
      });
    return () => {
      active = false;
    };
  }, [model.id, model.providerId]);

  const save = () => {
    if (
      [input, cachedInput, cacheWrite, output].some((value) => value < 0 || !Number.isFinite(value))
    ) {
      setSaveError('价格必须是大于或等于 0 的有效数字');
      return;
    }
    setSaveError(null);
    setSaving(true);
    void saveModelPriceOverride({
      providerId: model.providerId,
      modelId: model.id,
      currency: 'USD',
      updatedAt: 0,
      tiers: {
        standard: {
          bands: [
            {
              input,
              ...(cachedInput > 0 ? { cachedInput } : {}),
              ...(cacheWrite > 0 ? { cacheWrite } : {}),
              output,
            },
          ],
        },
      },
    })
      .then(() => onSaved('手动价格已保存'))
      .catch((err: unknown) => setSaveError(errorMessage(err, '保存手动价格失败')))
      .finally(() => setSaving(false));
  };

  return (
    <ShadcnDialog
      open
      onOpenChange={(open) => {
        if (!open) onClose();
      }}
    >
      <DialogContent className="max-w-xl">
        <DialogHeader>
          <DialogTitle>修改定价</DialogTitle>
          <DialogDescription>{model.id}</DialogDescription>
        </DialogHeader>
        <div className="pv-modal__bd provider-price-grid">
          {[
            ['输入 $/M', input, setInput],
            ['缓存读取 $/M', cachedInput, setCachedInput],
          ].map(([label, value, setter]) => (
            <label className="field" key={label as string}>
              <span>{label as string}</span>
              <input
                className="input mono"
                type="number"
                min="0"
                step="0.001"
                value={value as number}
                onChange={(event) =>
                  (setter as React.Dispatch<React.SetStateAction<number>>)(
                    Number(event.target.value || '0'),
                  )
                }
              />
            </label>
          ))}
          <label className="field">
            <span>输出 $/M</span>
            <input
              className="input mono"
              type="number"
              min="0"
              step="0.001"
              value={output}
              onChange={(event) => setOutput(Number(event.target.value || '0'))}
            />
            <small>留空表示未计价；当前以 0 处理。</small>
          </label>
          {saveError ? (
            <div className="provider-price-error" role="alert">
              {saveError}
            </div>
          ) : null}
        </div>
        <DialogFooter>
          <Button variant="subtle" onClick={onClose} type="button">
            取消
          </Button>
          <Button
            variant="primary"
            disabled={saving || loadingOverride}
            onClick={save}
            type="button"
          >
            {loadingOverride ? '读取中…' : '保存定价'}
          </Button>
        </DialogFooter>
      </DialogContent>
    </ShadcnDialog>
  );
}

/** 订阅登录一等公民（P3-1）：展示协议对应 CLI 的登录态，并给出登录指引 */
function SubscriptionLoginCard({
  protocol,
  providerId,
  onConfig,
  onNotice,
}: {
  protocol: ProviderProtocol;
  providerId: string;
  onConfig: (config: AppConfig) => void;
  onNotice: (message: string | null) => void;
}) {
  const engine = protocol === 'anthropic' ? 'claude-code' : 'codex';
  const [login, setLogin] = useState<CliLoginState | null>(null);
  const [detecting, setDetecting] = useState(false);
  const [accountAction, setAccountAction] = useState<'login' | 'logout' | null>(null);

  const syncAccountModels = useCallback(
    async (state: CliLoginState) => {
      if (state.state !== 'ok' || state.authMethod !== 'subscription') return;
      try {
        const next = await syncProviderModels(providerId);
        onConfig(next);
        const count = next.models.filter((model) => model.providerId === providerId).length;
        onNotice(`订阅账号已验证，可用模型已刷新：${count} 个`);
      } catch (err: unknown) {
        onNotice(errorMessage(err, '读取订阅账号模型失败'));
      }
    },
    [onConfig, onNotice, providerId],
  );

  const detect = useCallback(async () => {
    setDetecting(true);
    try {
      const state = await detectCliLogin(engine);
      setLogin(state);
      await syncAccountModels(state);
    } catch {
      // 浏览器预览无后端；桌面端极少失败，保持「未检测」态可重试
      setLogin(null);
    } finally {
      setDetecting(false);
    }
  }, [engine, syncAccountModels]);

  useEffect(() => {
    void detect();
  }, [detect]);

  const stateLabel = loginStateLabel(login);
  const stateClass =
    login?.state === 'ok' && login.authMethod === 'subscription'
      ? 'pv-login-pill pv-login-pill--ok'
      : login?.state === 'missing' || login?.state === 'expired'
        ? 'pv-login-pill pv-login-pill--missing'
        : 'pv-login-pill';
  const runAccountAction = async (action: 'login' | 'logout') => {
    setAccountAction(action);
    try {
      const state =
        action === 'login' ? await loginCliAccount(engine) : await logoutCliAccount(engine);
      setLogin(state);
      if (action === 'login') await syncAccountModels(state);
    } catch (err: unknown) {
      setLogin({
        state: 'unknown',
        authMethod: 'unknown',
        detail: errorMessage(err, action === 'login' ? '账号登录失败' : '退出登录失败'),
      });
    } finally {
      setAccountAction(null);
    }
  };

  return (
    <div className="cm-auth-card">
      <span className="cm-auth-card__icon">
        <Icon name="shield" />
      </span>
      <div className="cm-auth-card__main">
        <b>
          订阅登录 <span className={stateClass}>{login ? stateLabel : '检测中…'}</span>
        </b>
        <small>
          会话使用 Helm 独立 CLI Profile，不修改其他终端工具的登录态。
          {login ? ` ${login.detail}` : ''}
          {login?.accountLabel ? ` · ${login.accountLabel}` : ''}
          {login?.plan ? ` · ${login.plan}` : ''}
        </small>
      </div>
      <div className="pv-account-actions">
        {login?.state === 'ok' ? (
          <>
            <button
              className="btn btn--subtle btn--sm"
              type="button"
              disabled={accountAction !== null}
              onClick={() => void runAccountAction('login')}
            >
              {accountAction === 'login' ? '等待授权…' : '重新登录'}
            </button>
            <button
              className="btn btn--subtle btn--sm"
              type="button"
              disabled={accountAction !== null}
              onClick={() => void runAccountAction('logout')}
            >
              {accountAction === 'logout' ? '正在退出…' : '退出'}
            </button>
          </>
        ) : (
          <button
            className="btn btn--primary btn--sm"
            type="button"
            disabled={accountAction !== null}
            onClick={() => void runAccountAction('login')}
          >
            {accountAction === 'login' ? '等待浏览器授权…' : '登录账号'}
          </button>
        )}
        <button
          className="btn btn--subtle btn--sm"
          type="button"
          disabled={detecting || accountAction !== null}
          onClick={() => void detect()}
        >
          <Icon name="refresh" /> {detecting ? '检测中…' : '重新检测'}
        </button>
      </div>
    </div>
  );
}

function AuthFields({
  draft,
  keyValue,
  apiKey,
  showKey,
  editingKey,
  onDraft,
  onApiKey,
  onShowKey,
  onEditingKey,
  onSecretMissing,
  onConfig,
  onNotice,
}: {
  draft: ProviderConfig;
  keyValue: string;
  apiKey: string;
  showKey: boolean;
  editingKey: boolean;
  onDraft: (provider: ProviderConfig) => void;
  onApiKey: (value: string) => void;
  onShowKey: (value: boolean) => void;
  onEditingKey: (value: boolean) => void;
  onSecretMissing: () => void;
  onConfig: (config: AppConfig) => void;
  onNotice: (message: string | null) => void;
}) {
  if (draft.authMethod === 'oauth') {
    return (
      <SubscriptionLoginCard
        protocol={draft.protocol}
        providerId={draft.id}
        onConfig={onConfig}
        onNotice={onNotice}
      />
    );
  }
  if (draft.authMethod === 'cloud') {
    return (
      <div className="cm-auth-card">
        <span className="cm-auth-card__icon">
          <Icon name="server" />
        </span>
        <div className="grow">
          <b>云凭证</b>
          <small>使用本机云 SDK 凭证链，不在 Helm 保存密钥。</small>
        </div>
      </div>
    );
  }
  if (draft.authMethod === 'local') {
    return (
      <div className="cm-auth-card">
        <span className="cm-auth-card__icon">
          <Icon name="cpu" />
        </span>
        <div className="grow">
          <b>本地服务</b>
          <small>无需认证，通常用于 Ollama 或本机兼容网关。</small>
        </div>
      </div>
    );
  }
  return (
    <div className="cm-field">
      <label>API 密钥</label>
      <div className="keyfield">
        <input
          data-provider-field="api-key"
          className="cm-input mono"
          type={showKey ? 'text' : 'password'}
          value={keyValue}
          placeholder={draft.keyRef ? '已保存到钥匙串' : '粘贴 API 密钥'}
          readOnly={Boolean(draft.keyRef && !showKey && !apiKey && !editingKey)}
          onChange={(event) => {
            onApiKey(event.target.value);
            onShowKey(true);
            onEditingKey(true);
          }}
        />
        <button
          className="btn-icon"
          title="显示/隐藏密钥"
          onClick={() => {
            if (showKey) {
              onShowKey(false);
              onApiKey('');
              return;
            }
            if (!draft.keyRef) {
              onShowKey(true);
              return;
            }
            void revealProviderSecret(draft.id)
              .then((secret) => {
                onApiKey(secret);
                onShowKey(true);
              })
              .catch((err: unknown) => {
                // 系统确认框点了取消：不是故障，安静返回
                if (String(err).includes('已取消')) return;
                onSecretMissing();
                onNotice(`${errorMessage(err, '读取密钥失败')}，请重新粘贴 API 密钥并保存`);
              });
          }}
        >
          <Icon name={showKey ? 'eyeoff' : 'eye'} />
        </button>
        {draft.keyRef ? (
          <button
            className="cm-action"
            type="button"
            onClick={() => {
              onDraft({ ...draft, keyRef: null });
              onApiKey('');
              onShowKey(true);
              onEditingKey(true);
            }}
          >
            更换
          </button>
        ) : null}
      </div>
      <small>保存到系统钥匙串，不写入配置文件与日志。</small>
    </div>
  );
}

function ModelsPanel({
  config,
  onConfig,
  onNotice,
}: {
  config: AppConfig;
  onConfig: (config: AppConfig) => void;
  onNotice: (message: string | null) => void;
}) {
  const [query, setQuery] = useState('');
  const [pricingOpen, setPricingOpen] = useState(false);
  // S6：按规范模型 ID 聚合跨服务商接入路径（与 Rust pricing::normalize_model_id 同口径）
  // 搜索覆盖：规范 ID / 展示名 / 原始模型 ID / 服务商名（原始 ID 未规范前缀时也能搜到）
  const groups = useMemo(() => {
    const keyword = query.trim().toLowerCase();
    if (!keyword) return modelAccessGroups(config);
    return modelAccessGroups(config).filter(
      (group) =>
        group.key.includes(keyword) ||
        group.displayName.toLowerCase().includes(keyword) ||
        group.paths.some(
          (path) =>
            path.model.id.toLowerCase().includes(keyword) ||
            path.provider.name.toLowerCase().includes(keyword),
        ),
    );
  }, [config, query]);

  return (
    <>
      <div className="pv-head">
        <div>
          <h2 className="pv-head__title">模型管理</h2>
          <p className="pv-head__sub">
            跨服务商查看模型接入；每条接入路径可在此启用或停用，价格按定价目录口径分输入 / 缓存读取
            / 输出计费。
          </p>
        </div>
        <button className="btn" onClick={() => setPricingOpen(true)} type="button">
          <Icon name="book" /> 定价目录
        </button>
      </div>
      <div className="pv-toolbar">
        <label className="cm-search">
          <Icon name="search" />
          <input
            placeholder="搜索模型 ID 或名称"
            aria-label="搜索模型"
            value={query}
            onChange={(event) => setQuery(event.target.value)}
          />
        </label>
        <span className="pv-toolbar__hint">
          官方 API 与中转按 Token 计费；订阅与套餐按「订阅内 / 套餐内」折算，不单独计费。
        </span>
      </div>
      {groups.length ? (
        <div className="pv-models">
          {groups.map((group, groupIndex) => {
            const summary = modelGroupPriceSummary(group.paths);
            return (
              <details className="pv-model" key={group.key} open={groupIndex === 0 || undefined}>
                <summary className="pv-model__head">
                  <span className="pv-model__name">
                    <b>{group.displayName}</b>
                    <small>{group.key}</small>
                  </span>
                  {summary.segments.length ? (
                    <span className={'pv-model__price' + (summary.plan ? ' pv-plan' : '')}>
                      {summary.segments.map((segment) => (
                        <span key={segment.label}>
                          {segment.label} {segment.value}
                        </span>
                      ))}
                      <em>/ M</em>
                    </span>
                  ) : (
                    <span className={'pv-model__price' + (summary.plan ? ' pv-plan' : '')}>
                      {summary.text}
                    </span>
                  )}
                  <span className="pv-model__count">{group.paths.length} 条接入路径</span>
                  <span className="pv-model__chev">
                    <Icon name="right" />
                  </span>
                </summary>
                <div className="pv-model__paths">
                  {group.paths.map((path) => (
                    <ModelAccessRow
                      key={`${path.model.providerId}:${path.model.id}`}
                      path={path}
                      onConfig={onConfig}
                      onNotice={onNotice}
                    />
                  ))}
                </div>
              </details>
            );
          })}
        </div>
      ) : (
        <div className="providers-empty">暂无模型目录；先在服务商详情完成配置、探活与同步。</div>
      )}
      {pricingOpen ? (
        <PricingCatalogModal
          onClose={() => setPricingOpen(false)}
          onConfig={onConfig}
          onNotice={onNotice}
        />
      ) : null}
    </>
  );
}

/** S6：模型接入路径行——真实绑定（Binding）、就绪（ready/登录态）、探活（lastTest）状态 */
function ModelAccessRow({
  path,
  onConfig,
  onNotice,
}: {
  path: ReturnType<typeof modelAccessGroups>[number]['paths'][number];
  onConfig: (config: AppConfig) => void;
  onNotice: (message: string | null) => void;
}) {
  const { model, provider } = path;
  const tokenPriced =
    provider.kind !== 'subscription' &&
    (model.inputPricePerMtok > 0 || model.outputPricePerMtok > 0);
  return (
    <div className="pv-path">
      <span className="pv-path__main">
        <b>{provider.name}</b>
        <small>{model.id}</small>
      </span>
      <span className="pv-path__billing">{path.billing}</span>
      <span className={'pv-path__price' + (tokenPriced ? '' : ' pv-plan')}>
        {provider.kind === 'subscription' ? (
          '订阅内'
        ) : tokenPriced ? (
          <>
            <span>输入 {priceText(model.inputPricePerMtok)}</span>
            {model.cachedInputPricePerMtok && model.cachedInputPricePerMtok > 0 ? (
              <span>缓存 ${model.cachedInputPricePerMtok.toFixed(2)}</span>
            ) : null}
            <span>输出 {priceText(model.outputPricePerMtok)}</span>
            <em>/ M</em>
          </>
        ) : (
          `输入 ${priceText(model.inputPricePerMtok)} · 输出 ${priceText(model.outputPricePerMtok)}`
        )}
      </span>
      <label className="cm-switch pv-path__switch" title="启用 / 停用该接入路径">
        <input
          type="checkbox"
          checked={model.enabled}
          onChange={() => {
            void saveModelConfig({ ...model, enabled: !model.enabled })
              .then(onConfig)
              .catch((err: unknown) => onNotice(errorMessage(err, '保存模型启用状态失败')));
          }}
        />
        <i />
      </label>
    </div>
  );
}

const VENDOR_LABELS: Record<string, string> = {
  openai: 'OpenAI 官方',
  anthropic: 'Anthropic 官方',
  deepseek: 'DeepSeek 官方',
  zhipu: '智谱官方',
  moonshot: 'Moonshot 官方',
  minimax: 'MiniMax 官方',
  bytedance: '火山方舟官方',
  volc: '火山方舟官方',
  xiaomi: '小米官方',
  mimo: '小米官方',
  alibabacloud: '阿里云官方',
  bailian: '阿里云官方',
  qwen: '阿里云官方',
};

/** S6：全局定价目录——打开/刷新真实目录，显示签名与版本状态；标准价格表只读。 */
function PricingCatalogModal({
  onClose,
  onConfig,
  onNotice,
}: {
  onClose: () => void;
  onConfig: (config: AppConfig) => void;
  onNotice: (message: string | null) => void;
}) {
  const [status, setStatus] = useState<PricingCatalogStatus | null>(null);
  const [entries, setEntries] = useState<PricingCatalogEntry[] | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [query, setQuery] = useState('');
  const [vendor, setVendor] = useState('all');
  const [autoUpdate, setAutoUpdate] = useState(true);
  const [mirrorDraft, setMirrorDraft] = useState('');

  useEffect(() => {
    let active = true;
    loadSettings()
      .then((settings) => {
        if (!active) return;
        setAutoUpdate(settings.general.pricingAutoUpdate);
        setMirrorDraft((settings.general.pricingFeedUrls ?? [])[0] ?? '');
      })
      .catch(() => undefined);
    return () => {
      active = false;
    };
  }, []);

  const persistPricingSettings = async (patch: {
    pricingAutoUpdate?: boolean;
    pricingFeedUrls?: string[];
  }) => {
    try {
      const settings = await loadSettings();
      await saveSettings({ ...settings, general: { ...settings.general, ...patch } });
    } catch (err: unknown) {
      onNotice(errorMessage(err, '保存定价设置失败'));
    }
  };

  useEffect(() => {
    let active = true;
    getPricingCatalogStatus()
      .then((value) => {
        if (active) setStatus(value);
      })
      .catch(() => {
        /* 状态读取失败不阻断价格表展示 */
      });
    getPricingCatalogEntries()
      .then((rows) => {
        if (active) setEntries(rows);
      })
      .catch((err: unknown) => {
        if (active) setLoadError(errorMessage(err, '读取定价目录失败'));
      });
    return () => {
      active = false;
    };
  }, []);

  const refresh = () => {
    setBusy(true);
    refreshPricingCatalog()
      .catch((err: unknown) => {
        setBusy(false);
        const raw = errorMessage(err, '检查更新失败');
        const friendly = /fetch|network|dns|timeout|超时|连接/i.test(raw)
          ? '检查更新失败：当前无法访问目录源。可在高级设置配置备用镜像或导入离线目录；价格表继续使用当前数据。'
          : raw;
        onNotice(friendly);
        throw err;
      })
      .then(async (next) => {
        setStatus(next);
        onNotice(`价格目录已更新至 ${next.catalogVersion}`);
        // 目录更新后重拉配置，让模型行价格立即反映新目录
        onConfig(await getProviderConfig());
      })
      .catch((err: unknown) => onNotice(errorMessage(err, '更新价格目录失败')))
      .finally(() => setBusy(false));
  };

  const importOffline = async () => {
    try {
      const selected = await openFileDialog({
        title: '选择签名价格目录',
        multiple: false,
        filters: [{ name: 'Helm 价格目录', extensions: ['json'] }],
      });
      if (typeof selected !== 'string') return;
      setBusy(true);
      const next = await importPricingCatalog(selected);
      setStatus(next);
      setEntries(await getPricingCatalogEntries());
      onConfig(await getProviderConfig());
      onNotice(`离线价格目录已导入：${next.catalogVersion}`);
    } catch (err: unknown) {
      onNotice(errorMessage(err, '导入价格目录失败'));
    } finally {
      setBusy(false);
    }
  };

  const vendors = useMemo(
    () => [...new Set((entries ?? []).map((entry) => entry.vendor))].sort(),
    [entries],
  );
  const rows = (entries ?? []).filter(
    (entry) =>
      (vendor === 'all' || entry.vendor === vendor) &&
      (!query.trim() || entry.modelId.toLowerCase().includes(query.trim().toLowerCase())),
  );
  const verified = status ? status.source === 'builtin' || status.source === 'cache' : false;

  return (
    <ShadcnDialog
      open
      onOpenChange={(open) => {
        if (!open) onClose();
      }}
    >
      <DialogContent className="max-w-[720px]">
        <DialogHeader>
          <DialogTitle>定价目录</DialogTitle>
          <DialogDescription>
            Helm 标准参考费率；服务商实际覆盖价仍在「服务商详情 › 模型行 › 定价」维护。
          </DialogDescription>
        </DialogHeader>
        <div className="pv-modal__bd pv-pricing">
          <section className="pv-sec">
            <div className="pv-sec__head">
              <div>
                <h3>数据源与更新</h3>
                <small>Helm 官方目录</small>
              </div>
              <button className="cm-action" disabled={busy} onClick={refresh} type="button">
                <Icon name="refresh" className={busy ? 'spin' : undefined} /> 检查更新
              </button>
            </div>
            <div className="cm-setting-grid">
              <div className="cm-option-row">
                <div className="cm-option-row__main">
                  <b>目录版本</b>
                  <small className="mono">
                    {status
                      ? `${status.catalogVersion} · 发布于 ${status.publishedAt.slice(0, 10)}`
                      : '读取中…'}
                  </small>
                </div>
                <span className={'cm-status-pill' + (verified ? ' is-ready' : '')}>
                  {verified ? '已验签' : '未验签'}
                </span>
                {status?.stale ? <span className="cm-status-pill is-warn">已超期</span> : null}
              </div>
              <div className="cm-option-row">
                <div className="cm-option-row__main">
                  <b>最近检查</b>
                  <small>
                    {status?.lastCheckedAt
                      ? new Date(status.lastCheckedAt * 1000).toLocaleString('zh-CN')
                      : '尚未检查'}
                    {status?.lastError ? ` · 上次错误：${status.lastError}` : ''}
                  </small>
                </div>
                {!status?.stale && status?.lastCheckedAt ? (
                  <span className="cm-status-pill is-ready">最新</span>
                ) : null}
              </div>
              <div className="cm-option-row">
                <div className="cm-option-row__main">
                  <b>自动更新标准定价</b>
                  <small>更新失败时继续使用上一个已验签版本，不阻断任务。</small>
                </div>
                <label className="cm-switch" title="自动更新标准定价">
                  <input
                    type="checkbox"
                    checked={autoUpdate}
                    onChange={(event) => {
                      const next = event.target.checked;
                      setAutoUpdate(next);
                      void persistPricingSettings({ pricingAutoUpdate: next });
                    }}
                  />
                  <i />
                </label>
              </div>
            </div>
          </section>
          <section className="pv-sec">
            <div className="pv-sec__head">
              <div>
                <h3>标准价格表</h3>
                <p>
                  {entries
                    ? `${entries.length} 条只读参考费率 · 按模型 ID 搜索、按厂商筛选`
                    : (loadError ?? '读取中…')}
                </p>
              </div>
            </div>
            <div className="pv-pricing__toolbar">
              <label className="cm-search">
                <Icon name="search" />
                <input
                  placeholder="搜索模型 ID"
                  aria-label="搜索定价"
                  value={query}
                  onChange={(event) => setQuery(event.target.value)}
                />
              </label>
              <select
                className="cm-select w-150"
                aria-label="按厂商筛选"
                value={vendor}
                onChange={(event) => setVendor(event.target.value)}
              >
                <option value="all">全部厂商</option>
                {vendors.map((item) => (
                  <option key={item} value={item}>
                    {item}
                  </option>
                ))}
              </select>
            </div>
            <div className="cm-panel cm-table-wrap">
              <table className="cm-table">
                <thead>
                  <tr>
                    <th>模型 ID</th>
                    <th>输入</th>
                    <th>缓存读取</th>
                    <th>输出</th>
                    <th>来源</th>
                    <th>核对时间</th>
                  </tr>
                </thead>
                <tbody>
                  {rows.map((entry) => (
                    <tr key={`${entry.vendor}:${entry.modelId}`}>
                      <td>
                        <span className="strong mono">{entry.modelId}</span>
                      </td>
                      <td className="mono">${entry.input.toFixed(2)} / M</td>
                      <td className="mono">
                        {entry.cachedInput != null
                          ? `$${entry.cachedInput.toFixed(2)} / M`
                          : '暂无'}
                      </td>
                      <td className="mono">${entry.output.toFixed(2)} / M</td>
                      <td>{VENDOR_LABELS[entry.vendor] ?? entry.vendor}</td>
                      <td className="faint">{entry.observedAt.slice(0, 10)}</td>
                    </tr>
                  ))}
                  {rows.length === 0 ? (
                    <tr>
                      <td colSpan={6} className="faint">
                        没有匹配的条目。
                      </td>
                    </tr>
                  ) : null}
                </tbody>
              </table>
            </div>
          </section>
          <details className="pv-advanced pv-pricing__adv">
            <summary>
              <span className="pv-advanced__chev">
                <Icon name="right" />
              </span>
              <span>高级设置</span>
              <span className="pv-advanced__hint">备用镜像 · 离线目录 · 默认源</span>
            </summary>
            <div className="pv-advanced__body">
              <div className="cm-note">
                <Icon name="shield" />
                <span>
                  高级设置只影响标准目录的获取来源；用户实际报价仍在「服务商详情 › 模型行 ›
                  定价」创建、修改或恢复手工覆盖。
                </span>
              </div>
              <div className="pv-advsec">
                <div className="pv-advsec__head">
                  <h3>自定义备用镜像</h3>
                  <button
                    className="cm-action"
                    type="button"
                    onClick={() => {
                      const urls = [mirrorDraft.trim()].filter(Boolean);
                      void persistPricingSettings({ pricingFeedUrls: urls }).then(() =>
                        onNotice(urls.length ? '已保存备用镜像' : '已清空备用镜像'),
                      );
                    }}
                  >
                    保存
                  </button>
                </div>
                <p className="pv-advsec__note">
                  主源不可用时尝试；只接受已验签目录，保存前校验签名、版本和完整性。
                </p>
                <input
                  className="cm-input mono"
                  value={mirrorDraft}
                  placeholder="https://mirror.example.com/helm/pricing"
                  onChange={(event) => setMirrorDraft(event.target.value)}
                />
              </div>
              <div className="pv-advsec">
                <div className="pv-advsec__head">
                  <h3>导入离线目录</h3>
                  <button
                    className="cm-action"
                    disabled={busy}
                    onClick={() => void importOffline()}
                    type="button"
                  >
                    选择文件
                  </button>
                </div>
                <p className="pv-advsec__note">
                  导入带签名的离线目录；版本不高于当前版本时后端保持现状，不回退。
                </p>
              </div>
              <div className="pv-advsec">
                <div className="pv-advsec__head">
                  <h3>恢复 Helm 默认源</h3>
                  <button
                    className="cm-action"
                    type="button"
                    onClick={() => {
                      setMirrorDraft('');
                      void persistPricingSettings({ pricingFeedUrls: [] }).then(() =>
                        onNotice('已恢复 Helm 默认源'),
                      );
                    }}
                  >
                    恢复默认
                  </button>
                </div>
                <p className="pv-advsec__note">
                  清除自定义镜像并回到 Helm 官方目录；不影响已保存的服务商覆盖价。
                </p>
              </div>
            </div>
          </details>
        </div>
        <DialogFooter>
          <Button variant="subtle" onClick={onClose} type="button">
            关闭
          </Button>
        </DialogFooter>
      </DialogContent>
    </ShadcnDialog>
  );
}
