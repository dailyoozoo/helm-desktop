import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import test from 'node:test';
import path from 'node:path';

const script = path.join(import.meta.dirname, 'agent-evidence.mjs');

test('agent evidence redacts secrets and returns bounded summary', () => {
  const output = execFileSync(
    process.execPath,
    [
      script,
      '--name',
      'test-redact',
      '--max-lines',
      '2',
      '--',
      process.execPath,
      '-e',
      'console.log(\'token=secret-value\'); console.log(\'{"token":"json-secret"}\')',
    ],
    { cwd: path.resolve(import.meta.dirname, '..'), encoding: 'utf8' },
  );
  const result = JSON.parse(output);
  assert.equal(result.ok, true);
  assert.match(result.excerpt, /token=\[redacted\]/);
  assert.doesNotMatch(result.excerpt, /secret-value/);
  assert.doesNotMatch(result.excerpt, /json-secret/);
  assert.match(result.logPath, /^\.agent[\\/]evidence[\\/]/);
});

test('agent evidence stops a hung command with exit code 124', () => {
  const result = spawnSync(
    process.execPath,
    [
      script,
      '--name',
      'test-timeout',
      '--timeout-ms',
      '1000',
      '--',
      process.execPath,
      '-e',
      'setTimeout(() => {}, 5000)',
    ],
    { cwd: path.resolve(import.meta.dirname, '..'), encoding: 'utf8', timeout: 10000 },
  );
  assert.equal(result.status, 124);
  assert.equal(JSON.parse(result.stdout).timedOut, true);
});
