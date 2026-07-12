import type { SlashCommand } from '../extensions/extensionsApi';

// CJK 统一表意文字 + 扩展A + 日文假名 + 韩文音节 + 全角标点区（码点转义，避免 lint 全角空格告警）
const CJK_CHAR = /[\u3000-\u303f\u3040-\u30ff\u3400-\u4dbf\u4e00-\u9fff\uac00-\ud7af\uff00-\uffef]/;

/**
 * 客户端 token 粗估（变更-08）：CJK 字符约 1.6 字符/token，其余（英文/符号/空白）约 4 字符/token。
 * 仅供输入时参考，非后端真实值；显示时应标注「≈」。
 */
export function estimateTokens(text: string): number {
  let cjk = 0;
  let other = 0;
  for (const ch of text) {
    // CJK 统一表意文字 + 扩展A + 日文假名 + 韩文音节 + 全角标点
    if (CJK_CHAR.test(ch)) {
      cjk += 1;
    } else {
      other += 1;
    }
  }
  return Math.max(0, Math.round(cjk / 1.6 + other / 4));
}

/** 匹配排序：trigger 前缀 > trigger 词首（-/_ 分词）> trigger/描述子串，同级按 trigger 字典序。 */
export function filterSlashCommands(commands: SlashCommand[], query: string): SlashCommand[] {
  const normalized = query.trim().toLowerCase();
  const keyword =
    normalized.startsWith('/') || normalized.startsWith('$') ? normalized.slice(1) : normalized;
  return commands
    .filter((command) => command.enabled)
    .map((command) => ({ command, score: matchScore(command, keyword) }))
    .filter((entry) => entry.score >= 0)
    .sort((a, b) => a.score - b.score || a.command.trigger.localeCompare(b.command.trigger))
    .map((entry) => entry.command);
}

function matchScore(command: SlashCommand, keyword: string): number {
  if (!keyword) return 2;
  const trigger = command.trigger.replace(/^[/$]/, '').toLowerCase();
  if (trigger.startsWith(keyword)) return 0;
  if (
    trigger
      .split(/[-_]/)
      .slice(1)
      .some((part) => part.startsWith(keyword))
  ) {
    return 1;
  }
  if (`${trigger} ${command.description}`.toLowerCase().includes(keyword)) return 2;
  return -1;
}

/** 选中命令后输入框补全为 `/trigger `，等待参数；不再展开模板全文。 */
export function completeSlashCommand(command: SlashCommand): string {
  return `${command.trigger} `;
}

export function helmCommandAction(command: SlashCommand): string | undefined {
  return command.id.startsWith('__helm_') ? command.id.slice('__helm_'.length) : undefined;
}

/** 兼容旧调用：直接把命令模板展开成输入文本。 */
export function applySlashCommand(command: SlashCommand, args = ''): string {
  return expandTemplate(command.body, args);
}

/** 输入文本首 token 精确命中的已启用命令（用于参数提示行与发送展开）。 */
export function matchSlashCommand(
  commands: SlashCommand[],
  text: string,
): SlashCommand | undefined {
  const trimmed = text.trimStart();
  if (!trimmed.startsWith('/') && !trimmed.startsWith('$')) return undefined;
  const spaceIndex = trimmed.search(/\s/);
  const trigger = (spaceIndex === -1 ? trimmed : trimmed.slice(0, spaceIndex)).toLowerCase();
  return commands.find((command) => command.enabled && command.trigger.toLowerCase() === trigger);
}

/**
 * 发送边界的命令展开（变更-03 C.1 实测结论）：
 * - Claude Code 的扩展中心/项目级命令由 CLI 原生执行 → 透传 `/文件名 参数`
 *   （CLI 认文件名而非 x-helm-trigger，因此透传时统一重写为文件名）；
 * - Codex（codex exec 不展开 custom prompts）与内置命令（无真实文件）→ 本地展开模板。
 */
export function expandSlashCommand(
  commands: SlashCommand[],
  text: string,
  engine: 'claude-code' | 'codex',
): string {
  return expandSlashCommandDetailed(commands, text, engine).expanded;
}

export interface SlashExpansion {
  /** 发送给 CLI 的文本（透传原样命令或本地展开的模板） */
  expanded: string;
  /** 是否命中了斜杠命令（未命中时 expanded === 原文） */
  matched: boolean;
  /** true=透传给 CLI 原生执行（命令必须位于 prompt 开头）；false=本地已展开成普通文本 */
  passthrough: boolean;
}

/**
 * 展开的详细结果（变更-08）：透传标记让上层知道该命令要求「位于 prompt 开头」，
 * 从而在有附件/历史前缀时避免破坏 CLI 的命令识别。
 */
export function expandSlashCommandDetailed(
  commands: SlashCommand[],
  text: string,
  engine: 'claude-code' | 'codex',
): SlashExpansion {
  const trimmed = text.trim();
  const command = matchSlashCommand(commands, trimmed);
  if (!command) return { expanded: text, matched: false, passthrough: false };
  const spaceIndex = trimmed.search(/\s/);
  const args = spaceIndex === -1 ? '' : trimmed.slice(spaceIndex + 1).trim();

  if (
    engine === 'claude-code' &&
    (command.source === 'extension' || command.source === 'engine-project')
  ) {
    const fileStem =
      command.source === 'engine-project' ? command.id.replace(/^__proj_/, '') : command.id;
    return {
      expanded: args ? `/${fileStem} ${args}` : `/${fileStem}`,
      matched: true,
      passthrough: true,
    };
  }

  return { expanded: expandTemplate(command.body, args), matched: true, passthrough: false };
}

/** 替换 $ARGUMENTS（全部参数）与 $1..$9（分词，双引号内算一个词）；模板无占位符时参数附加在尾部。 */
function expandTemplate(body: string, args: string): string {
  const words = splitArgs(args);
  let used = false;
  const expanded = body.replace(/\$(ARGUMENTS|[1-9])/g, (_, token: string) => {
    used = true;
    if (token === 'ARGUMENTS') return args;
    return words[Number(token) - 1] ?? '';
  });
  if (!used && args) return `${expanded}\n\n${args}`;
  return expanded;
}

function splitArgs(args: string): string[] {
  const matches = args.match(/"([^"]*)"|\S+/g) ?? [];
  return matches.map((word) =>
    word.startsWith('"') && word.endsWith('"') && word.length >= 2 ? word.slice(1, -1) : word,
  );
}

/**
 * Composer 回车键处置（变更-08，抽成纯函数便于测试 IME 与未知命令分支）。
 * 返回本次 Enter 应触发的动作：
 * - 'ime'：正在用输入法组字，让给 IME，不发送也不选命令；
 * - 'newline'：Shift+Enter 或其他情况下应插入换行（交给 textarea 默认行为）；
 * - 'pick'：斜杠菜单有高亮项，Enter 补全该命令；
 * - 'block'：首 token 是未知命令，拦截发送（保留输入待修改）；
 * - 'queue'：轮次进行中，消息入队等待本轮结束后自动发送（变更-12）；
 * - 'send'：正常发送。
 */
export function resolveEnterAction(input: {
  shiftKey: boolean;
  isComposing: boolean;
  working: boolean;
  menuOpen: boolean;
  hasMenuMatches: boolean;
  unknownCommand: boolean;
}): 'ime' | 'newline' | 'pick' | 'block' | 'queue' | 'send' {
  if (input.isComposing) return 'ime';
  if (input.shiftKey) return 'newline';
  if (input.menuOpen && input.hasMenuMatches) return 'pick';
  if (input.unknownCommand) return 'block';
  if (input.working) return 'queue';
  return 'send';
}
