import { describe, expect, it, vi } from 'vitest';
import { applyExtensionLoadResult } from './extensionLoadState';

describe('extension initial load state', () => {
  it('records rejected sources instead of presenting them as empty data', () => {
    const setter = vi.fn();
    const failures: string[] = [];
    const consoleError = vi.spyOn(console, 'error').mockImplementation(() => undefined);

    applyExtensionLoadResult(
      { status: 'rejected', reason: new Error('settings.json is invalid') },
      setter,
      'MCP 服务器',
      failures,
    );

    expect(setter).not.toHaveBeenCalled();
    expect(failures).toEqual(['MCP 服务器']);
    consoleError.mockRestore();
  });
});
