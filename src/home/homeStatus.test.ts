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
    expect(status.readyProviderCount).toBe(1);
    expect(status.sessionCount).toBe(4);
    expect(status.monthCostText).toBe('$12.34');
    expect(status.engines[1].state).toBe('未安装');
    expect(status.providers[1].state).toBe('未验证');
    expect(status.consoleProviders[0].access).toBe('api');
  });

  it('uses authoritative CLI subscription login instead of HTTP lastTest', () => {
    const status = buildHomeStatus(
      {
        engines: [],
        providers: [
          {
            id: 'subscription-ok',
            name: 'Subscription',
            kind: 'subscription',
            ready: true,
            lastTest: null,
            login: { state: 'ok', authMethod: 'subscription' },
          },
          {
            id: 'subscription-api-key',
            name: 'Wrong auth',
            kind: 'subscription',
            ready: true,
            lastTest: { result: 'ok' },
            login: { state: 'ok', authMethod: 'apikey' },
          },
        ],
      },
      0,
      0,
    );

    expect(status.readyProviderCount).toBe(1);
    expect(status.providers[0].state).toBe('已就绪');
    expect(status.providers[1].state).toBe('登录方式不符');
  });

  it('distinguishes a failed login probe from a confirmed missing login', () => {
    const status = buildHomeStatus(
      {
        engines: [],
        providers: [
          {
            id: 'probe-failed',
            name: 'Probe failed',
            kind: 'subscription',
            ready: true,
            login: { state: 'unknown', authMethod: 'unknown' },
          },
          {
            id: 'not-logged-in',
            name: 'Not logged in',
            kind: 'subscription',
            ready: true,
            login: { state: 'missing', authMethod: 'unknown' },
          },
        ],
      },
      0,
      0,
    );

    expect(status.providers[0].state).toBe('检测失败');
    expect(status.providers[1].state).toBe('未登录');
  });

  it('keeps the prototype two-column status grid compact while retaining console detail', () => {
    const status = buildHomeStatus(
      {
        engines: [],
        providers: [
          { id: 'a', name: 'A', ready: true, lastTest: { result: 'ok' } },
          { id: 'b', name: 'B', ready: true, lastTest: { result: 'ok' } },
          { id: 'c', name: 'C', ready: true, lastTest: { result: 'ok' } },
        ],
      },
      0,
      0,
    );

    expect(status.providers).toHaveLength(2);
    expect(status.providers[1].name).toBe('B + C');
    expect(status.consoleProviders).toHaveLength(3);
  });
});
