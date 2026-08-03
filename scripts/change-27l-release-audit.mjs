import { readdir, readFile, stat } from 'node:fs/promises';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import path from 'node:path';

const scriptDir = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(scriptDir, '..');

export const forbiddenProductionPatterns = [
  { label: 'HelmProtected 产品符号', pattern: /helm[_ -]?protected/i },
  { label: '旧 enforcement 探针/证据', pattern: /enforcement[_ -]?(?:probe|manifest|evidence)/i },
  { label: '旧 Capability Broker', pattern: /capability[_ -]?broker/i },
  { label: '旧 protected profile', pattern: /protected[_ -]?profile/i },
  { label: '已退役 AppContainer 执行器', pattern: /appcontainer/i },
];

const sourceRoots = ['src', 'packages/protocol/src', 'src-tauri/src', 'src-tauri/tauri.conf.json'];
const artifactRoots = ['dist', 'src-tauri/target/release/bundle'];
const textExtensions = new Set([
  '.css',
  '.html',
  '.js',
  '.json',
  '.jsx',
  '.mjs',
  '.rs',
  '.ts',
  '.tsx',
  '.toml',
  '.txt',
]);

async function filesUnder(target) {
  let info;
  try {
    info = await stat(target);
  } catch (error) {
    if (error?.code === 'ENOENT') return [];
    throw error;
  }
  if (info.isFile()) return [target];
  const entries = await readdir(target, { withFileTypes: true });
  const nested = await Promise.all(
    entries
      .filter((entry) => !entry.isSymbolicLink())
      .map((entry) => filesUnder(path.join(target, entry.name))),
  );
  return nested.flat();
}

export async function scanReleaseSurface(root = repoRoot) {
  const findings = [];
  for (const relativeRoot of [...sourceRoots, ...artifactRoots]) {
    const absoluteRoot = path.join(root, relativeRoot);
    for (const file of await filesUnder(absoluteRoot)) {
      const extension = path.extname(file).toLowerCase();
      const isArtifact = artifactRoots.some((item) => file.startsWith(path.join(root, item)));
      if (!isArtifact && !textExtensions.has(extension)) continue;
      const content = (await readFile(file)).toString('utf8');
      for (const rule of forbiddenProductionPatterns) {
        if (rule.pattern.test(content)) {
          findings.push({
            file: path.relative(root, file).replaceAll('\\', '/'),
            label: rule.label,
          });
        }
      }
    }
  }
  return findings;
}

function run(command, args) {
  const result = spawnSync(command, args, {
    cwd: repoRoot,
    encoding: 'utf8',
    shell: process.platform === 'win32',
    stdio: 'inherit',
  });
  if (result.error) throw result.error;
  if (result.status !== 0) process.exit(result.status ?? 1);
}

async function main() {
  const findings = await scanReleaseSurface();
  if (findings.length > 0) {
    for (const finding of findings) {
      console.error('[27L release audit] ' + finding.label + ': ' + finding.file);
    }
    process.exit(1);
  }
  console.log('[27L release audit] 生产源码与现有构建产物无退役执行路径符号。');

  if (process.argv.includes('--with-migrations')) {
    run('cargo', [
      'test',
      '--manifest-path',
      'src-tauri/Cargo.toml',
      '--test',
      'session_history',
      'change_27l_',
      '--',
      '--test-threads=1',
    ]);
  }
}

if (path.resolve(process.argv[1] ?? '') === fileURLToPath(import.meta.url)) {
  await main();
}
