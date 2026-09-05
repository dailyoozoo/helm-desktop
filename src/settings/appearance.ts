import type { AppSettings } from './types';

type AppearanceSettings = AppSettings['appearance'];

export function appAppearanceAttributes(appearance: AppearanceSettings) {
  return {
    theme: appearance.theme,
  };
}

export function cssVariablesForAppearance(appearance: AppearanceSettings): Record<string, string> {
  const base = appearance.accentColor.base;
  return {
    '--accent': base,
    '--accent-hi': appearance.accentColor.hi,
    '--accent-soft': `color-mix(in oklch, ${base} 12%, transparent)`,
    '--accent-line': `color-mix(in oklch, ${base} 38%, transparent)`,
  };
}

export function applyAppearanceSettings(
  appearance: AppearanceSettings,
  root: HTMLElement = document.documentElement,
) {
  const attrs = appAppearanceAttributes(appearance);
  root.dataset.theme = attrs.theme;

  for (const [name, value] of Object.entries(cssVariablesForAppearance(appearance))) {
    root.style.setProperty(name, value);
  }
}
