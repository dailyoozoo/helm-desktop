import { useCallback, useEffect, useMemo, useState } from 'react';
import { Icon, type IconName } from '../shell/icons';
import { showResultToast } from '../components/toast';
import { ConfirmDialog } from '../components/ConfirmDialog';
import { useDialogBehavior } from '../components/useDialogBehavior';
import type { ReasoningEffort } from '@helm/protocol';
import { useReasoningEffortCapability } from '../engine/useReasoningEffortCapability';
import { normalizeReasoningEffort, reasoningEffortLabel } from '../reasoning';
import {
  deleteProviderConfig,
  detectCliLogin,
  getEquivalentEnv,
  listModelPriceOverrides,
  getProviderPricingPreference,
  getProviderConfig,
  loginCliAccount,
  logoutCliAccount,
  readEngineConfigFile,
  revealProviderSecret,
  saveBindingConfig,
  saveModelConfig,
  saveProviderModelSelection,
  saveModelPriceOverride,
  deleteModelPriceOverride,
  saveProviderPricingPreference,
  saveProviderConfig,
  syncProviderModels,
  testEngineConfig,
  testProviderConfig,
  writeEngineConfigFile,
  type AppConfig,
  type BindingConfig,
  type CliLoginState,
  type ConnectionResult,
  type EngineConfig,
  type EngineConfigFile,
  type ModelConfig,
  type ProviderPricingPreference,
  type ProviderAuthMethod,
  type ProviderConfig,
  type ProviderProtocol,
} from './api';
import {
  AUTH_METHOD_LABELS,
  bindingForEngine,
  PROVIDER_TEMPLATES,
  applicableEngineLabels,
  canBindProvider,
  compatibleProvidersForEngine,
  createProviderDraft,
  envPairsToText,
  failureCategoryLabel,
  lastTestText,
  lastTestTimeText,
  loginStateLabel,
  matchingSubscriptionProvider,
  modelCatalog,
  modelCatalogForProvider,
  modelsForProvider,
  normalizeBindingDraft,
  priceSourceText,
  providerDeleteConfirmation,
  providerDeleteBlockedReason,
  providerCapabilities,
  providerCanDelete,
  providerFailureCategory,
  providerModelEmptyState,
  providerSetupCopy,
  protocolLabel,
  reachabilityStatus,
  readinessText,
  subscriptionLoginWarning,
  type ProviderTemplateId,
} from './providerViewModel';
import './providers.css';

type Tab = 'bindings' | 'providers' | 'models';

const PROVIDER_ICONS: Record<string, IconName> = {
  anthropic: 'sparkles',
  openai: 'bot',
  openrouter: 'layers',
  ollama: 'cpu',
};

// Bedrock/Vertex 暂不支持（后端 engine_accepts 不允许绑定，等价环境变量也未实现），
// 在表单里隐藏，避免用户配置出永远无法生效的服务商（可靠性检查 G2）。
const PROTOCOLS: ProviderProtocol[] = ['anthropic', 'openai-responses', 'openai-chat'];

const AUTH_METHODS: ProviderAuthMethod[] = ['oauth', 'apikey', 'local'];

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

function engineStatusText(status: EngineConfig['status']) {
  if (status === 'ready') return '就绪';
  if (status === 'error') return '异常';
  return '未检测';
}

function providerIcon(providerId: string): IconName {
  return PROVIDER_ICONS[providerId] ?? 'server';
}

function providerName(config: AppConfig, providerId?: string) {
  if (!providerId) return '未绑定';
  return config.providers.find((provider) => provider.id === providerId)?.name ?? providerId;
}

function resultLabel(result?: ConnectionResult) {
  if (!result) return null;
  return `${result.message}${result.latencyMs ? ` · ${result.latencyMs}ms` : ''}`;
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
  const [activeProviderId, setActiveProviderId] = useState('anthropic');
  const [loading, setLoading] = useState(true);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [addProviderOpen, setAddProviderOpen] = useState(false);

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
        setActiveProviderId(loaded.providers[0]?.id ?? 'anthropic');
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

  const activeProvider = useMemo(
    () => config.providers.find((provider) => provider.id === activeProviderId),
    [activeProviderId, config.providers],
  );

  return (
    <main className="main">
      <div className="page scroll">
        <div className="providers-sticky">
          <div className="page__head providers-head">
            <div>
              <div className="page__title">服务商与模型</div>
              <div className="page__sub">
                使用 Claude / ChatGPT 订阅，或配置 API 服务商，并为每个 CLI 引擎绑定生效模型。
              </div>
            </div>
            <button className="btn btn--primary" onClick={() => setAddProviderOpen(true)}>
              <Icon name="plus" /> 添加服务商
            </button>
          </div>

          <div className="providers-tabs tabbar">
            {[
              ['bindings', '引擎与绑定'],
              ['providers', '服务商'],
              ['models', '模型目录'],
            ].map(([id, label]) => (
              <button
                key={id}
                className={'tab' + (tab === id ? ' is-active' : '')}
                onClick={() => setTab(id as Tab)}
              >
                {label}
              </button>
            ))}
          </div>
        </div>

        <div className="providers-wrap">
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
            <BindingsPanel config={config} onConfig={setConfig} onNotice={notify} />
          ) : null}
          {!loading && !loadError && tab === 'providers' ? (
            activeProvider ? (
              <ProvidersPanel
                config={config}
                activeProvider={activeProvider}
                onSelect={setActiveProviderId}
                onConfig={setConfig}
                onActiveProvider={setActiveProviderId}
                onNotice={notify}
              />
            ) : (
              <EmptyProvidersPrompt />
            )
          ) : null}
          {!loading && !loadError && tab === 'models' ? (
            <ModelsPanel config={config} onConfig={setConfig} onNotice={notify} />
          ) : null}
        </div>
        {addProviderOpen ? (
          <AddProviderModal
            providers={config.providers}
            onClose={() => setAddProviderOpen(false)}
            onCreate={(provider, apiKey) => {
              void saveProviderConfig(provider, apiKey)
                .then((next) => {
                  setConfig(next);
                  setActiveProviderId(provider.id);
                  setTab('providers');
                  setAddProviderOpen(false);
                  notify(
                    provider.kind === 'subscription'
                      ? '订阅接入已创建，请检测或登录账号后选择模型'
                      : '服务商已创建，下一步测试可达性并同步模型',
                  );
                })
                .catch((err: unknown) => notify(errorMessage(err, '创建服务商失败')));
            }}
          />
        ) : null}
      </div>
    </main>
  );
}

function AddProviderModal({
  onClose,
  onCreate,
  providers,
}: {
  onClose: () => void;
  onCreate: (provider: ProviderConfig, apiKey?: string) => void;
  providers: ProviderConfig[];
}) {
  const dialogRef = useDialogBehavior(onClose);
  const [selected, setSelected] = useState<ProviderTemplateId>(PROVIDER_TEMPLATES[0].id);
  const [name, setName] = useState(PROVIDER_TEMPLATES[0].name);
  const [baseUrl, setBaseUrl] = useState(PROVIDER_TEMPLATES[0].baseUrl);
  const [apiKey, setApiKey] = useState('');
  const template = PROVIDER_TEMPLATES.find((item) => item.id === selected) ?? PROVIDER_TEMPLATES[0];
  const needsApiKey = template.authMethod === 'apikey';
  const existingSubscription =
    template.kind === 'subscription'
      ? matchingSubscriptionProvider(providers, template.protocol)
      : undefined;
  const readyToCreate =
    Boolean(name.trim()) &&
    !existingSubscription &&
    (template.kind === 'subscription' || Boolean(baseUrl.trim())) &&
    (!needsApiKey || Boolean(apiKey.trim()));

  return (
    <div
      className="pv-modal-overlay open"
      role="dialog"
      aria-modal="true"
      aria-label="添加服务商"
      onClick={(event) => event.target === event.currentTarget && onClose()}
    >
      <div className="pv-modal card providers-add-modal" ref={dialogRef} tabIndex={-1}>
        <div className="pv-modal__hd">
          添加服务商
          <small>先选择它要服务的引擎，Helm 会带出合适的接口规范和认证方式。</small>
        </div>
        <div className="pv-modal__bd">
          <div className="provider-template-list">
            {PROVIDER_TEMPLATES.map((item) => (
              <button
                key={item.id}
                className={'provider-template' + (selected === item.id ? ' is-active' : '')}
                onClick={() => {
                  setSelected(item.id);
                  setName(item.name);
                  setBaseUrl(item.baseUrl);
                  setApiKey('');
                }}
                type="button"
              >
                <span className="provider-template__ic">
                  <Icon name={item.authMethod === 'local' ? 'cpu' : 'server'} />
                </span>
                <span className="grow">
                  <b>{item.title}</b>
                  <small>{item.description}</small>
                </span>
              </button>
            ))}
          </div>
          <label className="field">
            <span>显示名称</span>
            <input
              className="input"
              value={name}
              onChange={(event) => setName(event.target.value)}
            />
          </label>
          {template.kind !== 'subscription' ? (
            <label className="field">
              <span>Base URL</span>
              <input
                className="input mono"
                value={baseUrl}
                onChange={(event) => setBaseUrl(event.target.value)}
              />
            </label>
          ) : null}
          {needsApiKey ? (
            <label className="field">
              <span>API 密钥</span>
              <input
                className="input mono"
                type="password"
                value={apiKey}
                onChange={(event) => setApiKey(event.target.value)}
                placeholder="保存到系统钥匙串"
              />
            </label>
          ) : null}
          <div className="providers-next-box">
            <b>创建后下一步</b>
            <span>
              {existingSubscription
                ? `同协议订阅「${existingSubscription.name}」已存在，请关闭弹窗后直接复用。`
                : providerSetupCopy(template).nextStep}
            </span>
          </div>
        </div>
        <div className="pv-modal__ft">
          <button className="btn btn--subtle" onClick={onClose} type="button">
            取消
          </button>
          <button
            className="btn btn--primary"
            disabled={!readyToCreate}
            onClick={() => {
              const provider = createProviderDraft(selected);
              onCreate(
                {
                  ...provider,
                  name: name.trim(),
                  baseUrl: template.kind === 'subscription' ? '' : baseUrl.trim(),
                },
                apiKey.trim() || undefined,
              );
            }}
            type="button"
          >
            保存并继续
          </button>
        </div>
      </div>
    </div>
  );
}

function EmptyProvidersPrompt() {
  return (
    <section className="card card--pad providers-empty-card">
      <span className="provider-detail__big">
        <Icon name="server" />
      </span>
      <div>
        <h2>还没有服务商</h2>
        <p>添加 Claude / ChatGPT 订阅，或接入一个 API 服务商、本地模型服务。</p>
      </div>
    </section>
  );
}

function BindingsPanel({
  config,
  onConfig,
  onNotice,
}: {
  config: AppConfig;
  onConfig: (config: AppConfig) => void;
  onNotice: (message: string | null) => void;
}) {
  const [editingEngine, setEditingEngine] = useState<EngineConfig | null>(null);

  return (
    <>
      <div className="engines-grid">
        {config.engines.map((engine) => (
          <EngineBindingCard
            key={engine.id}
            config={config}
            engine={engine}
            onEdit={() => setEditingEngine(engine)}
            onNotice={onNotice}
          />
        ))}
      </div>
      <p className="providers-note">
        每个引擎只能绑定接口规范兼容且配置就绪的服务商。这里设的是全局默认，新建会话会以此为默认。
      </p>
      {editingEngine ? (
        <BindingModal
          config={config}
          engine={editingEngine}
          onClose={() => setEditingEngine(null)}
          onConfig={onConfig}
          onNotice={onNotice}
        />
      ) : null}
    </>
  );
}

function EngineBindingCard({
  config,
  engine,
  onEdit,
  onNotice,
}: {
  config: AppConfig;
  engine: EngineConfig;
  onEdit: () => void;
  onNotice: (message: string | null) => void;
}) {
  const binding = bindingForEngine(config, engine);
  const provider = config.providers.find((item) => item.id === binding?.providerId);
  const [envText, setEnvText] = useState('');
  const [configFile, setConfigFile] = useState<EngineConfigFile | null>(null);
  const [fileDraft, setFileDraft] = useState('');
  const [editingFile, setEditingFile] = useState(false);
  const [fileBusy, setFileBusy] = useState(false);
  const [testing, setTesting] = useState(false);

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
      .catch((err: unknown) => {
        if (live) onNotice(errorMessage(err, '读取引擎配置文件失败'));
      });
    return () => {
      live = false;
    };
  }, [engine.id, onNotice]);

  return (
    <section className="card engine-card">
      <div className="engine-card__head">
        <span className="engine-card__ic">
          <Icon name={engine.id === 'codex' ? 'cpu' : 'zap'} />
        </span>
        <div className="grow">
          <h3>
            {engine.name}
            <span className={'pill ' + (engine.status === 'ready' ? 'pill--success' : '')}>
              {engineStatusText(engine.status)}
            </span>
          </h3>
          <small className="mono">{engine.bin}</small>
        </div>
      </div>
      <div className="engine-card__body">
        <div className="bind-box">
          <div className="bind-row">
            <span>生效服务商</span>
            <b>{providerName(config, binding?.providerId)}</b>
            {provider ? (
              <span className="pill pill--proto">{protocolLabel(provider.protocol)}</span>
            ) : null}
          </div>
          <div className="bind-row">
            <span>主模型</span>
            <code>{binding?.primaryModel || '未绑定'}</code>
          </div>
          <div className="bind-row">
            <span>快速模型</span>
            <code>{binding?.fastModel || '未设置'}</code>
          </div>
        </div>
        <button className="btn btn--subtle btn--sm bind-edit" onClick={onEdit}>
          <Icon name="refresh" /> 更改绑定
        </button>
        <details className="envbox">
          <summary>
            等价配置
            {configFile ? <span className="envbox__path mono">{configFile.path}</span> : null}
          </summary>
          <pre className="env">{envText}</pre>
          <div className="engine-file">
            <div className="engine-file__actions">
              <button
                className="btn btn--subtle btn--sm"
                disabled={fileBusy}
                onClick={() => {
                  setFileBusy(true);
                  void readEngineConfigFile(engine.id)
                    .then((file) => {
                      setConfigFile(file);
                      setFileDraft(file.content);
                      setEditingFile(false);
                      onNotice('已从真实配置文件回填');
                    })
                    .catch((err: unknown) => onNotice(errorMessage(err, '从文件回填失败')))
                    .finally(() => setFileBusy(false));
                }}
              >
                <Icon name="refresh" className={fileBusy ? 'spin' : undefined} />
                从文件回填
              </button>
              <button
                className="btn btn--subtle btn--sm"
                onClick={() => setEditingFile((value) => !value)}
              >
                <Icon name="edit" /> 高级编辑
              </button>
              {editingFile ? (
                <button
                  className="btn btn--primary btn--sm"
                  disabled={fileBusy}
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
        </details>
        <div className="kv">
          <span>版本</span>
          <span className="mono">{engine.version || '未检测'}</span>
        </div>
      </div>
      <div className="engine-card__actions">
        <button
          className="btn btn--subtle btn--sm"
          disabled={testing}
          onClick={() => {
            setTesting(true);
            void testEngineConfig(engine.bin)
              .then((result) => onNotice(result.ok ? null : result.message))
              .catch((err: unknown) => onNotice(errorMessage(err, '检测引擎失败')))
              .finally(() => setTesting(false));
          }}
        >
          <Icon name="refresh" className={testing ? 'spin' : undefined} />
          重新检测
        </button>
      </div>
    </section>
  );
}

function BindingModal({
  config,
  engine,
  onClose,
  onConfig,
  onNotice,
}: {
  config: AppConfig;
  engine: EngineConfig;
  onClose: () => void;
  onConfig: (config: AppConfig) => void;
  onNotice: (message: string | null) => void;
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
  const models = modelsForProvider(config, providerId);
  const [primaryModel, setPrimaryModel] = useState(initialBinding.primaryModel);
  const [fastModel, setFastModel] = useState(initialBinding.fastModel ?? '');
  const [reasoningEffort, setReasoningEffort] = useState<ReasoningEffort>(
    initialBinding.reasoningEffort ?? 'auto',
  );
  const [envText, setEnvText] = useState('');
  const [testingProvider, setTestingProvider] = useState(false);
  const [syncingSubscriptionModels, setSyncingSubscriptionModels] = useState(false);
  const [subscriptionModelsRefreshed, setSubscriptionModelsRefreshed] = useState(false);
  const selectedProvider = config.providers.find((provider) => provider.id === providerId);
  const selectedProviderId = selectedProvider?.id;
  const selectedProviderKind = selectedProvider?.kind;
  const {
    capability: reasoningCapability,
    loading: reasoningLoading,
    error: reasoningError,
  } = useReasoningEffortCapability(engine.id, primaryModel, providerId);
  const [selectedLogin, setSelectedLogin] = useState<CliLoginState | null>(null);
  const cancelBindingChange = useCallback(() => {
    if (subscriptionModelsRefreshed) {
      onNotice('已取消绑定更改；账号模型刷新结果仍已保存，当前生效绑定没有改变');
    }
    onClose();
  }, [onClose, onNotice, subscriptionModelsRefreshed]);
  const dialogRef = useDialogBehavior(cancelBindingChange);
  const loginWarning =
    selectedProvider?.kind === 'subscription' ? subscriptionLoginWarning(selectedLogin) : null;

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
    if (reasoningLoading) return;
    if (reasoningCapability) {
      const normalized = normalizeReasoningEffort(reasoningCapability, reasoningEffort);
      if (normalized !== reasoningEffort) setReasoningEffort(normalized);
    } else if (reasoningError && reasoningEffort !== 'auto') {
      setReasoningEffort('auto');
    }
  }, [reasoningCapability, reasoningEffort, reasoningError, reasoningLoading]);

  useEffect(() => {
    let live = true;
    if (!providerId || !primaryModel) {
      setEnvText('没有可用的兼容服务商或模型');
      return;
    }
    const binding: BindingConfig = {
      engineId: engine.id,
      providerId,
      primaryModel,
      fastModel: fastModel || null,
      reasoningEffort,
    };
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
  }, [engine.id, fastModel, primaryModel, providerId, reasoningEffort]);

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
    <div
      className="pv-modal-overlay open"
      role="dialog"
      aria-modal="true"
      aria-label={`更改 ${engine.name} 绑定`}
      onClick={(event) => event.target === event.currentTarget && cancelBindingChange()}
    >
      <div className="pv-modal card" ref={dialogRef} tabIndex={-1}>
        <div className="pv-modal__hd">
          更改绑定
          <small>为 {engine.name} 选择协议兼容的服务商与模型</small>
        </div>
        <div className="pv-modal__bd">
          <label className="field">
            <span>服务商（仅显示协议兼容且配置就绪的）</span>
            <select
              className="select"
              value={providerId}
              onChange={(event) => {
                const nextProviderId = event.target.value;
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
                  {provider.name} - {protocolLabel(provider.protocol)} ·{' '}
                  {modelsForProvider(config, provider.id).length} 个模型
                </option>
              ))}
            </select>
          </label>
          {selectedProvider ? (
            <div className="reachability-box">
              <div>
                <b>
                  {selectedProvider.kind === 'subscription'
                    ? loginStateLabel(selectedLogin)
                    : lastTestText(selectedProvider)}
                </b>
                <small>
                  {selectedProvider.kind === 'subscription'
                    ? (selectedLogin?.detail ?? '正在检测 Helm 独立订阅登录状态')
                    : `可达性 · ${lastTestTimeText(selectedProvider)}`}
                </small>
              </div>
              <button
                className="btn btn--subtle btn--sm"
                disabled={testingProvider || syncingSubscriptionModels}
                onClick={() => {
                  setTestingProvider(true);
                  const action =
                    selectedProvider.kind === 'subscription'
                      ? detectCliLogin(engine.id).then(async (state) => {
                          setSelectedLogin(state);
                          if (state.state !== 'ok' || state.authMethod !== 'subscription') {
                            onNotice(state.detail);
                            return;
                          }
                          setSyncingSubscriptionModels(true);
                          try {
                            onConfig(await syncProviderModels(selectedProvider.id));
                            setSubscriptionModelsRefreshed(true);
                            onNotice('账号模型已保存；请选择模型并保存绑定后才会切换生效服务商');
                          } finally {
                            setSyncingSubscriptionModels(false);
                          }
                        })
                      : testProviderConfig(selectedProvider.id).then((result) => {
                          onNotice(result.ok ? null : result.message);
                          return getProviderConfig().then(onConfig);
                        });
                  void action
                    .catch((err: unknown) => onNotice(errorMessage(err, '测试可达性失败')))
                    .finally(() => setTestingProvider(false));
                }}
              >
                <Icon name="refresh" className={testingProvider ? 'spin' : undefined} />
                {selectedProvider.kind === 'subscription'
                  ? syncingSubscriptionModels
                    ? '读取账号模型…'
                    : '检测并刷新模型'
                  : '测试可达性'}
              </button>
            </div>
          ) : null}
          {loginWarning ? (
            <div className="providers-warning">{loginWarning}</div>
          ) : selectedProvider &&
            selectedProvider.kind !== 'subscription' &&
            selectedProvider.lastTest?.result !== 'ok' ? (
            <div className="providers-warning">
              {selectedProvider.lastTest
                ? '上次可达性测试失败，保存后建议先验证。'
                : '尚未测试可达性，保存后建议先测试。'}
            </div>
          ) : null}
          <div className="form-grid">
            <label className="field">
              <span>主模型</span>
              <select
                className="select mono"
                value={primaryModel}
                onChange={(event) => setPrimaryModel(event.target.value)}
              >
                {models.map((model) => (
                  <option key={`${model.providerId}:${model.id}`} value={model.id}>
                    {model.displayName && model.displayName !== model.id
                      ? `${model.displayName} · ${model.id}`
                      : model.id}
                  </option>
                ))}
              </select>
            </label>
            <label className="field">
              <span>快速模型 · 背景任务</span>
              <select
                className="select mono"
                value={fastModel}
                onChange={(event) => setFastModel(event.target.value)}
              >
                {models.map((model) => (
                  <option key={`${model.providerId}:${model.id}`} value={model.id}>
                    {model.displayName && model.displayName !== model.id
                      ? `${model.displayName} · ${model.id}`
                      : model.id}
                  </option>
                ))}
              </select>
            </label>
            <label className="field">
              <span>新会话默认推理强度</span>
              <select
                className="select"
                value={reasoningEffort}
                disabled={reasoningLoading}
                onChange={(event) => setReasoningEffort(event.target.value as ReasoningEffort)}
              >
                {(reasoningCapability?.options ?? ['auto']).map((effort) => (
                  <option key={effort} value={effort}>
                    {reasoningEffortLabel(effort)}
                    {effort !== 'auto' && effort === reasoningCapability?.defaultEffort
                      ? '（模型默认）'
                      : ''}
                  </option>
                ))}
              </select>
              <small>
                {reasoningLoading
                  ? '正在读取 CLI 模型能力…'
                  : reasoningError
                    ? '未能确认可调档位，将使用模型默认值'
                    : '仅影响主模型；快速模型继续使用自身默认值'}
              </small>
            </label>
          </div>
          <pre className="env">{envText}</pre>
        </div>
        <div className="pv-modal__ft">
          <button className="btn btn--subtle" onClick={cancelBindingChange}>
            取消
          </button>
          <button
            className="btn btn--primary"
            disabled={
              !selectedProvider ||
              !providerId ||
              !primaryModel ||
              reasoningLoading ||
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
            <Icon name="check" />
            保存并启用绑定
          </button>
        </div>
      </div>
    </div>
  );
}

function ProvidersPanel({
  config,
  activeProvider,
  onSelect,
  onConfig,
  onActiveProvider,
  onNotice,
}: {
  config: AppConfig;
  activeProvider: ProviderConfig;
  onSelect: (id: string) => void;
  onConfig: (config: AppConfig) => void;
  onActiveProvider: (id: string) => void;
  onNotice: (message: string | null) => void;
}) {
  const [draft, setDraft] = useState(activeProvider);
  const [apiKey, setApiKey] = useState('');
  const [showKey, setShowKey] = useState(false);
  const [editingKey, setEditingKey] = useState(false);
  const [testing, setTesting] = useState(false);
  const [testResult, setTestResult] = useState<ConnectionResult | undefined>();
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [modelDrafts, setModelDrafts] = useState(() =>
    modelCatalogForProvider(config, activeProvider.id),
  );

  useEffect(() => {
    setDraft(activeProvider);
    setApiKey('');
    setShowKey(false);
    setEditingKey(false);
    setTestResult(undefined);
  }, [activeProvider]);

  useEffect(() => {
    setModelDrafts(modelCatalogForProvider(config, activeProvider.id));
  }, [activeProvider.id, config]);

  const keyValue = apiKey || (!editingKey && draft.keyRef ? '••••••••••••••••••••' : '');
  const providerModels = modelDrafts;
  const usableEngines = applicableEngineLabels(draft.protocol);
  const capabilities = providerCapabilities(draft);
  const markSecretMissing = () => {
    setDraft({ ...draft, keyRef: null });
    setApiKey('');
    setShowKey(true);
    setEditingKey(true);
  };
  const requireApiKeyWhenEditing = () => {
    if (draft.authMethod !== 'apikey') return false;
    if (!editingKey || apiKey.trim()) return false;
    onNotice('请先粘贴新的 API 密钥，再保存或测试');
    return true;
  };
  const saveProvider = () => {
    if (requireApiKeyWhenEditing()) return;
    void saveProviderConfig(draft, apiKey)
      .then((next) => {
        onConfig(next);
        return saveProviderModelSelection(
          draft.id,
          modelDrafts.filter((model) => model.enabled).map((model) => model.id),
        );
      })
      .then((next) => {
        onConfig(next);
        setModelDrafts(modelCatalogForProvider(next, draft.id));
        onNotice('服务商与启用模型已保存；当前引擎绑定未改变');
        setApiKey('');
        setShowKey(false);
        setEditingKey(false);
      })
      .catch((err: unknown) => {
        onNotice(errorMessage(err, '保存服务商失败'));
      });
  };
  const testProvider = () => {
    if (requireApiKeyWhenEditing()) return;
    setTesting(true);
    setTestResult(undefined);
    void saveProviderConfig(draft, apiKey)
      .then((next) => {
        onConfig(next);
        const saved = next.providers.find((provider) => provider.id === draft.id);
        if (saved) setDraft(saved);
        return testProviderConfig(draft.id);
      })
      .then((result) => {
        setTestResult(result);
        onNotice(result.ok ? '可达性测试通过' : result.message);
        if (result.ok) {
          setApiKey('');
          setShowKey(false);
          setEditingKey(false);
        }
        return getProviderConfig();
      })
      .then((next) => {
        onConfig(next);
        const saved = next.providers.find((provider) => provider.id === draft.id);
        if (saved) setDraft(saved);
      })
      .catch((err: unknown) => {
        if (errorMessage(err, '').includes('钥匙串中没有找到 API 密钥')) {
          markSecretMissing();
        }
        onNotice(errorMessage(err, '测试可达性失败'));
      })
      .finally(() => setTesting(false));
  };
  const repairProviderFailure = () => {
    const category = providerFailureCategory(draft);
    if (category === 'auth') {
      setShowKey(true);
      setEditingKey(true);
      requestAnimationFrame(() =>
        document.querySelector<HTMLInputElement>('[data-provider-field="api-key"]')?.focus(),
      );
      return;
    }
    if (category === 'network') {
      document.querySelector<HTMLInputElement>('[data-provider-field="base-url"]')?.focus();
      return;
    }
    testProvider();
  };
  const providerModelCount = config.models.filter((model) => model.providerId === draft.id).length;
  const providerBindingCount = config.bindings.filter(
    (binding) => binding.providerId === draft.id,
  ).length;
  const deleteBlockedReason = providerDeleteBlockedReason(providerBindingCount);
  const deleteCopy = providerDeleteConfirmation(draft, providerModelCount, providerBindingCount);
  const failureCategory = providerFailureCategory(draft);

  return (
    <div className="providers-split">
      <div className="provider-list">
        {config.providers.map((provider) => {
          const rStatus = reachabilityStatus(provider);
          return (
            <button
              key={provider.id}
              className={'provider-item' + (provider.id === activeProvider.id ? ' is-active' : '')}
              onClick={() => onSelect(provider.id)}
            >
              <span className="provider-item__ic">
                <Icon name={providerIcon(provider.id)} />
              </span>
              <span className="provider-item__m">
                <b>{provider.name}</b>
                <small>
                  {protocolLabel(provider.protocol)} · {readinessText(provider)}
                </small>
              </span>
              <span
                className={
                  'dot ' +
                  (rStatus === 'reachable'
                    ? 'dot--on'
                    : rStatus === 'unreachable'
                      ? 'dot--fail'
                      : 'dot--off')
                }
                title={
                  rStatus === 'reachable' ? '可达' : rStatus === 'unreachable' ? '不可达' : '未测试'
                }
              />
            </button>
          );
        })}
      </div>

      <section className="provider-detail card card--pad">
        <div className="provider-detail__head">
          <span className="provider-detail__big">
            <Icon name={providerIcon(activeProvider.id)} />
          </span>
          <div className="grow">
            <h2>
              {draft.name}
              <span className="pill pill--proto">{protocolLabel(draft.protocol)}</span>
            </h2>
            <div className="muted">
              {draft.kind === 'subscription'
                ? '订阅凭证由 Helm 独立 CLI Profile 管理，不影响其他终端工具。'
                : '密钥仅写入操作系统钥匙串，配置文件只保存 keyRef。'}
            </div>
          </div>
          <span className={'pill ' + (draft.ready ? 'pill--success' : '')}>
            {readinessText(draft)}
          </span>
        </div>
        <div className="provider-section">
          <div className="provider-section__t">基础配置</div>
          <div className="form-grid">
            <label className="field">
              <span>名称</span>
              <input
                className="input"
                value={draft.name}
                onChange={(event) => setDraft({ ...draft, name: event.target.value })}
              />
            </label>
            {draft.kind === 'subscription' ? (
              <div className="field pv-account-source">
                <span>接入方式</span>
                <b>{draft.protocol === 'anthropic' ? 'Claude 官方订阅' : 'ChatGPT 官方订阅'}</b>
                <div className="hint">
                  固定用于：{usableEngines.length ? usableEngines.join('、') : '暂无兼容引擎'}；
                  官方地址和认证方式不可在这里修改。
                </div>
              </div>
            ) : (
              <>
                <label className="field">
                  <span>接口规范</span>
                  <select
                    className="select"
                    value={draft.protocol}
                    onChange={(event) =>
                      setDraft({ ...draft, protocol: event.target.value as ProviderProtocol })
                    }
                  >
                    {PROTOCOLS.map((protocol) => (
                      <option key={protocol} value={protocol}>
                        {protocolLabel(protocol)}
                      </option>
                    ))}
                  </select>
                  <div className="hint">
                    决定哪些引擎能使用它 · 当前可用于：
                    {usableEngines.length ? usableEngines.join('、') : '暂无兼容引擎'}
                  </div>
                </label>
                <label className="field">
                  <span>认证方式</span>
                  <select
                    className="select"
                    value={draft.authMethod}
                    onChange={(event) =>
                      setDraft({ ...draft, authMethod: event.target.value as ProviderAuthMethod })
                    }
                  >
                    {AUTH_METHODS.filter((method) => method !== 'oauth').map((method) => (
                      <option key={method} value={method}>
                        {AUTH_METHOD_LABELS[method]}
                      </option>
                    ))}
                  </select>
                </label>
                {capabilities.showBaseUrl ? (
                  <label className="field">
                    <span>基础 URL</span>
                    <input
                      data-provider-field="base-url"
                      className="input mono"
                      value={draft.baseUrl}
                      disabled={draft.authMethod === 'cloud'}
                      onChange={(event) => setDraft({ ...draft, baseUrl: event.target.value })}
                    />
                  </label>
                ) : null}
              </>
            )}
          </div>
        </div>
        <div className="provider-section">
          <div className="provider-section__t">
            {draft.kind === 'subscription' ? '订阅登录' : '认证'}
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
        </div>
        {draft.kind !== 'subscription' && capabilities.canTestHttp ? (
          <div className="provider-section">
            <div className="provider-section__t">连接与状态</div>
            <div className="connectivity">
              <span className="connectivity__ic">
                <Icon name="zap" />
              </span>
              <div className="grow">
                <b>
                  {resultLabel(testResult) ?? lastTestText(draft)}
                  {failureCategory ? (
                    <span className={'failure-tag failure-tag--' + failureCategory}>
                      {failureCategoryLabel(failureCategory)}
                    </span>
                  ) : null}
                </b>
                <small>
                  {resultLabel(testResult) ? '本次测试' : `上次测试 · ${lastTestTimeText(draft)}`}
                </small>
              </div>
              <button
                className="btn btn--subtle"
                disabled={testing}
                onClick={testProvider}
                type="button"
              >
                <Icon name="refresh" className={testing ? 'spin' : undefined} />
                测试可达性
              </button>
            </div>
            {failureCategory ? (
              <div className="connectivity-note">
                {failureCategory === 'network' && '无法连接到服务商：请检查网络和基础 URL。'}
                {failureCategory === 'auth' && '认证失败：请确认 API 密钥是否正确。'}
                {failureCategory === 'timeout' && '连接超时：请检查网络状况或服务商状态。'}
                {failureCategory === 'unknown' && '未识别的连接错误，可稍后重试。'}{' '}
                <button className="btn-link" onClick={repairProviderFailure} type="button">
                  去修复
                </button>
              </div>
            ) : null}
          </div>
        ) : null}
        <ProviderModelCatalog
          config={config}
          provider={draft}
          models={providerModels}
          onConfig={onConfig}
          onModelsChange={setModelDrafts}
          onNotice={onNotice}
          onRequestSave={saveProvider}
          onRequestTest={testProvider}
        />
        <div className="provider-actions">
          <button
            className="btn btn--danger"
            onClick={() => {
              if (!providerCanDelete(providerBindingCount)) {
                onNotice(deleteBlockedReason);
                return;
              }
              setConfirmDelete(true);
            }}
            title={deleteBlockedReason ?? undefined}
          >
            移除服务商
          </button>
          <span className="grow" />
          <button className="btn btn--primary" onClick={saveProvider}>
            <Icon name="check" />
            保存服务商与模型
          </button>
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
                  onActiveProvider(next.providers[0]?.id ?? '');
                  onNotice('服务商已删除，关联模型也已移除');
                  setConfirmDelete(false);
                })
                .catch((err: unknown) => {
                  onNotice(errorMessage(err, '删除服务商失败'));
                });
            }}
          />
        ) : null}
      </section>
    </div>
  );
}

function ProviderModelCatalog({
  config,
  provider,
  models,
  onConfig,
  onModelsChange,
  onNotice,
  onRequestSave,
  onRequestTest,
}: {
  config: AppConfig;
  provider: ProviderConfig;
  models: ReturnType<typeof modelCatalogForProvider>;
  onConfig: (config: AppConfig) => void;
  onModelsChange: (models: ModelConfig[]) => void;
  onNotice: (message: string | null) => void;
  onRequestSave: () => void;
  onRequestTest: () => void;
}) {
  const [syncing, setSyncing] = useState(false);
  const [editingPrice, setEditingPrice] = useState<ModelConfig | null>(null);
  const [preference, setPreference] = useState<ProviderPricingPreference | null>(null);
  const [savingPreference, setSavingPreference] = useState(false);
  const emptyState = providerModelEmptyState(provider);
  const capabilities = providerCapabilities(provider);
  useEffect(() => {
    let active = true;
    getProviderPricingPreference(provider.id)
      .then((value) => active && setPreference(value))
      .catch(() => active && setPreference(null));
    return () => {
      active = false;
    };
  }, [provider.id]);
  const syncModels = () => {
    setSyncing(true);
    void syncProviderModels(provider.id)
      .then((next) => {
        onConfig(next);
        const refreshed = modelCatalogForProvider(next, provider.id);
        onModelsChange(refreshed);
        const count = refreshed.length;
        onNotice(`模型列表已同步：${count} 个`);
      })
      .catch((err: unknown) => onNotice(errorMessage(err, '同步模型列表失败')))
      .finally(() => setSyncing(false));
  };
  const runEmptyAction = () => {
    if (emptyState.action === '保存更改') {
      onRequestSave();
      return;
    }
    if (emptyState.action === '测试可达性') {
      onRequestTest();
      return;
    }
    syncModels();
  };

  return (
    <section className="provider-models">
      <div className="section-h">
        <b>
          模型目录 <span className="faint">（{models.length}）</span>
        </b>
        {capabilities.canSyncModels ? (
          <button
            className="btn btn--subtle btn--sm"
            disabled={syncing}
            onClick={syncModels}
            type="button"
          >
            <Icon name="refresh" className={syncing ? 'spin' : undefined} />
            {provider.kind === 'subscription' ? '刷新账号模型' : '同步模型列表'}
          </button>
        ) : null}
      </div>
      {provider.kind !== 'subscription' && preference ? (
        <div className="provider-pricing-policy">
          <div className="grow">
            <b>价格策略</b>
            <small>官方目录只作为估算；手动覆盖始终按服务商 + 模型隔离。</small>
          </div>
          <select
            className="select"
            value={preference.mode}
            onChange={(event) =>
              setPreference({
                ...preference,
                mode: event.target.value as ProviderPricingPreference['mode'],
              })
            }
          >
            <option value="auto">自动：手动 → 服务商 → 官方参考</option>
            <option value="official-reference">只用官方参考</option>
            <option value="provider">只用服务商价格</option>
            <option value="manual">只用手动价格</option>
            <option value="disabled">不计算价格</option>
          </select>
          <label className="provider-pricing-multiplier">
            倍率
            <input
              className="input mono"
              type="number"
              min="0.01"
              max="100"
              step="0.01"
              value={preference.multiplierBasisPoints / 10000}
              onChange={(event) =>
                setPreference({
                  ...preference,
                  multiplierBasisPoints: Math.max(
                    1,
                    Math.round(Number(event.target.value || '1') * 10000),
                  ),
                })
              }
            />
          </label>
          <button
            className="btn btn--subtle btn--sm"
            disabled={savingPreference}
            onClick={() => {
              setSavingPreference(true);
              void saveProviderPricingPreference(preference)
                .then(async () => {
                  onConfig(await getProviderConfig());
                  onNotice('价格策略已保存');
                })
                .catch((error: unknown) => onNotice(errorMessage(error, '保存价格策略失败')))
                .finally(() => setSavingPreference(false));
            }}
            type="button"
          >
            保存策略
          </button>
        </div>
      ) : null}
      {models.length ? (
        <div className="provider-model-list">
          {models.map((model) => (
            <div key={`${model.providerId}:${model.id}`} className="provider-model-row">
              <div className="grow">
                <b className="mono">{model.displayName || model.id}</b>
                <small>
                  {providerName(config, model.providerId)} · {protocolLabel(provider.protocol)}
                </small>
              </div>
              <span className="model-price">
                {provider.kind === 'subscription' ? (
                  <>
                    <span>订阅内</span>
                    <small>当前账号可用</small>
                  </>
                ) : (
                  <>
                    <span className="mono">
                      {priceText(model.inputPricePerMtok)} / {priceText(model.outputPricePerMtok)}
                    </span>
                    <small>{priceSourceText(model)}</small>
                  </>
                )}
              </span>
              {provider.kind !== 'subscription' ? (
                <button
                  className="btn btn--subtle btn--sm"
                  onClick={() => setEditingPrice(model)}
                  type="button"
                >
                  定价
                </button>
              ) : null}
              <label className="switch">
                <input
                  type="checkbox"
                  checked={model.enabled}
                  onChange={() =>
                    onModelsChange(
                      models.map((item) =>
                        item.id === model.id ? { ...item, enabled: !item.enabled } : item,
                      ),
                    )
                  }
                />
                <i />
              </label>
            </div>
          ))}
        </div>
      ) : (
        <div className="provider-model-empty">
          <span className="provider-model-empty__ic">
            <Icon name="layers" />
          </span>
          <div className="grow">
            <b>{emptyState.title}</b>
            <p>{emptyState.body}</p>
          </div>
          {emptyState.action !== '测试可达性' ? (
            <button
              className="btn btn--subtle btn--sm"
              disabled={syncing}
              onClick={runEmptyAction}
              type="button"
            >
              <Icon name={emptyState.action === '同步模型列表' ? 'refresh' : 'check'} />
              {emptyState.action}
            </button>
          ) : null}
        </div>
      )}
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
    </section>
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
  const dialogRef = useDialogBehavior(onClose);
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
    <div
      className="pv-modal-overlay open"
      role="dialog"
      aria-modal="true"
      aria-label={`配置 ${model.id} 价格`}
      onClick={(event) => event.target === event.currentTarget && onClose()}
    >
      <div className="pv-modal card provider-price-modal" ref={dialogRef} tabIndex={-1}>
        <div className="pv-modal__hd">
          配置模型价格
          <small>{model.id} · 美元 / 百万 token；手动价格只作用于当前服务商。</small>
        </div>
        <div className="pv-modal__bd provider-price-grid">
          {[
            ['输入', input, setInput],
            ['缓存读取', cachedInput, setCachedInput],
            ['缓存写入', cacheWrite, setCacheWrite],
            ['输出', output, setOutput],
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
          {saveError ? (
            <div className="provider-price-error" role="alert">
              {saveError}
            </div>
          ) : null}
        </div>
        <div className="pv-modal__ft">
          <button
            className="btn btn--subtle"
            disabled={saving || loadingOverride}
            onClick={() => {
              setSaving(true);
              void deleteModelPriceOverride(model.providerId, model.id)
                .then(() => onSaved('已恢复自动价格'))
                .catch((err: unknown) => setSaveError(errorMessage(err, '恢复自动价格失败')))
                .finally(() => setSaving(false));
            }}
            type="button"
          >
            恢复自动
          </button>
          <span className="grow" />
          <button className="btn btn--subtle" onClick={onClose} type="button">
            取消
          </button>
          <button
            className="btn btn--primary"
            disabled={saving || loadingOverride}
            onClick={save}
            type="button"
          >
            {loadingOverride ? '读取中…' : '保存价格'}
          </button>
        </div>
      </div>
    </div>
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
    <div className="authcard">
      <span className="authcard__ic">
        <Icon name="shield" />
      </span>
      <div className="grow">
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
      <div className="authcard">
        <span className="authcard__ic">
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
      <div className="authcard">
        <span className="authcard__ic">
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
    <div className="field">
      <label>API 密钥</label>
      <div className="keyfield">
        <input
          data-provider-field="api-key"
          className="input mono"
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
            className="btn btn--subtle"
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
      <div className="hint">密钥存储在操作系统钥匙串中，绝不写入数据库或日志。</div>
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
  const [providerFilter, setProviderFilter] = useState('all');
  const catalog = modelCatalog(config);
  const rows = catalog.filter(
    (model) =>
      (providerFilter === 'all' || model.providerId === providerFilter) &&
      (!query || model.id.toLowerCase().includes(query.toLowerCase())),
  );

  return (
    <>
      <div className="filterbar">
        <label className="search providers-search">
          <Icon name="search" />
          <input
            placeholder="搜索模型 ID…"
            value={query}
            onChange={(event) => setQuery(event.target.value)}
          />
        </label>
        <div className="row gap-sm">
          <button
            className={'chip' + (providerFilter === 'all' ? ' is-active' : '')}
            onClick={() => setProviderFilter('all')}
          >
            全部
          </button>
          {config.providers.map((provider) => (
            <button
              key={provider.id}
              className={'chip' + (providerFilter === provider.id ? ' is-active' : '')}
              onClick={() => setProviderFilter(provider.id)}
            >
              {provider.name}
            </button>
          ))}
        </div>
        <span className="grow" />
        <span className="faint">
          {rows.length} / {catalog.length} 个模型
        </span>
      </div>
      <div className="card table-card">
        <table className="table">
          <thead>
            <tr>
              <th>模型 ID</th>
              <th>服务商</th>
              <th>接口规范</th>
              <th>输入</th>
              <th>输出</th>
              <th>上下文窗口</th>
              <th>能力</th>
              <th className="models-enabled-cell">启用</th>
            </tr>
          </thead>
          <tbody>
            {rows.map((model) => {
              const provider = config.providers.find((item) => item.id === model.providerId);
              return (
                <tr key={`${model.providerId}:${model.id}`}>
                  <td>
                    <span className="strong mono">{model.displayName}</span>
                  </td>
                  <td>{providerName(config, model.providerId)}</td>
                  <td>{provider ? protocolLabel(provider.protocol) : '-'}</td>
                  <td>
                    <span className="model-price">
                      <span className="mono">{priceText(model.inputPricePerMtok)}</span>
                      <small>{priceSourceText(model)}</small>
                    </span>
                  </td>
                  <td>
                    <span className="model-price">
                      <span className="mono">{priceText(model.outputPricePerMtok)}</span>
                      <small>{priceSourceText(model)}</small>
                    </span>
                  </td>
                  <td>
                    <span className="faint">
                      {model.contextWindow ? `${(model.contextWindow / 1000).toFixed(0)}k` : '暂无'}
                    </span>
                  </td>
                  <td>
                    <span className="faint">
                      {model.capabilities && model.capabilities.length > 0
                        ? model.capabilities.join(', ')
                        : '暂无'}
                    </span>
                  </td>
                  <td className="models-enabled-cell">
                    <label className="switch">
                      <input
                        type="checkbox"
                        checked={model.enabled}
                        onChange={() => {
                          void saveModelConfig({ ...model, enabled: !model.enabled })
                            .then(onConfig)
                            .catch((err: unknown) =>
                              onNotice(errorMessage(err, '保存模型启用状态失败')),
                            );
                        }}
                      />
                      <i />
                    </label>
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>
      <p className="providers-note">
        价格按每百万 token 计，随服务商不同而不同。测试可达性后 Helm 可从各服务商接口同步目录。
      </p>
    </>
  );
}
