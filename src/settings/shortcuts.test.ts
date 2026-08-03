import { describe, expect, it } from 'vitest';
import { reduceAppShortcut, shortcutFromKeyboardEvent } from './shortcuts';
import { DEFAULT_SETTINGS } from './types';

const key = (value: string, ctrlKey = false, shiftKey = false) => ({
  key: value,
  ctrlKey,
  metaKey: false,
  altKey: false,
  shiftKey,
});

describe('settings navigation shortcuts', () => {
  it('records Windows-style shortcut labels and ignores modifier-only presses', () => {
    expect(shortcutFromKeyboardEvent(key('k', true))).toBe('Ctrl+K');
    expect(shortcutFromKeyboardEvent(key('.', true, true))).toBe('Ctrl+Shift+.');
    expect(shortcutFromKeyboardEvent(key('Control', true))).toBeNull();
  });
  it('uses G as a navigation prefix for workspace and session history', () => {
    const prefix = reduceAppShortcut(null, key('g'));
    expect(prefix).toEqual({ action: null, page: null, prefix: 'g' });

    expect(reduceAppShortcut(prefix.prefix, key('w'))).toEqual({
      action: null,
      page: 'workspace',
      prefix: null,
    });
    expect(reduceAppShortcut('g', key('s'))).toEqual({
      action: null,
      page: 'sessions',
      prefix: null,
    });
  });

  it('maps the remaining shell pages without requiring the mouse', () => {
    expect(reduceAppShortcut('g', key('h'))).toEqual({ action: null, page: 'home', prefix: null });
    expect(reduceAppShortcut('g', key('p'))).toEqual({
      action: null,
      page: 'providers',
      prefix: null,
    });
    expect(reduceAppShortcut('g', key('e'))).toEqual({
      action: null,
      page: 'extensions',
      prefix: null,
    });
    expect(reduceAppShortcut('g', key('u'))).toEqual({ action: null, page: 'usage', prefix: null });
    expect(reduceAppShortcut('g', key(','))).toEqual({
      action: null,
      page: 'settings',
      prefix: null,
    });
  });

  it('clears the prefix when the second key is not a known destination', () => {
    expect(reduceAppShortcut('g', key('z'))).toEqual({ action: null, page: null, prefix: null });
    expect(reduceAppShortcut('g', key('Escape'))).toEqual({
      action: null,
      page: null,
      prefix: null,
    });
  });

  it('maps Ctrl shortcuts to real app actions', () => {
    expect(reduceAppShortcut(null, key('k', true))).toEqual({
      action: 'open-command-palette',
      page: null,
      prefix: null,
    });
    expect(reduceAppShortcut(null, key('n', true))).toEqual({
      action: 'new-session',
      page: null,
      prefix: null,
    });
    expect(reduceAppShortcut(null, key('.', true))).toEqual({
      action: 'toggle-context',
      page: null,
      prefix: null,
    });
    expect(reduceAppShortcut(null, key('e', true))).toEqual({
      action: 'cycle-engine',
      page: null,
      prefix: null,
    });
  });

  it('does not treat modified G navigation keys as navigation', () => {
    expect(reduceAppShortcut(null, key('g', true))).toEqual({
      action: null,
      page: null,
      prefix: null,
    });
  });

  it('uses customized action shortcuts from settings', () => {
    const shortcuts = {
      ...DEFAULT_SETTINGS.shortcuts,
      newSession: 'Ctrl+Shift+N',
    };

    expect(reduceAppShortcut(null, key('n', true), shortcuts)).toEqual({
      action: null,
      page: null,
      prefix: null,
    });
    expect(reduceAppShortcut(null, key('N', true, true), shortcuts)).toEqual({
      action: 'new-session',
      page: null,
      prefix: null,
    });
  });

  it('uses customized navigation prefix and page keys from settings', () => {
    const shortcuts = {
      ...DEFAULT_SETTINGS.shortcuts,
      navigationPrefix: 'J',
      workspace: 'D',
    };

    const prefix = reduceAppShortcut(null, key('j'), shortcuts);
    expect(prefix).toEqual({ action: null, page: null, prefix: 'j' });
    expect(reduceAppShortcut(prefix.prefix, key('d'), shortcuts)).toEqual({
      action: null,
      page: 'workspace',
      prefix: null,
    });
  });
});
