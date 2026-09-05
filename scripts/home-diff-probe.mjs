// 新任务页（PageId home）状态矩阵探针 —— docs/可靠性检查-新任务页-状态矩阵-2026-08-23.md 第一步执行器。
// 复用 visual-audit 的构建/CDP 通道，对矩阵 A/B/H 等区块做真实点击与 DOM/计算样式断言。
// 用法：node scripts/home-diff-probe.mjs [--only=A,B,H]   证据输出 .agent/evidence/home-diff/
import { createRequire } from 'node:module';
import { spawn } from 'node:child_process';
import fs from 'node:fs';
import fsPromises from 'node:fs/promises';
import http from 'node:http';
import os from 'node:os';
import path from 'node:path';
// —— 沙箱边界重构：vite 在 Windows 首次解析模块路径时会 exec('net use') 做网络盘
// 映射优化；本环境的文件沙箱禁止管道 stdio 子进程（spawn EPERM）。该探测只影响
// 网络盘到盘符的路径缩写，本地盘构建不需要：补丁让它安静失败，
// vite 会保持 fs.realpathSync 直读（windowsSafeRealPathSync 的默认分支）。
import childProcess from 'node:child_process';
childProcess.exec = ((command, options, callback) => {
  const cb = typeof options === 'function' ? options : callback;
  if (typeof cb === 'function')
    queueMicrotask(() => cb(new Error('exec disabled for sandbox'), ''));
  return /** @type {any} */ ({});
})();
const react = (await import('@vitejs/plugin-react')).default;
const tailwindcss = (await import('@tailwindcss/vite')).default;
const { build: viteBuild } = await import('vite');

const root = path.resolve(import.meta.dirname, '..');
const port = Number(process.env.HELM_HOME_PROBE_PORT || 4271);
const outputDir =
  process.env.HELM_HOME_EVIDENCE || path.join(root, '.agent', 'evidence', 'home-diff');
const onlyArg = process.argv.find((arg) => arg.startsWith('--only='));
const only = onlyArg ? onlyArg.slice(7).split(',') : null;
const want = (name) => !only || only.includes(name);

const chromeCandidates = [
  process.env.CHROME_PATH,
  'C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe',
  'C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe',
  'C:\\Program Files\\Microsoft\\Edge\\Application\\msedge.exe',
].filter(Boolean);
const chrome = chromeCandidates.find((candidate) => fs.existsSync(candidate));
if (!chrome) throw new Error('未找到 Chrome 或 Edge；可通过 CHROME_PATH 指定浏览器路径。');

async function waitFor(url, timeoutMs = 15_000) {
  const started = Date.now();
  while (Date.now() - started < timeoutMs) {
    try {
      if ((await fetch(url)).ok) return;
    } catch {
      // 服务仍在启动。
    }
    await new Promise((resolve) => setTimeout(resolve, 150));
  }
  throw new Error(`等待服务超时：${url}`);
}

async function connectCdp() {
  // Chromium 151 起远程调试端口/WS 通道对第三方客户端关闭了页面域方法（全部 -32601），
  // 但官方自动化通道 --remote-debugging-pipe（fd3/fd4，空字符分帧 JSON）不受影响：
  // 见 .agent/evidence/home-diff/cdp-pipe-test.mjs。这里以管道承载扁平会话。
  const input = browser.stdio[3];
  const output = browser.stdio[4];
  if (!input || !output) throw new Error('浏览器缺少调试管道 fd3/fd4。');
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
      }, 60_000);
      pending.set(id, { resolve, reject, timer });
      const payload = { id, method, params };
      if (sessionId) payload.sessionId = sessionId;
      input.write(Buffer.from(JSON.stringify(payload), 'utf8'));
      input.write(Buffer.from([0]));
    });
  const created = await rawCall('Target.createTarget', { url: 'about:blank#probe' });
  const attached = await rawCall('Target.attachToTarget', {
    targetId: created.targetId,
    flatten: true,
  });
  const call = (method, params = {}) => rawCall(method, params, attached.sessionId);
  await call('Page.enable');
  await call('Runtime.enable');
  return { call };
}
async function evaluate(call, expression) {
  const result = await call('Runtime.evaluate', {
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
async function waitForExpression(call, expression, timeoutMs = 10_000) {
  const started = Date.now();
  while (Date.now() - started < timeoutMs) {
    if (await evaluate(call, expression)) return;
    await new Promise((resolve) => setTimeout(resolve, 150));
  }
  throw new Error(`就绪条件超时：${expression}`);
}
async function capture(call, name) {
  await new Promise((resolve) => setTimeout(resolve, 500));
  const result = await call('Page.captureScreenshot', {
    format: 'png',
    captureBeyondViewport: false,
  });
  await fsPromises.writeFile(path.join(outputDir, name), Buffer.from(result.data, 'base64'));
}

// —— 断言记录 ——
const failures = [];
const evidence = {};
function record(section, id, pass, detail = '') {
  const line = `${pass ? 'PASS' : 'FAIL'} [${section}] ${id}${detail ? ' — ' + detail : ''}`;
  console.log(line);
  if (!pass) failures.push(line);
  (evidence[section] ||= []).push({ id, pass, detail });
}
const eq = (section, id, actual, expected) =>
  record(
    section,
    id,
    actual === expected,
    `actual=${JSON.stringify(actual)} expected=${JSON.stringify(expected)}`,
  );
const ok = (section, id, cond, detail = '') => record(section, id, Boolean(cond), detail);

await fsPromises.mkdir(outputDir, { recursive: true });
const siteDir = path.join(outputDir, 'site');

// 沙箱内没有 esbuild 可用（spawn EPERM，见 S2 验收记录 §3.1 与本文件头注）：
// 先用进程内 TypeScript API 把 TS/TSX 图谱预编译为 JS，再用 esbuild:false /
// minify:false 的 vite 构建纯 JS 树。产物走同一插件管线（react/tailwind）。
async function buildSite() {
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
  const emitResult = program.emit();
  // 工作区可能存在其他切片的在途改动（类型错误不阻断 JS 产出）。这里只记录不抛错；
  // 真正的语法级失败会表现为入口产物缺失，由下方 access 兜底。
  const diagnostics = ts.getPreEmitDiagnostics(program).concat(emitResult.diagnostics);
  const problems = diagnostics
    .filter((diagnostic) => diagnostic.category === ts.DiagnosticCategory.Error)
    .map((diagnostic) => ts.flattenDiagnosticMessageText(diagnostic.messageText, '\n'));
  if (problems.length) console.log(`[probe] tsc 预编译带 ${problems.length} 条类型诊断（仅告警）`);
  await fsPromises.access(path.join(distSrc, 'src', 'visualAuditMain.js'));
  // tsc 只产出 JS；把 src 树里的样式原样镜像过去供 vite/tailwind 处理。
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
    <title>Helm 视觉审计</title>
  </head>
  <body>
    <div id="root"></div>
    <script>
      window.__visualAuditErrors = [];
      window.__PROBE_ENV__ = {};
      window.addEventListener('error', (event) => window.__visualAuditErrors.push(event.message));
      window.addEventListener('unhandledrejection', (event) =>
        window.__visualAuditErrors.push(String(event.reason?.stack || event.reason)),
      );
    </script>
    <script type="module" src="/src/visualAuditMain.js"></script>
  </body>
</html>
`,
  );
  // 在 vite:define 之前把 node_modules 里的 process.env / import.meta.env 全部替换掉，
  // 使其 pattern.test 永不命中，从根源跳过 esbuild.transform（沙箱禁管道子进程）。
  const requireCjs = createRequire(import.meta.url);
  const { replaceDefines } = requireCjs(path.join(outputDir, 'define-replace.cjs'));
  const PROBE_ENV_FALLBACK =
    '({ MODE:"production", PROD:true, DEV:false, BASE_URL:"/", SSR:false })';
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
}
await buildSite();
console.log('视觉入口构建完成（tsc 预编译 + 无 esbuild vite）');

const mimeTypes = {
  '.css': 'text/css',
  '.html': 'text/html; charset=utf-8',
  '.js': 'text/javascript',
  '.png': 'image/png',
  '.svg': 'image/svg+xml',
  '.woff2': 'font/woff2',
};
const preview = http.createServer(async (request, response) => {
  try {
    const pathname = decodeURIComponent(new URL(request.url || '/', 'http://127.0.0.1').pathname);
    if (pathname.startsWith('/proto/')) {
      const protoFile = path.resolve(path.join(root, 'prototype'), pathname.slice(7));
      const body = await fsPromises.readFile(protoFile);
      response
        .writeHead(200, {
          'Content-Type': mimeTypes[path.extname(protoFile)] || 'application/octet-stream',
          'Cache-Control': 'no-store',
        })
        .end(body);
      return;
    }
    const relativePath = pathname === '/' ? 'probe-entry.html' : pathname.replace(/^\/+/, '');
    const filePath = path.resolve(siteDir, relativePath);
    const body = await fsPromises.readFile(filePath);
    response
      .writeHead(200, {
        'Content-Type': mimeTypes[path.extname(filePath)] || 'application/octet-stream',
        'Cache-Control': 'no-store',
      })
      .end(body);
  } catch {
    response.writeHead(404).end('Not found');
  }
});
await new Promise((resolve, reject) => {
  preview.once('error', reject);
  preview.listen(port, '127.0.0.1', resolve);
});
const browser = spawn(
  chrome,
  [
    '--headless=new',
    '--disable-gpu',
    '--hide-scrollbars',
    '--allow-file-access-from-files',
    '--no-first-run',
    '--disable-extensions',
    '--disable-background-networking',
    '--remote-debugging-pipe',
    `--user-data-dir=${path.join(os.tmpdir(), `helm-home-probe-${process.pid}`)}`,
    '--window-size=1366,768',
    'about:blank',
  ],
  { stdio: ['ignore', 'ignore', 'ignore', 'pipe', 'pipe'], windowsHide: true },
);

// —— 页面交互助手 ——
const esc = (value) => String(value).replace(/'/g, "\\'");
let call;
async function goHome(variant = '') {
  await call('Emulation.setDeviceMetricsOverride', {
    width: 1366,
    height: 768,
    deviceScaleFactor: 1,
    mobile: false,
  });
  await evaluate(call, `document.documentElement.dataset.theme = 'light'`);
  await call('Page.navigate', {
    url: `http://127.0.0.1:${port}/probe-entry.html${variant ? `?fixture=${variant}` : ''}`,
  });
  await waitForExpression(call, `document.body?.dataset.visualBoot === 'mounted'`, 120_000);
  await evaluate(call, `document.querySelector('button[aria-label="新任务"]')?.click()`);
  await waitForExpression(call, `document.querySelectorAll('.cm-composer').length === 1`);
}
async function goProto() {
  await call('Emulation.setDeviceMetricsOverride', {
    width: 1366,
    height: 768,
    deviceScaleFactor: 1,
    mobile: false,
  });
  await evaluate(call, `document.documentElement.dataset.theme = 'light'`);
  await call('Page.navigate', { url: `http://127.0.0.1:${port}/proto/index.html` });
  await waitForExpression(call, `Boolean(document.querySelector('.cm-start'))`, 20000);
  // headless 下 IntersectionObserver 可能不触发，reveal 停在起始 translateY(16px)
  // 造成测量漂移；探针统一禁用原型入场动画后测量布局终位。
  await evaluate(
    call,
    "(function(){ var s = document.createElement('style'); s.textContent = '.cm-motion-reveal{transform:none !important}'; document.head.appendChild(s); })()",
  );
  await new Promise((resolve) => setTimeout(resolve, 300));
}
async function measureGeo() {
  return evaluate(
    call,
    `(() => {
    const rect = (sel) => { const el = document.querySelector(sel); if (!el) return null; const r = el.getBoundingClientRect(); return { top: Math.round(r.top), h: Math.round(r.height), left: Math.round(r.left), w: Math.round(r.width) }; };
    const start = rect('.cm-start') || rect('.home-start');
    const composer = rect('.cm-composer') || rect('.cm-composer');
    const h1 = rect('.cm-start__heading h1') || rect('.home-start__titleline h1');
    const meta = rect('.cm-meta') || rect('.home-meta');
    const starters = rect('.cm-starters') || rect('.home-starters');
    const container = rect('.home--start');
    return { viewportH: window.innerHeight, startTop: start?.top ?? -1, startH: start?.h ?? -1, composerTop: composer?.top ?? -1, composerH: composer?.h ?? -1, headingTop: h1?.top ?? -1, metaTop: meta?.top ?? -1, startersTop: starters?.top ?? -1, startersH: starters?.h ?? 0, containerTop: container?.top ?? -1, containerH: container?.h ?? -1 };
  })()`,
  );
}
const setText = (selector, value, inputEl = false) =>
  evaluate(
    call,
    `(() => {
      const el = document.querySelector('${esc(selector)}');
      if (!el) throw new Error('缺少元素 ' + '${esc(selector)}');
      const proto = ${inputEl ? 'HTMLInputElement' : 'HTMLTextAreaElement'}.prototype;
      Object.getOwnPropertyDescriptor(proto, 'value').set.call(el, '${esc(value)}');
      el.dispatchEvent(new Event('input', { bubbles: true }));
    })()`,
  );
const clickEl = (selector) =>
  evaluate(
    call,
    `(() => { const el = document.querySelector('${esc(selector)}'); if (!el) throw new Error('缺少元素 ' + '${esc(selector)}'); el.click(); })()`,
  );
const clickText = (scopeSelector, text) =>
  evaluate(
    call,
    `(() => {
      const scope = document.querySelector('${esc(scopeSelector)}') || document;
      const target = [...scope.querySelectorAll('button')].find((item) => item.textContent?.includes('${esc(text)}'));
      if (!target) throw new Error('找不到按钮文本 ' + '${esc(text)}');
      target.click();
    })()`,
  );
const getText = (selector) =>
  evaluate(call, `document.querySelector('${esc(selector)}')?.textContent?.trim() ?? null`);
const exists = (selector) => evaluate(call, `Boolean(document.querySelector('${esc(selector)}'))`);
const toasts = () =>
  evaluate(
    call,
    `[...document.querySelectorAll('[role="status"]')].map((el) => el.textContent.trim())`,
  );
async function openReadiness() {
  await setText('.cm-composer textarea', '审查当前改动');
  await clickEl('.cm-tool--send');
  await waitForExpression(call, `Boolean(document.querySelector('.cm-readiness'))`);
}
const readModal = () =>
  evaluate(
    call,
    `(() => ({
      rows: [...document.querySelectorAll('.cm-readiness__row')].map((row) => ({
        key: row.dataset.readinessKey,
        cls: row.className,
        title: row.querySelector('b')?.textContent ?? '',
        detail: row.querySelector('small')?.textContent ?? '',
        action: row.querySelector('.cm-action')?.textContent ?? null,
        doneHidden: row.querySelector('.cm-readiness__done')?.hidden ?? true,
        deps: [...row.querySelectorAll('.cm-readiness__dep')].map((dep) => dep.className),
      })),
      count: document.querySelector('.cm-readiness__count')?.textContent ?? '',
      rail: [...document.querySelectorAll('.cm-readiness__rail > span')].map((span) => span.className),
      note: document.querySelector('.cm-readiness__note')?.textContent?.trim() ?? '',
      continueHidden: document.querySelector('.cm-action--primary')?.hidden ?? true,
    }))()`,
  );
const closeReadiness = () =>
  evaluate(
    call,
    `document.querySelector('.cm-readiness .btn-icon, .cm-readiness .btn-icon')?.click()`,
  );

try {
  await waitFor(`http://127.0.0.1:${port}/probe-entry.html`, 20_000);
  ({ call } = await connectCdp());

  // ========== A 挂载与数据加载 ==========
  if (want('A')) {
    await goHome();
    // 第二轮决议：副行移除、品牌图标替代（见 W 段断言）。
    ok('A', 'H-02 引擎品牌图', await exists('.cm-meta-select__icon img'));
    eq('A', 'H-02 引擎名', await getText('.cm-meta-select b'), 'Claude Code');
    eq(
      'A',
      'H-02 目录名',
      await getText('.cm-start-meta > button.cm-meta-select:not(#engineSelect) b'),
      'helm',
    );
    eq('A', 'H-02 模型 chip', await getText('[title="模型"] span.mono'), 'claude-sonnet-4-6');
    eq('A', 'H-02 强度 chip', await getText('[title="推理强度"] span'), '自动');

    ok('A', 'H-02 发送不 blocked', !(await exists('.cm-tool--send.is-blocked')));
    await capture(call, 'A-H02-default-light.png');

    await goHome('home-pending');

    eq('A', 'H-01 模型 chip 未绑定', await getText('[title="模型"] span.mono'), '未绑定模型');
    ok('A', 'H-01 发送 blocked', await exists('.cm-tool--send.is-blocked'));
    await openReadiness();
    const m1 = await readModal();
    eq('A', 'H-01 计数（种子目录即就绪）', m1.count, '1 / 3 项就绪');
    eq('A', 'H-01 agent 缺失', m1.rows[0].cls.includes('is-missing'), true);
    eq('A', 'H-01 服务商缺失', m1.rows[1].cls.includes('is-missing'), true);
    eq('A', 'H-01 目录种子就绪', m1.rows[2].cls.includes('is-ready'), true);
    eq('A', 'H-01 agent detail', m1.rows[0].detail, '需要安装 Claude Code 与 Git');
    eq('A', 'H-01 继续发送隐藏', m1.continueHidden, true);
    await capture(call, 'A-H01-pending-modal.png');

    await goHome('home-reject-report');
    await waitForExpression(
      call,
      `[...document.querySelectorAll('[role="status"]')].some((el) => el.textContent.includes('就绪检查失败'))`,
      8000,
    );
    ok(
      'A',
      'H-03 toast 文案',
      (await toasts()).some((t) => t.includes('就绪检查失败：无法读取本地环境报告')),
    );

    await goHome('home-reject-config');
    await waitForExpression(
      call,
      `[...document.querySelectorAll('[role="status"]')].some((el) => el.textContent.includes('AI 配置读取失败'))`,
      8000,
    );
    eq('A', 'H-04 模型 chip 未绑定', await getText('[title="模型"] span.mono'), '未绑定模型');
    await clickEl('[title="模型"]');
    await waitForExpression(call, `Boolean(document.querySelector('.home-floatmenu'))`);
    // 第四轮：空态带引导副行；推理强度改为按引擎静态档位表（不再随模型探测为空）
    ok(
      'A',
      'H-04 模型菜单空态含引导文案',
      (await getText('.home-menu__empty')).includes('当前引擎尚未绑定模型'),
    );
    await clickEl('.home-overlay');
    await clickEl('[title="推理强度"]');
    eq(
      'A',
      'H-04 强度按引擎目录展示（claude-code 六档）',
      await evaluate(call, `[...document.querySelectorAll('.home-floatmenu__item')].length`),
      6,
    );
    await clickEl('.home-overlay');

    await goHome('home-reject-skills');
    await setText('.cm-composer textarea', '/');
    await waitForExpression(call, `Boolean(document.querySelector('.cm-command-list'))`);
    eq(
      'A',
      'H-05 命令中心空态',
      await getText('.cm-command-list .home-menu__empty'),
      '当前环境没有可用命令或技能',
    );
  }

  // ========== B 就绪三行组合 ==========
  if (want('B')) {
    // 目录行按 2026-08-23 种子决策恒就绪（defaultDirectory=C:\\code\\helm）。
    const cases = [
      [
        'home-r2',
        '1 / 3 项就绪',
        [
          ['agent', 'is-missing', '需要安装 Claude Code 与 Git', '安装 Agent 与 Git'],
          ['provider', 'is-missing', '尚无可用于当前 Agent 的服务商', '去配置'],
          ['directory', 'is-ready', '当前目录 · helm', null],
        ],
      ],
      [
        'home-r3',
        '2 / 3 项就绪',
        [
          ['agent', 'is-missing', 'Git 已就绪，还需安装 Claude Code', '下载并安装 Agent'],
          ['provider', 'is-ready', '已配置当前 Agent 可用的服务商', null],
          ['directory', 'is-ready', '当前目录 · helm', null],
        ],
      ],
      // r4：hasReadyProvider=true 但 boundEngines 为空 → 服务商未绑定当前引擎仍算缺失。
      [
        'home-r4',
        '1 / 3 项就绪',
        [
          ['agent', 'is-missing', 'Claude Code 可运行，还需安装 Git', '安装 Git'],
          ['provider', 'is-missing', '尚无可用于当前 Agent 的服务商', '去配置'],
          ['directory', 'is-ready', '当前目录 · helm', null],
        ],
      ],
      [
        'home-r6',
        '2 / 3 项就绪',
        [
          ['agent', 'is-ready', 'Agent CLI 与 Git 均已通过检测', null],
          ['provider', 'is-missing', '尚无可用于当前 Agent 的服务商', '去配置'],
          ['directory', 'is-ready', '当前目录 · helm', null],
        ],
      ],
    ];
    for (const [variant, count, rowsExpect] of cases) {
      await goHome(variant);
      await openReadiness();
      const modal = await readModal();
      eq('B', `${variant} 计数`, modal.count, count);
      for (const [key, cls, detail, action] of rowsExpect) {
        const row = modal.rows.find((item) => item.key === key);
        if (!row) {
          record('B', `${variant}:${key}`, false, '缺行');
          continue;
        }
        ok('B', `${variant}:${key} 类名含 ${cls}`, row.cls.includes(cls), row.cls);
        eq('B', `${variant}:${key} detail`, row.detail, detail);
        eq('B', `${variant}:${key} action`, row.action, action);
        if (cls === 'is-ready')
          ok('B', `${variant}:${key} 已就绪标签可见`, row.doneHidden === false);
      }
      eq('B', `${variant} 继续发送可见性`, modal.continueHidden, count !== '3 / 3 项就绪');
      await capture(call, `B-${variant}.png`);
      await closeReadiness();
    }
    // r7：三项全就绪（种子目录），发送应直接开始任务而不弹就绪层。
    await goHome('home-r7');
    await setText('.cm-composer textarea', '审查当前改动');
    await clickEl('.cm-tool--send');
    await waitForExpression(call, `!document.querySelector('.cm-composer')`, 15000);
    ok('B', 'r7 全就绪直接发送', !(await exists('.cm-readiness')));

    await goHome('home-r2');
    await openReadiness();
    const m2 = await readModal();
    ok(
      'B',
      'R-2 deps 双缺失',
      m2.rows[0].deps.every((dep) => dep.includes('is-missing')),
      JSON.stringify(m2.rows[0].deps),
    );
    await closeReadiness();
    await goHome('home-r4');
    await openReadiness();
    const m4 = await readModal();
    eq(
      'B',
      'R-4 deps CLI ok/Git missing',
      JSON.stringify(m4.rows[0].deps.map((dep) => (dep.includes('is-ready') ? 'ok' : 'missing'))),
      JSON.stringify(['ok', 'missing']),
    );
    await closeReadiness();
  }

  // ========== H 安装链路 ==========
  if (want('H')) {
    const READY = {
      installed: true,
      path: 'C:\\Users\\demo\\AppData\\Roaming\\npm\\claude.cmd',
      version: '2.1.206',
      error: null,
      login: { state: 'ok', detail: '订阅登录有效' },
    };
    const readyReport = JSON.stringify({
      claudeCode: READY,
      codex: {
        ...READY,
        path: 'C:\\Users\\demo\\AppData\\Roaming\\npm\\codex.cmd',
        version: '0.144.1',
      },
      hasProvider: true,
      hasReadyProvider: true,
      defaultEngine: 'claude-code',
      boundEngines: ['claude-code', 'codex'],
      cwd: { configured: true, exists: true, path: 'C:\\code\\helm' },
    });
    const gitOk = JSON.stringify({
      node: { available: true, version: 'v22.14.0' },
      npm: { available: true, version: '10.9.2' },
      git: { available: true, version: 'git version 2.47.1.windows.1' },
    });

    // I-1/I-2 安装进行中 → 复检通过
    await goHome('home-r2');
    await evaluate(
      call,
      `window.__setFixture('install_cli_engine', () => new Promise((resolve) => setTimeout(() => resolve({ path: 'x', version: '2.1.206', output: '' }), 1200)))`,
    );
    await evaluate(call, `window.__setFixture('get_readiness_report', ${readyReport})`);
    await evaluate(call, `window.__setFixture('detect_workspace_deps', ${gitOk})`);
    await openReadiness();
    await clickText('.cm-readiness', '安装 Agent 与 Git');
    await waitForExpression(
      call,
      `document.querySelector('[data-readiness-key="agent"] .cm-action')?.disabled === true`,
    );
    eq(
      'H',
      'I-1 按钮 正在准备…',
      await getText('[data-readiness-key="agent"] .cm-action'),
      '正在准备…',
    );
    ok('H', 'I-1 agent 行 installing', (await readModal()).rows[0].cls.includes('is-installing'));
    await capture(call, 'H-I1-installing.png');
    await waitForExpression(
      call,
      `[...document.querySelectorAll('[role="status"]')].some((el) => el.textContent.includes('复检通过'))`,
      15_000,
    );
    ok(
      'H',
      'I-2 复检通过 toast',
      (await toasts()).some((t) => t.includes('复检通过：Agent 与 Git 均已就绪')),
    );
    const modalOk = await readModal();
    eq('H', 'I-2 三项全就绪', modalOk.count, '3 / 3 项就绪');
    eq('H', 'I-2 继续发送出现', modalOk.continueHidden, false);

    // I-4 复检仍未通过
    await goHome('home-r2');
    await evaluate(
      call,
      `window.__setFixture('install_node', () => Promise.resolve({ path: 'n', version: 'v22', restartRequired: false }))`,
    );
    await evaluate(
      call,
      `window.__setFixture('install_git', () => Promise.resolve({ path: 'g', version: 'git', restartRequired: false }))`,
    );
    await openReadiness();
    await clickText('.cm-readiness', '安装 Agent 与 Git');
    await waitForExpression(
      call,
      `[...document.querySelectorAll('[role="status"]')].some((el) => el.textContent.includes('复检仍未通过'))`,
      15_000,
    );
    ok(
      'H',
      'I-4 toast',
      (await toasts()).some((t) => t.includes('安装动作已执行，复检仍未通过')),
    );
    ok('H', 'I-4 foot note', (await readModal()).note.includes('复检未通过'));

    // I-3 restartRequired
    await goHome('home-r2');
    await evaluate(
      call,
      `window.__setFixture('install_node', () => Promise.resolve({ path: 'n', version: 'v22', restartRequired: false }))`,
    );
    await evaluate(
      call,
      `window.__setFixture('install_git', () => Promise.resolve({ path: 'g', version: 'git', restartRequired: true }))`,
    );
    await openReadiness();
    await clickText('.cm-readiness', '安装 Agent 与 Git');
    await waitForExpression(
      call,
      `[...document.querySelectorAll('[role="status"]')].some((el) => el.textContent.includes('需要重启 Helm'))`,
      15_000,
    );
    ok(
      'H',
      'I-3 restart toast',
      (await toasts()).some((t) =>
        t.includes('安装完成，但需要重启 Helm 刷新 PATH 后复检才会通过'),
      ),
    );

    // I-5 安装失败
    await goHome('home-r2');
    await evaluate(
      call,
      `window.__setFixture('install_cli_engine', () => Promise.reject(new Error('网络不可达')))`,
    );
    await openReadiness();
    await clickText('.cm-readiness', '安装 Agent 与 Git');
    await waitForExpression(
      call,
      `[...document.querySelectorAll('[role="status"]')].some((el) => el.textContent.includes('安装未完成'))`,
      15_000,
    );
    ok(
      'H',
      'I-5 失败 toast',
      (await toasts()).some((t) => t.includes('安装未完成：网络不可达')),
    );
    ok('H', 'I-5 foot note 显示 failure', (await readModal()).note.includes('网络不可达'));
  }

  // ========== V 第三步修复验证 ==========
  if (want('V')) {
    await goHome();
    ok('V', 'D-01 标题徽标存在', await exists('.cm-start__logo svg'));
    eq(
      'V',
      'D-01 徽标描边 1.55',
      await evaluate(
        call,
        `getComputedStyle(document.querySelector('.cm-start__logo svg')).strokeWidth`,
      ),
      '1.55px',
    );
    // D-02 药丸 accent 体系：经 @ 中心挂真实文件。
    // 2026-09 四次修订：应用内浏览——空关键词即出当前层条目（fixture 返回
    // src/workspace/Composer.tsx、src/workspace/、docs/PRD.md），点「文件」行挂药丸。
    await setText('.cm-composer textarea', '@');
    await waitForExpression(call, `Boolean(document.querySelector('.cm-search input'))`, 8000);
    await waitForExpression(
      call,
      `Boolean(document.querySelector('.cm-command-list .cm-command-row[data-home-add-dir]'))`,
      8000,
    );
    // 等结果行（带 __meta 文件角标）出现再点，避免防抖窗口内误点「添加此目录」。
    await waitForExpression(
      call,
      `Boolean(document.querySelector('.cm-command-list .cm-command-row .cm-command-row__meta'))`,
      8000,
    );
    await evaluate(
      call,
      `document.querySelector('.cm-command-list .cm-command-row .cm-command-row__meta').closest('button').click()`,
    );
    await waitForExpression(call, `Boolean(document.querySelector('.cm-pills .cpill'))`, 8000);
    const pill = await evaluate(
      call,
      `(() => {
      const p = document.querySelector('.cm-pills .cpill');
      const cs = getComputedStyle(p);
      return { radius: cs.borderRadius, minH: cs.minHeight, fs: cs.fontSize, bg: cs.backgroundColor };
    })()`,
    );
    eq('V', 'D-02 药丸方角 r-xs', pill.radius, '5px');
    eq('V', 'D-02 药丸最小高', pill.minH, '24px');
    eq('V', 'D-02 药丸字号', pill.fs, '10.5px');
    ok(
      'V',
      'D-02 药丸非透明底',
      !pill.bg
        .replace(/rgba?\(|\)/g, '')
        .split(',')
        .slice(-1)[0]
        .trim()
        .startsWith('0)') && pill.bg !== 'transparent',
    );
    // D-04 浮层菜单形态（模式菜单）。
    await clickEl('[title="任务模式"]');
    await waitForExpression(call, `Boolean(document.querySelector('.home-floatmenu'))`, 8000);
    eq(
      'V',
      'D-04 条目字号 12.5',
      await evaluate(
        call,
        `getComputedStyle(document.querySelector('.home-floatmenu__item')).fontSize`,
      ),
      '12.5px',
    );
    eq(
      'V',
      'D-04 条目上内边距 7px',
      await evaluate(
        call,
        `getComputedStyle(document.querySelector('.home-floatmenu__item')).paddingTop`,
      ),
      '7px',
    );
    eq(
      'V',
      'D-04 容器圆角 r=9px',
      await evaluate(
        call,
        `getComputedStyle(document.querySelector('.home-floatmenu')).borderRadius`,
      ),
      '9px',
    );
    ok('V', 'D-04 hint 右侧存在', await exists('.home-floatmenu__hint'));
    ok('V', 'D-04 desc 下挂存在', await exists('.home-floatmenu__copy small'));
    await evaluate(call, `document.querySelector('.home-overlay')?.click()`);
    // D-08 强度图标 gauge。
    await clickEl('[title="推理强度"]');
    await waitForExpression(call, `Boolean(document.querySelector('.home-floatmenu'))`, 8000);
    ok(
      'V',
      'D-08 触发钮与菜单均为 gauge',
      await evaluate(
        call,
        `[...document.querySelectorAll('[title="推理强度"] path, .home-floatmenu path')].some((p) => p.getAttribute('d') === 'm12 14 4-4')`,
      ),
    );
    await evaluate(call, `document.querySelector('.home-overlay')?.click()`);
    // D-06 composer 聚焦反馈（第五轮修订）：聚焦只允许描边+阴影，禁止 transform——
    // transform 会把 composer 变成 fixed 浮层的包含块与独立 stacking context，
    // 真实鼠标点击聚焦后浮层整体右移/被遮罩拦截（新任务页四弹层失灵的根因）。
    await evaluate(call, `document.querySelector('.cm-composer textarea').focus()`);
    await waitForExpression(
      call,
      `getComputedStyle(document.querySelector('.cm-composer')).borderColor !== 'rgba(0, 0, 0, 0)'`,
      4000,
    );
    eq(
      'V',
      'D-06 聚焦无 transform（浮层包含块守卫）',
      await evaluate(call, `getComputedStyle(document.querySelector('.cm-composer')).transform`),
      'none',
    );
    // D-07 描边分档抽查（工具钮 1.7 / meta 图标 1.75）。
    eq(
      'V',
      'D-07 工具钮描边 1.7',
      await evaluate(call, `getComputedStyle(document.querySelector('.cm-tool svg')).strokeWidth`),
      '1.7px',
    );
    eq(
      'V',
      'D-07 meta 图标描边 1.75',
      await evaluate(
        call,
        `getComputedStyle(document.querySelector('.cm-meta-select__icon svg')).strokeWidth`,
      ),
      '1.75px',
    );
    // D-05 就绪弹层 padding 20px + item-icon 描边 1.65（未就绪才能打开弹层）。
    await goHome('home-pending');
    await openReadiness();
    const readinessStyle = await evaluate(
      call,
      `(() => {
      const m = document.querySelector('.cm-readiness');
      const cs = getComputedStyle(m);
      const icon = document.querySelector('.cm-readiness__item-icon svg');
      return { pad: cs.paddingTop, sw: icon ? getComputedStyle(icon).strokeWidth : null };
    })()`,
    );
    eq('V', 'D-05 弹层上内边距 20px', readinessStyle.pad, '20px');
    if (readinessStyle.sw) eq('V', 'D-07 行图标描边 1.65', readinessStyle.sw, '1.65px');
    await closeReadiness();
    // D-09 文案三处。
    await goHome();
    await setText('.cm-composer textarea', '/');
    await waitForExpression(call, `Boolean(document.querySelector('.cm-search input'))`, 8000);
    eq(
      'V',
      'D-09 命令中心占位符',
      await evaluate(call, `document.querySelector('.cm-search input').placeholder`),
      // 原型 index.html:450 JS 按中心切占位：命令中心「搜索命令」、文件中心「搜索文件与目录」。
      '搜索命令',
    );
    ok(
      'V',
      'D-09 中心描述含「以 / 调用」',
      (await getText('.home-modal__head p')).includes('技能同样以 / 调用。'),
    );
    ok('V', 'D-11 内置命令分组头', await exists('.cm-command-head'));
    await evaluate(call, `document.querySelector('[aria-label="关闭"], .btn-icon').click()`);
    // D-10 从电脑选择文件行。
    await evaluate(
      call,
      `(() => {
      const ta = document.querySelector('.cm-composer textarea');
      const set = Object.getOwnPropertyDescriptor(window.HTMLTextAreaElement.prototype, 'value').set;
      set.call(ta, '@');
      ta.dispatchEvent(new Event('input', { bubbles: true }));
    })()`,
    );
    // D-10 应用内浏览（2026-09 四次修订）：@ 打开即出当前层条目 +「添加此目录」
    // +「上一层」。点目录行进入子目录（路径头变化）；「添加此目录」挂药丸并关闭。
    await waitForExpression(call, `Boolean(document.querySelector('.cm-search input'))`, 8000);
    await waitForExpression(
      call,
      `Boolean(document.querySelector('.cm-command-list .cm-command-row[data-home-add-dir]'))`,
      6000,
    );
    ok('V', 'D-10 浏览动作行', await exists('[data-home-add-dir]'));
    ok('V', 'D-10 上一层行', await exists('[data-home-browse-up]'));
    await waitForExpression(
      call,
      `Boolean(document.querySelector('.cm-command-list .cm-command-row .cm-command-row__meta'))`,
      8000,
    );
    ok(
      'V',
      'D-10 空关键词即浏览列表',
      await exists('.cm-command-list .cm-command-row .cm-command-row__meta'),
    );
    // 点目录行（meta=进入）进子目录：路径头应变为子目录绝对路径。
    await evaluate(
      call,
      `document.querySelector('.cm-command-list .cm-command-row .cm-command-row__meta').closest('button')` +
        " && document.evaluate(\"//span[@class='cm-command-row__meta'][text()='进入']\", document, null, XPathResult.FIRST_ORDERED_NODE_TYPE, null).singleNodeValue.closest('button').click()",
    );
    await waitForExpression(
      call,
      `(() => { const h = document.querySelector('.cm-command-head'); return h && h.textContent.includes('src/workspace'); })()`,
      8000,
    );
    ok('V', 'D-10 目录行进入子目录', (await getText('.cm-command-head')).includes('src/workspace'));
    // 「添加此目录」：挂药丸并关闭弹框（三次修订：选中后弹框必须关闭）。
    await evaluate(call, `document.querySelector('[data-home-add-dir]').click()`);
    await waitForExpression(call, `Boolean(document.querySelector('.cm-pills .cpill'))`, 6000);
    await waitForExpression(call, `!document.querySelector('.cm-search input')`, 6000);
    ok('V', 'D-10 添加此目录后弹框关闭', !(await exists('.cm-search input')));
    await evaluate(call, `document.querySelector('[aria-label="关闭"], .btn-icon').click()`);
    // D-13 草稿保护：跳服务商再返回，文本保留。
    await evaluate(
      call,
      `(() => {
      const ta = document.querySelector('.cm-composer textarea');
      const set = Object.getOwnPropertyDescriptor(window.HTMLTextAreaElement.prototype, 'value').set;
      set.call(ta, '草稿保护验证 ABC');
      ta.dispatchEvent(new Event('input', { bubbles: true }));
    })()`,
    );
    await clickEl('[title="模型"]');
    await clickText('.home-floatmenu', '更改服务商绑定…');
    await new Promise((r2) => setTimeout(r2, 2500));
    // 只断言「已离开新任务页」：服务商页头部类名属 AI 配置切片在途改版（.providers-head
    // 已被移除，P 段同因挂、随其修复），D-13 关心的是草稿暂存/恢复本身。
    await waitForExpression(call, `!document.querySelector('.cm-composer')`, 15000);
    await goHome();
    eq(
      'V',
      'D-13 草稿恢复',
      await evaluate(call, `document.querySelector('.cm-composer textarea').value`),
      '草稿保护验证 ABC',
    );
    // 用户决议（2026-09 四次修订）：未选工作目录时 @/菜单直接开文件中心（单一弹框，
    // 不两步跳目录弹层）；框内是「选择开始位置…」入口（系统目录选择器只用于挑起点，
    // 浏览与选择都在弹框内完成）。
    // home-no-dir：设置无默认目录 + 就绪报告无 cwd（home-r2 的报告无 cwd 但设置仍带
    // 默认目录，页面种子会把它填上，测不到真正的未选态）。
    await goHome('home-no-dir');
    await setText('.cm-composer textarea', '@');
    await waitForExpression(call, `Boolean(document.querySelector('.cm-search input'))`, 8000);
    eq(
      'V',
      'D-10 无目录直开文件中心',
      await evaluate(call, `document.querySelector('.cm-search input').placeholder`),
      '搜索文件与目录',
    );
    ok(
      'V',
      'D-10 无目录说明文案',
      (await getText('.home-modal__head p')).includes('浏览电脑上的文件与目录'),
    );
    ok('V', 'D-10 无目录选择开始位置行', await exists('[data-home-browse-start]'));
    ok(
      'V',
      'D-10 无目录无框内选目录行',
      !(await exists('[data-home-pick-dir-from-center], [data-home-add-dir]')),
    );
  }

  // ========== W 第二轮用户反馈整改验证 ==========
  if (want('W')) {
    await goHome();
    // W-1 标题栏（第三轮决议修正）：左品牌 logo+Helm、右仅三键，无中间标题与搜索
    ok('W', '标题栏含左品牌', await exists('.titlebar--home .titlebar__brand'));
    eq(
      'W',
      '标题栏无中间文本',
      await evaluate(call, `Boolean(document.querySelector('.titlebar__center span'))`),
      false,
    );
    eq(
      'W',
      '标题栏无搜索按钮',
      await evaluate(call, `Boolean(document.querySelector('.titlebar__k'))`),
      false,
    );
    eq(
      'W',
      '标题栏仅三键',
      await evaluate(call, `document.querySelectorAll('.win-caption button').length`),
      3,
    );
    // W-1b 五轮决议（2026-08-24）：三键左侧恢复紧凑搜索图标（原型 commercial.js 同款）
    ok('W', '标题栏搜索图标', await exists('.titlebar--home .titlebar__search'));
    // W-2 快捷开始已移除
    eq('W', '快捷开始已移除', await exists('.home-starters'), false);
    // W-3 运行于：品牌 img + 无版本副行
    ok('W', '引擎钮为品牌图', await exists('.cm-meta-select__icon img'));
    eq(
      'W',
      '引擎钮无副行',
      await evaluate(call, `Boolean(document.querySelector('[title="更换 Agent"] small'))`),
      false,
    );
    // W-4 目录钮无路径副行
    eq(
      'W',
      '目录钮无副行',
      await evaluate(call, `Boolean(document.querySelector('[title="更换工作目录"] small'))`),
      false,
    );
    // W-5 浮层菜单 fixed + 左对齐触发钮 + 悬于其上
    const modeBtn = await evaluate(
      call,
      `(() => { const el = document.querySelector('[title="任务模式"]'); const r = el.getBoundingClientRect(); return { left: r.left, top: r.top, bottom: r.bottom }; })()`,
    );
    await clickEl('[title="任务模式"]');
    await waitForExpression(
      call,
      `Boolean(document.querySelector('.home-floatmenu--fixed'))`,
      8000,
    );
    const menuRect = await evaluate(
      call,
      `(() => { const m = document.querySelector('.home-floatmenu--fixed'); const cs = getComputedStyle(m); const r = m.getBoundingClientRect(); return { pos: cs.position, left: r.left, bottomEdge: r.bottom }; })()`,
    );
    eq('W', '菜单 position fixed', menuRect.pos, 'fixed');
    ok(
      'W',
      '菜单左对齐触发钮(±2)',
      Math.abs(menuRect.left - modeBtn.left) <= 2.5 ||
        (console.log(
          '[W-delta]',
          JSON.stringify({ menuLeft: menuRect.left, btnLeft: modeBtn.left }),
        ),
        false),
    );
    ok(
      'W',
      '菜单悬于按钮上方',
      menuRect.bottomEdge <= modeBtn.top + 2 ||
        (console.log(
          '[W-hover]',
          JSON.stringify({
            bottomEdge: menuRect.bottomEdge,
            btnTop: modeBtn.top,
            btnBottom: modeBtn.bottom,
          }),
        ),
        false),
    );
    // Esc 关闭（组件监听 window）
    await waitForExpression(call, `!document.querySelector('.home-floatmenu--fixed')`, 4000).catch(
      () => {},
    );
    await evaluate(call, `window.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape' }))`);
    ok('W', 'Esc 关闭浮层', !(await exists('.home-floatmenu--fixed')));
    // W-6 选择工作目录弹层
    await clickEl('[title="更换工作目录"]');
    await waitForExpression(call, `Boolean(document.querySelector('#homeDirTitle'))`, 8000);
    ok('W', '目录弹层标题', (await getText('#homeDirTitle')) === '选择工作目录');
    // 第三轮整改：目录弹层的「从电脑选择…」动作行属性为 data-home-pick-dir
    ok('W', '系统选择行存在', await exists('[data-home-pick-dir]'));
    await evaluate(call, `window.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape' }))`);
    await waitForExpression(call, `!document.querySelector('#homeDirTitle')`, 4000).catch(() => {});
    ok('W', 'Esc 关闭目录弹层', !(await exists('#homeDirTitle')));
  }

  // ========== P 服务商未配置引导链路 ==========
  if (want('P')) {
    await goHome('home-r2');
    await openReadiness();
    // provider 行「去配置」→ navigateAway('providers')。
    // 进页前注入空服务商配置：验证「还没有服务商」引导卡真实可达。
    await evaluate(
      call,
      `window.__setFixture('get_provider_config', { providers: [], engines: [], models: [], bindings: [], defaultEngine: 'claude-code', defaultModel: '' })`,
    );
    await clickText('.cm-readiness__list', '去配置');
    await new Promise((r3) => setTimeout(r3, 2500));
    await waitForExpression(call, `Boolean(document.querySelector('.providers-head'))`, 15000);
    ok('P', '跳转 AI 配置页', await exists('.providers-head'));
    ok('P', '默认落在执行引擎绑定网格', Boolean(await exists('.engines-grid')));
    // 服务商 Tab：无服务商时显示「还没有服务商」引导卡。
    await clickText('.providers-tabs', '服务商');
    await waitForExpression(
      call,
      `Boolean(document.querySelector('.providers-empty-card'))`,
      8000,
    ).catch(() => {});
    const guide = await exists('.providers-empty-card');
    console.log('[P] 未配置引导卡可见：', guide);
    ok('P', '未配置引导卡存在', guide);
  }
  // ========== GEO 原型 vs 实现几何对比 ==========
  if (want('GEO')) {
    await goProto();
    const protoGeo = await measureGeo();
    await goHome();
    const implGeo = await measureGeo();
    console.log('[GEO] 原型 :', JSON.stringify(protoGeo));
    console.log('[GEO] 实现 :', JSON.stringify(implGeo));
    // 第五轮用户决议：固定顶距（四轮 clamp(36px,9vh,96px)+26px）在实际窗口高度下
    // 仍观感偏上，起始块改为 flex + margin auto 的安全垂直居中，随窗口高度自适应。
    // GEO 断言改为「start 块 = 可用高度的居中位（±8px)」；与原型只比 composer 高度。
    const expectedCenterTop =
      implGeo.containerTop + Math.max(0, (implGeo.containerH - implGeo.startH) / 2);
    const deltaCenter = implGeo.startTop - expectedCenterTop;
    console.log(
      '[GEO] 居中期望顶距(px)：' +
        expectedCenterTop.toFixed(1) +
        '（container ' +
        implGeo.containerTop +
        '+' +
        implGeo.containerH +
        ' / startH ' +
        implGeo.startH +
        '）',
    );
    ok('GEO', 'start 块在可用高度内安全居中 ≤8px', Math.abs(deltaCenter) <= 8);
    ok('GEO', 'composer 高度差 ≤8px', Math.abs(protoGeo.composerH - implGeo.composerH) <= 8);
  }
  console.log(
    `\n探针完成：${failures.length} 项失败。${failures.length ? '\n' + failures.join('\n') : ''}`,
  );
} catch (error) {
  failures.push(String(error?.stack || error));
  console.error(error);
} finally {
  await fsPromises.writeFile(
    path.join(outputDir, 'probe-results.json'),
    JSON.stringify(
      { generatedAt: new Date().toISOString(), failures, sections: evidence },
      null,
      2,
    ),
  );
  browser.kill();
  await new Promise((resolve) => {
    preview.close(() => resolve());
  });
  process.exit(failures.length ? 1 : 0);
}
