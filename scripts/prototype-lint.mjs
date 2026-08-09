// 原型静态一致性检查：抓「JS 引用了但 DOM/CSS/图标里不存在」这类接线断裂。
// 关键：先剥离字符串字面量，否则 mock 数据里的代码片段会被当成真实调用。
import fs from 'node:fs';
import path from 'node:path';

const root = path.resolve(import.meta.dirname, '..');

const html = fs.readFileSync(root + '/prototype/workspace.html', 'utf8');
const css = fs.readFileSync(root + '/prototype/assets/app.css', 'utf8');
const js = fs.readFileSync(root + '/prototype/assets/app.js', 'utf8');
const script = html.slice(html.lastIndexOf('<script>') + 8, html.lastIndexOf('</script>'));

const BS = String.fromCharCode(92);
const SQ = String.fromCharCode(39);
const BT = String.fromCharCode(96);

// 把字符串字面量替换成空串，只留下真实代码结构
function stripStrings(src) {
  let out = '',
    i = 0,
    str = null,
    line = false,
    block = false;
  while (i < src.length) {
    const a = src[i],
      b = src[i + 1];
    if (line) {
      if (a === '\n') {
        line = false;
        out += a;
      }
      i++;
      continue;
    }
    if (block) {
      if (a === '*' && b === '/') {
        block = false;
        i += 2;
        continue;
      }
      if (a === '\n') out += a;
      i++;
      continue;
    }
    if (str) {
      if (a === BS) {
        i += 2;
        continue;
      }
      if (a === str) {
        str = null;
        out += '""';
      } else if (a === '\n') out += a;
      i++;
      continue;
    }
    if (a === '/' && b === '/') {
      line = true;
      i += 2;
      continue;
    }
    if (a === '/' && b === '*') {
      block = true;
      i += 2;
      continue;
    }
    if (a === '"' || a === SQ || a === BT) {
      str = a;
      i++;
      continue;
    }
    out += a;
    i++;
  }
  return out;
}

const code = stripStrings(script);
const fail = [];
const warn = [];
const uniq = (a) => [...new Set(a)];
const grab = (re, s) => uniq([...s.matchAll(re)].map((m) => m[1]));

// 1) getElementById 的 id 必须在标记中存在
const domIds = new Set(grab(/id="([A-Za-z0-9_-]+)"/g, html));
grab(/getElementById\("([A-Za-z0-9_-]+)"\)/g, script)
  .filter((i) => !domIds.has(i))
  .forEach((i) => fail.push(`getElementById("${i}") 无对应 id`));

// 2) 图标必须在 app.js 图标表中定义
const iconKeys = new Set(grab(/^\s{4}([A-Za-z_][A-Za-z0-9_]*)\s*:/gm, js));
uniq([...grab(/data-ic="([a-z0-9]+)"/g, html), ...grab(/\bic\("([a-z0-9]+)"\)/g, script)])
  .filter((i) => !iconKeys.has(i))
  .forEach((i) => fail.push(`图标 "${i}" 未在 app.js 中定义`));

// 3) var(--token) 必须在 :root 声明
const tokens = new Set(grab(/(--[a-z0-9-]+)\s*:/g, css.match(/:root\s*\{([\s\S]*?)\n\}/)[1]));
uniq([...html.matchAll(/var\((--[a-z0-9-]+)\)/g)].map((m) => m[1]))
  .filter((t) => !tokens.has(t))
  .forEach((t) => fail.push(`设计 token ${t} 未在 :root 声明`));

// 4) querySelector 用到的类，必须在本项目某处出现过（CSS 规则或 JS 生成的标记）
//    行为钩子类（无样式）只要在生成的 HTML 里出现即可，不强制有 CSS 规则。
const known = new Set([
  ...grab(/\.([a-zA-Z][\w-]*)/g, css),
  ...grab(/class="([^"]+)"/g, html).flatMap((c) => c.split(/\s+/)),
  ...grab(/classList\.(?:add|toggle|remove)\("([\w-]+)"/g, script),
  ...grab(/class=\\?"([^"\\]+)/g, script).flatMap((c) => c.split(/\s+/)),
]);
uniq(
  [...script.matchAll(/querySelector(?:All)?\("([^"]+)"\)/g)].flatMap((m) =>
    [...m[1].matchAll(/\.([a-zA-Z][\w-]*)/g)].map((x) => x[1]),
  ),
)
  .filter((c) => !known.has(c))
  .forEach((c) => warn.push(`querySelector 用到 .${c}（可能由字符串拼接生成，人工确认）`));

// 5) 真实代码里调用的自定义函数必须有定义
const defined = new Set([
  ...grab(/function\s+([A-Za-z_]\w*)\s*\(/g, code),
  ...grab(/(?:const|let|var)\s+([A-Za-z_]\w*)\s*=/g, code),
  ...grab(/([A-Za-z_]\w*)\s*:\s*(?:function|\()/g, code),
]);
const builtins = new Set([
  'if',
  'for',
  'while',
  'switch',
  'catch',
  'return',
  'typeof',
  'function',
  'Math',
  'String',
  'Number',
  'Array',
  'Object',
  'JSON',
  'Date',
  'Set',
  'Map',
  'Boolean',
  'Promise',
  'setTimeout',
  'setInterval',
  'clearTimeout',
  'clearInterval',
  'parseInt',
  'parseFloat',
  'console',
  'document',
  'window',
  'localStorage',
  'Helm',
  'CustomEvent',
  'requestAnimationFrame',
  'prompt',
  'confirm',
  'alert',
  'isNaN',
  'encodeURIComponent',
  'decodeURIComponent',
  'getComputedStyle',
  'navigator',
]);
grab(/(?:^|[^.\w$])([a-z][A-Za-z0-9_]{2,})\s*\(/gm, code)
  .filter((n) => !defined.has(n) && !builtins.has(n))
  .forEach((n) => fail.push(`调用了未定义的函数 ${n}()`));

if (warn.length) {
  console.log('提示（不阻断）:');
  warn.forEach((w) => console.log('  ~ ' + w));
  console.log('');
}
if (fail.length) {
  console.log('发现 ' + fail.length + ' 个问题:');
  fail.forEach((f) => console.log('  ✗ ' + f));
  process.exit(1);
}
console.log('静态一致性检查通过：id / 图标 / token / 类 / 函数引用全部闭合');
