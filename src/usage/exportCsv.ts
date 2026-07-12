import type { Budget, DailyUsage, ModelUsage, TopSession, UsageStats } from './api';

export interface UsageCsvInput {
  periodLabel: string;
  stats: UsageStats | null;
  dailyUsage: DailyUsage[];
  modelUsage: ModelUsage[];
  topSessions: TopSession[];
  budget: Budget | null;
  generatedAt: Date;
}

function csvCell(value: string | number | boolean | null | undefined): string {
  const text = value == null ? '' : String(value);
  if (!/[",\r\n]/.test(text)) return text;
  return `"${text.replace(/"/g, '""')}"`;
}

function row(values: Array<string | number | boolean | null | undefined>): string {
  return values.map(csvCell).join(',');
}

function money(value: number): string {
  return value.toFixed(4);
}

function percent(value: number): string {
  return `${(value * 100).toFixed(2)}%`;
}

export function buildUsageCsv(input: UsageCsvInput): string {
  const lines: string[] = [];
  const stats = input.stats;

  lines.push(row(['Helm 用量与成本导出']));
  lines.push(row(['生成时间', input.generatedAt.toISOString()]));
  lines.push(row(['统计周期', input.periodLabel]));
  lines.push('');

  lines.push(row(['统计项', '值']));
  lines.push(row(['总花费 USD', stats ? money(stats.total_cost) : money(0)]));
  lines.push(row(['总 token', stats?.total_tokens ?? 0]));
  lines.push(row(['输入 token', stats?.input_tokens ?? 0]));
  lines.push(row(['输出 token', stats?.output_tokens ?? 0]));
  lines.push(row(['请求数', stats?.request_count ?? 0]));
  lines.push(row(['会话数', stats?.session_count ?? 0]));
  lines.push('');

  lines.push(row(['每日花费']));
  lines.push(row(['日期', '花费 USD']));
  input.dailyUsage.forEach((item) => lines.push(row([item.date, money(item.cost_usd)])));
  lines.push('');

  lines.push(row(['按模型花费']));
  lines.push(row(['模型', '引擎', '请求数', '输入 token', '输出 token', '花费 USD', '占比']));
  input.modelUsage.forEach((item) =>
    lines.push(
      row([
        item.model,
        item.engine,
        item.request_count,
        item.input_tokens,
        item.output_tokens,
        money(item.cost_usd),
        percent(item.share),
      ]),
    ),
  );
  lines.push('');

  lines.push(row(['花费最高的会话']));
  lines.push(row(['会话 ID', '标题', '模型', '引擎', '花费 USD', '总 token']));
  input.topSessions.forEach((item) =>
    lines.push(
      row([item.id, item.title, item.model, item.engine, money(item.cost_usd), item.total_tokens]),
    ),
  );
  lines.push('');

  lines.push(row(['预算']));
  lines.push(row(['月度预算 USD', input.budget ? money(input.budget.monthly_limit) : money(0)]));
  lines.push(
    row(['本月已用 USD', input.budget ? money(input.budget.current_month_cost) : money(0)]),
  );
  lines.push(row(['已用比例', input.budget ? `${input.budget.percentage.toFixed(2)}%` : '0.00%']));
  lines.push(row(['80% 提醒', input.budget?.alert_at_80 ?? false]));
  lines.push(row(['100% 停止', input.budget?.stop_at_100 ?? false]));

  return lines.join('\n');
}
