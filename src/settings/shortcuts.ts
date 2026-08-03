import type { PageId } from '../shell/Rail';
import { DEFAULT_SETTINGS, type AppSettings } from './types';

type ShortcutEvent = Pick<KeyboardEvent, 'key' | 'ctrlKey' | 'metaKey' | 'altKey' | 'shiftKey'>;

export type NavigationPrefix = string | null;
export type AppShortcutAction =
  | 'open-command-palette'
  | 'new-session'
  | 'toggle-context'
  | 'cycle-engine';

export interface AppShortcutResult {
  action: AppShortcutAction | null;
  page: PageId | null;
  prefix: NavigationPrefix;
}

const ACTION_SHORTCUTS: Array<{
  key: keyof AppSettings['shortcuts'];
  action: AppShortcutAction;
}> = [
  { key: 'commandPalette', action: 'open-command-palette' },
  { key: 'newSession', action: 'new-session' },
  { key: 'toggleContext', action: 'toggle-context' },
  { key: 'cycleEngine', action: 'cycle-engine' },
];

const PAGE_SHORTCUTS: Array<{
  key: keyof AppSettings['shortcuts'];
  page: PageId;
}> = [
  { key: 'home', page: 'home' },
  { key: 'workspace', page: 'workspace' },
  { key: 'providers', page: 'providers' },
  { key: 'sessions', page: 'sessions' },
  { key: 'extensions', page: 'extensions' },
  { key: 'usage', page: 'usage' },
  { key: 'settings', page: 'settings' },
];

interface ParsedShortcut {
  key: string;
  ctrl: boolean;
  alt: boolean;
  shift: boolean;
  meta: boolean;
}

function parseShortcut(shortcut: string): ParsedShortcut | null {
  const parts = shortcut
    .split('+')
    .map((part) => part.trim())
    .filter(Boolean);
  const key = parts.pop();
  if (!key) return null;
  const modifiers = new Set(parts.map((part) => part.toLowerCase()));
  return {
    key: key.toLowerCase(),
    ctrl: modifiers.has('ctrl'),
    alt: modifiers.has('alt'),
    shift: modifiers.has('shift'),
    meta: modifiers.has('meta'),
  };
}

function shortcutMatches(event: ShortcutEvent, shortcut: string): boolean {
  const parsed = parseShortcut(shortcut);
  if (!parsed) return false;
  const ctrlOrMeta = event.ctrlKey || event.metaKey;
  if (parsed.ctrl !== ctrlOrMeta) return false;
  if (parsed.meta && !event.metaKey) return false;
  if (parsed.alt !== event.altKey) return false;
  if (parsed.shift !== event.shiftKey) return false;
  return event.key.toLowerCase() === parsed.key;
}

export function shortcutLabel(shortcut: string): string[] {
  return shortcut
    .split('+')
    .map((part) => part.trim())
    .filter(Boolean);
}

export function shortcutFromKeyboardEvent(event: ShortcutEvent): string | null {
  if (['Control', 'Shift', 'Alt', 'Meta'].includes(event.key)) return null;
  const key =
    event.key === ' '
      ? 'Space'
      : event.key.length === 1
        ? event.key.toUpperCase()
        : event.key === 'Esc'
          ? 'Escape'
          : event.key;
  const parts: string[] = [];
  if (event.ctrlKey || event.metaKey) parts.push('Ctrl');
  if (event.altKey) parts.push('Alt');
  if (event.shiftKey) parts.push('Shift');
  parts.push(key);
  return parts.join('+');
}

export function matchesAppShortcut(
  event: ShortcutEvent,
  action: AppShortcutAction,
  shortcuts: AppSettings['shortcuts'] = DEFAULT_SETTINGS.shortcuts,
): boolean {
  const entry = ACTION_SHORTCUTS.find((item) => item.action === action);
  return entry ? shortcutMatches(event, shortcuts[entry.key]) : false;
}

function normalizedPrefix(shortcuts: AppSettings['shortcuts']): string {
  return shortcuts.navigationPrefix.trim().toLowerCase();
}

export function reduceAppShortcut(
  prefix: NavigationPrefix,
  event: ShortcutEvent,
  shortcuts: AppSettings['shortcuts'] = DEFAULT_SETTINGS.shortcuts,
): AppShortcutResult {
  for (const item of ACTION_SHORTCUTS) {
    if (shortcutMatches(event, shortcuts[item.key])) {
      return { action: item.action, page: null, prefix: null };
    }
  }

  if (event.ctrlKey || event.metaKey || event.altKey || event.shiftKey) {
    return { action: null, page: null, prefix: null };
  }

  const key = event.key.toLowerCase();
  const prefixKey = normalizedPrefix(shortcuts);
  if (prefix === prefixKey) {
    const destination = PAGE_SHORTCUTS.find((item) => shortcutMatches(event, shortcuts[item.key]));
    return { action: null, page: destination?.page ?? null, prefix: null };
  }

  if (prefixKey && key === prefixKey) {
    return { action: null, page: null, prefix: prefixKey };
  }

  return { action: null, page: null, prefix: null };
}

export function shouldIgnoreNavigationShortcut(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  if (target.isContentEditable) return true;
  return ['INPUT', 'TEXTAREA', 'SELECT'].includes(target.tagName);
}
