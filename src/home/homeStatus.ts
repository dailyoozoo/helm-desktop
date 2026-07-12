interface HomeEngine {
  id: string;
  name: string;
  status: 'ready' | 'missing' | 'error';
  version?: string | null;
}

interface HomeProvider {
  id: string;
  name: string;
  ready: boolean;
  lastTest?: { result: 'ok' | 'fail' | 'unverified' } | null;
}

interface HomeConfig {
  engines: HomeEngine[];
  providers: HomeProvider[];
}

export function buildHomeStatus(config: HomeConfig, sessionCount: number, monthCost: number) {
  return {
    readyEngineCount: config.engines.filter((engine) => engine.status === 'ready').length,
    connectedProviderCount: config.providers.filter(
      (provider) => provider.ready && provider.lastTest?.result === 'ok',
    ).length,
    sessionCount,
    monthCostText: `$${monthCost.toFixed(2)}`,
    engines: config.engines.map((engine) => ({
      id: engine.id,
      name: engine.name,
      detail: engine.version || engine.id,
      state: engine.status === 'ready' ? '就绪' : engine.status === 'missing' ? '未安装' : '异常',
      ok: engine.status === 'ready',
    })),
    providers: config.providers.map((provider) => ({
      id: provider.id,
      name: provider.name,
      detail: provider.ready ? '配置就绪' : '待配置',
      state:
        provider.lastTest?.result === 'ok'
          ? '已连接'
          : provider.lastTest?.result === 'unverified'
            ? '未验证'
            : provider.lastTest?.result === 'fail'
              ? '失败'
              : '待测试',
      ok: provider.lastTest?.result === 'ok',
    })),
  };
}

export type HomeStatus = ReturnType<typeof buildHomeStatus>;
