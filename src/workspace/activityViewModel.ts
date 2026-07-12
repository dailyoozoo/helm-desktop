import type { SessionState } from '../engine/useSession';

function toolText(toolName?: string, target?: string): string {
  const suffix = target ? ` ${target}` : '';
  switch (toolName) {
    case 'Read':
      return target ? `正在读取${suffix}…` : '正在读取文件…';
    case 'Edit':
      return target ? `正在编辑${suffix}…` : '正在编辑文件…';
    case 'Bash':
      return '正在运行命令…';
    case 'Grep':
      return target ? `正在搜索${suffix}…` : '正在搜索内容…';
    case 'Glob':
      return target ? `正在查找${suffix}…` : '正在查找文件…';
    case 'Write':
      return target ? `正在写入${suffix}…` : '正在写入文件…';
    default:
      return toolName ? `正在运行 ${toolName}…` : '正在运行工具…';
  }
}

export interface ActivityPresentationParts {
  label: string;
  elapsedText: string | null;
}

export function activityPresentationParts(
  state: SessionState,
  now = Date.now(),
): ActivityPresentationParts | null {
  const activity = state.turnActivity;
  if (!activity) return null;

  let text: string;
  switch (activity.stage) {
    case 'preparing':
      text = '正在准备本轮…';
      break;
    case 'starting_engine':
      text = `正在启动 ${state.engine === 'codex' ? 'Codex' : 'Claude Code'}…`;
      break;
    case 'restoring_session':
      text = '正在恢复会话…';
      break;
    case 'waiting_model':
      text = '已连接引擎，等待模型响应…';
      break;
    case 'reasoning':
      text = '正在分析…';
      break;
    case 'using_tool':
      text = toolText(activity.toolName, activity.target);
      break;
    case 'responding':
      text = '正在生成回复…';
      break;
    case 'finalizing':
      text = '正在整理结果…';
      break;
    case 'waiting_approval':
      text = '等待审批…';
      break;
    case 'retrying':
      text = activity.retryAttempt
        ? `连接异常，正在重试（${activity.retryAttempt}）…`
        : '连接异常，正在重试…';
      break;
  }

  const elapsedSeconds = Math.floor(Math.max(0, now - activity.since) / 1_000);
  return {
    label: text,
    elapsedText: elapsedSeconds >= 8 ? `（已用时 ${elapsedSeconds} 秒）` : null,
  };
}

export function activityPresentation(state: SessionState, now = Date.now()): string | null {
  const presentation = activityPresentationParts(state, now);
  if (!presentation) return null;
  return `${presentation.label}${presentation.elapsedText ?? ''}`;
}

export function thinkingOpenAfterItemUpdate(
  open: boolean,
  wasDone: boolean,
  isDone: boolean,
): boolean {
  return !wasDone && isDone ? false : open;
}
