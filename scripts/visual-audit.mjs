import { spawn } from 'node:child_process';
import fs from 'node:fs';
import fsPromises from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { pathToFileURL } from 'node:url';

const root = path.resolve(import.meta.dirname, '..');
const port = Number(process.env.HELM_VISUAL_PORT || 4187);
const debugPort = port + 1000;
const outputDir = process.env.HELM_VISUAL_OUTPUT || path.join(os.tmpdir(), 'helm-ui-audit');
const pageSpecs = [
  {
    id: 'workspace',
    aria: '工作区',
    ready: '.turn-process',
    expected: 1,
    prototype: 'workspace.html',
  },
  {
    id: 'sessions',
    aria: '会话历史',
    ready: '.sessions-row',
    expected: 4,
    prototype: 'sessions.html',
  },
  {
    id: 'providers',
    aria: '服务商与模型',
    ready: '.engines-grid .engine-card',
    expected: 2,
    prototype: 'providers.html',
  },
  {
    id: 'extensions',
    aria: '扩展中心',
    ready: '.xsum .ministat',
    expected: 4,
    prototype: 'extensions.html',
  },
  {
    id: 'usage',
    aria: '用量与成本',
    ready: '.usage-stats .stat',
    expected: 4,
    prototype: 'usage.html',
  },
  {
    id: 'settings',
    aria: '设置',
    ready: '.setlayout .snav',
    expected: 1,
    prototype: 'settings.html',
  },
];
const allPageSpecs = [
  { id: 'home', aria: '总览', ready: '.cards .scard', expected: 6, prototype: 'index.html' },
  ...pageSpecs,
];
const viewports = [
  { width: 1366, height: 768 },
  { width: 1280, height: 720 },
  { width: 1024, height: 720 },
  { width: 800, height: 600 },
];
const themes = ['light', 'dark'];
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
  await waitFor(`http://127.0.0.1:${debugPort}/json/version`);
  const pages = await (await fetch(`http://127.0.0.1:${debugPort}/json`)).json();
  const page = pages.find((item) => item.type === 'page');
  if (!page) throw new Error('浏览器未创建页面。');
  const socket = new WebSocket(page.webSocketDebuggerUrl);
  await new Promise((resolve, reject) => {
    socket.addEventListener('open', resolve, { once: true });
    socket.addEventListener('error', reject, { once: true });
  });
  let sequence = 0;
  const call = (method, params = {}) =>
    new Promise((resolve, reject) => {
      const id = ++sequence;
      const onMessage = (event) => {
        const message = JSON.parse(event.data);
        if (message.id !== id) return;
        socket.removeEventListener('message', onMessage);
        if (message.error) reject(new Error(message.error.message));
        else resolve(message.result);
      };
      socket.addEventListener('message', onMessage);
      socket.send(JSON.stringify({ id, method, params }));
    });
  return { socket, call };
}

async function evaluate(call, expression) {
  const result = await call('Runtime.evaluate', {
    expression,
    returnByValue: true,
    awaitPromise: true,
  });
  if (result.exceptionDetails) throw new Error(result.exceptionDetails.text || '页面脚本执行失败');
  return result.result.value;
}

async function waitForExpression(call, expression, timeoutMs = 10_000) {
  const started = Date.now();
  while (Date.now() - started < timeoutMs) {
    if (await evaluate(call, expression)) return;
    await new Promise((resolve) => setTimeout(resolve, 150));
  }
  const diagnostics = await evaluate(
    call,
    `({ url: location.href, boot: document.body?.dataset.visualBoot || null, errors: window.__visualAuditErrors || [], text: document.body?.innerText.slice(0, 800) || '', html: document.documentElement.outerHTML.slice(0, 500) })`,
  );
  throw new Error(`页面就绪条件超时：${expression}\n${JSON.stringify(diagnostics)}`);
}

async function ensureVisualBoot(call, attempts = 1) {
  let lastError;
  for (let attempt = 0; attempt < attempts; attempt += 1) {
    try {
      // Windows 冷启动时 Vite 需要先转换视觉入口及其懒加载依赖，首次转换可能超过 10 秒。
      await waitForExpression(call, `document.body?.dataset.visualBoot === 'mounted'`, 180_000);
      return;
    } catch (error) {
      lastError = error;
      if (attempt < attempts - 1) await call('Page.reload', { ignoreCache: true });
    }
  }
  throw lastError;
}

async function capture(call, name) {
  const result = await call('Page.captureScreenshot', {
    format: 'png',
    captureBeyondViewport: false,
  });
  await fsPromises.writeFile(path.join(outputDir, name), Buffer.from(result.data, 'base64'));
}

async function pageOverflow(call) {
  return evaluate(
    call,
    `(() => ({
      overflowX: document.documentElement.scrollWidth > document.documentElement.clientWidth,
      visibleRightOverflow: [...document.querySelectorAll('body *')].filter((element) => {
        const style = getComputedStyle(element); const rect = element.getBoundingClientRect();
        return style.display !== 'none' && style.visibility !== 'hidden' && rect.width > 0 && rect.right > innerWidth + 1;
      }).length,
    }))()`,
  );
}

const preview = spawn(
  process.execPath,
  [
    path.join(root, 'node_modules', 'vite', 'bin', 'vite.js'),
    '--host',
    '127.0.0.1',
    '--port',
    String(port),
    '--strictPort',
  ],
  { cwd: root, stdio: 'ignore', windowsHide: true },
);
const browser = spawn(
  chrome,
  [
    '--headless=new',
    '--disable-gpu',
    '--hide-scrollbars',
    '--allow-file-access-from-files',
    `--remote-debugging-port=${debugPort}`,
    `--user-data-dir=${path.join(os.tmpdir(), `helm-ui-audit-${process.pid}`)}`,
    '--window-size=1366,768',
    'about:blank',
  ],
  { stdio: 'ignore', windowsHide: true },
);

try {
  await fsPromises.mkdir(outputDir, { recursive: true });
  // Windows 上冷启动 Vite 可能需要先完成依赖优化，给服务 60 秒而不是误报启动失败。
  await waitFor(`http://127.0.0.1:${port}`, 60_000);
  // 先触发 Vite 对视觉入口的依赖转换，避免冷启动时浏览器拿到 HTML 后模块仍在优化而空白。
  await waitFor(`http://127.0.0.1:${port}/src/visualAuditMain.ts`, 120_000);
  const { socket, call } = await connectCdp();
  const prepareWorkspace = async () => {
    if (await evaluate(call, `Boolean(document.querySelector('.turn-process'))`)) return;
    await waitForExpression(
      call,
      `[...document.querySelectorAll('.sitem')].some((item) => item.textContent?.includes('修复鉴权令牌刷新'))`,
      180_000,
    );
    await evaluate(
      call,
      `[...document.querySelectorAll('.sitem')].find((item) =>
        item.textContent?.includes('修复鉴权令牌刷新'))?.click()`,
    );
  };
  await call('Page.navigate', { url: `http://127.0.0.1:${port}/visual-audit.html` });
  await ensureVisualBoot(call);
  await evaluate(call, `document.querySelector('button[aria-label="总览"]')?.click()`);
  await waitForExpression(call, `document.querySelectorAll('.cards .scard').length === 6`);
  const react = await evaluate(
    call,
    `(() => {
    const rect = (selector) => { const r = document.querySelector(selector)?.getBoundingClientRect(); return r ? {x:r.x,y:r.y,width:r.width,height:r.height} : null; };
    return { url: location.href, visualEntry: Boolean(document.querySelector('script[src*="visualAuditMain"]')),
      boot: document.body.dataset.visualBoot || null, rootHtml: document.getElementById('root')?.innerHTML.slice(0, 120) || '',
      errors: window.__visualAuditErrors || [],
      errorText: document.body.innerText.includes('设置加载失败'),
      eyebrow: document.querySelectorAll('.hero .eyebrow').length, cards: document.querySelectorAll('.cards .scard').length,
      engines: document.querySelectorAll('.statgrid .estat').length, sessions: document.querySelectorAll('.cont .rrow').length,
      usage: document.querySelectorAll('.cont .umini').length, overflowX: document.documentElement.scrollWidth > document.documentElement.clientWidth,
      hero: rect('.hero'), console: rect('.console'), cardsRect: rect('.cards') };
  })()`,
  );
  await capture(call, 'react-home-1366x768.png');
  console.log('React 视觉入口状态：', react);

  const reactPages = {};
  for (const spec of pageSpecs) {
    const openPage = () =>
      evaluate(
        call,
        `document.querySelector('button[aria-label=${JSON.stringify(spec.aria)}]')?.click()`,
      );
    const preparePage = () => (spec.id === 'workspace' ? prepareWorkspace() : Promise.resolve());
    await openPage();
    await preparePage();
    try {
      await waitForExpression(
        call,
        `document.querySelectorAll(${JSON.stringify(spec.ready)}).length >= ${spec.expected}`,
      );
    } catch {
      // 首次加载懒分包可能触发 Vite 依赖预构建；刷新后重新进入同一生产页面。
      await call('Page.reload', { ignoreCache: true });
      await ensureVisualBoot(call);
      await openPage();
      await preparePage();
      await waitForExpression(
        call,
        `document.querySelectorAll(${JSON.stringify(spec.ready)}).length >= ${spec.expected}`,
      );
    }
    reactPages[spec.id] = await evaluate(
      call,
      `(() => ({
        coreCount: document.querySelectorAll(${JSON.stringify(spec.ready)}).length,
        overflowX: document.documentElement.scrollWidth > document.documentElement.clientWidth,
        visibleRightOverflow: [...document.querySelectorAll('body *')].filter((element) => {
          const style = getComputedStyle(element); const rect = element.getBoundingClientRect();
          return style.display !== 'none' && style.visibility !== 'hidden' && rect.width > 0 && rect.right > innerWidth + 1;
        }).length,
        errors: window.__visualAuditErrors || [],
      }))()`,
    );
    await capture(call, `react-${spec.id}-1366x768.png`);
  }

  const matrixFailures = [];
  for (const viewport of viewports) {
    await call('Emulation.setDeviceMetricsOverride', {
      width: viewport.width,
      height: viewport.height,
      deviceScaleFactor: 1,
      mobile: false,
    });
    for (const theme of themes) {
      await evaluate(call, `document.documentElement.dataset.theme = ${JSON.stringify(theme)}`);
      for (const spec of allPageSpecs) {
        await evaluate(
          call,
          `document.querySelector('button[aria-label=${JSON.stringify(spec.aria)}]')?.click()`,
        );
        if (spec.id === 'workspace') {
          await prepareWorkspace();
        }
        await waitForExpression(
          call,
          `document.querySelectorAll(${JSON.stringify(spec.ready)}).length >= ${spec.expected}`,
        );
        const overflow = await pageOverflow(call);
        if (overflow.overflowX || overflow.visibleRightOverflow > 0) {
          matrixFailures.push(
            `${spec.aria} ${viewport.width}x${viewport.height} ${theme} 存在横向或可见元素越界`,
          );
        }
        await capture(call, `react-${spec.id}-${viewport.width}x${viewport.height}-${theme}.png`);
      }
    }
  }
  await call('Emulation.setDeviceMetricsOverride', {
    width: 1366,
    height: 768,
    deviceScaleFactor: 1,
    mobile: false,
  });

  await call('Page.navigate', {
    url: pathToFileURL(path.join(root, 'prototype', 'index.html')).href,
  });
  await new Promise((resolve) => setTimeout(resolve, 700));
  const prototype = await evaluate(
    call,
    `(() => {
    const rect = (selector) => { const r = document.querySelector(selector)?.getBoundingClientRect(); return r ? {x:r.x,y:r.y,width:r.width,height:r.height} : null; };
    return { overflowX: document.documentElement.scrollWidth > document.documentElement.clientWidth, hero: rect('.hero'), console: rect('.console'), cardsRect: rect('.cards') };
  })()`,
  );
  await capture(call, 'prototype-home-1366x768.png');

  const prototypePages = {};
  for (const spec of pageSpecs) {
    await call('Page.navigate', {
      url: pathToFileURL(path.join(root, 'prototype', spec.prototype)).href,
    });
    await new Promise((resolve) => setTimeout(resolve, 500));
    prototypePages[spec.id] = await evaluate(
      call,
      `({ overflowX: document.documentElement.scrollWidth > document.documentElement.clientWidth })`,
    );
    await capture(call, `prototype-${spec.id}-1366x768.png`);
  }

  for (const viewport of viewports) {
    await call('Emulation.setDeviceMetricsOverride', {
      width: viewport.width,
      height: viewport.height,
      deviceScaleFactor: 1,
      mobile: false,
    });
    for (const theme of themes) {
      for (const spec of allPageSpecs) {
        await call('Page.navigate', {
          url: pathToFileURL(path.join(root, 'prototype', spec.prototype)).href,
        });
        await new Promise((resolve) => setTimeout(resolve, 150));
        await evaluate(call, `document.documentElement.dataset.theme = ${JSON.stringify(theme)}`);
        const overflow = await pageOverflow(call);
        if (overflow.overflowX || overflow.visibleRightOverflow > 0) {
          matrixFailures.push(
            `原型 ${spec.aria} ${viewport.width}x${viewport.height} ${theme} 存在横向或可见元素越界`,
          );
        }
        await capture(
          call,
          `prototype-${spec.id}-${viewport.width}x${viewport.height}-${theme}.png`,
        );
      }
    }
  }
  socket.close();

  const failures = [];
  if (react.eyebrow !== 1) failures.push('Hero eyebrow 缺失');
  if (react.cards !== 6) failures.push(`快速进入卡片应为 6，实际 ${react.cards}`);
  if (react.engines < 4) failures.push(`引擎/服务商状态项应至少为 4，实际 ${react.engines}`);
  if (react.sessions !== 4) failures.push(`最近会话应为 4，实际 ${react.sessions}`);
  if (react.usage !== 1) failures.push('本月用量卡缺失');
  if (react.overflowX || prototype.overflowX) failures.push('页面存在横向溢出');
  for (const key of ['hero', 'console', 'cardsRect'])
    if (!react[key] || !prototype[key]) failures.push(`${key} 无法测量`);
  for (const spec of pageSpecs) {
    const page = reactPages[spec.id];
    if (page.coreCount < spec.expected)
      failures.push(`${spec.aria}核心区块应至少为 ${spec.expected}，实际 ${page.coreCount}`);
    if (page.overflowX || page.visibleRightOverflow > 0 || prototypePages[spec.id].overflowX)
      failures.push(`${spec.aria}存在横向或可见元素越界`);
    if (page.errors.length)
      failures.push(`${spec.aria}出现页面脚本错误：${page.errors.join('；')}`);
  }
  failures.push(...matrixFailures);
  if (failures.length) throw new Error(`视觉审计失败：\n- ${failures.join('\n- ')}`);
  console.log(
    `视觉审计通过：7 个生产 React 页面与原型的正常数据态、4 视口、双主题和溢出检查；截图：${outputDir}`,
  );
} finally {
  browser.kill();
  preview.kill();
}
