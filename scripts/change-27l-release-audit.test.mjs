import { mkdtemp, mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import { spawnSync } from 'node:child_process';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { afterEach, describe, expect, it } from 'vitest';
import { scanReleaseSurface } from './change-27l-release-audit.mjs';

const roots = [];
const windowsIt = process.platform === 'win32' ? it : it.skip;
const realMatrixScript = fileURLToPath(new URL('change-27l-real-matrix.ps1', import.meta.url));
const performanceScript = fileURLToPath(new URL('change-27l-performance.ps1', import.meta.url));

afterEach(async () => {
  await Promise.all(roots.splice(0).map((root) => rm(root, { recursive: true, force: true })));
});

async function fixtureRoot() {
  const root = await mkdtemp(path.join(tmpdir(), 'helm-change-27l-audit-'));
  roots.push(root);
  await Promise.all([
    mkdir(path.join(root, 'src'), { recursive: true }),
    mkdir(path.join(root, 'packages/protocol/src'), { recursive: true }),
    mkdir(path.join(root, 'src-tauri/src'), { recursive: true }),
    mkdir(path.join(root, 'dist/assets'), { recursive: true }),
  ]);
  await writeFile(path.join(root, 'src-tauri/tauri.conf.json'), '{}');
  return root;
}

async function fakeCliEnvironment(scriptContent) {
  const root = await mkdtemp(path.join(tmpdir(), 'helm-change-27l-cli-'));
  roots.push(root);
  const bin = path.join(root, 'bin');
  const appConfig = path.join(root, 'config');
  await Promise.all([
    mkdir(bin, { recursive: true }),
    mkdir(path.join(appConfig, 'cli-profiles/claude-subscription'), { recursive: true }),
    mkdir(path.join(appConfig, 'cli-profiles/codex-subscription'), { recursive: true }),
  ]);
  await Promise.all([
    writeFile(path.join(bin, 'claude.ps1'), scriptContent.claude),
    writeFile(path.join(bin, 'codex.ps1'), scriptContent.codex),
    writeFile(path.join(bin, 'node.ps1'), "Write-Output 'v9.9.9'"),
    writeFile(path.join(bin, 'npm.ps1'), "Write-Output '9.9.9'"),
    writeFile(path.join(bin, 'rustc.ps1'), "Write-Output 'rustc 9.9.9'"),
    writeFile(path.join(bin, 'cargo.ps1'), "Write-Output 'cargo 9.9.9'"),
  ]);
  return { bin, appConfig };
}

function runRealMatrix({ bin, appConfig }, timeoutSeconds) {
  const inheritedPath = process.env.Path ?? process.env.PATH ?? '';
  const commandPath = `${bin}${path.delimiter}${inheritedPath}`;
  return spawnSync(
    'powershell.exe',
    [
      '-NoProfile',
      '-ExecutionPolicy',
      'Bypass',
      '-File',
      realMatrixScript,
      '-AppConfigDir',
      appConfig,
      '-StatusTimeoutSeconds',
      String(timeoutSeconds),
    ],
    {
      encoding: 'utf8',
      env: { ...process.env, Path: commandPath, PATH: commandPath },
      timeout: 15_000,
    },
  );
}

describe('change 27L release audit', () => {
  it('keeps Windows PowerShell 5.1 entrypoints ASCII-safe', async () => {
    for (const script of ['change-27l-performance.ps1', 'change-27l-real-matrix.ps1']) {
      const content = await readFile(new URL(script, import.meta.url), 'utf8');
      expect([...content].every((character) => character.charCodeAt(0) <= 0x7f)).toBe(true);
    }
  });

  windowsIt('classifies PowerShell CLI shims without reading credentials', async () => {
    const environment = await fakeCliEnvironment({
      claude: `
if ($args -contains '--version') { Write-Output 'Claude Code 9.9.9'; exit 0 }
Write-Output '{"loggedIn":true,"authMethod":"oauth"}'
`,
      codex: `
if ($args -contains '--version') { Write-Output 'codex-cli 9.9.9'; exit 0 }
Write-Output 'Logged in using ChatGPT'
`,
    });
    const result = runRealMatrix(environment, 2);
    expect(result.error).toBeUndefined();
    expect(result.status).toBe(0);
    const report = JSON.parse(result.stdout);
    expect(report.engines.claude).toMatchObject({
      version: 'claude code 9.9.9',
      auth_method: 'subscription',
    });
    expect(report.engines.codex).toMatchObject({
      version: 'codex-cli 9.9.9',
      auth_method: 'subscription',
    });
  });

  windowsIt('bounds hung status checks without launching redundant version probes', async () => {
    const environment = await fakeCliEnvironment({
      claude: 'Start-Sleep -Seconds 30',
      codex: 'Start-Sleep -Seconds 30',
    });
    const startedAt = Date.now();
    const result = runRealMatrix(environment, 1);
    expect(result.error).toBeUndefined();
    expect(result.status).toBe(0);
    expect(Date.now() - startedAt).toBeLessThan(10_000);
    const report = JSON.parse(result.stdout);
    expect(report.engines.claude).toMatchObject({ version: 'timeout', auth_method: 'timeout' });
    expect(report.engines.codex).toMatchObject({ version: 'timeout', auth_method: 'timeout' });
  });

  windowsIt('accepts successful Cargo output written to stderr', async () => {
    const result = spawnSync(
      'powershell.exe',
      [
        '-NoProfile',
        '-ExecutionPolicy',
        'Bypass',
        '-File',
        performanceScript,
        '-Runs',
        '1',
        '-CargoExecutable',
        'cmd.exe',
        '-CargoPrefixArguments',
        '/d /c echo cargo 9.9.9 & echo test result: ok. 1 passed; 0 failed 1>&2',
      ],
      {
        encoding: 'utf8',
        env: process.env,
        timeout: 15_000,
      },
    );
    expect(result.error).toBeUndefined();
    expect(result.status).toBe(0);
    const report = JSON.parse(result.stdout);
    expect(report.cargo.trim()).toBe('cargo 9.9.9');
    expect(Object.values(report.results).every((entry) => entry.passed)).toBe(true);
  });

  it('accepts the RuntimeManaged production surface', async () => {
    const root = await fixtureRoot();
    await writeFile(path.join(root, 'src-tauri/src/runtime.rs'), 'RuntimeApprovalBridge');
    expect(await scanReleaseSurface(root)).toEqual([]);
  });

  it('rejects retired symbols in source and built artifacts', async () => {
    const root = await fixtureRoot();
    await writeFile(path.join(root, 'src/legacy.ts'), 'const mode = "HelmProtected";');
    await writeFile(path.join(root, 'dist/assets/app.js'), 'windows-appcontainer');
    expect(await scanReleaseSurface(root)).toEqual(
      expect.arrayContaining([
        expect.objectContaining({ file: 'src/legacy.ts' }),
        expect.objectContaining({ file: 'dist/assets/app.js' }),
      ]),
    );
  });
});
