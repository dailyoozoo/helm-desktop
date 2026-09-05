import type {
  AppConfig,
  BindingConfig,
  EngineConfig,
  ModelConfig,
  ProviderConfig,
} from '../providers/api';
import type { SessionSummary } from '../sessions/sessionTypes';
import { compatibleProvidersForEngine, sessionModelOptions } from '../providers/providerViewModel';
import type { EngineId } from '@helm/protocol';

export interface WorkspaceEngineOption {
  engine: EngineConfig;
  binding?: BindingConfig;
  provider?: ProviderConfig;
  models: ModelConfig[];
}

export function sameWorkspacePath(left: string, right: string): boolean {
  const normalize = (value: string) => {
    let normalized = value.replace(/\\/g, '/').replace(/^\/\/\?\//, '');
    if (/^unc\//i.test(normalized)) normalized = `//${normalized.slice(4)}`;
    return normalized.replace(/\/+$/, '').toLocaleLowerCase();
  };
  return normalize(left) === normalize(right);
}

export function workspaceEngineOptions(config: AppConfig): WorkspaceEngineOption[] {
  return config.engines.map((engine) => {
    const binding = config.bindings.find((item) => item.engineId === engine.id);
    const provider =
      config.providers.find((item) => item.id === binding?.providerId) ??
      compatibleProvidersForEngine(config, engine.id)[0];
    return {
      engine,
      binding,
      provider,
      models: provider ? sessionModelOptions(config, provider.id) : [],
    };
  });
}

export function defaultModelForEngine(config: AppConfig, engineId: EngineId): string {
  const binding = config.bindings.find((item) => item.engineId === engineId);
  if (binding?.primaryModel) return binding.primaryModel;
  return (
    workspaceEngineOptions(config).find((option) => option.engine.id === engineId)?.models[0]?.id ??
    ''
  );
}

export function workspaceSessionIsActive(
  session: Pick<SessionSummary, 'id' | 'cliSessionId'>,
  ids: { historyId?: string | null; handleId?: string | null; cliSessionId?: string | null },
): boolean {
  return Boolean(
    (ids.historyId && session.id === ids.historyId) ||
    (ids.handleId && session.id === ids.handleId) ||
    (ids.cliSessionId && session.cliSessionId === ids.cliSessionId),
  );
}
