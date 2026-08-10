// 原型运行时走查：无头浏览器真实加载 workspace.html，
// 触发变更-34 的每个新交互，收集 console 错误与异常，并截图留证。
import { spawn } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { pathToFileURL } from 'node:url';

const root = path.resolve(import.meta.dirname, '..');
const outDir = path.join(root, '.agent', 'evidence', 'proto-34');
fs.mkdirSync(outDir, { recursive: true });

const debugPort = 9333;
const chromeCandidates = [
  process.env.CHROME_PATH,
  'C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe',
  'C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe',
  'C:\\Program Files\\Microsoft\\Edge\\Application\\msedge.exe',
].filter(Boolean);
const chrome = chromeCandidates.find((c) => fs.existsSync(c));
if (!chrome) throw new Error('未找到 Chrome 或 Edge；可用 CHROME_PATH 指定。');

const profile = fs.mkdtempSync(path.join(os.tmpdir(), 'helm-proto-'));
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
    width: 1600,
    height: 950,
    deviceScaleFactor: 1,
    mobile: false,
  });

  const url = pathToFileURL(path.join(root, 'prototype', 'workspace.html')).href;
  await call('Page.navigate', { url });
  await sleep(1200);

  // 基线：默认会话渲染出轮次容器与条目
  check(
    '线程渲染出条目',
    (await evaluate('document.querySelectorAll("#threadInner .item").length')) > 0,
  );
  check(
    '轮次容器存在（B2）',
    (await evaluate('document.querySelectorAll("#threadInner .turn").length')) > 0,
  );
  check(
    '轮次摘要头存在',
    (await evaluate('document.querySelectorAll("#threadInner .turn__head").length')) > 0,
  );
  await shot('01-default-standard');

  // B1 专注模式两态（Ctrl+O 切换，三档密度已取消）
  check('头部无密度选择器', (await evaluate('!document.getElementById("densBtn")')) === true);
  check(
    '头部无跟随 Agent 按钮',
    (await evaluate('!document.getElementById("followBtn")')) === true,
  );
  check('头部无重复分屏按钮', (await evaluate('!document.getElementById("artToggle")')) === true);
  // 用真实快捷键驱动，不给原型加测试专用全局
  await evaluate(
    'document.dispatchEvent(new KeyboardEvent("keydown",{key:"o",ctrlKey:true,bubbles:true}))',
  );
  await sleep(250);
  check(
    '专注模式生效',
    await evaluate('document.querySelector(".thread").classList.contains("dens-lite")'),
  );
  check(
    '专注模式下过程条目被收起',
    await evaluate(
      '[...document.querySelectorAll(\'#threadInner .item[data-kind="tgrp"]\')].every((e)=>getComputedStyle(e).display==="none")',
    ),
  );
  await shot('02-focus-on');
  await evaluate(
    'document.dispatchEvent(new KeyboardEvent("keydown",{key:"o",ctrlKey:true,bubbles:true}))',
  );
  await sleep(250);
  check(
    '关闭专注模式恢复标准',
    !(await evaluate('document.querySelector(".thread").classList.contains("dens-lite")')),
  );
  await shot('03-focus-off');

  // A 交付物区
  // auth 会话有 changes，启动时会自动打开右栏；如果已经打开就不需要点击 toggle
  const hasCtxBefore = await evaluate(
    '!document.getElementById("ws").classList.contains("no-ctx")',
  );
  if (!hasCtxBefore) {
    await evaluate('document.getElementById("ctxToggle").click()');
    await sleep(400);
  }
  check(
    '交付物区打开（A5）',
    await evaluate('!document.getElementById("ws").classList.contains("no-ctx")'),
  );
  check(
    '变更文件清单有内容（A1）',
    (await evaluate('document.querySelectorAll("#chgFiles .afile").length')) > 0,
  );
  check('diff 渲染出行', (await evaluate('document.querySelectorAll("#chgView .dvl").length')) > 0);
  check(
    '未变更行折叠条存在',
    (await evaluate('document.querySelectorAll("#chgView .dskip").length')) > 0,
  );
  await shot('04-artifact-changes');

  // A1 并排视图
  await evaluate('document.querySelector(\'#dvMode [data-dv="split"]\').click()');
  await sleep(300);
  check('并排视图生效', await evaluate('!!document.querySelector("#chgView .dvw.is-split")'));
  await shot('05-diff-split');
  await evaluate('document.querySelector(\'#dvMode [data-dv="unified"]\').click()');
  await sleep(200);

  // 变更-34 v4：轮次结构与文件链接
  check(
    '用户消息不在 .turn 容器内',
    (await evaluate('document.querySelectorAll(\'#threadInner .item[data-kind="user"]\').length')) >
      0 &&
      (await evaluate(
        '[...document.querySelectorAll(\'#threadInner .item[data-kind="user"]\')].every(u => !u.closest(".turn"))',
      )),
  );
  check(
    '轮次容器只包含过程条目',
    (await evaluate('document.querySelectorAll("#threadInner .turn").length')) > 0 &&
      (await evaluate(
        '[...document.querySelectorAll("#threadInner .turn")].every(t => !t.querySelector(\'.item[data-kind="user"]\'))',
      )),
  );
  check(
    '轮次折叠按钮存在',
    (await evaluate('document.querySelectorAll("#threadInner .turn__lite").length')) > 0,
  );
  check(
    '文件链接被标记',
    (await evaluate('document.querySelectorAll("#threadInner .file-link").length')) > 0,
  );
  const fileLink = await evaluate('document.querySelector("#threadInner .file-link")');
  if (fileLink) {
    await evaluate('document.querySelector("#threadInner .file-link").click()');
    await sleep(300);
    check(
      '点击文件链接打开右栏',
      !(await evaluate('document.getElementById("ws").classList.contains("no-ctx")')),
    );
  }
  await shot('04a-turn-structure-file-links');

  // A2 行级审阅意见 → 攒批 → 回灌
  await evaluate('document.querySelector("#chgView .dvl__add").click()');
  await sleep(250);
  check(
    '行内意见编辑器打开（A2）',
    await evaluate('!!document.querySelector("#chgView .rnote.is-draft textarea")'),
  );
  await evaluate(
    '(()=>{const t=document.querySelector("#chgView .rnote.is-draft textarea");t.value="这里要考虑并发刷新风暴";document.querySelector("#chgView .rnote.is-draft .nsave").click();})()',
  );
  await sleep(300);
  check('意见已记录', (await evaluate('document.querySelectorAll("#chgView .rnote").length')) > 0);
  check(
    '回灌条出现',
    await evaluate('document.getElementById("noteBar").classList.contains("is-on")'),
  );
  await shot('06-review-note');

  // A3 自评审
  await evaluate('document.getElementById("selfReview").click()');
  await sleep(400);
  check(
    '自评审产出 AI 意见（A3）',
    (await evaluate('document.querySelectorAll("#chgView .rnote.is-ai").length')) > 0,
  );
  await shot('07-self-review');

  // A2 回灌进线程
  const beforeItems = await evaluate('document.querySelectorAll("#threadInner .item").length');
  await evaluate('document.getElementById("noteSend").click()');
  await sleep(500);
  check(
    '审阅意见回灌成一条线程消息',
    (await evaluate(
      'document.querySelectorAll(\'#threadInner .item[data-kind="review"]\').length',
    )) > 0,
    'before=' + beforeItems,
  );
  await shot('08-review-fed-back');
  await sleep(1600);

  // 计划 / 终端：动态 tab —— 默认不常驻，只有线程内点开才出现
  check(
    '计划 tab 默认不常驻',
    (await evaluate('!document.querySelector(\'.ctx__tabs [data-tab="plan"]\')')) === true,
  );
  check(
    '终端 tab 默认不常驻',
    (await evaluate('!document.querySelector(\'.ctx__tabs [data-tab="term"]\')')) === true,
  );
  await evaluate(
    '(()=>{const b=document.querySelector(\'#threadInner [data-open="plan"]\');b&&b.click();})()',
  );
  await sleep(350);
  check(
    '线程内打开后计划 tab 出现并有内容（A4）',
    (await evaluate('document.querySelectorAll("#dockPlan .plan li").length')) > 0,
  );
  check(
    '计划动态 tab 可关闭',
    await evaluate('!!document.querySelector(\'.ctx__tabs [data-tab="plan"] .tab__x\')'),
  );
  await shot('09-artifact-plan');
  await evaluate(
    '(()=>{const b=document.querySelector(\'#threadInner [data-open="term"]\');b&&b.click();})()',
  );
  await sleep(350);
  check(
    '线程内打开后终端 tab 出现并有内容（A4）',
    (await evaluate('document.getElementById("dockTerm").textContent.trim().length')) > 0,
  );
  await shot('10-artifact-term');
  // 关掉动态 tab，验证它真的退出常驻
  await evaluate(
    '(()=>{const x=document.querySelector(\'.ctx__tabs [data-tab="term"] .tab__x\');x&&x.click();})()',
  );
  await sleep(300);
  check(
    '关闭后终端 tab 从 tabbar 移除',
    (await evaluate('!document.querySelector(\'.ctx__tabs [data-tab="term"]\')')) === true,
  );

  // E 右栏任务面板与归因（切到并行子代理会话）
  await evaluate(
    '(()=>{const b=[...document.querySelectorAll(".sitem")].find(x=>x.dataset.id==="refactor");b&&b.click();})()',
  );
  await sleep(600);
  check(
    '切换到 refactor 会话',
    (await evaluate('document.getElementById("threadTitle").textContent')).includes('错误处理'),
  );
  check(
    '子代理卡渲染（C1）',
    (await evaluate("document.querySelectorAll('#threadInner .sagent .sarow').length")) > 0,
  );
  check(
    '压缩标记渲染（B4）',
    (await evaluate('document.querySelectorAll("#threadInner .compact").length')) > 0,
  );
  check(
    '建议任务渲染（F3）',
    (await evaluate('document.querySelectorAll("#threadInner .sugg").length')) > 0,
  );
  check(
    '上下文压缩 banner 出现（84%）',
    await evaluate('document.getElementById("ctxBanner").classList.contains("is-on")'),
  );
  await shot('11-subagents-thread');

  await evaluate(
    '(()=>{const b=document.querySelector(\'#threadInner [data-open="tasks"]\');b&&b.click();})()',
  );
  await sleep(350);
  check(
    '任务面板有子代理（E1）',
    (await evaluate('document.querySelectorAll("#saList .toolrow").length')) > 0,
  );
  check(
    '任务面板有后台命令（C2）',
    (await evaluate('document.querySelectorAll("#bgList .toolrow").length')) > 0,
  );
  await shot('12-tasks-panel');

  // 上下文三级递进：composer 圆环 → popover → 归因全文（不再是右栏 tab）
  // 只断言 is-on 会漏掉「内容内联铺在 composer 下」的破版，所以按真实可见性断言。
  const popVisible =
    '(()=>{const e=document.getElementById("ctxPop");' +
    'return e.getClientRects().length>0&&getComputedStyle(e).display!=="none"})()';
  const mcpVisible =
    '(()=>{const e=[...document.querySelectorAll("#ctxPop .csec__t")]' +
    '.find((x)=>x.textContent.includes("MCP"));return !!e&&e.getClientRects().length>0})()';
  check('composer 上下文圆环常驻', await evaluate('!!document.getElementById("ctxRing")'));
  check('未点开时 popover 不占位', !(await evaluate(popVisible)));
  check('未点开时 MCP 区块不可见', !(await evaluate(mcpVisible)));
  await evaluate('document.getElementById("ctxRing").click()');
  await sleep(300);
  check(
    '上下文 popover 打开',
    await evaluate('document.getElementById("ctxPop").classList.contains("is-on")'),
  );
  check('打开后 popover 真实可见', await evaluate(popVisible));
  check('打开后 MCP 区块可见（原右栏工具内容已并入）', await evaluate(mcpVisible));
  const attribRows = await evaluate('document.querySelectorAll("#attribList .attrow").length');
  const attribEmpty = await evaluate('!!document.querySelector("#attribList .sfolder-empty")');
  const attribHtml = await evaluate(
    'document.getElementById("attribList").innerHTML.slice(0, 100)',
  );
  check(
    '占用归因有行或显示暂无数据（E2）',
    attribRows > 0 || attribEmpty,
    'rows=' + attribRows + ' empty=' + attribEmpty + ' html=' + attribHtml,
  );
  await shot('13-attribution');
  await evaluate('document.getElementById("ctxRing").click()');
  await sleep(150);

  // Codex 会话：压缩能力按引擎差异化
  await evaluate(
    '(()=>{const b=[...document.querySelectorAll(".sitem")].find(x=>x.dataset.id==="etl");b&&b.click();})()',
  );
  await sleep(600);
  check(
    'Codex 下压缩按钮禁用（能力诚实）',
    await evaluate('document.getElementById("ctxCompact").disabled === true'),
  );
  check(
    'Codex 归因显示暂无（无逐项数据）',
    (await evaluate('document.getElementById("attribList").textContent')).includes('暂无'),
  );
  await shot('14-codex-capability');

  // F1 会话筛选 + 归档
  await evaluate('document.querySelector(\'#sFilter [data-sf="running"]\').click()');
  await sleep(350);
  check('运行中筛选生效（F1）', (await evaluate('document.querySelectorAll(".sitem").length')) > 0);
  await shot('15-filter-running');
  await evaluate(
    '[...document.querySelectorAll("#sFilter .fchip")].find((c)=>c.dataset.sf==="all").click()',
  );
  await sleep(250);

  // D3 旁路提问
  await evaluate(
    'document.dispatchEvent(new KeyboardEvent("keydown",{key:";",ctrlKey:true,bubbles:true}))',
  );
  await sleep(300);
  check(
    '旁路提问面板打开（D3）',
    await evaluate('document.getElementById("sideChat").classList.contains("is-on")'),
  );
  await shot('16-side-query');

  // C4 失败终态 + F1 失败筛选（migrate 会话）
  await evaluate(
    '[...document.querySelectorAll("#sFilter .fchip")].find((c)=>c.dataset.sf==="failed").click()',
  );
  await sleep(350);
  check(
    '失败筛选出会话（F1）',
    (await evaluate('document.querySelectorAll(".sitem").length')) === 1,
  );
  await evaluate(
    '(()=>{const b=[...document.querySelectorAll(".sitem")].find(x=>x.dataset.id==="migrate");b&&b.click();})()',
  );
  await sleep(500);
  check(
    '失败卡渲染（C4）',
    (await evaluate('document.querySelectorAll("#threadInner .failc").length')) > 0,
  );
  check(
    '失败卡有分类标签',
    (await evaluate('document.querySelector("#threadInner .failc .pill").textContent')).includes(
      '失败',
    ),
  );
  check(
    '失败卡有重试入口',
    (await evaluate('document.querySelectorAll("#threadInner .fail-retry").length')) > 0,
  );
  await shot('20-failed-state');
  await evaluate(
    '[...document.querySelectorAll("#sFilter .fchip")].find((c)=>c.dataset.sf==="archived").click()',
  );
  await sleep(300);
  check(
    '归档筛选可见已归档会话（F1）',
    (await evaluate('document.querySelectorAll(".sitem.is-arch").length')) > 0,
  );
  await shot('21-archived');
  await evaluate(
    '[...document.querySelectorAll("#sFilter .fchip")].find((c)=>c.dataset.sf==="all").click()',
  );
  await sleep(250);

  // D1 @提及药丸
  await evaluate(
    '(()=>{const i=document.getElementById("composerInput");i.value="@token";i.dispatchEvent(new Event("input",{bubbles:true}));})()',
  );
  await sleep(300);
  check(
    '@ 触发文件联想（D1）',
    await evaluate('document.getElementById("slashMenu").classList.contains("open")'),
  );
  await evaluate('document.querySelector("#slashMenu [data-at]").click()');
  await sleep(250);
  check(
    '@提及生成药丸（D1）',
    (await evaluate('document.querySelectorAll("#ctxPills .cpill").length')) > 0,
  );
  await shot('22-context-pills');

  // F2 会话卡信息
  check(
    '会话卡显示当前动作/变更规模（F2）',
    (await evaluate('document.querySelectorAll(".sitem__do").length')) > 0,
  );

  // E1 右栏可拖
  check('右栏分割条存在（E1）', await evaluate('!!document.getElementById("ctxSplit")'));

  // 深色主题
  await evaluate('Helm.setTheme("dark")');
  await sleep(400);
  await evaluate('document.getElementById("ctxToggle").click()');
  await sleep(400);
  await shot('17-dark-theme');
  await evaluate('Helm.setTheme("light")');

  // 窄视口降级
  await call('Emulation.setDeviceMetricsOverride', {
    width: 900,
    height: 800,
    deviceScaleFactor: 1,
    mobile: false,
  });
  await sleep(500);
  await shot('18-narrow-900');
  await call('Emulation.setDeviceMetricsOverride', {
    width: 700,
    height: 800,
    deviceScaleFactor: 1,
    mobile: false,
  });
  await sleep(500);
  await shot('19-narrow-700');
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
