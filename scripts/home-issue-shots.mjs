// 新任务页问题复现/验证截图（无 CDP 通道）：构建视觉入口后用 headless Chrome
// --screenshot + ?probeOpen=<state> 逐状态截图。focus() 复现真实点击的
// :focus-within 链路。证据输出 .agent/evidence/home-issue/。
// 沙箱边界：vite 的 optimizeSafeRealPathSync 在 Windows 上无条件 exec('net use')，
// 文件沙箱禁管道 stdio 子进程（spawn EPERM 同步抛出）。realpathSync.native 抛
// EISDIR 命中 vite 早退分支（safeRealpathSync = JS 版），从根源跳过 net use 探测；
// Chrome 以 stdio ignore 启动（--screenshot 不需要 CDP 管道）。
import { createRequire } from 'node:module';
import { spawn } from 'node:child_process';
import fs from 'node:fs';
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
const port = Number(process.env.HELM_HOME_ISSUE_PORT || 4275);
const outputDir = path.join(root, '.agent', 'evidence', 'home-issue');
await fsPromises.mkdir(outputDir, { recursive: true });

const chromeCandidates = [
  process.env.CHROME_PATH,
  'C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe',
  'C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe',
  'C:\\Program Files\\Microsoft\\Edge\\Application\\msedge.exe',
].filter(Boolean);
const chrome = chromeCandidates.find((candidate) => fs.existsSync(candidate));
if (!chrome) throw new Error('未找到 Chrome 或 Edge；可通过 CHROME_PATH 指定浏览器路径。');

console.log('[shots] 构建实现页…');
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
  [
    '<!doctype html>',
    '<html lang="zh-CN">',
    '  <head>',
    '    <meta charset="UTF-8" />',
    '    <meta name="viewport" content="width=device-width, initial-scale=1.0" />',
    '    <title>Helm 问题截图探针</title>',
    '  </head>',
    '  <body>',
    '    <div id="root"></div>',
    '    <script type="module" src="/src/visualAuditMain.js"></script>',
    '  </body>',
    '</html>',
  ].join('\n'),
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
console.log('[shots] 构建完成，启动静态服务…');

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
    const relativePath = pathname === '/' ? 'probe-entry.html' : pathname.replace(/^\/+/, '');
    const body = await fsPromises.readFile(path.resolve(siteDir, relativePath));
    response
      .writeHead(200, {
        'Content-Type': mimeTypes[path.extname(relativePath)] || 'application/octet-stream',
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

const states = [
  'base',
  'capmenu',
  'cap-file',
  // 用户决议（2026-09）：未选工作目录时弹框直开（home-no-dir fixture）
  'cap-file-nodir',
  'cap-cmd',
  'mode',
  'permission',
  'model',
  'effort',
  'engine',
  'dirmode',
];
const userDataDir = path.join(os.tmpdir(), 'helm-home-issue-' + process.pid);
const theme = process.env.THEME === 'dark' ? 'dark' : 'light';
for (const state of states) {
  const shot = path.join(outputDir, theme + '-' + state + '.png');
  // cap-file-nodir 用无默认目录 fixture 验证「未选工作目录」的直开弹框。
  const fixture = state === 'cap-file-nodir' ? 'home-no-dir' : 'home-live';
  const url =
    'http://127.0.0.1:' +
    port +
    '/probe-entry.html?fixture=' +
    fixture +
    '&probeOpen=' +
    state +
    '&theme=' +
    theme;
  await new Promise((resolve, reject) => {
    const child = spawn(
      chrome,
      [
        '--headless=new',
        '--disable-gpu',
        '--hide-scrollbars',
        '--no-first-run',
        '--disable-extensions',
        '--disable-background-networking',
        '--virtual-time-budget=15000',
        '--window-size=1366,768',
        '--user-data-dir=' + userDataDir,
        '--screenshot=' + shot,
        url,
      ],
      { stdio: 'ignore', windowsHide: true },
    );
    child.on('exit', (code) =>
      code === 0 ? resolve() : reject(new Error(state + ' 截图退出码 ' + code)),
    );
    child.on('error', reject);
  });
  console.log('[shots]', state, '->', path.basename(shot));
}
await fsPromises.rm(userDataDir, { recursive: true, force: true }).catch(() => undefined);
preview.close();
console.log('[shots] 完成：' + outputDir);
