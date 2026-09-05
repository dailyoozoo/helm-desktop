// 上下文药丸（变更-34 · D1）：@提及与附件在输入框内成为可见、可移除的实体，
// 而不是埋在正文里的一串路径文本 —— 发送前能一眼看清 Agent 会拿到什么。
import type { IconName } from '../shell/icons';
import { Icon } from '../shell/icons';

export interface ContextPillItem {
  /** 引用类型：@提及（相对路径展示）或 附件（文件名展示） */
  kind: 'mention' | 'attachment';
  /** 绝对路径，发送时随 prompt 交给 CLI */
  path: string;
  /** 展示名 */
  label: string;
}

export function contextPillLabel(
  path: string,
  kind: ContextPillItem['kind'],
  cwd?: string,
): string {
  if (kind === 'mention' && cwd) {
    const normalizedCwd = cwd.replace(/[\\/]+$/, '');
    const normalizedPath = path.replace(/\\/g, '/');
    return normalizedPath.startsWith(`${normalizedCwd.replace(/\\/g, '/')}/`)
      ? normalizedPath.slice(normalizedCwd.replace(/\\/g, '/').length + 1)
      : (path.split(/[\\/]/).filter(Boolean).pop() ?? path);
  }
  return path.split(/[\\/]/).filter(Boolean).pop() ?? path;
}

export function ContextPill({
  item,
  onRemove,
  disabled = false,
  icon = 'file',
}: {
  item: ContextPillItem;
  onRemove: (path: string) => void;
  disabled?: boolean;
  icon?: IconName;
}) {
  return (
    <span className="cpill" title={item.path} data-kind={item.kind}>
      <Icon name={icon} />
      <span className="nm">{item.label}</span>
      <button
        type="button"
        title="移除"
        aria-label={`移除 ${item.label}`}
        disabled={disabled}
        onClick={() => onRemove(item.path)}
      >
        <Icon name="x" />
      </button>
    </span>
  );
}
