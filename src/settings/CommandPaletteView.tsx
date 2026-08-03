import { useEffect, useMemo, useRef, useState } from 'react';
import type { AppConfig } from '../providers/api';
import { getProviderConfig } from '../providers/api';
import { listFolders, listSessions } from '../sessions/api';
import { Icon } from '../shell/icons';
import { useDialogBehavior } from '../components/useDialogBehavior';
import { commandPaletteResults, type CommandPaletteCommand } from './commandPalette';

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
  const [folders, setFolders] = useState<Awaited<ReturnType<typeof listFolders>>>([]);
  const [providers, setProviders] = useState<AppConfig['providers']>([]);
  const inputRef = useRef<HTMLInputElement>(null);
  const activeItemRef = useRef<HTMLButtonElement>(null);
  const dialogRef = useDialogBehavior(onClose, open);

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
    listFolders()
      .then((next) => {
        if (active) setFolders(next);
      })
      .catch(() => {
        if (active) setFolders([]);
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
    return commandPaletteResults(query, sessions, providers, folders);
  }, [query, sessions, providers, folders]);

  useEffect(() => {
    setActive(0);
  }, [query, commands.length]);

  useEffect(() => {
    if (!open) return;
    activeItemRef.current?.scrollIntoView({ block: 'nearest' });
  }, [active, open, commands.length]);

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
      <div
        ref={dialogRef}
        className="palette"
        role="dialog"
        aria-modal="true"
        aria-labelledby="command-palette-title"
        tabIndex={-1}
      >
        <div className="palette__in">
          <Icon name="search" />
          <span id="command-palette-title" className="sr-only">
            命令面板
          </span>
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
                  ref={index === active ? activeItemRef : undefined}
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
