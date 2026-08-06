import { mkdir, writeFile } from 'node:fs/promises';
import { spawn } from 'node:child_process';
import path from 'node:path';

const root = path.resolve(import.meta.dirname, '..');
const evidenceDir = path.join(root, '.agent', 'evidence');

function option(name, fallback) {
  const index = process.argv.indexOf(name);
  return index >= 0 ? process.argv[index + 1] : fallback;
}

function redact(value) {
  return value
    .replace(
      /(["']?(?:api[_-]?key|token|secret|password)["']?\s*:\s*["'])[^"']*(["'])/gi,
      '$1[redacted]$2',
    )
    .replace(/(authorization\s*[:=]\s*(?:Bearer\s+)?|bearer\s+)[^\s,]+/gi, '$1[redacted]')
    .replace(/((?:api[_-]?key|token|secret|password)\s*[:=]\s*)[^\s,]+/gi, '$1[redacted]')
    .replace(/(https?:\/\/[^\s?]+)\?[^\s]+/gi, '$1?[query-redacted]');
}

const separator = process.argv.indexOf('--');
const command = separator >= 0 ? process.argv[separator + 1] : undefined;
const args = separator >= 0 ? process.argv.slice(separator + 2) : [];
if (!command) {
  console.error(
    'Usage: node scripts/agent-evidence.mjs --name <label> [--timeout-ms <n>] -- <command> [args...]',
  );
  process.exit(2);
}

const label = (option('--name', 'command') ?? 'command').replace(/[^a-zA-Z0-9._-]+/g, '-');
const timeoutMs = Math.max(1000, Number(option('--timeout-ms', '120000')) || 120000);
const maxLines = Math.max(1, Number(option('--max-lines', '40')) || 40);
const maxChars = Math.max(200, Number(option('--max-chars', '6000')) || 6000);
const stamp = new Date()
  .toISOString()
  .replace(/[-:]/g, '')
  .replace(/\.\d{3}Z$/, 'Z');
const logPath = path.join(evidenceDir, `${stamp}-${label}.log`);

await mkdir(evidenceDir, { recursive: true });
const started = Date.now();
let output = '';
const executable = process.platform === 'win32' && command === 'npm' ? 'npm.cmd' : command;
const child = spawn(executable, args, {
  cwd: root,
  env: process.env,
  shell: process.platform === 'win32' && executable.endsWith('.cmd'),
  windowsHide: true,
});
child.stdout.on('data', (chunk) => {
  output += chunk.toString();
});
child.stderr.on('data', (chunk) => {
  output += chunk.toString();
});

let timedOut = false;
const timeout = setTimeout(() => {
  timedOut = true;
  child.kill();
}, timeoutMs);
const exitCode = await new Promise((resolve) => {
  child.once('close', resolve);
  child.once('error', () => resolve(null));
});
clearTimeout(timeout);

const safeOutput = redact(output);
await writeFile(logPath, safeOutput, 'utf8');
const lines = safeOutput.split(/\r?\n/).filter(Boolean);
const result = {
  label,
  command,
  exitCode: timedOut ? 124 : exitCode,
  ok: !timedOut && exitCode === 0,
  timedOut,
  durationMs: Date.now() - started,
  outputLines: lines.length,
  logPath: path.relative(root, logPath),
  excerpt: lines.slice(-maxLines).join('\n').slice(-maxChars),
};
console.log(JSON.stringify(result, null, 2));
process.exit(result.ok ? 0 : result.exitCode || 1);
