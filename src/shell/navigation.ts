import type { IconName } from './icons';

/** S0 导航协议：PageId 是路由唯一真值；workspace 是工作区详情态，不占一级主入口。 */
export type PageId =
  | 'home'
  | 'workspace'
  | 'sessions'
  | 'providers'
  | 'extensions'
  | 'usage'
  | 'settings';

export interface RailEntry {
  id: PageId;
  icon: IconName;
  label: string;
}

export const PRIMARY_RAIL_ENTRIES: readonly RailEntry[] = [
  { id: 'home', icon: 'squarepen', label: '新任务' },
  { id: 'providers', icon: 'server', label: 'AI 配置' },
  { id: 'extensions', icon: 'plug', label: '插件' },
  { id: 'usage', icon: 'chartcolumn', label: '用量' },
] as const satisfies readonly RailEntry[];
