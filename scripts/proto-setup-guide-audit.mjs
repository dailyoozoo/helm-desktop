// 原型走查：setup-guide.html（CLI 安装引导演示页）
// 无头浏览器真实加载，触发依赖状态机（缺失→安装中→已装）、引擎切换、装完解锁，
// 收集 console 错误与异常，并截图留证。
import { spawn } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { pathToFileURL } from 'node:url';

const root = path.resolve(import.meta.dirname, '..');
const outDir = path.join(root, '.agent', 'evidence', 'proto-setup-guide');
fs.mkdirSync(outDir, { recursive: true });

const debugPort = 9334;
const chromeCandidates = [
  process.env.CHROME_PATH,
  'C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe',
  'C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe',
  'C:\\Program Files\\Microsoft\\Edge\\Application\\msedge.exe',
].filter(Boolean);
const chrome = chromeCandidates.find((c) => fs.existsSync(c));
if (!chrome) throw new Error('未找到 Chrome 或 Edge；可用 CHROME_PATH 指定。');

const profile = fs.mkdtempSync(path.join(os.tmpdir(), 'helm-proto-sg-'));
const browser = spawn(
  chrome,
  [
    `--remote-debugging-port=${debugPort}`,
    `--user-data-dir=${profile}`,
    '--headless=new',
    '--disable-gpu',
    '--no-first-run',
    '--no-default-browser-check',
    '--allow-file-access-from-files',
    'about:blank',
  ],
  { stdio: 'ignore' },
);

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function waitFor(url, timeoutMs = 20000) {
  const start = Date.now();
  while (Date.now() - start < timeoutMs) {
    try {
      if ((await fetch(url)).ok) return;
    } catch {
      /* 浏览器还没起来，继续轮询 */
    }
    await sleep(150);
  }
  throw new Error('等待浏览器超时');
}

const problems = [];
let call, socket;

async function connect() {
  await waitFor(`http://127.0.0.1:${debugPort}/json/version`);
  const pages = await (await fetch(`http://127.0.0.1:${debugPort}/json`)).json();
  const page = pages.find((p) => p.type === 'page');
  socket = new WebSocket(page.webSocketDebuggerUrl);
  await new Promise((res, rej) => {
    socket.addEventListener('open', res, { once: true });
    socket.addEventListener('error', rej, { once: true });
  });
  let seq = 0;
  socket.addEventListener('message', (ev) => {
    const m = JSON.parse(ev.data);
    if (m.id) return;
    if (m.method === 'Runtime.exceptionThrown') {
      problems.push(
        '异常: ' + (m.params.exceptionDetails?.exception?.description || '').split('\n')[0],
      );
    }
    if (m.method === 'Runtime.consoleAPICalled' && m.params.type === 'error') {
      problems.push(
        'console.error: ' + m.params.args.map((a) => a.value ?? a.description ?? '').join(' '),
      );
    }
  });
  call = (method, params = {}) =>
    new Promise((res, rej) => {
      const id = ++seq;
      const timer = setTimeout(() => rej(new Error('CDP 超时: ' + method)), 20000);
      const onMsg = (ev) => {
        const m = JSON.parse(ev.data);
        if (m.id !== id) return;
        clearTimeout(timer);
        socket.removeEventListener('message', onMsg);
        if (m.error) rej(new Error(m.error.message));
        else res(m.result);
      };
      socket.addEventListener('message', onMsg);
      socket.send(JSON.stringify({ id, method, params }));
    });
}

const evaluate = async (expr) => {
  const r = await call('Runtime.evaluate', {
    expression: expr,
    returnByValue: true,
    awaitPromise: true,
  });
  if (r.exceptionDetails) throw new Error(expr.slice(0, 60) + ' -> ' + r.exceptionDetails.text);
  return r.result.value;
};

async function shot(name) {
  const r = await call('Page.captureScreenshot', { format: 'png' });
  fs.writeFileSync(path.join(outDir, name + '.png'), Buffer.from(r.data, 'base64'));
}

const checks = [];
const check = (label, ok, detail = '') => {
  checks.push({ label, ok, detail });
  if (!ok) problems.push('断言失败: ' + label + (detail ? ' — ' + detail : ''));
};

try {
  await connect();
  await call('Runtime.enable');
  await call('Page.enable');
  await call('Log.enable');
  await call('Emulation.setDeviceMetricsOverride', {
    width: 1500,
    height: 940,
    deviceScaleFactor: 1,
    mobile: false,
  });

  const url = pathToFileURL(path.join(root, 'prototype', 'setup-guide.html')).href;
  await call('Page.navigate', { url });
  await sleep(1200);

  // 基线：三行依赖渲染
  check('三行依赖渲染', (await evaluate('document.querySelectorAll("#sgList .sgd").length')) === 3);
  check(
    '初始全部缺失态',
    (await evaluate('document.querySelectorAll("#sgList .sgd__st.is-missing").length')) === 3,
  );
  check('引导卡可见', await evaluate('!!document.querySelector("#sgGuide")'));
  check('发送按钮初始禁用', await evaluate('document.getElementById("sgSend").disabled'));
  check('去绑定服务商初始禁用', await evaluate('document.getElementById("sgBind").disabled'));
  check(
    '含国内镜像文案',
    (await evaluate('document.getElementById("sgGuide").textContent')).includes('国内镜像'),
  );
  check(
    '不含「科学上网」字样',
    !(await evaluate('document.getElementById("sgGuide").textContent')).includes('科学上网'),
  );
  check(
    '无跳过/收起按钮（git 强制）',
    await evaluate('!document.getElementById("sgDismiss") && !document.getElementById("sgBar")'),
  );
  await shot('01-initial-missing');

  // 一键安装 Node → 安装中 → 已装
  await evaluate('document.querySelector(\'#sgList [data-install="node"]\').click()');
  await sleep(200);
  check(
    'Node 进入安装中态',
    (await evaluate(
      'document.querySelector(\'[data-dep="node"] .sgd__st.is-installing\') !== null',
    )) &&
      (await evaluate('document.querySelector(\'[data-dep="node"] .sgd__act button\').disabled')),
  );
  await shot('02-installing-node');
  await sleep(2100);
  check(
    'Node 已装',
    await evaluate('document.querySelector(\'[data-dep="node"] .sgd__st.is-ok\') !== null'),
  );

  // 装 git + claude CLI
  await evaluate('document.querySelector(\'#sgList [data-install="git"]\').click()');
  await evaluate('document.querySelector(\'#sgList [data-install="cli"]\').click()');
  await sleep(2500);
  check(
    '全部已装',
    (await evaluate('document.querySelectorAll("#sgList .sgd__st.is-ok").length')) === 3,
  );
  check('发送按钮解锁', !(await evaluate('document.getElementById("sgSend").disabled')));
  check('去绑定服务商解锁', !(await evaluate('document.getElementById("sgBind").disabled')));
  check(
    '引导卡收起',
    await evaluate('document.getElementById("sgGuide").style.display === "none"'),
  );
  await shot('03-all-installed-unlocked');

  // 引擎切换 → Codex（CLI 行回到缺失，Node/Git 保留已装）
  await evaluate('document.querySelectorAll(\'[data-engine="codex"]\')[0].click()');
  await sleep(300);
  check(
    '引擎切换为 Codex',
    (await evaluate('document.getElementById("sgEngineLabel").textContent')).includes('Codex'),
  );
  check(
    'Codex CLI 行为缺失',
    await evaluate('document.querySelector(\'[data-dep="cli"] .sgd__st.is-missing\') !== null'),
  );
  check(
    'Node/Git 保留已装',
    (await evaluate('document.querySelectorAll("#sgList .sgd__st.is-ok").length')) === 2,
  );
  await shot('04-codex-cli-missing');

  // 装 Codex CLI → 全绿 → 解锁
  await evaluate('document.querySelector(\'#sgList [data-install="cli"]\').click()');
  await sleep(2100);
  check('Codex 全绿解锁', !(await evaluate('document.getElementById("sgSend").disabled')));
  await shot('05-codex-unlocked');
} catch (e) {
  problems.push('走查中断: ' + e.message);
} finally {
  try {
    socket?.close();
  } catch {
    /* 连接可能已断，无需处理 */
  }
  browser.kill();
}

const passed = checks.filter((c) => c.ok).length;
console.log(`\n断言 ${passed}/${checks.length} 通过`);
checks
  .filter((c) => !c.ok)
  .forEach((c) => console.log('  ✗ ' + c.label + (c.detail ? ' — ' + c.detail : '')));
if (problems.length) {
  console.log('\n运行时问题 ' + problems.length + ' 条:');
  [...new Set(problems)].forEach((p) => console.log('  ! ' + p));
}
console.log('\n截图输出: ' + outDir);
process.exit(problems.length || passed !== checks.length ? 1 : 0);
