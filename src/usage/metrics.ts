import type { CliLoginState, ProviderConfig } from '../providers/api';
import { providerRuntimeReady } from '../providers/providerViewModel';
import type { ProviderUsage } from './api';

export function percentageChange(current: number, previous: number): number | null {
  if (previous === 0) return current === 0 ? 0 : null;
  return ((current - previous) / previous) * 100;
}

export function comparisonText(current: number, previous: number, label = '较前一期'): string {
  const change = percentageChange(current, previous);
  if (change === null) return `${label}无基数`;
  if (Math.abs(change) < 0.5) return `${label}持平`;
  return `${label} ${change > 0 ? '+' : ''}${Math.round(change)}%`;
}

export function projectedMonthEndCost(currentMonthCost: number, now = new Date()): number {
  const day = Math.max(1, now.getDate());
  const daysInMonth = new Date(now.getFullYear(), now.getMonth() + 1, 0).getDate();
  return (currentMonthCost / day) * daysInMonth;
}

export function createRequestGate() {
  let current = 0;
  return {
    begin: () => ++current,
    isCurrent: (generation: number) => generation === current,
  };
}

export interface ProviderUsageRow extends ProviderUsage {
  name: string;
  kind: ProviderConfig['kind'] | 'unknown';
  ready: boolean;
}

export function mergeProviderUsage(
  providers: readonly ProviderConfig[],
  usage: readonly ProviderUsage[],
  loginByProvider: Readonly<Record<string, CliLoginState | null>> = {},
): ProviderUsageRow[] {
  const providerById = new Map(providers.map((provider) => [provider.id, provider]));
  const rows = usage.map((item) => {
    const provider = providerById.get(item.provider);
    return {
      ...item,
      name: provider?.name ?? (item.provider ? item.provider : '未标注（旧会话）'),
      kind: provider?.kind ?? 'unknown',
      ready: provider
        ? providerRuntimeReady(provider, loginByProvider[provider.id] ?? null)
        : false,
    } satisfies ProviderUsageRow;
  });
  const usedIds = new Set(usage.map((item) => item.provider));
  for (const provider of providers) {
    const ready = providerRuntimeReady(provider, loginByProvider[provider.id] ?? null);
    if (!ready || usedIds.has(provider.id)) continue;
    rows.push({
      provider: provider.id,
      name: provider.name,
      kind: provider.kind,
      ready: true,
      cost_usd: 0,
      share: 0,
    });
  }
  return rows;
}
