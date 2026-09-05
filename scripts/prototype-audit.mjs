import { spawn } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { pathToFileURL } from 'node:url';

const root = path.resolve(import.meta.dirname, '..');
const prototypeDir = path.join(root, 'prototype');
const outDir = path.join(root, '.agent', 'evidence', 'prototype-commercial');
const pages = ['index', 'workspace', 'sessions', 'providers', 'extensions', 'usage', 'settings'];
const indexOnly = process.argv.includes('--index-only');
const staticOnly = process.argv.includes('--static-only');
const visualPages = indexOnly ? ['index'] : pages;
const viewports = [
  [1600, 950],
  [1024, 720],
  [800, 600],
];
const checks = [];
const runtimeProblems = [];

fs.mkdirSync(outDir, { recursive: true });

function check(label, ok, detail = '') {
  checks.push({ label, ok: Boolean(ok), detail });
  if (!ok) console.error(`x ${label}${detail ? `: ${detail}` : ''}`);
}

function staticAudit() {
  const rootHtml = fs
    .readdirSync(prototypeDir)
    .filter((name) => name.endsWith('.html'))
    .sort();
  check(
    '原型根目录只保留 7 个正式页面',
    rootHtml.join(',') ===
      pages
        .map((p) => `${p}.html`)
        .sort()
        .join(','),
    rootHtml.join(', '),
  );

  const appJs = fs.readFileSync(path.join(prototypeDir, 'assets', 'app.js'), 'utf8');
  const knownIcons = new Set(
    [...appJs.matchAll(/^\s{4}([a-z0-9]+):\s*'/gim)].map((match) => match[1]),
  );
  const allHtml = pages
    .map((name) => fs.readFileSync(path.join(prototypeDir, `${name}.html`), 'utf8'))
    .join('\n');
  const usedIcons = new Set(
    [...allHtml.matchAll(/data-ic="([^"]+)"/g)]
      .map((match) => match[1])
      .filter((name) => /^[a-z0-9]+$/.test(name)),
  );
  const unknownIcons = [...usedIcons].filter((name) => !knownIcons.has(name));
  check('所有 data-ic 都能在共享图标集中解析', unknownIcons.length === 0, unknownIcons.join(', '));

  for (const name of pages) {
    const file = path.join(prototypeDir, `${name}.html`);
    const html = fs.readFileSync(file, 'utf8');
    const inlineScripts = [...html.matchAll(/<script(?![^>]*\bsrc=)[^>]*>([\s\S]*?)<\/script>/gi)];
    for (const [index, match] of inlineScripts.entries()) {
      try {
        Function(match[1]);
        check(`${name} 内联脚本 ${index + 1} 可解析`, true);
      } catch (error) {
        check(`${name} 内联脚本 ${index + 1} 可解析`, false, error.message);
      }
    }
    const refs = [...html.matchAll(/(?:src|href)="([^"]+)"/g)]
      .map((match) => match[1])
      .filter(
        (value) =>
          !/^(?:#|https?:|mailto:|javascript:)/.test(value) &&
          !value.includes("' +") &&
          !value.includes('+ "'),
      );
    const missing = refs.filter((ref) => {
      const clean = ref.split(/[?#]/)[0];
      return clean && !fs.existsSync(path.resolve(path.dirname(file), clean));
    });
    check(`${name} 本地资源引用完整`, missing.length === 0, missing.join(', '));
  }

  const usage = fs.readFileSync(path.join(prototypeDir, 'usage.html'), 'utf8');
  check('用量页不再包含预算和 CSV 导出', !usage.includes('预算') && !usage.includes('CSV'));
  const settings = fs.readFileSync(path.join(prototypeDir, 'settings.html'), 'utf8');
  check(
    '设置页只保留四个二级入口',
    (settings.match(/data-cm-tab="(?:general|theme|shortcuts|about)"/g) || []).length === 4,
  );
  check(
    '设置使用独立二级页面导航',
    settings.includes('class="cm-sidebar cm-settings-sidebar"') &&
      settings.includes('data-settings-back') &&
      !settings.includes('data-cm-sidebar'),
  );
  const extensions = fs.readFileSync(path.join(prototypeDir, 'extensions.html'), 'utf8');
  check(
    '插件页只保留技能与连接器',
    (extensions.match(/data-cm-tab="(?:skills|mcp)"/g) || []).length === 2,
  );
  const workspace = fs.readFileSync(path.join(prototypeDir, 'workspace.html'), 'utf8');
  // 工作台脚本已拆到 assets/workspace*.js（旧版内联时回退到页面本身）
  let workspaceLogic = '';
  for (const f of ['workspace-data.js', 'workspace.js', 'workspace-rail.js']) {
    const p = path.join(prototypeDir, 'assets', f);
    if (fs.existsSync(p)) workspaceLogic += fs.readFileSync(p, 'utf8') + '\n';
  }
  if (!workspaceLogic) workspaceLogic = workspace;
  const workspaceCssPath = path.join(prototypeDir, 'assets', 'workspace.css');
  const workspaceCss = fs.existsSync(workspaceCssPath)
    ? fs.readFileSync(workspaceCssPath, 'utf8')
    : '';
  const workspaceAll = workspace + '\n' + workspaceLogic + '\n' + workspaceCss;
  const sessionSource = workspaceLogic.slice(
    workspaceLogic.indexOf('const SESSIONS = {'),
    workspaceLogic.indexOf('const ORDER = ['),
  );
  const workspaceTaskIds = new Set(
    [...sessionSource.matchAll(/^ {4}([a-z][a-z0-9_-]*): \{$/gm)].map((match) => match[1]),
  );
  const commercial = fs.readFileSync(path.join(prototypeDir, 'assets', 'commercial.js'), 'utf8');
  const recentTaskSource = commercial.slice(
    commercial.indexOf('var recentGroups = ['),
    commercial.indexOf('function requestedTaskId()'),
  );
  const recentTaskIds = [...recentTaskSource.matchAll(/\{ id: "([a-z][a-z0-9_-]*)"/g)].map(
    (match) => match[1],
  );
  const taskEntrySource = [
    fs.readFileSync(path.join(prototypeDir, 'sessions.html'), 'utf8'),
    fs.readFileSync(path.join(prototypeDir, 'usage.html'), 'utf8'),
  ].join('\n');
  const missingTaskTargets = [
    ...new Set(
      [
        ...recentTaskIds,
        ...[...taskEntrySource.matchAll(/workspace\.html\?task=([a-z][a-z0-9_-]*)/g)].map(
          (match) => match[1],
        ),
      ].filter((taskId) => !workspaceTaskIds.has(taskId)),
    ),
  ];
  check(
    '所有任务入口都能打开工作区中的真实原型任务',
    missingTaskTargets.length === 0,
    missingTaskTargets.join(', '),
  );
  check(
    '工作区复用全局一级导航并把任务列表收为抽屉',
    workspace.includes('data-cm-sidebar') &&
      workspace.includes('assets/commercial.js') &&
      workspace.includes('class="sbar" aria-label="任务列表"') &&
      !workspace.includes('<aside class="sbar">\n        <nav class="cm-nav"'),
  );
  check(
    '工作区包含轮次轨道（超过 3 轮才显示）',
    workspace.includes('id="turnRail"') && workspaceLogic.includes('turns.length > 3'),
  );
  check(
    '工作区旧四按钮导航不再显示',
    !workspaceAll.includes('class="navfab"') && !workspaceAll.includes('#navFab'),
  );
  check(
    '工作区右栏保留全部文件与修改记录两个稳定视图',
    workspace.includes('data-tab="changes"') &&
      workspace.includes('data-tab="files"') &&
      !workspace.includes('data-panel="tasks"'),
  );
  check(
    '工作区右栏支持最大化、关闭与动态文件预览',
    workspace.includes('id="ctxMax"') &&
      workspace.includes('id="ctxClose"') &&
      workspaceLogic.includes('function openFilePreview(path)'),
  );
  check(
    '最终答复保留聚合交付入口',
    workspaceLogic.includes('查看全部文件') && workspaceLogic.includes('查看修改记录'),
  );
  check(
    'Todo 与子代理位于 Composer 上方执行区',
    workspace.includes('id="workStrip"') &&
      workspace.includes('id="workAgents"') &&
      !workspace.includes('data-open="tasks"'),
  );
  check(
    'Composer 不重复展示累计花费或字符换算 Token',
    !workspace.includes('id="stCost"') && !workspace.includes('id="tokEstimate"'),
  );
}

const debugPort = 9333;
const chromeCandidates = [
  process.env.CHROME_PATH,
  'C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe',
  'C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe',
  'C:\\Program Files\\Microsoft\\Edge\\Application\\msedge.exe',
].filter(Boolean);
const chrome = staticOnly ? null : chromeCandidates.find((candidate) => fs.existsSync(candidate));
if (!staticOnly && !chrome) throw new Error('未找到 Chrome 或 Edge；可通过 CHROME_PATH 指定。');

const profile = fs.mkdtempSync(path.join(os.tmpdir(), 'helm-prototype-audit-'));
const browser = staticOnly
  ? null
  : spawn(
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

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

async function waitFor(url, timeoutMs = 20_000) {
  const started = Date.now();
  while (Date.now() - started < timeoutMs) {
    try {
      if ((await fetch(url)).ok) return;
    } catch {
      // Browser startup is still in progress.
    }
    await sleep(150);
  }
  throw new Error('等待浏览器调试端口超时');
}

let socket;
let call;

async function connect() {
  await waitFor(`http://127.0.0.1:${debugPort}/json/version`);
  const targets = await (await fetch(`http://127.0.0.1:${debugPort}/json`)).json();
  const target = targets.find((candidate) => candidate.type === 'page');
  socket = new WebSocket(target.webSocketDebuggerUrl);
  await new Promise((resolve, reject) => {
    socket.addEventListener('open', resolve, { once: true });
    socket.addEventListener('error', reject, { once: true });
  });
  let sequence = 0;
  const pending = new Map();
  socket.addEventListener('message', (event) => {
    const message = JSON.parse(event.data);
    if (message.id) {
      const handler = pending.get(message.id);
      if (!handler) return;
      pending.delete(message.id);
      if (message.error) handler.reject(new Error(message.error.message));
      else handler.resolve(message.result);
      return;
    }
    if (message.method === 'Runtime.exceptionThrown') {
      runtimeProblems.push(
        message.params.exceptionDetails?.exception?.description || '页面运行时异常',
      );
    }
    if (message.method === 'Runtime.consoleAPICalled' && message.params.type === 'error') {
      runtimeProblems.push(
        message.params.args.map((arg) => arg.value ?? arg.description ?? '').join(' '),
      );
    }
  });
  call = (method, params = {}) =>
    new Promise((resolve, reject) => {
      const id = ++sequence;
      pending.set(id, { resolve, reject });
      socket.send(JSON.stringify({ id, method, params }));
    });
}

async function evaluate(expression) {
  const result = await call('Runtime.evaluate', {
    expression,
    returnByValue: true,
    awaitPromise: true,
  });
  if (result.exceptionDetails) throw new Error(result.exceptionDetails.text || '表达式执行失败');
  return result.result.value;
}

async function navigate(name, search = '') {
  await call('Page.navigate', {
    url: pathToFileURL(path.join(prototypeDir, `${name}.html`)).href + search,
  });
  await sleep(name === 'workspace' ? 900 : 700);
}

async function setViewport(width, height) {
  await call('Emulation.setDeviceMetricsOverride', {
    width,
    height,
    deviceScaleFactor: 1,
    mobile: false,
  });
}

async function screenshot(label) {
  const result = await call('Page.captureScreenshot', { format: 'png', fromSurface: true });
  fs.writeFileSync(path.join(outDir, `${label}.png`), Buffer.from(result.data, 'base64'));
}

async function visualAudit() {
  await connect();
  await call('Page.enable');
  await call('Runtime.enable');

  for (const name of visualPages) {
    for (const [width, height] of viewports) {
      await setViewport(width, height);
      await navigate(name);
      const metrics = await evaluate(`({
        innerWidth: window.innerWidth,
        documentWidth: document.documentElement.scrollWidth,
        bodyWidth: document.body.scrollWidth,
        textLength: document.body.innerText.trim().length,
        images: [...document.images].map(img => ({ src: img.getAttribute('src'), complete: img.complete, width: img.naturalWidth })),
        titlebar: !!document.querySelector('.cm-titlebar,.titlebar'),
        navLabels: [...document.querySelectorAll('.cm-nav__item')].map(item => item.textContent.trim())
        ,shellHeight: document.querySelector('.cm-app')?.clientHeight || null
        ,mainClientHeight: document.querySelector('.cm-main')?.clientHeight || null
        ,mainScrollHeight: document.querySelector('.cm-main')?.scrollHeight || null
        ,mainOverflowY: document.querySelector('.cm-main') ? getComputedStyle(document.querySelector('.cm-main')).overflowY : null
      })`);
      check(
        `${name} ${width}x${height} 无页面级横向溢出`,
        metrics.documentWidth <= metrics.innerWidth && metrics.bodyWidth <= metrics.innerWidth,
        JSON.stringify(metrics),
      );
      check(
        `${name} ${width}x${height} 页面非空`,
        metrics.textLength > (name === 'index' ? 60 : 80),
        String(metrics.textLength),
      );
      check(`${name} ${width}x${height} 标题栏存在`, metrics.titlebar);
      const broken = metrics.images.filter((image) => !image.complete || image.width === 0);
      check(
        `${name} ${width}x${height} 品牌图片加载成功`,
        broken.length === 0,
        JSON.stringify(broken),
      );
      if (name !== 'workspace') {
        if (name === 'settings') {
          check(
            `${name} ${width}x${height} 使用二级页面导航`,
            await evaluate(
              `!!document.querySelector('[data-settings-back]') && [...document.querySelectorAll('.cm-settings-tabs [data-cm-tab]')].length === 4 && [...document.querySelectorAll('.cm-settings-tabs [data-cm-tab]')].every(item => item.clientWidth >= 120) && !document.querySelector('[data-cm-sidebar]')`,
            ),
          );
        } else {
          check(
            `${name} ${width}x${height} 全局导航完整`,
            ['新任务', 'AI 配置', '插件', '用量'].every((label) =>
              metrics.navLabels.some((text) => text.includes(label)),
            ) && !metrics.navLabels.some((text) => text.includes('全部任务')),
            metrics.navLabels.join(', '),
          );
          check(
            `${name} ${width}x${height} 全部任务入口位于最近任务标题`,
            await evaluate(
              `document.querySelector('[data-cm-sidebar] .cm-recent__all[href="sessions.html"]')?.textContent.trim() === '全部'`,
            ),
          );
        }
        check(
          `${name} ${width}x${height} 应用外壳受视口约束`,
          metrics.shellHeight === height,
          JSON.stringify(metrics),
        );
        check(
          `${name} ${width}x${height} 主内容使用独立纵向滚动`,
          ['auto', 'scroll'].includes(metrics.mainOverflowY),
          JSON.stringify(metrics),
        );
      } else {
        check(
          `${name} ${width}x${height} 全局导航完整且当前任务在最近任务中高亮`,
          await evaluate(
            `['新任务','AI 配置','插件','用量'].every(label => [...document.querySelectorAll('[data-cm-sidebar] .cm-nav__item')].some(item => item.textContent.includes(label))) && !document.querySelector('[data-cm-sidebar] .cm-nav__item[href="sessions.html"]') && document.querySelector('[data-cm-sidebar] [data-cm-task-id].is-active')`,
          ),
        );
        check(
          `${name} ${width}x${height} 任务列表默认收起且入口可见`,
          await evaluate(
            `getComputedStyle(document.querySelector('.sbar')).visibility === 'hidden' && getComputedStyle(document.getElementById('sessionToggle')).display !== 'none'`,
          ),
        );
      }
      await screenshot(`${name}-${width}x${height}-light`);
      if (
        name !== 'workspace' &&
        width === 800 &&
        metrics.mainScrollHeight > metrics.mainClientHeight
      ) {
        const didScroll = await evaluate(`(() => {
          const main = document.querySelector('.cm-main');
          main.scrollTop = main.scrollHeight;
          return main.scrollTop > 0;
        })()`);
        check(`${name} ${width}x${height} 可以滚动到底部`, didScroll);
        await sleep(120);
        await screenshot(`${name}-${width}x${height}-bottom-light`);
      }
    }
    await setViewport(1600, 950);
    await evaluate(
      `document.querySelector('.cm-main') && (document.querySelector('.cm-main').scrollTop = 0)`,
    );
    await evaluate(`document.documentElement.dataset.theme = 'dark'`);
    await sleep(180);
    await screenshot(`${name}-1600x950-dark`);
  }

  await setViewport(1600, 950);
  await navigate('index');
  check(
    '新任务导航使用统一 AI 配置与设置图标',
    await evaluate(
      `document.querySelector('.cm-nav__item[href="providers.html"] [data-ic="server"]') && document.querySelector('.cm-sidebar__footer [data-ic="settings2"]')`,
    ),
  );
  check(
    '标题栏贯通且位于菜单与页面上方',
    await evaluate(
      `(() => { const title = document.querySelector('.cm-titlebar').getBoundingClientRect(); const side = document.querySelector('.cm-sidebar').getBoundingClientRect(); const main = document.querySelector('.cm-main').getBoundingClientRect(); return title.left === 0 && title.width === innerWidth && side.top >= title.bottom && main.top >= title.bottom; })()`,
    ),
  );
  check(
    '主导航固定且最近任务独立滚动',
    await evaluate(
      `getComputedStyle(document.querySelector('.cm-sidebar')).overflow === 'hidden' && getComputedStyle(document.querySelector('.cm-recent')).overflow === 'hidden' && ['auto','scroll'].includes(getComputedStyle(document.querySelector('.cm-recent__body')).overflowY) && getComputedStyle(document.querySelector('.cm-recent__toolbar')).flexShrink === '0'`,
    ),
  );
  check(
    '四个主导航保持单列紧凑排列且最近任务提前出现',
    await evaluate(
      `(() => { const nav = document.querySelector('.cm-nav').getBoundingClientRect(); const recent = document.querySelector('.cm-recent__toolbar').getBoundingClientRect(); const title = document.querySelector('.cm-titlebar').getBoundingClientRect(); const items = [...document.querySelectorAll('.cm-sidebar > .cm-nav .cm-nav__item')].map(item => item.getBoundingClientRect()); return items.length === 4 && nav.height <= 124 && recent.top <= title.bottom + 138 && items.every((item, index) => item.height <= 30 && item.width >= nav.width - 1 && (index === 0 || item.top > items[index - 1].top)); })()`,
    ),
  );
  check(
    '最近任务目录只显示末级名称并保留完整路径提示',
    await evaluate(
      `(() => { const folders = [...document.querySelectorAll('.cm-folder__head')]; return folders.length > 0 && folders.every(folder => { const label = folder.querySelector('.cm-task__title').textContent.trim(); const hasSeparator = folder.title.includes('/') || folder.title.includes(String.fromCharCode(92)); return !label.includes(':') && !label.includes('/') && !label.includes(String.fromCharCode(92)) && hasSeparator && folder.getAttribute('aria-label').includes(folder.title); }); })()`,
    ),
  );
  check(
    '全部任务入口与最近任务处于同一上下文',
    await evaluate(
      `document.querySelector('.cm-recent__toolbar > .cm-recent__all[href="sessions.html"]')?.textContent.trim() === '全部' && !document.querySelector('.cm-sidebar > .cm-nav [href="sessions.html"]')`,
    ),
  );
  check(
    '新任务入场后恢复正常对比度',
    await evaluate(
      `parseFloat(getComputedStyle(document.querySelector('.cm-start')).opacity) >= .99`,
    ),
  );
  check(
    '侧栏与输入区使用表面层级而非硬分割线',
    await evaluate(
      `parseFloat(getComputedStyle(document.querySelector('.cm-sidebar')).borderRightWidth) === 0 && parseFloat(getComputedStyle(document.querySelector('.cm-titlebar')).borderBottomWidth) === 0 && parseFloat(getComputedStyle(document.querySelector('.cm-composer__bar')).borderTopWidth) === 0`,
    ),
  );
  check(
    '新任务标题只保留 Helm Logo，Agent 与工作目录位于 Composer 边框外',
    await evaluate(
      `(() => { const form = document.getElementById('quickStart'); const formRect = form.getBoundingClientRect(); const meta = document.querySelector('.cm-start-meta'); const metaRect = meta.getBoundingClientRect(); const prompt = document.getElementById('newTaskTitle'); const titleline = prompt.closest('.cm-start__titleline'); const desc = prompt.closest('.cm-start__heading').querySelector('p'); const folderLabel = document.getElementById('folderLabel').textContent.trim(); const folderButton = document.querySelector('[data-cm-open="folderModal"]'); window.__initialFolderButtonWidth = folderButton.getBoundingClientRect().width; return prompt.innerText.includes('今天想和 Agent 一起完成什么？') && desc.innerText.includes('真实 CLI') && titleline.querySelectorAll('[data-ic="helm"]').length === 1 && !titleline.querySelector('[data-ic="code"],[data-ic="sparkles"]') && getComputedStyle(prompt.closest('.cm-start__heading')).textAlign === 'center' && !form.contains(meta) && metaRect.top >= formRect.bottom + 4 && metaRect.width < formRect.width && !meta.querySelector('small') && folderLabel === 'helm' && !folderLabel.includes(':') && folderButton.getBoundingClientRect().width < 110 && !document.getElementById('environmentStatus'); })()`,
    ),
  );
  check(
    '快捷开始只提供三个轻量文本入口且不堆叠卡片',
    await evaluate(
      `(() => { const starters = document.getElementById('quickStarters'); const buttons = [...starters.querySelectorAll('[data-starter]')]; return buttons.length === 3 && ['审查当前改动','实现一个功能','排查错误原因'].every(value => starters.innerText.includes(value)) && buttons.every(button => getComputedStyle(button).backgroundColor === 'rgba(0, 0, 0, 0)') && !starters.querySelector('.cm-panel,.cm-card'); })()`,
    ),
  );
  await evaluate(
    `document.querySelector('#quickStarters [data-starter*="实现一个新功能"]').click()`,
  );
  check(
    '点击快捷任务后填入输入框并自动收起入口',
    await evaluate(
      `document.getElementById('quickTask').value.includes('实现一个新功能') && document.getElementById('quickStarters').hidden`,
    ),
  );
  await evaluate(
    `document.getElementById('quickTask').value='';document.getElementById('quickTask').dispatchEvent(new Event('input'))`,
  );
  await evaluate(`document.querySelector('[data-cm-open="folderModal"]').click()`);
  check(
    '工作目录选择层只展示文件夹名称，不展示完整路径',
    await evaluate(
      `(() => { const modal = document.getElementById('folderModal'); const names = [...modal.querySelectorAll('[data-folder-name]')].map(item => item.dataset.folderName); return modal.classList.contains('is-open') && names.join('|') === 'helm|data-console' && !modal.innerText.includes(':') && !modal.innerText.includes('Projects'); })()`,
    ),
  );
  await screenshot('index-folder-menu');
  await evaluate(`document.querySelector('[data-folder-name="data-console"]').click()`);
  check(
    '工作目录只展示末级名称且宽度随名称变化',
    await evaluate(
      `(() => { const button = document.querySelector('[data-cm-open="folderModal"]'); const label = document.getElementById('folderLabel'); return label.textContent.trim() === 'data-console' && !label.textContent.includes(':') && button.getAttribute('aria-label').includes('data-console') && button.getBoundingClientRect().width > window.__initialFolderButtonWidth && button.getBoundingClientRect().width < 150; })()`,
    ),
  );
  check(
    '新任务发送前不展示上下文占用',
    await evaluate(
      `!document.querySelector('#contextRing,.ctx-ring,[data-context-usage]') && !document.getElementById('effortSelect').textContent.includes('✦')`,
    ),
  );
  await evaluate(`document.getElementById('modeSelect').click()`);
  check(
    '新任务三种模式提供用户向边界说明',
    await evaluate(
      `(() => { const text = document.querySelector('.floatmenu').innerText; return ['直接完成开发任务', '先看方案再决定是否执行', '了解代码或排查思路'].every(value => text.includes(value)); })()`,
    ),
  );
  await screenshot('index-mode-menu');
  await evaluate(`document.getElementById('permissionSelect').click()`);
  check(
    '自动执行与全部放开有明确警示和范围说明',
    await evaluate(
      `(() => { const menu = document.querySelector('.floatmenu'); const text = menu.innerText; return menu.querySelector('button.is-warn') && menu.querySelector('button.is-danger') && ['可信项目', '高风险命令仍会阻断', '应用重启后自动失效'].every(value => text.includes(value)); })()`,
    ),
  );
  await screenshot('index-permission-menu');
  await evaluate(
    `document.getElementById('engineSelect').click();[...document.querySelectorAll('.floatmenu button')].find(button => button.textContent.includes('Codex')).click()`,
  );
  await evaluate(`document.getElementById('modelSelect').click()`);
  check(
    'Codex 只展示自身模型且模型项不带说明',
    await evaluate(
      `(() => { const menu = document.querySelector('.floatmenu'); const labels = [...menu.querySelectorAll('button')].map(button => button.querySelector('.floatmenu__copy > span')?.textContent); return labels.join('|') === 'GPT-5.2 Codex|GPT-5.2 mini' && !menu.querySelector('.floatmenu__copy small'); })()`,
    ),
  );
  await evaluate(`document.getElementById('effortSelect').click()`);
  check(
    'Codex 推理强度按当前能力集展示',
    await evaluate(
      `[...document.querySelectorAll('.floatmenu button')].map(button => button.querySelector('.floatmenu__copy > span')?.textContent).join('|') === '自动|低|中|高|超高'`,
    ),
  );
  await evaluate(
    `document.getElementById('engineSelect').click();[...document.querySelectorAll('.floatmenu button')].find(button => button.textContent.includes('Claude Code')).click();document.getElementById('effortSelect').click()`,
  );
  check(
    'Claude Code 推理强度包含 CLI 声明的超高与最大档位',
    await evaluate(
      `[...document.querySelectorAll('.floatmenu button')].map(button => button.querySelector('.floatmenu__copy > span')?.textContent).join('|') === '自动|低|中|高|超高|最大'`,
    ),
  );
  await evaluate(`document.getElementById('capTrigger').click()`);
  check(
    '新任务 + 只保留文件、命令与技能入口',
    await evaluate(
      `(() => { const menu = document.getElementById('capMenu'); const text = menu.innerText; return !menu.hidden && menu.querySelectorAll('[data-open-center]').length === 3 && text.includes('文件与目录') && text.includes('常用命令') && text.includes('技能') && !text.includes('MCP'); })()`,
    ),
  );
  await evaluate(`document.querySelector('#capMenu [data-open-center="commandCenter"]').click()`);
  check(
    '命令列表独立展示常用真实命令与中文说明',
    await evaluate(
      `(() => { const text = document.getElementById('centerList').innerText; return ['/review','/test','/resume','/permissions','/extensions','/help'].every(value => text.includes(value)) && text.includes('审查当前工作区变更') && !text.includes('/compact') && document.querySelectorAll('#centerList .cm-command-row').length >= 6; })()`,
    ),
  );
  await sleep(220);
  await screenshot('index-command-center');
  await setViewport(800, 600);
  check(
    '命令中心 800x600 完整收纳在视口内',
    await evaluate(
      `(() => { const rect = document.querySelector('#compactCenter .cm-modal').getBoundingClientRect(); return rect.left >= 0 && rect.top >= 0 && rect.right <= innerWidth && rect.bottom <= innerHeight && document.documentElement.scrollWidth <= innerWidth; })()`,
    ),
  );
  await screenshot('index-command-center-800x600-light');
  await setViewport(1600, 950);
  await evaluate(
    `document.querySelector('#compactCenter [data-cm-close]').click();document.getElementById('capTrigger').click();document.querySelector('#capMenu [data-open-center="skillCenter"]').click()`,
  );
  check(
    '技能列表与命令分开展示并带用途说明',
    await evaluate(
      `(() => { const text = document.getElementById('centerList').innerText; return ['frontend-skill','openai-docs','pdf','imagegen','skill-creator'].every(value => text.includes(value)) && text.includes('视觉一致性') && !text.includes('/review'); })()`,
    ),
  );
  await evaluate(
    `document.querySelector('#centerList [data-center-label="frontend-skill"]').click()`,
  );
  check(
    'Claude Code 技能按斜杠语义插入输入框',
    await evaluate(`document.getElementById('quickTask').value.trim() === '/frontend-skill'`),
  );

  await navigate('index', '?setup=git');
  await evaluate(
    `document.getElementById('quickTask').value='检查仓库状态';document.getElementById('quickStart').requestSubmit()`,
  );
  check(
    'Git 作为 Agent 完整能力的必选依赖但不增加第四个主检查项',
    await evaluate(
      `(() => { const modal = document.getElementById('readinessModal'); return modal.querySelectorAll('[data-readiness-key]').length === 3 && document.getElementById('readinessAgentRow').classList.contains('is-missing') && document.getElementById('agentCliState').classList.contains('is-ready') && document.getElementById('agentGitState').classList.contains('is-missing') && document.getElementById('installAgentButton').textContent.trim() === '安装 Git' && document.getElementById('readinessCount').textContent.includes('2 / 3'); })()`,
    ),
  );

  await navigate('index', '?setup=missing');
  check(
    '就绪项缺失时首次发送前仍保持新任务页简洁',
    await evaluate(`!document.getElementById('readinessModal').classList.contains('is-open')`),
  );
  await evaluate(
    `document.getElementById('quickTask').value='检查登录流程';document.getElementById('quickStart').requestSubmit()`,
  );
  check(
    '发送条件不足时以弹窗展示 Agent、服务商配置与工作目录',
    await evaluate(
      `(() => { const modal = document.getElementById('readinessModal'); const rows = [...modal.querySelectorAll('[data-readiness-key]')]; const text = modal.innerText; const segments = [...modal.querySelectorAll('[data-readiness-segment]')]; return modal.classList.contains('is-open') && document.getElementById('quickTask').value === '检查登录流程' && rows.length === 3 && rows.every(row => row.classList.contains('is-missing')) && ['Claude Code','Git for Windows','服务商配置','工作目录'].every(value => text.includes(value)) && !/Node|npm/.test(text) && document.getElementById('readinessCount').textContent.includes('0 / 3') && segments.length === 3 && segments.every(item => !item.classList.contains('is-ready')) && document.getElementById('sendTaskButton').classList.contains('is-blocked'); })()`,
    ),
  );
  check(
    '三项缺失分别提供下载 Agent、配置服务商和选择目录动作',
    await evaluate(
      `document.getElementById('installAgentButton').textContent.trim() === '安装 Agent 与 Git' && document.getElementById('configureProviderButton').getAttribute('href') === 'providers.html?from=new-task' && document.getElementById('chooseReadinessDirectory').textContent.trim() === '选择目录' && getComputedStyle(document.getElementById('continueTaskButton')).display === 'none'`,
    ),
  );
  await screenshot('index-readiness-1600x950-light');
  await setViewport(800, 600);
  check(
    '就绪检查弹窗在 800x600 内完整可见且无横向溢出',
    await evaluate(
      `(() => { const modal = document.querySelector('#readinessModal .cm-modal'); const rect = modal.getBoundingClientRect(); return rect.left >= 0 && rect.top >= 0 && rect.right <= innerWidth && rect.bottom <= innerHeight && modal.scrollWidth <= modal.clientWidth && document.documentElement.scrollWidth <= innerWidth; })()`,
    ),
  );
  await screenshot('index-readiness-800x600-light');
  await setViewport(1600, 950);
  await evaluate(`document.getElementById('installAgentButton').click()`);
  check(
    'Agent 与 Git 进入真实安装态且不伪造百分比',
    await evaluate(
      `document.getElementById('readinessAgentRow').classList.contains('is-installing') && document.getElementById('agentCliState').classList.contains('is-installing') && document.getElementById('agentGitState').classList.contains('is-installing') && document.getElementById('installAgentButton').textContent.includes('正在准备') && !document.getElementById('readinessAgentRow').textContent.includes('%')`,
    ),
  );
  await sleep(760);
  check(
    'Agent 下载完成后原地复检并打勾',
    await evaluate(
      `document.getElementById('readinessAgentRow').classList.contains('is-ready') && document.getElementById('agentCliState').classList.contains('is-ready') && document.getElementById('agentGitState').classList.contains('is-ready') && document.querySelector('#readinessAgentRow .cm-readiness__state').dataset.ic === 'checkc' && document.querySelector('#readinessAgentRow .cm-readiness__done').textContent.includes('已就绪') && document.getElementById('readinessCount').textContent.includes('1 / 3')`,
    ),
  );
  await evaluate(`document.getElementById('chooseReadinessDirectory').click()`);
  check(
    '目录缺失从就绪弹窗进入目录选择层',
    await evaluate(
      `document.getElementById('folderModal').classList.contains('is-open') && !document.getElementById('readinessModal').classList.contains('is-open')`,
    ),
  );
  await evaluate(
    `document.querySelector('#folderModal [data-folder-name="data-console"]').click()`,
  );
  check(
    '选中目录后返回就绪弹窗并为工作目录打勾',
    await evaluate(
      `document.getElementById('readinessModal').classList.contains('is-open') && document.getElementById('readinessDirectoryRow').classList.contains('is-ready') && document.getElementById('folderLabel').textContent.trim() === 'data-console' && document.getElementById('readinessCount').textContent.includes('2 / 3')`,
    ),
  );

  await navigate('index', '?setup=agent');
  await evaluate(
    `document.getElementById('quickTask').value='修复支付页问题';document.getElementById('quickStart').requestSubmit();document.getElementById('installAgentButton').click()`,
  );
  await sleep(760);
  check(
    '三项全绿后解锁发送并提供继续动作',
    await evaluate(
      `(() => { const rows = [...document.querySelectorAll('[data-readiness-key]')]; const segments = [...document.querySelectorAll('[data-readiness-segment]')]; const action = document.getElementById('continueTaskButton'); return rows.every(row => row.classList.contains('is-ready')) && segments.every(item => item.classList.contains('is-ready')) && document.getElementById('readinessCount').textContent.includes('3 / 3') && !document.getElementById('sendTaskButton').classList.contains('is-blocked') && !action.hidden && getComputedStyle(action).display !== 'none' && document.getElementById('readinessNote').textContent.includes('三项均已就绪'); })()`,
    ),
  );

  if (indexOnly) return;

  await navigate('providers');
  await evaluate(`document.querySelector('[data-bind-engine]').click()`);
  check(
    '执行引擎卡片可打开绑定操作',
    await evaluate(`document.getElementById('bindingModal').classList.contains('is-open')`),
  );
  await evaluate(
    `document.querySelector('[data-cm-tab="models"]').click();document.getElementById('openPricing').click()`,
  );
  check(
    '模型页可打开定价目录弹框',
    await evaluate(`document.getElementById('pricingModal').classList.contains('is-open')`),
  );

  await navigate('extensions');
  check(
    '插件页视觉层级与 AI 配置页一致',
    await evaluate(
      `(() => { const search = getComputedStyle(document.querySelector('[data-cm-panel="skills"] .cm-search')); const grid = getComputedStyle(document.querySelector('[data-cm-panel="skills"] .cm-skill-grid')); return search.borderTopWidth === '0px' && grid.gridTemplateColumns.split(' ').length === 2; })()`,
    ),
  );
  await evaluate(`document.querySelector('[data-agent="codex"]').click()`);
  check(
    'Skill 引擎切换同步更新分组数量',
    await evaluate(
      `document.getElementById('skillCount').textContent.includes('Codex · 4 个已安装') && [...document.querySelectorAll('[data-skill-count]')].map(item => item.textContent).join(',') === '1,2,1'`,
    ),
  );
  await evaluate(
    `document.getElementById('skillSearch').value='OpenAI';document.getElementById('skillSearch').dispatchEvent(new Event('input',{bubbles:true}))`,
  );
  check(
    'Skill 搜索同步收敛卡片与分组',
    await evaluate(
      `document.querySelectorAll('[data-skill-card]:not([hidden])').length === 1 && [...document.querySelectorAll('[data-skill-count]')].filter(item => !item.closest('.cm-section').hidden).length === 1 && document.getElementById('skillCount').textContent.includes('1 个已安装')`,
    ),
  );
  await evaluate(
    `document.getElementById('skillSearch').value='';document.getElementById('skillSearch').dispatchEvent(new Event('input',{bubbles:true}));document.querySelector('[data-agent="claude"]').click()`,
  );
  await evaluate(`document.querySelector('[data-skill-card]').click()`);
  check(
    'Skill 卡片打开右侧详情',
    await evaluate(`document.getElementById('skillDrawer').classList.contains('is-open')`),
  );
  await evaluate(
    `document.querySelector('#skillDrawer [data-cm-close]').click();document.querySelector('[data-cm-tab="mcp"]').click()`,
  );
  await sleep(220);
  await screenshot('extensions-mcp');
  check(
    'MCP 同页包含精选与已安装',
    await evaluate(
      `document.querySelectorAll('#mcpFeatured [data-mcp-item]').length === 6 && document.querySelectorAll('#mcpInstalled [data-mcp-item]').length === 3`,
    ),
  );

  await navigate('settings');
  await evaluate(
    `document.querySelector('[data-cm-tab="theme"]').click();document.querySelector('[data-theme-mode="dark"]').click()`,
  );
  check(
    '主题设置即时切换深色',
    await evaluate(`document.documentElement.dataset.theme === 'dark'`),
  );
  await evaluate(`Helm.setTheme('light')`);

  await navigate('workspace');
  await evaluate(`localStorage.setItem('helm:session', 'auth')`);
  await navigate('workspace', '?task=refactor');
  check(
    'URL 任务优先于上次打开记录并在三处同步高亮',
    await evaluate(
      `document.getElementById('threadTitle').textContent.trim() === '统一全站错误处理' && document.querySelector('[data-cm-sidebar] [data-cm-task-id="refactor"]')?.classList.contains('is-active') && document.querySelector('.sitem[data-id="refactor"]')?.classList.contains('is-active')`,
    ),
  );
  await evaluate(`document.querySelector('.sitem[data-id="etl"]').click()`);
  check(
    '任务抽屉切换后同步 URL、最近任务与线程标题',
    await evaluate(
      `new URLSearchParams(location.search).get('task') === 'etl' && document.getElementById('threadTitle').textContent.trim() === '重构 ETL 聚合阶段' && document.querySelector('[data-cm-sidebar] [data-cm-task-id="etl"]')?.classList.contains('is-active')`,
    ),
  );
  check(
    '工作台模型菜单统一承载模型与推理强度',
    await evaluate(
      `!!document.querySelector('.thread__head #modelBtn') && document.querySelectorAll('#modelMenu [data-model]').length >= 2 && document.querySelectorAll('#modelMenu [data-effort]').length >= 5 && !document.getElementById('effortBtn')`,
    ),
  );
  check(
    '工作区旧四箭头导航不可见',
    await evaluate(`getComputedStyle(document.getElementById('navFab')).display === 'none'`),
  );
  await evaluate(`document.querySelector('.ctx__tabs [data-tab="files"]').click()`);
  await evaluate(`document.querySelector('#allFiles .file-link').click()`);
  check(
    '全部文件可打开动态文件预览',
    await evaluate(
      `!!document.querySelector('#ctxDynTabs [data-tab].is-active') && !!document.querySelector('.filepreview:not(.hide)')`,
    ),
  );
  await screenshot('workspace-file-preview-1600x950-light');
  await evaluate(`document.getElementById('ctxMax').click()`);
  check(
    '右侧工作区最大化保留全局导航',
    await evaluate(
      `document.getElementById('ws').classList.contains('ctx-max') && getComputedStyle(document.querySelector('[data-cm-sidebar]')).display !== 'none' && getComputedStyle(document.querySelector('.sbar')).visibility === 'hidden' && document.getElementById('ctxMax').dataset.ic === 'minimize'`,
    ),
  );
  await screenshot('workspace-right-maximized-1600x950-light');
  await evaluate(
    `document.getElementById('ctxMax').click();document.getElementById('ctxClose').click()`,
  );
  check(
    '右侧工作区可关闭并还原主线程',
    await evaluate(
      `document.getElementById('ws').classList.contains('no-ctx') && !document.getElementById('ws').classList.contains('ctx-max')`,
    ),
  );
  await evaluate(`document.querySelector('.sitem[data-id="refactor"]').click()`);
  check(
    '运行中 Todo 与子代理在 Composer 上方收为摘要',
    await evaluate(
      `document.getElementById('workStrip').classList.contains('is-on') && document.getElementById('workStrip').classList.contains('is-collapsed') && document.getElementById('workTodoToggle').getAttribute('aria-expanded') === 'false' && document.querySelectorAll('#workAgents .agentchip').length === 3 && document.querySelectorAll('#workTodo .workstrip__todo-row').length === 3`,
    ),
  );
  await screenshot('workspace-execution-strip-1600x950-light');
  await evaluate(`document.querySelector('.sitem[data-id="etl"]').click()`);
  check(
    '完成任务的最终答复提供文件与修改记录入口',
    await evaluate(
      `!!document.querySelector('.deliverables [data-open="files"]') && !!document.querySelector('.deliverables [data-open="changes"]')`,
    ),
  );
  await evaluate(`document.querySelector('.sitem[data-id="auth"]').click()`);
  await evaluate(
    `document.getElementById('composerInput').value='补充第三轮检查';document.getElementById('sendBtn').click()`,
  );
  await sleep(350);
  check(
    '达到三轮后展示轮次刻度',
    await evaluate(
      `document.getElementById('turnRail').classList.contains('is-on') && document.querySelectorAll('#turnRail .turn-rail__item').length >= 3`,
    ),
  );
  check(
    '工作区头部展示执行路由，Composer 只保留模式与权限',
    await evaluate(
      `!!document.querySelector('.thread__head #modelBtn') && !!document.querySelector('.thread__head #engineBtn') && !!document.querySelector('.composer__bar #modeBtn') && !!document.querySelector('.composer__bar #profBtn') && !document.querySelector('.composer__bar #modelBtn') && !document.querySelector('.composer__bar #effortBtn')`,
    ),
  );
  await screenshot('workspace-turn-rail-1600x950-light');
}

staticAudit();
if (staticOnly) {
  const staticFailed = checks.filter((item) => !item.ok);
  console.log(`\n原型静态审计：${checks.length - staticFailed.length}/${checks.length} 通过`);
  if (staticFailed.length) {
    console.log('\n失败断言：');
    for (const item of staticFailed)
      console.log(`- ${item.label}${item.detail ? `: ${item.detail}` : ''}`);
  }
  process.exit(staticFailed.length ? 1 : 0);
}
try {
  await visualAudit();
} catch (error) {
  runtimeProblems.push(error.stack || error.message);
} finally {
  try {
    socket?.close();
  } catch {
    // Socket may already be closed.
  }
  browser.kill();
}

const failed = checks.filter((item) => !item.ok);
console.log(`\n原型审计：${checks.length - failed.length}/${checks.length} 通过`);
if (failed.length) {
  console.log('\n失败断言：');
  for (const item of failed) console.log(`- ${item.label}${item.detail ? `: ${item.detail}` : ''}`);
}
if (runtimeProblems.length) {
  console.log('\n运行时问题：');
  for (const problem of [...new Set(runtimeProblems)]) console.log(`- ${problem}`);
}
console.log(`\n截图：${outDir}`);
process.exit(failed.length || runtimeProblems.length ? 1 : 0);
