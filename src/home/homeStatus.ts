interface HomeEngine {
  id: string;
  name: string;
  status: 'ready' | 'missing' | 'error';
  version?: string | null;
}

interface HomeProvider {
  id: string;
  name: string;
  kind?: 'subscription' | 'api' | 'local';
  ready: boolean;
  lastTest?: { result: 'ok' | 'fail' | 'unverified' } | null;
  login?: { state: 'ok' | 'missing' | 'expired' | 'unknown'; authMethod?: string | null } | null;
}

interface HomeConfig {
  engines: HomeEngine[];
  providers: HomeProvider[];
}

export function buildHomeStatus(config: HomeConfig, sessionCount: number, monthCost: number) {
  const providerRows = config.providers.map((provider) => {
    const subscriptionReady =
      provider.kind === 'subscription' &&
      provider.ready &&
      provider.login?.state === 'ok' &&
      provider.login.authMethod === 'subscription';
    const apiReady =
      provider.kind !== 'subscription' && provider.ready && provider.lastTest?.result === 'ok';
    const ok = subscriptionReady || apiReady;
    const state = ok
      ? '已就绪'
      : provider.kind === 'subscription'
        ? provider.login?.state === 'expired'
          ? '登录失效'
          : provider.login?.state === 'ok'
            ? '登录方式不符'
            : provider.login?.state === 'unknown'
              ? '检测失败'
              : '未登录'
        : provider.lastTest?.result === 'unverified'
          ? '未验证'
          : provider.lastTest?.result === 'fail'
            ? '失败'
            : '待测试';
    return {
      id: provider.id,
      name: provider.name,
      detail: provider.ready ? '配置就绪' : '待配置',
      state,
      ok,
    };
  });
  const providers =
    providerRows.length > 2
      ? [
          providerRows[0],
          {
            id: providerRows
              .slice(1)
              .map((provider) => provider.id)
              .join('+'),
            name: providerRows
              .slice(1)
              .map((provider) => provider.name)
              .join(' + '),
            detail: `${providerRows.length - 1} 个服务商已配置`,
            state: providerRows.slice(1).every((provider) => provider.ok) ? '已就绪' : '需检查',
            ok: providerRows.slice(1).every((provider) => provider.ok),
          },
        ]
      : providerRows;

  return {
    readyEngineCount: config.engines.filter((engine) => engine.status === 'ready').length,
    readyProviderCount: providerRows.filter((provider) => provider.ok).length,
    sessionCount,
    monthCostText: `$${monthCost.toFixed(2)}`,
    engines: config.engines.map((engine) => ({
      id: engine.id,
      name: engine.name,
      detail: engine.version || engine.id,
      state: engine.status === 'ready' ? '就绪' : engine.status === 'missing' ? '未安装' : '异常',
      ok: engine.status === 'ready',
    })),
    providers,
    consoleProviders: config.providers.map((provider) => ({
      id: provider.id,
      name: provider.name,
      access: provider.kind === 'subscription' ? '订阅' : provider.kind || 'api',
      state: providerRows.find((row) => row.id === provider.id)?.state ?? '需检查',
      ok: providerRows.find((row) => row.id === provider.id)?.ok ?? false,
    })),
  };
}

export type HomeStatus = ReturnType<typeof buildHomeStatus>;
