import type { AppSettings } from './types';

type AppearanceSettings = AppSettings['appearance'];

export function appAppearanceAttributes(appearance: AppearanceSettings) {
  return {
    theme: appearance.theme,
    density: appearance.uiDensity,
    motion: appearance.reduceMotion ? 'reduced' : 'normal',
    monospaceFont: appearance.monospaceFont,
  };
}

export function cssVariablesForAppearance(appearance: AppearanceSettings): Record<string, string> {
  const base = appearance.accentColor.base;
  return {
    '--accent': base,
    '--accent-hi': appearance.accentColor.hi,
    '--accent-soft': `color-mix(in oklch, ${base} 12%, transparent)`,
    '--accent-line': `color-mix(in oklch, ${base} 38%, transparent)`,
    '--font-mono': `'${appearance.monospaceFont}', 'JetBrains Mono', 'SF Mono', ui-monospace, 'Segoe UI Mono', Menlo, Consolas, monospace`,
  };
}

export function applyAppearanceSettings(
  appearance: AppearanceSettings,
  root: HTMLElement = document.documentElement,
) {
  const attrs = appAppearanceAttributes(appearance);
  root.dataset.theme = attrs.theme;
  root.dataset.density = attrs.density;
  root.dataset.motion = attrs.motion;
  root.dataset.monospaceFont = attrs.monospaceFont;

  for (const [name, value] of Object.entries(cssVariablesForAppearance(appearance))) {
    root.style.setProperty(name, value);
  }
}
