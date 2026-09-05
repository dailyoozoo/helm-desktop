import { spawn } from 'node:child_process';
import fs from 'node:fs';
import fsPromises from 'node:fs/promises';
import http from 'node:http';
import os from 'node:os';
import path from 'node:path';
import { pathToFileURL } from 'node:url';
import react from '@vitejs/plugin-react';
import tailwindcss from '@tailwindcss/vite';
import { build as viteBuild } from 'vite';

const root = path.resolve(import.meta.dirname, '..');
const port = Number(process.env.HELM_VISUAL_PORT || 4187);
const debugPort = port + 1000;
const outputDir = process.env.HELM_VISUAL_OUTPUT || path.join(os.tmpdir(), 'helm-ui-audit');
const siteDir = path.join(outputDir, 'site');
const pageSpecs = [
  {
    id: 'workspace',
    aria: '新建任务',
    // 原型 workspace.html workspace-titlebar__task：工作区标题栏中段显示任务标题，
    // 而非页面名「工作区」（2026-08-27 对齐原型，ThreadHead 行退役）。
    title: '修复鉴权令牌刷新',
    ready: '.turn-process',
    expected: 1,
    prototype: 'workspace.html',
  },
  {
    id: 'sessions',
    aria: '全部任务',
    // 原型 commercial.js titlebar()：icon 搜索模式页标题栏中段无页名文字（仅品牌+搜索+三键）；
    // 「原型标题栏无页名文字」为 2026-08-24 五轮决议（Titlebar.tsx 注释），页名断言过时于 8/22。
    // 无独立原型页：prototype/sessions.html 已随变更-34/35 删除（全部任务由设置页任务 Tab 承载，
    // 2026-08-23 决议），生产 sessions 路由保留但原型对照跳过（f4a0b90 删除文件，断言悬空待清理）。
    title: '',
    ready: '.tcard',
    expected: 4,
  },
  {
    id: 'providers',
    aria: 'AI 配置',
    title: '',
    ready: '.cm-grid--2 .cm-engine-card',
    expected: 2,
    prototype: 'providers.html',
  },
  {
    id: 'extensions',
    aria: '插件',
    title: '',
    ready: '.cm-skill-grid',
    expected: 1,
    prototype: 'extensions.html',
  },
  {
    id: 'usage',
    aria: '用量',
    title: '',
    ready: '.cm-kpi-grid .cm-kpi',
    expected: 4,
    prototype: 'usage.html',
  },
  {
    id: 'settings',
    aria: '设置',
    // 二级页唯一例外：原型 settings 页标题栏显示页名「设置」+ 返回键。
    title: '设置',
    ready: '.cm-settings-layout .cm-settings-tabs',
    expected: 1,
    prototype: 'settings.html',
  },
];
const allPageSpecs = [
  {
    id: 'home',
    aria: '新任务',
    ready: '.cm-composer',
    expected: 1,
    prototype: 'index.html',
  },
  ...pageSpecs,
];
const viewports = [
  { width: 1600, height: 900 },
  { width: 1366, height: 768 },
  { width: 1280, height: 800 },
  { width: 1024, height: 720 },
  { width: 860, height: 720 },
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
  const diagnostics = [];
  socket.addEventListener('message', (event) => {
    const message = JSON.parse(event.data);
    if (message.id) return;
    if (
      message.method === 'Network.loadingFailed' ||
      message.method === 'Runtime.exceptionThrown' ||
      message.method === 'Log.entryAdded'
    ) {
      diagnostics.push({ method: message.method, params: message.params });
    }
  });
  const call = (method, params = {}) =>
    new Promise((resolve, reject) => {
      const id = ++sequence;
      const timer = setTimeout(() => {
        socket.removeEventListener('message', onMessage);
        reject(new Error(`CDP 调用超时：${method}`));
      }, 30_000);
      const onMessage = (event) => {
        const message = JSON.parse(event.data);
        if (message.id !== id) return;
        clearTimeout(timer);
        socket.removeEventListener('message', onMessage);
        if (message.error) reject(new Error(message.error.message));
        else resolve(message.result);
      };
      socket.addEventListener('message', onMessage);
      socket.send(JSON.stringify({ id, method, params }));
    });
  await call('Network.enable');
  await call('Runtime.enable');
  await call('Log.enable');
  return { socket, call, diagnostics };
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
  await new Promise((resolve) => setTimeout(resolve, 800));
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
      const visible = element.checkVisibility?.({ checkOpacity: true, checkVisibilityCSS: true }) ??
        (style.display !== 'none' && style.visibility !== 'hidden' && Number(style.opacity) > 0);
      const clipped = (() => { let parent = element.parentElement; while (parent) {
        const parentStyle = getComputedStyle(parent); const parentRect = parent.getBoundingClientRect();
        if (['hidden', 'auto', 'scroll', 'clip'].includes(parentStyle.overflowX) &&
          parentRect.right <= innerWidth + 1 && rect.right > parentRect.right + 1) return true;
        parent = parent.parentElement;
      } return false; })();
      return visible && !clipped && rect.width > 0 && rect.right > innerWidth + 1;
      }).length,
    }))()`,
  );
}

await fsPromises.mkdir(outputDir, { recursive: true });
await viteBuild({
  root,
  configFile: false,
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: {
      '@helm/protocol': path.join(root, 'packages', 'protocol', 'src', 'index.ts'),
      '@': path.join(root, 'src'),
    },
  },
  build: {
    outDir: siteDir,
    emptyOutDir: true,
    rollupOptions: { input: path.join(root, 'visual-audit.html') },
  },
});
console.log(`视觉测试入口构建完成：${siteDir}`);

let previewOutput = '';
const preview = http.createServer(async (request, response) => {
  try {
    const pathname = decodeURIComponent(new URL(request.url || '/', 'http://127.0.0.1').pathname);
    const relativePath = pathname === '/' ? 'visual-audit.html' : pathname.replace(/^\/+/, '');
    const filePath = path.resolve(siteDir, relativePath);
    if (filePath !== siteDir && !filePath.startsWith(`${siteDir}${path.sep}`)) {
      response.writeHead(403).end('Forbidden');
      return;
    }
    const body = await fsPromises.readFile(filePath);
    const contentType =
      {
        '.css': 'text/css',
        '.html': 'text/html; charset=utf-8',
        '.js': 'text/javascript',
        '.png': 'image/png',
        '.svg': 'image/svg+xml',
        '.woff2': 'font/woff2',
      }[path.extname(filePath)] || 'application/octet-stream';
    response.writeHead(200, { 'Content-Type': contentType, 'Cache-Control': 'no-store' }).end(body);
  } catch (error) {
    previewOutput = `${previewOutput}\n${error?.stack || error}`.slice(-16_384);
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
    `--remote-debugging-port=${debugPort}`,
    `--user-data-dir=${path.join(os.tmpdir(), `helm-ui-audit-${process.pid}`)}`,
    '--window-size=1366,768',
    'about:blank',
  ],
  { stdio: 'ignore', windowsHide: true },
);

try {
  await waitFor(`http://127.0.0.1:${port}/visual-audit.html`, 15_000);
  const { socket, call, diagnostics } = await connectCdp();
  const prepareWorkspace = async () => {
    // S1：打开动作本身已经过最近任务行恢复该会话，这里只等线程内容就绪。
    if (await evaluate(call, `Boolean(document.querySelector('.turn-process'))`)) return;
    await waitForExpression(call, `Boolean(document.querySelector('.turn-process'))`, 180_000);
  };
  await call('Page.navigate', { url: `http://127.0.0.1:${port}/visual-audit.html` });
  try {
    await ensureVisualBoot(call);
  } catch (error) {
    const resources = await evaluate(
      call,
      `performance.getEntriesByType('resource').map((entry) => ({ name: entry.name, duration: entry.duration, transferSize: entry.transferSize }))`,
    );
    throw new Error(
      `${error.message}\n浏览器诊断：${JSON.stringify(diagnostics)}\n资源：${JSON.stringify(resources)}\nVite：${previewOutput}`,
    );
  }
  await evaluate(call, `document.querySelector('button[aria-label="新任务"]')?.click()`);
  await waitForExpression(call, `document.querySelectorAll('.cm-composer').length === 1`);
  const react = await evaluate(
    call,
    `(() => {
    const rect = (selector) => { const r = document.querySelector(selector)?.getBoundingClientRect(); return r ? {x:r.x,y:r.y,width:r.width,height:r.height} : null; };
    return { url: location.href, visualEntry: Boolean(document.querySelector('script[src*="visualAuditMain"]')),
      boot: document.body.dataset.visualBoot || null, rootHtml: document.getElementById('root')?.innerHTML.slice(0, 120) || '',
      errors: window.__visualAuditErrors || [],
      errorText: document.body.innerText.includes('设置加载失败'),
      titlebar: document.querySelector('.titlebar__center')?.textContent?.trim() || '',
      helper: document.querySelectorAll('.cm-start__heading > p').length,
      composerCount: document.querySelectorAll('.cm-composer').length,
      metaCount: document.querySelectorAll('.cm-start-meta button').length,
      sessions: document.querySelectorAll('.cm-start .rail-task__link').length,
      starterCount: document.querySelectorAll('.home-starters > button').length,
      backgroundImage: getComputedStyle(document.querySelector('.home')).backgroundImage,
      railLogoCount: document.querySelectorAll('.rail__logo').length,
      overflowX: document.documentElement.scrollWidth > document.documentElement.clientWidth,
      start: rect('.cm-start'), composer: rect('.cm-composer'), meta: rect('.cm-start-meta') };
  })()`,
  );
  await capture(call, 'react-home-1366x768.png');
  console.log('React 视觉入口状态：', react);

  const openReactPage = async (spec) => {
    if (spec.id !== 'settings') {
      // 设置页是全页布局，打开期间 Rail 不在 DOM（.cm-settings-back 点击无法恢复导航）。
      // 离开设置统一用全局导航事件回工作区，让 Rail 重新挂载后再进入目标页；
      // 非设置页上下文中该事件为幂等 no-op。
      await evaluate(
        call,
        `window.dispatchEvent(new CustomEvent('helm:navigate', { detail: { page: 'workspace' } }))`,
      );
      await new Promise((resolve) => setTimeout(resolve, 300));
    }
    if (spec.id === 'sessions') {
      // 2026-08-23 像素对齐：主侧栏「全部」入口移除（全部任务由设置页任务 Tab 承载），
      // 审计改走 App 全局导航事件进入 sessions 路由。
      await evaluate(
        call,
        `window.dispatchEvent(new CustomEvent('helm:navigate', { detail: { page: 'sessions' } }))`,
      );
      return;
    }
    if (spec.id === 'workspace') {
      // S1 任务型主侧栏：工作区经最近任务行进入（与原型一致）；独立「新建任务」按钮已随图标栏移除。
      await evaluate(
        call,
        `[...document.querySelectorAll('.rail-task')]
           .find((item) => item.textContent?.includes('修复鉴权令牌刷新'))
           ?.querySelector('.rail-task__link')?.click()`,
      );
      return;
    }
    await evaluate(
      call,
      `document.querySelector('button[aria-label=${JSON.stringify(spec.aria)}]')?.click()`,
    );
  };

  const reactPages = {};
  for (const spec of pageSpecs) {
    const preparePage = () => (spec.id === 'workspace' ? prepareWorkspace() : Promise.resolve());
    await openReactPage(spec);
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
      await openReactPage(spec);
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
        titlebar: document.querySelector('.titlebar__center')?.textContent?.trim() || '',
        overflowX: document.documentElement.scrollWidth > document.documentElement.clientWidth,
        visibleRightOverflow: [...document.querySelectorAll('body *')].filter((element) => {
          const style = getComputedStyle(element); const rect = element.getBoundingClientRect();
          const visible = element.checkVisibility?.({ checkOpacity: true, checkVisibilityCSS: true }) ??
            (style.display !== 'none' && style.visibility !== 'hidden' && Number(style.opacity) > 0);
          const clipped = (() => { let parent = element.parentElement; while (parent) {
            const parentStyle = getComputedStyle(parent); const parentRect = parent.getBoundingClientRect();
            if (['hidden', 'auto', 'scroll', 'clip'].includes(parentStyle.overflowX) &&
              parentRect.right <= innerWidth + 1 && rect.right > parentRect.right + 1) return true;
            parent = parent.parentElement;
          } return false; })();
          return visible && !clipped && rect.width > 0 && rect.right > innerWidth + 1;
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
      console.log(`检查 React 页面矩阵：${viewport.width}x${viewport.height} ${theme}`);
      await evaluate(call, `document.documentElement.dataset.theme = ${JSON.stringify(theme)}`);
      for (const spec of allPageSpecs) {
        await openReactPage(spec);
        if (spec.id === 'workspace') {
          await prepareWorkspace();
        }
        await waitForExpression(
          call,
          `document.querySelectorAll(${JSON.stringify(spec.ready)}).length >= ${spec.expected}`,
        );
        if (spec.id === 'workspace') {
          const workspaceHierarchy = await evaluate(
            call,
            // S1 任务型主侧栏：workspace 是详情态，一级导航不激活；「全部任务」降级为最近任务旁次级入口。
            // 批次①（2026-09 用户裁决）更新：任务列表侧栏全视口默认收起为抽屉（.sbar display:none），
            // 抽屉唤起按钮已随原型删除（.ws-sidebar-toggle 应不存在）；.ctx 关闭时不画左边线。
            `!document.querySelector('.rail-nav .is-active') && Boolean(document.querySelector('.rail-recent')) && (!document.querySelector('.sbar') || getComputedStyle(document.querySelector('.sbar')).display === 'none') && !document.querySelector('.ws-sidebar-toggle') && (!document.querySelector('.ctx') || document.querySelector('.ctx').getBoundingClientRect().width <= 2) && !document.querySelector('.composer__session-status > span.mono')`,
          );
          if (!workspaceHierarchy) {
            matrixFailures.push(
              `工作区 ${viewport.width}x${viewport.height} ${theme} 导航或信息层级未收敛`,
            );
          }
          // S9 §5 待办落地：工具就地语义正断言（ADR 0019 就地交错红线）。
          // 工具就地折叠：turn-1 静止工具组默认收起，.tool 基样式全态无边框（防盒套盒回归；
          // .tgrp 是组卡、按原型带边框，不在此列）；失败工具就地展开：终态失败轮（turn-2）
          // 过程区默认展开，失败卡 .failc 就地常驻可见。
          const toolInline = await evaluate(
            call,
            `(() => {
              const groups = [...document.querySelectorAll('[data-kind="tgrp"] > .tgrp')];
              const standalone = [...document.querySelectorAll('[data-kind="tool"] > .tool')];
              const visible = (el) => el.checkVisibility?.({ checkOpacity: true, checkVisibilityCSS: true }) ?? true;
              const bordered = standalone.filter((el) => {
                const s = getComputedStyle(el);
                return ['Top', 'Right', 'Bottom', 'Left'].some(
                  (side) => parseFloat(s['border' + side + 'Width']) > 0,
                );
              }).length;
              const failed = [...document.querySelectorAll('[data-kind="fail"] > .failc')];
              return {
                groups: groups.length,
                collapsed: groups.filter((el) => el.classList.contains('collapsed')).length,
                standalone: standalone.length,
                bordered,
                failCards: failed.length,
                failVisible: failed.filter(visible).length,
              };
            })()`,
          );
          if (
            toolInline.groups === 0 ||
            toolInline.collapsed !== toolInline.groups ||
            toolInline.bordered > 0 ||
            toolInline.failCards === 0 ||
            toolInline.failVisible !== toolInline.failCards
          ) {
            matrixFailures.push(
              `工作区 ${viewport.width}x${viewport.height} ${theme} 工具就地语义未收敛（${JSON.stringify(toolInline)}）`,
            );
          }
        }
        if (spec.id === 'settings') {
          // ≤900px 时 .cm-settings-sidebar 是设计上的横向滚动导航（app.css @media ≤820px），按钮收窄换行属预期；
          // 只在桌面宽度要求 ≥120×48 的纵向导航密度。
          const minWidth = viewport.width <= 900 ? 88 : 120;
          const maxHeight = viewport.width <= 900 ? 72 : 48;
          const settingsNavReadable = await evaluate(
            call,
            `(() => {
              const buttons = [...document.querySelectorAll('.cm-settings-tabs > button')];
              return {
                ok: buttons.every((item) => item.clientWidth >= ${minWidth} && item.clientHeight <= ${maxHeight}),
                sizes: buttons.map((item) => item.clientWidth + 'x' + item.clientHeight),
                cols: getComputedStyle(document.querySelector('.body')).gridTemplateColumns,
              };
            })()`,
          );
          if (!settingsNavReadable.ok) {
            matrixFailures.push(
              `设置 ${viewport.width}x${viewport.height} ${theme} 分类导航被挤压（${settingsNavReadable.sizes.join(', ')}；cols=${settingsNavReadable.cols}）`,
            );
          }
        }
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
    return { overflowX: document.documentElement.scrollWidth > document.documentElement.clientWidth,
      start: rect('.cm-start'), composer: rect('.cm-composer'), meta: rect('.cm-start-meta') };
  })()`,
  );
  await capture(call, 'prototype-home-1366x768.png');

  const prototypePages = {};
  for (const spec of pageSpecs) {
    // 无独立原型页的条目（如 sessions，2026-08-23 决议移除）跳过原型对照。
    if (!spec.prototype) continue;
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
      console.log(`检查原型页面矩阵：${viewport.width}x${viewport.height} ${theme}`);
      for (const spec of allPageSpecs) {
        // 无独立原型页的条目（如 sessions）跳过原型矩阵。
        if (!spec.prototype) continue;
        await call('Page.navigate', {
          url: pathToFileURL(path.join(root, 'prototype', spec.prototype)).href,
        });
        // 就绪等待替代固定盲等：长会话高负载下 file:// 导航 + commercial.js 渲染
        // 可能超过 150ms，导致断言打在未提交的文档上（2026-08-23 实测竞态）。
        await waitForExpression(
          call,
          "document.readyState === 'complete' && Boolean(document.body && document.body.children.length > 0)",
          15_000,
        );
        await evaluate(call, `document.documentElement.dataset.theme = ${JSON.stringify(theme)}`);
        if (spec.id === 'workspace') {
          const prototypeWorkspaceHierarchy = await evaluate(
            call,
            // 对齐变更-34/35 原型升级后的 workspace.html 真值（2026-08-22 探针实测）：
            // 模型/强度选择在 Composer 条内；sessionToggle 已移除；最近任务行有激活态。
            // 「全部」入口：当前原型 commercial.js 未渲染，React 按术语表实现并已在验收记录登记差异。
            `!document.querySelector('[data-cm-sidebar] .cm-nav__item[href="sessions.html"]') && document.querySelector('[data-cm-sidebar] [data-cm-task-id].is-active') && !document.getElementById('sessionToggle') && !document.querySelector('.thread__head #modelBtn') && document.querySelector('.composer__bar #modelBtn') && document.querySelector('.composer__bar #effortBtn')`,
          );
          if (!prototypeWorkspaceHierarchy) {
            matrixFailures.push(
              `原型工作区 ${viewport.width}x${viewport.height} ${theme} 导航或信息层级未收敛`,
            );
          }
        }
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
  // 2026-08-23 用户决议：新任务页标题栏无内容（bare），仅保留右侧三键。
  if (react.titlebar !== '')
    failures.push(`新任务标题栏应为空（仅三键），实际为「${react.titlebar}」`);
  if (react.helper !== 1) failures.push('新任务页执行说明缺失');
  if (react.composerCount !== 1) failures.push('新任务 Composer 缺失');
  if (react.metaCount !== 2) failures.push(`新任务元数据入口应为 2，实际 ${react.metaCount}`);
  // 2026-08-23 用户决议：快捷开始整块移除。
  if (react.starterCount !== 0)
    failures.push(`新任务快捷开始应已移除，实际 ${react.starterCount} 项`);
  if (react.sessions !== 0) failures.push(`新任务主区不应重复最近任务，实际 ${react.sessions}`);
  if (react.backgroundImage !== 'none') failures.push('新任务页仍有网格或装饰线背景');
  if (react.railLogoCount !== 0) failures.push('标题栏下方不应重复产品 Logo');
  if (react.overflowX || prototype.overflowX) failures.push('页面存在横向溢出');
  for (const key of ['start', 'composer', 'meta'])
    if (!react[key] || !prototype[key]) failures.push(`${key} 无法测量`);
  for (const spec of pageSpecs) {
    const page = reactPages[spec.id];
    if (page.titlebar !== spec.title)
      failures.push(`${spec.aria}标题栏应为「${spec.title}」，实际为「${page.titlebar}」`);
    if (page.coreCount < spec.expected)
      failures.push(`${spec.aria}核心区块应至少为 ${spec.expected}，实际 ${page.coreCount}`);
    if (page.overflowX || page.visibleRightOverflow > 0 || prototypePages[spec.id]?.overflowX)
      failures.push(`${spec.aria}存在横向或可见元素越界`);
    if (page.errors.length)
      failures.push(`${spec.aria}出现页面脚本错误：${page.errors.join('；')}`);
  }
  failures.push(...matrixFailures);
  if (failures.length) throw new Error(`视觉审计失败：\n- ${failures.join('\n- ')}`);
  console.log(
    `视觉审计通过：7 个生产 React 页面与原型的正常数据态、${viewports.length} 视口、双主题和溢出检查；截图：${outputDir}`,
  );
} finally {
  browser.kill();
  await new Promise((resolve) => {
    let timer;
    const finish = () => {
      if (timer) clearTimeout(timer);
      resolve();
    };
    timer = setTimeout(finish, 5_000);
    preview.close(finish);
    preview.closeAllConnections?.();
  });
}
