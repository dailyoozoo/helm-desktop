/** 工具头部的目标摘要（原型：工具名后跟加粗文件名/命令），从入参提取，无则为空。
 *  ContextPanel 与 ToolBlock 共用（变更-23 C-3 合并，以 ToolBlock 版为准）。 */
export function toolTarget(name: string, input: unknown): string {
  if (!input || typeof input !== 'object') return '';
  const record = input as Record<string, unknown>;
  const candidate =
    record.file_path ?? record.notebook_path ?? record.path ?? record.pattern ?? record.url;
  if (typeof candidate === 'string' && candidate) return candidate;
  if (name === 'Bash' && typeof record.command === 'string') {
    const first = record.command.trim().split('\n')[0];
    return first.length > 60 ? `${first.slice(0, 60)}…` : first;
  }
  return '';
}

/**
 * 工具真正触碰的**文件路径**（交付物行专用，2026-08-30）。
 *
 * 与 `toolTarget` 的区别：后者是「工具头部显示什么」，为了让 Bash/Grep/WebFetch 也有
 * 抬头，会回落到命令行、搜索模式和 URL。用它统计「本轮碰过的文件」会把一条
 * `pwsh -Command 'echo ...'`、一次 Grep 的 pattern 或一个网址算成文件，于是查天气这类
 * 完全没碰文件的轮次也会冒出「查看全部文件 1」。这里只认结构化的文件路径入参，
 * 拿不到就返回空——宁可不显示，也不显示假的文件数。
 */
export function toolFilePath(input: unknown): string {
  if (!input || typeof input !== 'object') return '';
  const record = input as Record<string, unknown>;
  const candidate = record.file_path ?? record.notebook_path ?? record.path;
  if (typeof candidate !== 'string') return '';
  const value = candidate.trim();
  // `path` 在部分工具里承载 URL（抓取类）；那不是本地文件，不计入交付物。
  if (!value || /^[a-z][a-z0-9+.-]*:\/\//i.test(value)) return '';
  return value;
}

/** 交付物统计的最小输入形状（取自线程里的 tool 条目）。 */
export interface DeliverableToolInput {
  input: unknown;
  diff?: { path: string } | undefined;
  reverted?: boolean | undefined;
}

export interface TurnDeliverables {
  /** 本轮写入过的文件（有真实 diff 才算），用于「产出文档」链接与变更计数 */
  documents: string[];
  /** 本轮触碰过的文件数（读 + 写），只统计真实文件路径 */
  fileCount: number;
  /** 本轮产生变更的文件数 */
  changeCount: number;
}

/**
 * 汇总一个 Turn 的交付物事实（2026-08-30）。
 *
 * 两条口径互相独立，这一点曾经写错过：
 * - 变更只认真实 diff，与工具入参无关——没有 `file_path` 的写工具（diff 由 Runtime 给出）
 *   也必须计入，所以 diff 的收集不能被路径判空提前 `continue` 掉；
 * - 触碰只认结构化文件路径，shell 命令、搜索模式和 URL 都不算文件。
 */
export function collectTurnDeliverables(tools: readonly DeliverableToolInput[]): TurnDeliverables {
  const written = new Set<string>();
  const touched = new Set<string>();
  for (const tool of tools) {
    if (tool.reverted) continue;
    if (tool.diff) {
      written.add(tool.diff.path);
      touched.add(tool.diff.path);
    }
    const target = toolFilePath(tool.input);
    if (target) touched.add(target);
  }
  return {
    documents: [...written],
    fileCount: touched.size,
    changeCount: written.size,
  };
}
