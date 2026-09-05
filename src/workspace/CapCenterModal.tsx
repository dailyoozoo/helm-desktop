import { useEffect, useRef, useState } from 'react';
import { Icon } from '../shell/icons';
import type { SlashCommand } from '../extensions/extensionsApi';
import { searchWorkspaceFiles } from './workspaceApi';

/**
 * 「+」能力菜单的居中搜索弹窗（对齐原型 workspace.html #workspaceCenter + workspace.js
 * openWorkspaceCenter/L986-1021）：文件与目录 / 命令与技能两个入口共用一个 cm-modal，
 * 顶部搜索框实时过滤，行点击后关闭弹窗并把选择交回 Composer。
 * - 文件与目录：真实 search_workspace_files（目录行带尾斜杠）+「从电脑选择文件…」原生行
 * - 命令与技能：动态发现的 / 命令与技能（sourceLabel 分组头），选择后追加进输入框
 * 视觉全部消费共享类：cm-modal-backdrop / cm-modal / cm-search / cm-command-*。
 */
export type CapCenterKey = 'files' | 'commands';

const CENTER_META: Record<CapCenterKey, { title: string; desc: string; placeholder: string }> = {
  files: {
    title: '添加到任务',
    desc: '选择文件或目录。',
    placeholder: '搜索文件与目录',
  },
  commands: {
    title: '添加到任务',
    desc: '按来源分组；选择后插入输入框，技能同样以 / 调用。',
    placeholder: '搜索命令或技能',
  },
};

const SEARCH_DELAY_MS = 150;

function commandMeta(command: SlashCommand): string {
  return command.id.startsWith('__skill_') ? '技能' : '命令';
}

function commandIcon(command: SlashCommand): Parameters<typeof Icon>[0]['name'] {
  return command.id.startsWith('__skill_') ? 'sparkles' : 'terminal';
}

export function CapCenterModal({
  cap,
  cwd,
  commands,
  onClose,
  onPickContext,
  onPickCommand,
  onPickNativeFile,
}: {
  /** 当前打开的入口；null 关闭 */
  cap: CapCenterKey | null;
  cwd?: string;
  /** 已按引擎组装好的 / 命令与技能（Composer 的 triggerCommands） */
  commands: SlashCommand[];
  onClose: () => void;
  /** 文件/目录行：绝对路径（目录尾斜杠已去掉） */
  onPickContext: (absolutePath: string) => void;
  /** 命令/技能行：触发词原文追加进输入框（原型 input.value += label） */
  onPickCommand: (label: string) => void;
  /** 「从电脑选择文件…」行 */
  onPickNativeFile: () => void;
}) {
  const [query, setQuery] = useState('');
  const [files, setFiles] = useState<string[]>([]);
  const searchRef = useRef<HTMLInputElement>(null);
  const meta = cap ? CENTER_META[cap] : null;

  // 每次打开重置搜索词；文件入口防抖走真实 search_workspace_files（空查询=最浅 30 条）
  useEffect(() => {
    setQuery('');
    setFiles([]);
    if (cap) window.setTimeout(() => searchRef.current?.focus(), 0);
  }, [cap]);

  useEffect(() => {
    if (cap !== 'files' || !cwd) return;
    let active = true;
    const timer = window.setTimeout(() => {
      searchWorkspaceFiles(cwd, query.trim())
        .then((rows) => {
          if (active) setFiles(rows);
        })
        .catch(() => {
          if (active) setFiles([]);
        });
    }, SEARCH_DELAY_MS);
    return () => {
      active = false;
      window.clearTimeout(timer);
    };
  }, [cap, cwd, query]);

  useEffect(() => {
    if (!cap) return;
    const onKeyDown = (event: globalThis.KeyboardEvent) => {
      if (event.key === 'Escape') onClose();
    };
    document.addEventListener('keydown', onKeyDown, true);
    return () => document.removeEventListener('keydown', onKeyDown, true);
  }, [cap, onClose]);

  if (!cap || !meta) return null;

  const normalizedQuery = query.trim().toLocaleLowerCase('zh-CN');
  const match = (text: string) =>
    !normalizedQuery || text.toLocaleLowerCase('zh-CN').includes(normalizedQuery);

  const toAbsolute = (relativePath: string) =>
    `${(cwd ?? '').replace(/[\\/]+$/, '')}/${relativePath.replace(/\/+$/, '')}`;

  const renderRow = ({
    key,
    icon,
    label,
    desc,
    meta: rowMeta,
    onClick,
  }: {
    key: string;
    icon: Parameters<typeof Icon>[0]['name'];
    label: string;
    desc: string;
    meta?: string;
    onClick: () => void;
  }) => (
    <button key={key} type="button" className="cm-command-row" onClick={onClick}>
      <span className="cm-command-row__icon">
        <Icon name={icon} />
      </span>
      <span className="cm-command-row__copy">
        <b>{label}</b>
        <small>{desc}</small>
      </span>
      {rowMeta ? <span className="cm-command-row__meta">{rowMeta}</span> : null}
    </button>
  );

  const slashCommands = commands.filter((command) => !command.id.startsWith('__skill_'));
  const skillCommands = commands.filter((command) => command.id.startsWith('__skill_'));

  return (
    <div
      className="cm-modal-backdrop is-open"
      role="presentation"
      onClick={(event) => {
        if (event.target === event.currentTarget) onClose();
      }}
    >
      <section
        className="cm-modal"
        role="dialog"
        aria-modal="true"
        aria-labelledby="capCenterTitle"
      >
        <div className="cm-modal__head">
          <div>
            <h2 id="capCenterTitle">{meta.title}</h2>
            <p className="cm-pagehead__desc">{meta.desc}</p>
          </div>
          <button className="btn-icon" type="button" aria-label="关闭" onClick={onClose}>
            <Icon name="x" />
          </button>
        </div>
        <div className="cm-search cm-search--block">
          <Icon name="search" />
          <input
            ref={searchRef}
            value={query}
            placeholder={meta.placeholder}
            aria-label="搜索"
            onChange={(event) => setQuery(event.target.value)}
          />
        </div>
        <div className="cm-command-list mt-12" role="listbox">
          {cap === 'files'
            ? [
                // 2026-08-28 用户决议：文件与目录只展示前 5 条，下方直接是「从电脑选择文件…」。
                ...files
                  .filter(match)
                  .slice(0, 5)
                  .map((file) =>
                    renderRow({
                      key: file,
                      icon: file.endsWith('/') ? 'folder' : 'file',
                      label: file.replace(/\/$/, ''),
                      desc: '',
                      meta: file.endsWith('/') ? '目录' : '文件',
                      onClick: () => onPickContext(toAbsolute(file)),
                    }),
                  ),
                renderRow({
                  key: '__native_file',
                  icon: 'folderopen',
                  label: '从电脑选择文件…',
                  desc: '选择文件',
                  onClick: onPickNativeFile,
                }),
              ]
            : [
                ...(slashCommands.some((command) =>
                  match(`${command.trigger} ${command.description ?? ''}`),
                )
                  ? [<div className="cm-command-head">内置命令</div>]
                  : []),
                ...slashCommands
                  .filter((command) => match(`${command.trigger} ${command.description ?? ''}`))
                  .map((command) =>
                    renderRow({
                      key: command.id,
                      icon: commandIcon(command),
                      label: command.trigger,
                      desc: command.description ?? '',
                      meta: commandMeta(command),
                      onClick: () => onPickCommand(command.trigger),
                    }),
                  ),
                ...(skillCommands.some((command) =>
                  match(`${command.trigger} ${command.description ?? ''}`),
                )
                  ? [<div className="cm-command-head">技能 Skills</div>]
                  : []),
                ...skillCommands
                  .filter((command) => match(`${command.trigger} ${command.description ?? ''}`))
                  .map((command) =>
                    renderRow({
                      key: command.id,
                      icon: commandIcon(command),
                      label: command.trigger,
                      desc: command.description ?? '',
                      meta: commandMeta(command),
                      onClick: () => onPickCommand(command.trigger),
                    }),
                  ),
              ]}
        </div>
      </section>
    </div>
  );
}
