import { describe, expect, it } from 'vitest';
import { buildHomeStatus } from './homeStatus';

describe('buildHomeStatus', () => {
  it('derives dashboard counts only from real readiness and test results', () => {
    const status = buildHomeStatus(
      {
        engines: [
          { id: 'claude-code', name: 'Claude Code', status: 'ready', version: '2.1.206' },
          { id: 'codex', name: 'Codex', status: 'missing', version: null },
        ],
        providers: [
          { id: 'a', name: 'A', ready: true, lastTest: { result: 'ok' } },
          { id: 'b', name: 'B', ready: true, lastTest: { result: 'unverified' } },
        ],
      },
      4,
      12.34,
    );

    expect(status.readyEngineCount).toBe(1);
    expect(status.connectedProviderCount).toBe(1);
    expect(status.sessionCount).toBe(4);
    expect(status.monthCostText).toBe('$12.34');
    expect(status.engines[1].state).toBe('未安装');
    expect(status.providers[1].state).toBe('未验证');
  });
});
