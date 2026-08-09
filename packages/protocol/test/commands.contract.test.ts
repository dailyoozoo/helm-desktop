import { existsSync, readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';
import type {
  CreateSessionArgs,
  SendMessageArgs,
  SetSessionPermissionProfileArgs,
  SetSessionTurnPreferenceArgs,
  TurnMode,
} from '@helm/protocol';
import { isAgentEvent } from '@helm/protocol';

const root = resolve(import.meta.dirname, '../../..');

describe('Tauri command contract', () => {
  it('keeps shared TypeScript payloads aligned with Rust signatures and documentation', () => {
    const mode: TurnMode = 'plan';
    const create = {
      engine: 'codex',
      model: 'gpt-test',
      cwd: 'C:/workspace',
      mode,
      permissionProfile: 'auto',
    } satisfies CreateSessionArgs;
    const send = {
      handleId: 'handle-test',
      text: 'hello',
      displayText: 'hello',
      mode,
      model: 'gpt-test',
      reasoningEffort: 'auto',
    } satisfies SendMessageArgs;
    const profile = {
      handleId: 'handle-test',
      profile: 'full_access',
    } satisfies SetSessionPermissionProfileArgs;
    const preference = {
      handleId: 'handle-test',
      model: 'gpt-next',
      reasoningEffort: 'high',
    } satisfies SetSessionTurnPreferenceArgs;
    expect({ create, send, profile, preference }).toBeTruthy();

    const rust = readFileSync(resolve(root, 'src-tauri/src/commands.rs'), 'utf8');
    expect(rust).toMatch(
      /pub async fn create_session[\s\S]*mode: Option<String>[\s\S]*permission_profile: Option<String>/,
    );
    expect(rust).toMatch(
      /pub async fn send_message[\s\S]*handle_id: String[\s\S]*display_text: Option<String>[\s\S]*mode: Option<String>[\s\S]*model: Option<String>[\s\S]*reasoning_effort: Option<String>/,
    );
    expect(rust).toMatch(
      /pub async fn set_session_permission_profile[\s\S]*handle_id: String[\s\S]*profile: String/,
    );
    expect(rust).toMatch(
      /pub async fn set_session_turn_preference[\s\S]*handle_id: String[\s\S]*model: String[\s\S]*reasoning_effort: Option<String>/,
    );

    const docsPath = resolve(root, 'docs/技术方案.md');
    if (existsSync(docsPath)) {
      const docs = readFileSync(docsPath, 'utf8');
      expect(docs).toContain("export type TurnMode = 'build' | 'plan' | 'ask'");
      expect(docs).toMatch(/create_session[^\n]*permissionProfile\?: PermissionProfile/);
      expect(docs).toMatch(
        /send_message[\s\S]*mode\?: TurnMode[\s\S]*permissionProfile\?: PermissionProfile/,
      );
    }
  });

  it('keeps frontend invoke names and backend registrations in exact sync', () => {
    const lib = readFileSync(resolve(root, 'src-tauri/src/lib.rs'), 'utf8');
    const handler = lib.match(/generate_handler!\[([\s\S]*?)\]\)/)?.[1] ?? '';
    const registered = new Set(
      [...handler.matchAll(/^\s*([A-Za-z_][A-Za-z0-9_:]*)\s*,?\s*$/gm)].map(
        (match) => match[1].split('::').at(-1)!,
      ),
    );
    const frontend = [
      'src/engine/transport.ts',
      'src/extensions/extensionsApi.ts',
      'src/providers/api.ts',
      'src/sessions/api.ts',
      'src/settings/api.ts',
      'src/usage/api.ts',
      'src/workspace/workspaceApi.ts',
    ]
      .flatMap((path) => [
        ...readFileSync(resolve(root, path), 'utf8').matchAll(
          /invoke(?:<[^;()]*?>)?\(\s*['"]([A-Za-z0-9_]+)['"]/g,
        ),
      ])
      .map((match) => match[1]);
    expect([...registered].sort()).toEqual([...new Set(frontend)].sort());
  });

  it('keeps visual audit fixtures out of the production entry', () => {
    const index = readFileSync(resolve(root, 'index.html'), 'utf8');
    const vite = readFileSync(resolve(root, 'vite.config.ts'), 'utf8');
    expect(index).not.toContain('visualAuditMain');
    expect(vite).toContain("input: fileURLToPath(new URL('./index.html'");
  });

  it('accepts future backend string values without weakening structural validation', () => {
    expect(
      isAgentEvent({
        type: 'token_usage',
        sessionId: 'session-test',
        inputTokens: 1,
        outputTokens: 1,
        costUsd: 0,
        serviceTier: 'future-tier',
      }),
    ).toBe(true);
    expect(
      isAgentEvent({
        type: 'error',
        message: 'future error',
        recoverable: false,
        kind: 'future_error_kind',
      }),
    ).toBe(true);
    expect(
      isAgentEvent({ type: 'error', message: 'invalid kind', recoverable: false, kind: 42 }),
    ).toBe(false);
  });
});
