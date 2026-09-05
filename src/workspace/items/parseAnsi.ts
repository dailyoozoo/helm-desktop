/**
 * 轻量 ANSI SGR 解析（渲染形态 B · 终端输出上色）。
 *
 * 终端输出里大量出现 `\x1b[32;1m` 这类 CSI 序列；若直接当纯文本渲染，用户就会看到
 * `[]32;1mCount` 这种"乱码"（PowerShell 7 默认带 ANSI 着色）。这里把它切成带样式的
 * token，由 React 渲染成 `<span>`——**不用 innerHTML**，转义交给 React，天然免疫 XSS。
 *
 * 覆盖范围（够用即止，不追求全量 xterm 兼容）：
 * - SGR：0 重置、1 粗体、3 斜体、4 下划线、22/23/24 关闭、30-37/90-97 前景、
 *   39/49 恢复默认、40-47/100-107 背景；其余 SGR 忽略；
 * - 非 SGR 的 CSI（光标移动、清屏 `\x1b[2J` 等）对静态输出无意义，**直接丢弃**。
 */

export interface AnsiToken {
  text: string;
  color?: string;
  bg?: string;
  bold?: boolean;
  italic?: boolean;
  underline?: boolean;
}

/** 前景：标准 30-37 + 亮色 90-97。取值兼顾深底/浅底可读性，不用纯白（浅底会糊）。 */
const FG: Record<number, string> = {
  30: '#5c6370',
  31: '#e06c75',
  32: '#98c379',
  33: '#e5c07b',
  34: '#61afef',
  35: '#c678dd',
  36: '#56b6c2',
  37: '#c8ccd4',
  90: '#7f848e',
  91: '#ff7b86',
  92: '#b5e890',
  93: '#ffcb6b',
  94: '#82c8ff',
  95: '#e39ef7',
  96: '#7ee0ea',
  97: '#e6e9ef',
};

/** 背景：标准 40-47 + 亮色 100-107，低饱和以免盖住前景。 */
const BG: Record<number, string> = {
  40: '#2b303b',
  41: '#4b2327',
  42: '#27392a',
  43: '#453a22',
  44: '#1f3550',
  45: '#3f2745',
  46: '#1d3b40',
  47: '#3b3f46',
  100: '#3a4049',
  101: '#6b2f34',
  102: '#35513a',
  103: '#5f5029',
  104: '#2d4a6b',
  105: '#573460',
  106: '#275058',
  107: '#4a4f57',
};

// eslint-disable-next-line no-control-regex -- 解析 ANSI 的本职就是匹配 ESC(\x1b) 控制字符，属规则误报场景
const CSI = /\x1b\[([0-9;]*)([A-Za-z])/g;

/** 把可能含 ANSI 转义的文本切成 token 序列；无转义时返回单个 token。 */
export function parseAnsi(input: string): AnsiToken[] {
  if (!input) return [];
  if (!input.includes('\x1b')) return [{ text: input }];

  const tokens: AnsiToken[] = [];
  let color: string | undefined;
  let bg: string | undefined;
  let bold = false;
  let italic = false;
  let underline = false;
  let last = 0;

  const push = (text: string) => {
    if (!text) return;
    tokens.push({
      text,
      ...(color ? { color } : {}),
      ...(bg ? { bg } : {}),
      ...(bold ? { bold: true } : {}),
      ...(italic ? { italic: true } : {}),
      ...(underline ? { underline: true } : {}),
    });
  };

  CSI.lastIndex = 0;
  for (let match = CSI.exec(input); match !== null; match = CSI.exec(input)) {
    push(input.slice(last, match.index));
    last = match.index + match[0].length;
    // 只有 SGR（以 m 结尾）携带样式；其余 CSI 一律丢弃，不出现在输出里
    if (match[2] !== 'm') continue;
    const codes = match[1] === '' ? [0] : match[1].split(';').map((part) => Number(part) || 0);
    for (const code of codes) {
      if (code === 0) {
        color = undefined;
        bg = undefined;
        bold = false;
        italic = false;
        underline = false;
      } else if (code === 1) bold = true;
      else if (code === 3) italic = true;
      else if (code === 4) underline = true;
      else if (code === 22) bold = false;
      else if (code === 23) italic = false;
      else if (code === 24) underline = false;
      else if (code === 39) color = undefined;
      else if (code === 49) bg = undefined;
      else if (FG[code]) color = FG[code];
      else if (BG[code]) bg = BG[code];
    }
  }
  push(input.slice(last));
  return tokens;
}
