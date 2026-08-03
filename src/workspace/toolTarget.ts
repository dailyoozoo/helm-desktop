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
