import { describe, expect, it } from 'vitest';
import { DEFAULT_SETTINGS } from './types';
import { appAppearanceAttributes, cssVariablesForAppearance } from './appearance';

describe('settings appearance helpers', () => {
  it('maps appearance settings to app-level data attributes', () => {
    expect(
      appAppearanceAttributes({
        ...DEFAULT_SETTINGS.appearance,
        theme: 'dark',
        uiDensity: 'compact',
        reduceMotion: true,
      }),
    ).toEqual({
      theme: 'dark',
      density: 'compact',
      motion: 'reduced',
      monospaceFont: 'JetBrains Mono',
    });
  });

  it('maps accent and font settings to CSS variables', () => {
    expect(
      cssVariablesForAppearance({
        ...DEFAULT_SETTINGS.appearance,
        accentColor: { base: 'oklch(53% 0.14 162)', hi: 'oklch(47% 0.15 162)' },
        monospaceFont: 'Cascadia Code',
      }),
    ).toEqual({
      '--accent': 'oklch(53% 0.14 162)',
      '--accent-hi': 'oklch(47% 0.15 162)',
      '--accent-soft': 'color-mix(in oklch, oklch(53% 0.14 162) 12%, transparent)',
      '--accent-line': 'color-mix(in oklch, oklch(53% 0.14 162) 38%, transparent)',
      '--font-mono':
        "'Cascadia Code', 'JetBrains Mono', 'SF Mono', ui-monospace, 'Segoe UI Mono', Menlo, Consolas, monospace",
    });
  });
});
