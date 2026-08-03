import type { ThreadItem } from '../engine/useSession';

export interface ActivityLogGroup {
  id: string;
  label: string;
  items: ThreadItem[];
}

const isActivityItem = (item: ThreadItem) =>
  item.kind === 'tool' ||
  item.kind === 'checkpoint' ||
  item.kind === 'approval' ||
  item.kind === 'plan';

export function activityLogGroups(items: ThreadItem[]): ActivityLogGroup[] {
  const groups: ActivityLogGroup[] = [];
  const byId = new Map<string, ActivityLogGroup>();
  let fallbackTurn = 0;
  for (const item of items) {
    if (item.kind === 'user') fallbackTurn += 1;
    if (!isActivityItem(item) || ('reverted' in item && item.reverted)) continue;
    const id = item.turnId ?? `fallback-${Math.max(1, fallbackTurn)}`;
    let group = byId.get(id);
    if (!group) {
      group = { id, label: `第 ${groups.length + 1} 轮`, items: [] };
      byId.set(id, group);
      groups.push(group);
    }
    group.items.push(item);
  }
  return groups;
}
