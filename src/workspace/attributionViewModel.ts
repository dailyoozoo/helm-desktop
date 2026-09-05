/**
 * 变更-34/35 · E2：占用归因视图模型。
 * 只列 Runtime 真实报告过规模的来源；没有逐项数据就整体显示「暂无」——
 * 绝不拿累计计费值反推每一项占了多少（口径不同，AGENTS.md 红线）。
 */

export interface AttributionEntry {
  /** 图标名（IconName 子集）；渲染端按需映射。 */
  icon?: string;
  label: string;
  /** 次要说明（如「3 个文件」）。 */
  sublabel?: string;
  /** 展示值（如「45%」「11.2K」）。 */
  value: string;
  /** 真实占比 0..1；用于判定占比最高项。 */
  ratio?: number;
  /** 是否为占比最高项（is-hot 高亮）。 */
  isHot?: boolean;
  /** 占比最高的来源给出降低建议；无则用通用文案。 */
  tip?: string;
}

/** 归因标题旁的小注（如「按来源」）；无条目返回 null。 */
export function attributionNote(entries: AttributionEntry[]): string | null {
  return entries.length > 0 ? '按来源' : null;
}

/** 占比最高项的降低建议；无条目或未标记 isHot 返回 null。 */
export function attributionTip(entries: AttributionEntry[]): string | null {
  const hot = entries.find((entry) => entry.isHot);
  return hot?.tip ?? null;
}

export const ATTRIBUTION_EMPTY = '当前引擎未按来源报告输入规模 —— 暂无归因数据';
