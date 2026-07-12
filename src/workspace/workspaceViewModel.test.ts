import { describe, expect, it } from 'vitest';
import type { AppConfig } from '../providers/api';
import {
  defaultModelForEngine,
  workspaceEngineOptions,
  workspaceSessionIsActive,
} from './workspaceViewModel';

const config: AppConfig = {
  defaultEngine: 'claude-code',
  defaultModel: '',
  providers: [
    {
      id: 'anthropic',
      name: 'Anthropic',
      baseUrl: 'https://api.anthropic.com',
      keyRef: null,
      ready: true,
      lastTest: null,
      protocol: 'anthropic',
      authMethod: 'apikey',
    },
    {
      id: 'openai',
      name: 'OpenAI',
      baseUrl: 'https://api.openai.com/v1',
      keyRef: null,
      ready: true,
      lastTest: null,
      protocol: 'openai-responses',
      authMethod: 'apikey',
    },
  ],
  models: [
    {
      id: 'claude-sonnet-4.6',
      providerId: 'anthropic',
      displayName: 'claude-sonnet-4.6',
      inputPricePerMtok: 3,
      outputPricePerMtok: 15,
      enabled: true,
    },
    {
      id: 'gpt-5-codex',
      providerId: 'openai',
      displayName: 'gpt-5-codex',
      inputPricePerMtok: 1.25,
      outputPricePerMtok: 10,
      enabled: true,
    },
  ],
  engines: [
    {
      id: 'claude-code',
      name: 'Claude Code',
      bin: 'claude',
      defaultModel: '',
      status: 'ready',
      version: null,
    },
    {
      id: 'codex',
      name: 'Codex',
      bin: 'codex',
      defaultModel: '',
      status: 'ready',
      version: null,
    },
  ],
  bindings: [
    {
      engineId: 'claude-code',
      providerId: 'anthropic',
      primaryModel: 'claude-sonnet-4.6',
      fastModel: null,
    },
    {
      engineId: 'codex',
      providerId: 'openai',
      primaryModel: 'gpt-5-codex',
      fastModel: null,
    },
  ],
};

describe('workspace view model', () => {
  it('uses engine bindings to build new-session choices', () => {
    const options = workspaceEngineOptions(config);

    expect(
      options.map((option) => [
        option.engine.id,
        option.provider?.id,
        option.binding?.primaryModel,
      ]),
    ).toEqual([
      ['claude-code', 'anthropic', 'claude-sonnet-4.6'],
      ['codex', 'openai', 'gpt-5-codex'],
    ]);
  });

  it('returns the bound model as the default model per engine', () => {
    expect(defaultModelForEngine(config, 'claude-code')).toBe('claude-sonnet-4.6');
    expect(defaultModelForEngine(config, 'codex')).toBe('gpt-5-codex');
  });

  it('matches workspace sessions by local handle id or attached cli session id', () => {
    const session = {
      id: 's-1',
      cliSessionId: 'claude-real-1',
    };

    expect(workspaceSessionIsActive(session, { handleId: 's-1', cliSessionId: null })).toBe(true);
    expect(
      workspaceSessionIsActive(session, { handleId: 's-2', cliSessionId: 'claude-real-1' }),
    ).toBe(true);
    expect(workspaceSessionIsActive(session, { handleId: 's-2', cliSessionId: 'other' })).toBe(
      false,
    );
  });
});
