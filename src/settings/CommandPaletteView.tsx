import { useEffect, useMemo, useRef, useState } from 'react';
import type { AppConfig } from '../providers/api';
import { getProviderConfig } from '../providers/api';
import { listSessions } from '../sessions/api';
import { Icon } from '../shell/icons';
import {
  filterCommandPaletteCommands,
  filterProviders,
  paletteCommands,
  providerToCommand,
  sessionToCommand,
  type CommandPaletteCommand,
} from './commandPalette';
import { filterSessions } from '../sessions/sessionViewModel';

export function CommandPaletteView({
  open,
  onClose,
  onRun,
}: {
  open: boolean;
  onClose: () => void;
  onRun: (command: CommandPaletteCommand) => void;
}) {
  const [query, setQuery] = useState('');
  const [active, setActive] = useState(0);
  const [sessions, setSessions] = useState<Awaited<ReturnType<typeof listSessions>>>([]);
  const [providers, setProviders] = useState<AppConfig['providers']>([]);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (!open) return;
    setQuery('');
    setActive(0);
    window.setTimeout(() => inputRef.current?.focus(), 20);
  }, [open]);

  useEffect(() => {
    if (!open) return;
    let active = true;
    listSessions()
      .then((next) => {
        if (active) setSessions(next);
      })
      .catch(() => {
        // 豁免提示：面板要保持轻量，会话源失败只降级为"搜不到会话"，静态命令仍可用
        if (active) setSessions([]);
      });
    getProviderConfig()
      .then((next) => {
        if (active) setProviders(next.providers);
      })
      .catch(() => {
        // 豁免提示：同上，服务商源失败只影响服务商搜索结果
        if (active) setProviders([]);
      });
    return () => {
      active = false;
    };
  }, [open]);

  const commands = useMemo(() => {
    const trimmed = query.trim();
    const staticCommands = filterCommandPaletteCommands(paletteCommands, query);
    if (!trimmed) return staticCommands;
    const sessionCommands = filterSessions(sessions, { query, engine: 'all', status: 'all' }).map(
      sessionToCommand,
    );
    const providerCommands = filterProviders(providers, query).map(providerToCommand);
    return [...staticCommands, ...sessionCommands, ...providerCommands];
  }, [query, sessions, providers]);

  useEffect(() => {
    setActive(0);
  }, [query, commands.length]);

  const run = (command: CommandPaletteCommand | undefined) => {
    if (!command) return;
    onRun(command);
  };

  const onKeyDown = (event: React.KeyboardEvent<HTMLInputElement>) => {
    // IME 防护（变更-08）：中文组字期间的 Enter/↑↓/Esc 让给输入法
    if (event.nativeEvent.isComposing || event.keyCode === 229) return;
    if (event.key === 'Escape') {
      event.preventDefault();
      onClose();
      return;
    }
    if (event.key === 'ArrowDown') {
      event.preventDefault();
      setActive((current) => (commands.length ? (current + 1) % commands.length : 0));
      return;
    }
    if (event.key === 'ArrowUp') {
      event.preventDefault();
      setActive((current) =>
        commands.length ? (current - 1 + commands.length) % commands.length : 0,
      );
      return;
    }
    if (event.key === 'Enter') {
      event.preventDefault();
      run(commands[active]);
    }
  };

  let lastGroup = '';

  return (
    <div
      className={'palette-overlay' + (open ? ' open' : '')}
      onMouseDown={(event) => {
        if (event.target === event.currentTarget) onClose();
      }}
    >
      <div className="palette" role="dialog" aria-modal="true" aria-label="命令面板">
        <div className="palette__in">
          <Icon name="search" />
          <input
            ref={inputRef}
            type="text"
            placeholder="搜索命令、会话、服务商…"
            aria-label="命令面板"
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            onKeyDown={onKeyDown}
          />
          <span className="kbd">Esc</span>
        </div>
        <div className="palette__list">
          {commands.length === 0 ? <div className="palette__empty">没有匹配项</div> : null}
          {commands.map((command, index) => {
            const showGroup = command.group !== lastGroup;
            lastGroup = command.group;
            return (
              <div key={command.id}>
                {showGroup ? <div className="palette__group">{command.group}</div> : null}
                <button
                  className={'palette__item' + (index === active ? ' is-active' : '')}
                  onMouseEnter={() => setActive(index)}
                  onClick={() => run(command)}
                  type="button"
                >
                  <Icon name={command.icon} />
                  <span>{command.title}</span>
                  {command.hint ? <span className="meta">{command.hint}</span> : null}
                </button>
              </div>
            );
          })}
        </div>
      </div>
    </div>
  );
}
