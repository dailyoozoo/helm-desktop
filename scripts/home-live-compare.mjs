// 新任务页双端实机比对（第三轮反馈整改验证）：原型 index.html 与实现页在同一
// Chromium 视口下逐状态截图 + 几何 dump，供人工读图核对像素级一致性。
// 用法：node scripts/home-live-compare.mjs   证据输出 .agent/evidence/home-live/
import { createRequire } from 'node:module';
import { spawn } from 'node:child_process';
import fs from 'node:fs';
// 沙箱边界：vite 的 optimizeSafeRealPathSync 在 Windows 上无条件 exec('net use')，
// 文件沙箱禁管道 stdio 子进程（spawn EPERM 同步抛出，构建直接崩）。exec 是 ESM
// 快照绑定，补丁拦不到；改为让 realpathSync.native 抛 EISDIR，命中 vite 的
// 早退分支（safeRealpathSync = JS 版 realpathSync），从根源跳过 net use 探测。
fs.realpathSync.native = () => {
  throw new Error('EISDIR: illegal operation on a directory (sandbox probe bypass)');
};
import fsPromises from 'node:fs/promises';
import http from 'node:http';
import os from 'node:os';
import path from 'node:path';
const react = (await import('@vitejs/plugin-react')).default;
const tailwindcss = (await import('@tailwindcss/vite')).default;
const { build: viteBuild } = await import('vite');

const root = path.resolve(import.meta.dirname, '..');
const port = Number(process.env.HELM_HOME_LIVE_PORT || 4273);
const outputDir = path.join(root, '.agent', 'evidence', 'home-live');
await fsPromises.mkdir(outputDir, { recursive: true });

const chromeCandidates = [
  process.env.CHROME_PATH,
  'C:/Program Files/Google/Chrome/Application/chrome.exe',
  'C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe',
  'C:/Program Files/Microsoft/Edge/Application/msedge.exe',
].filter(Boolean);
const chrome = chromeCandidates.find((candidate) => fs.existsSync(candidate));
if (!chrome) throw new Error('未找到 Chrome 或 Edge；可通过 CHROME_PATH 指定浏览器路径。');

async function waitFor(url, timeoutMs = 15_000) {
  const started = Date.now();
  while (Date.now() - started < timeoutMs) {
    try {
      if ((await fetch(url)).ok) return;
    } catch {
      // 服务仍在启动
    }
    await new Promise((resolve) => setTimeout(resolve, 150));
  }
  throw new Error(`等待服务超时：${url}`);
}

function connectCdp(browserProcess) {
  const input = browserProcess.stdio[3];
  const output = browserProcess.stdio[4];
  let buffer = Buffer.alloc(0);
  const pending = new Map();
  output.on('data', (chunk) => {
    buffer = Buffer.concat([buffer, chunk]);
    let index = buffer.indexOf(0);
    while (index >= 0) {
      const frameText = buffer.slice(0, index).toString('utf8');
      buffer = buffer.slice(index + 1);
      index = buffer.indexOf(0);
      let message;
      try {
        message = JSON.parse(frameText);
      } catch {
        continue;
      }
      if (process.env.HELM_CDP_DEBUG) console.log('[cdp<-]', JSON.stringify(message).slice(0, 160));
      const entry = message.id ? pending.get(message.id) : null;
      if (!entry) continue;
      clearTimeout(entry.timer);
      pending.delete(message.id);
      if (message.error)
        entry.reject(new Error(`${message.error.message} ${message.error.data || ''}`));
      else entry.resolve(message.result ?? {});
    }
  });
  let sequence = 0;
  const rawCall = (method, params = {}, sessionId) =>
    new Promise((resolve, reject) => {
      const id = ++sequence;
      const timer = setTimeout(() => {
        pending.delete(id);
        reject(new Error(`CDP 调用超时：${method}`));
      }, 90_000);
      pending.set(id, { resolve, reject, timer });
      const payload = { id, method, params };
      if (process.env.HELM_CDP_DEBUG) console.log('[cdp->]', JSON.stringify(payload).slice(0, 200));
      if (sessionId) payload.sessionId = sessionId;
      input.write(Buffer.from(JSON.stringify(payload), 'utf8'));
      input.write(Buffer.from([0]));
    });
  console.log('[cdp] createTarget…');
  return rawCall('Target.createTarget', { url: 'about:blank#live' }).then((created) => {
    console.log('[cdp] created', created.targetId);
    return rawCall('Target.attachToTarget', { targetId: created.targetId, flatten: true }).then(
      (attached) => {
        console.log('[cdp] attached', attached.sessionId);
        const call = (method, params = {}) => rawCall(method, params, attached.sessionId);
        // 注：不调用 Runtime.enable——本环境 Chromium 151 管道下该命令偶发挂起；
        // Runtime.evaluate 无需 enable 即可使用（enable 只影响执行上下文事件）。
        return call('Page.enable')
          .then(() => {
            console.log('[cdp] Page.enable ok');
          })
          .then(() => ({ call }));
      },
    );
  });
}

async function evaluate(callObj, expression) {
  const result = await callObj('Runtime.evaluate', {
    expression,
    returnByValue: true,
    awaitPromise: true,
  });
  if (result.exceptionDetails)
    throw new Error(
      `页面脚本执行失败：${result.exceptionDetails.text} ${(result.exceptionDetails.exception?.description || '').slice(0, 300)}`,
    );
  return result.result.value;
}
async function waitForExpression(callObj, expression, timeoutMs = 20_000) {
  const started = Date.now();
  while (Date.now() - started < timeoutMs) {
    if (await evaluate(callObj, expression)) return;
    await new Promise((resolve) => setTimeout(resolve, 120));
  }
  let diag = '';
  try {
    diag = await evaluate(
      callObj,
      "(function(){try{return JSON.stringify({url:location.href.slice(-40),helm:typeof window.Helm,count:(document.querySelector('#readinessCount')||{}).textContent||null,fm:document.querySelectorAll('.floatmenu').length,mode:!!document.getElementById('modeSelect'),errs:(window.__errs||[]).slice(0,3)})}catch(e){return 'diagfail:'+e.message}})()",
    );
  } catch {
    diag = 'diag-unavailable';
  }
  throw new Error(`就绪条件超时：${expression} | diag=${diag}`);
}
async function capture(callObj, name) {
  // ≥1.3s：原型 .cm-motion-reveal 入场动画（translateY 16px→0 + delay）结束后再定格
  await new Promise((resolve) => setTimeout(resolve, 1300));
  const result = await callObj('Page.captureScreenshot', {
    format: 'png',
    captureBeyondViewport: false,
  });
  await fsPromises.writeFile(path.join(outputDir, name), Buffer.from(result.data, 'base64'));
}
const esc = (value) => String(value).split("'").join("\\'");
const clickExpr = (selector) =>
  `(() => { const el = document.querySelector('${esc(selector)}'); if (!el) throw new Error('missing ${esc(selector)}'); el.click(); })()`;
const clickTextExpr = (scopeSelector, text) =>
  `(() => { const scope = document.querySelector('${esc(scopeSelector)}') || document; const list = scope.querySelectorAll('button'); for (const item of list) { if (item.textContent && item.textContent.includes('${esc(text)}')) { item.click(); return; } } throw new Error('missing text ${esc(text)}'); })()`;
const clickSel = (callObj, selector) => evaluate(callObj, clickExpr(selector));
const clickText = (callObj, scopeSelector, text) =>
  evaluate(callObj, clickTextExpr(scopeSelector, text));
const setViewport = (callObj, width, height) =>
  callObj('Emulation.setDeviceMetricsOverride', {
    width,
    height,
    deviceScaleFactor: 1,
    mobile: false,
  });
async function goProto(callObj, url) {
  await evaluate(callObj, "document.documentElement.dataset.theme = 'light'");
  await callObj('Page.navigate', { url });
  // 静态 readinessCount 文本本就含「项就绪」；必须等 app.js 的 Helm 就位且
  // 内联脚本把默认页计数渲染成 3 / 3，才保证监听绑定完成。
  await waitForExpression(
    callObj,
    "Boolean(window.Helm && window.Helm.menu) && (((document.querySelector('#readinessCount') || {}).textContent || '').trim().indexOf('3 / 3') === 0)",
    30_000,
  );
  await evaluate(
    callObj,
    "window.__errs=[]; window.addEventListener('error', function(e){ window.__errs.push(String(e.message)); });",
  );
  // headless 下 reveal 可能停在起始态；禁用入场动画保证测量/截图为布局终位。
  await evaluate(
    callObj,
    "(function(){ var s = document.createElement('style'); s.textContent = '.cm-motion-reveal{transform:none !important}'; document.head.appendChild(s); })()",
  );
  await new Promise((resolve) => setTimeout(resolve, 300));
}
async function goImpl(callObj, url) {
  await evaluate(callObj, "document.documentElement.dataset.theme = 'light'");
  await callObj('Page.navigate', { url });
  await waitForExpression(
    callObj,
    "document.body ? document.body.dataset.visualBoot === 'mounted' : false",
    120_000,
  );
  await evaluate(
    callObj,
    `(() => { const b = document.querySelector('button[aria-label="新任务"]'); if (b) b.click(); })()`,
  );
  await waitForExpression(callObj, "document.querySelectorAll('.cm-composer').length === 1");
}
const rectExpr = (selector) =>
  `(() => { const el = document.querySelector('${esc(selector)}'); if (!el) return null; const r = el.getBoundingClientRect(); return { top: Math.round(r.top), left: Math.round(r.left), w: Math.round(r.width), h: Math.round(r.height) }; })()`;

console.log('[live] 构建实现页…');
const distSrc = path.join(outputDir, 'dist-src');
await fsPromises.rm(distSrc, { recursive: true, force: true });
const ts = (await import('typescript')).default;
const configFile = ts.readConfigFile(path.join(root, 'tsconfig.app.json'), ts.sys.readFile);
const parsedConfig = ts.parseJsonConfigFileContent(configFile.config, ts.sys, root);
const program = ts.createProgram({
  rootNames: [
    path.join(root, 'src', 'visualAuditMain.ts'),
    path.join(root, 'packages', 'protocol', 'src', 'index.ts'),
  ],
  options: {
    ...parsedConfig.options,
    rootDir: root,
    outDir: distSrc,
    noEmit: false,
    declaration: false,
    declarationMap: false,
    sourceMap: false,
    incremental: false,
  },
});
program.emit();
await fsPromises.access(path.join(distSrc, 'src', 'visualAuditMain.js'));
async function copyCss(fromDir, toDir) {
  for (const entry of await fsPromises.readdir(fromDir, { withFileTypes: true })) {
    const from = path.join(fromDir, entry.name);
    const to = path.join(toDir, entry.name);
    if (entry.isDirectory()) await copyCss(from, to);
    else if (entry.name.endsWith('.css')) {
      await fsPromises.mkdir(path.dirname(to), { recursive: true });
      await fsPromises.copyFile(from, to);
    }
  }
}
await copyCss(path.join(root, 'src'), path.join(distSrc, 'src'));
await fsPromises.cp(path.join(root, 'src', 'assets'), path.join(distSrc, 'src', 'assets'), {
  recursive: true,
});
await fsPromises.writeFile(
  path.join(distSrc, 'probe-entry.html'),
  `<!doctype html>
<html lang="zh-CN">
  <head>
    <meta charset="UTF-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1.0" />
    <title>Helm 实机比对</title>
  </head>
  <body>
    <div id="root"></div>
    <script type="module" src="/src/visualAuditMain.js"></script>
  </body>
</html>
`,
);
const requireCjs = createRequire(import.meta.url);
const sharedDefines = path.join(root, '.agent', 'evidence', 'home-diff', 'define-replace.cjs');
const { replaceDefines } = requireCjs(sharedDefines);
const PROBE_ENV_FALLBACK = '({ MODE:"production", PROD:true, DEV:false, BASE_URL:"/", SSR:false })';
const defineStubPlugin = {
  name: 'probe-define-stub',
  enforce: 'pre',
  transform(code, id) {
    if (!id.includes('node_modules')) return null;
    if (!code.includes('process.env') && !code.includes('import.meta.env')) return null;
    const replaced = replaceDefines(code, {
      'process.env.NODE_ENV': '"production"',
      'process.env.VITEST': 'undefined',
      'import.meta.env.MODE': '"production"',
      'import.meta.env.PROD': 'true',
      'import.meta.env.DEV': 'false',
      'import.meta.env.BASE_URL': '"/"',
      'import.meta.env.SSR': 'false',
      'import.meta.env': PROBE_ENV_FALLBACK,
      'process.env': 'globalThis.__PROBE_ENV__',
    });
    return { code: replaced, map: null };
  },
};
const siteDir = path.join(outputDir, 'site');
await viteBuild({
  root: distSrc,
  configFile: false,
  plugins: [defineStubPlugin, react(), tailwindcss()],
  resolve: {
    alias: [
      {
        find: '@helm/protocol',
        replacement: path.join(distSrc, 'packages', 'protocol', 'src', 'index.js'),
      },
      { find: '@', replacement: path.join(distSrc, 'src') },
    ],
  },
  esbuild: false,
  logLevel: 'warn',
  build: {
    outDir: siteDir,
    emptyOutDir: true,
    minify: false,
    cssMinify: false,
    sourcemap: false,
    target: 'esnext',
    rollupOptions: { input: path.join(distSrc, 'probe-entry.html') },
  },
});
console.log('[live] 构建完成，启动静态服务…');

const mimeTypes = {
  '.css': 'text/css',
  '.html': 'text/html; charset=utf-8',
  '.js': 'text/javascript',
  '.png': 'image/png',
  '.svg': 'image/svg+xml',
};
const preview = http.createServer(async (request, response) => {
  try {
    const pathname = decodeURIComponent(new URL(request.url || '/', 'http://127.0.0.1').pathname);
    if (pathname.startsWith('/proto/')) {
      const protoFile = path.resolve(path.join(root, 'prototype'), pathname.slice(7));
      const body2 = await fsPromises.readFile(protoFile);
      response
        .writeHead(200, {
          'Content-Type': mimeTypes[path.extname(protoFile)] || 'application/octet-stream',
          'Cache-Control': 'no-store',
        })
        .end(body2);
      return;
    }
    const relativePath = pathname === '/' ? 'probe-entry.html' : pathname.replace(/^\/+/, '');
    const body2 = await fsPromises.readFile(path.resolve(siteDir, relativePath));
    response
      .writeHead(200, {
        'Content-Type': mimeTypes[path.extname(relativePath)] || 'application/octet-stream',
        'Cache-Control': 'no-store',
      })
      .end(body2);
  } catch {
    response.writeHead(404).end('Not found');
  }
});
await new Promise((resolve, reject) => {
  preview.once('error', reject);
  preview.listen(port, '127.0.0.1', resolve);
});
await waitFor(`http://127.0.0.1:${port}/probe-entry.html`);

const browser = spawn(
  chrome,
  [
    '--headless=new',
    '--disable-gpu',
    '--hide-scrollbars',
    '--no-first-run',
    '--disable-extensions',
    '--disable-background-networking',
    '--remote-debugging-pipe',
    `--user-data-dir=${path.join(os.tmpdir(), `helm-home-live-${process.pid}`)}`,
    '--window-size=1366,768',
    'about:blank',
  ],
  { stdio: ['ignore', 'ignore', 'inherit', 'pipe', 'pipe'], windowsHide: true },
);
const { call } = await connectCdp(browser);

const protoUrl = `http://127.0.0.1:${port}/proto/index.html`;
const implUrl = `http://127.0.0.1:${port}/probe-entry.html?fixture=home-live`;
const geo = {};

async function runState(name, protoFn, implFn, protoShotSel, implShotSel) {
  process.stdout.write(`[live] ${name} … `);
  await setViewport(call, 1366, 768);
  await protoFn();
  await capture(call, `${name}-proto.png`);
  if (protoShotSel) geo[`${name}-proto`] = await evaluate(call, rectExpr(protoShotSel));
  await setViewport(call, 1366, 768);
  await implFn();
  await capture(call, `${name}-impl.png`);
  if (implShotSel) geo[`${name}-impl`] = await evaluate(call, rectExpr(implShotSel));
  console.log('ok');
}

const centerOpenExpr =
  "[...document.querySelectorAll('.home-modal h2')].some((el) => el.textContent === '添加到任务')";
const dirOpenExpr =
  "[...document.querySelectorAll('.home-modal h2')].some((el) => el.textContent === '选择工作目录')";
const floatOpenProto = "Boolean(document.querySelector('.floatmenu'))";
const floatOpenImpl = "Boolean(document.querySelector('.home-floatmenu--fixed'))";

await runState(
  '01-base',
  () => goProto(call, protoUrl),
  () => goImpl(call, implUrl),
);

await runState(
  '02-cap-menu',
  async () => {
    await goProto(call, protoUrl);
    await clickSel(call, '#capTrigger');
    await waitForExpression(call, "!document.querySelector('#capMenu').hidden");
  },
  async () => {
    await goImpl(call, implUrl);
    await clickSel(call, '.cap-anchor > .home-tool');
    await waitForExpression(call, "Boolean(document.querySelector('.cm-menu--above'))");
  },
  '#capMenu',
  '.cm-menu--above',
);

for (const [name, label] of [
  ['03-file-center', '文件与目录'],
  ['04-command-center', '命令与技能'],
]) {
  await runState(
    name,
    async () => {
      await goProto(call, protoUrl);
      await clickSel(call, '#capTrigger');
      await clickText(call, '#capMenu', label);
      await waitForExpression(
        call,
        "document.querySelector('#compactCenter').classList.contains('is-open')",
      );
    },
    async () => {
      await goImpl(call, implUrl);
      await clickSel(call, '.cap-anchor > .home-tool');
      await clickText(call, '.cm-menu--above', label);
      await waitForExpression(call, centerOpenExpr);
    },
    '#compactCenter .cm-modal',
    '.home-modal',
  );
}

const menuStates = [
  ['05-mode-menu', '#modeSelect', 'button[title="任务模式"]'],
  ['06-permission-menu', '#permissionSelect', 'button[title="权限"]'],
  ['07-model-menu', '#modelSelect', 'button[title="模型"]'],
  ['08-effort-menu', '#effortSelect', 'button[aria-label="选择推理强度"]'],
  ['09-engine-menu', '#engineSelect', 'button[title="更换 Agent"]'],
];
for (const [name, protoSel, implSel] of menuStates) {
  await runState(
    name,
    async () => {
      await goProto(call, protoUrl);
      await clickSel(call, protoSel);
      await waitForExpression(call, floatOpenProto);
    },
    async () => {
      await goImpl(call, implUrl);
      await clickSel(call, implSel);
      await waitForExpression(call, floatOpenImpl);
    },
    '.floatmenu',
    '.home-floatmenu--fixed',
  );
}

await runState(
  '10-dir-modal',
  async () => {
    await goProto(call, protoUrl);
    await clickSel(call, '[data-cm-open="folderModal"]');
    await waitForExpression(
      call,
      "document.querySelector('#folderModal').classList.contains('is-open')",
    );
  },
  async () => {
    await goImpl(call, implUrl);
    await clickSel(call, 'button[title="更换工作目录"]');
    await waitForExpression(call, dirOpenExpr);
  },
  '#folderModal .cm-modal',
  '.home-modal',
);

for (const [tag, height] of [
  ['h768', 768],
  ['h1040', 1040],
]) {
  await setViewport(call, 1366, height);
  await goProto(call, protoUrl);
  geo[`base-${tag}-proto`] = {
    start: await evaluate(call, rectExpr('.cm-start')),
    composer: await evaluate(call, rectExpr('.cm-composer')),
    titlebar: await evaluate(call, rectExpr('.cm-titlebar')),
  };
  geo[`computed-${tag}-proto`] = await evaluate(
    call,
    "(function(){var g=function(s,p){var e=document.querySelector(s);return e?getComputedStyle(e)[p]:null};var r=function(s){var e=document.querySelector(s);if(!e)return null;var cs=getComputedStyle(e);return {mt:cs.marginTop,pt:cs.paddingTop,bt:cs.borderTopWidth}};return {main:r('.cm-main'),pb:r('.cm-pagebody'),start:r('.cm-start'),body:r('body'),vh:innerHeight}})()",
  );
  await setViewport(call, 1366, height);
  await goImpl(call, implUrl);
  geo[`base-${tag}-impl`] = {
    start: await evaluate(call, rectExpr('.cm-start')),
    composer: await evaluate(call, rectExpr('.cm-composer')),
    titlebar: await evaluate(call, rectExpr('.titlebar')),
  };
  geo[`computed-${tag}-impl`] = {
    proto: null,
    impl: await evaluate(
      call,
      "(function(){var g=function(s,p){var e=document.querySelector(s);return e?getComputedStyle(e)[p]:null};return {mainPT:g('.home-main','paddingTop')||g('main','paddingTop'),startMT:g('.cm-start','marginTop'),vh:innerHeight}})()",
    ),
  };
}

await fsPromises.writeFile(path.join(outputDir, 'geometry.json'), JSON.stringify(geo, null, 2));
browser.kill();
preview.close();
console.log('[live] 完成：截图与 geometry.json 见 .agent/evidence/home-live/');
