import { describe, expect, it } from 'vitest';
import { reduceAppShortcut, reduceNavigationShortcut } from './shortcuts';
import { DEFAULT_SETTINGS } from './types';

const key = (value: string, ctrlKey = false, shiftKey = false) => ({
  key: value,
  ctrlKey,
  metaKey: false,
  altKey: false,
  shiftKey,
});

describe('settings navigation shortcuts', () => {
  it('uses G as a navigation prefix for workspace and session history', () => {
    const prefix = reduceNavigationShortcut(null, key('g'));
    expect(prefix).toEqual({ page: null, prefix: 'g' });

    expect(reduceNavigationShortcut(prefix.prefix, key('w'))).toEqual({
      page: 'workspace',
      prefix: null,
    });
    expect(reduceNavigationShortcut('g', key('s'))).toEqual({
      page: 'sessions',
      prefix: null,
    });
  });

  it('maps the remaining shell pages without requiring the mouse', () => {
    expect(reduceNavigationShortcut('g', key('h'))).toEqual({ page: 'home', prefix: null });
    expect(reduceNavigationShortcut('g', key('p'))).toEqual({ page: 'providers', prefix: null });
    expect(reduceNavigationShortcut('g', key('x'))).toEqual({ page: 'extensions', prefix: null });
    expect(reduceNavigationShortcut('g', key('u'))).toEqual({ page: 'usage', prefix: null });
    expect(reduceNavigationShortcut('g', key(','))).toEqual({ page: 'settings', prefix: null });
  });

  it('clears the prefix when the second key is not a known destination', () => {
    expect(reduceNavigationShortcut('g', key('z'))).toEqual({ page: null, prefix: null });
    expect(reduceNavigationShortcut('g', key('Escape'))).toEqual({ page: null, prefix: null });
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

    const prefix = reduceNavigationShortcut(null, key('j'), shortcuts);
    expect(prefix).toEqual({ page: null, prefix: 'j' });
    expect(reduceNavigationShortcut(prefix.prefix, key('d'), shortcuts)).toEqual({
      page: 'workspace',
      prefix: null,
    });
  });
});
