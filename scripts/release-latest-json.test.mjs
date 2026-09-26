import { mkdtempSync, writeFileSync, readFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import assert from 'node:assert/strict';
import { buildLatestJson } from './release-latest-json.mjs';

const SIG = 'dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZSBmcm9tIHRhdXJpIHNlY3JldCBrZXkK';
const URL_HTTPS =
  'https://github.com/dailyoozoo/helm-desktop/releases/download/v0.6.4/Helm_0.6.4_x64-setup.exe';

test('生成符合 Tauri v2 结构的 latest.json', () => {
  const json = buildLatestJson({
    version: '0.6.4',
    signature: SIG,
    url: URL_HTTPS,
    notes: '修复了若干问题',
    pubDate: '2026-09-26T03:00:00.000Z',
  });
  assert.equal(json.version, '0.6.4');
  assert.equal(json.pub_date, '2026-09-26T03:00:00.000Z');
  assert.equal(json.platforms['windows-x86_64'].url, URL_HTTPS);
  assert.equal(json.platforms['windows-x86_64'].signature, SIG);
  assert.equal(json.notes, '修复了若干问题');
});

test('拒绝非 https 下载地址与空签名（防供应链投毒）', () => {
  assert.throws(
    () => buildLatestJson({ version: '0.6.4', signature: SIG, url: 'http://example.com/a.exe' }),
    /https/,
  );
  assert.throws(
    () => buildLatestJson({ version: '0.6.4', signature: '   ', url: URL_HTTPS }),
    /签名内容为空/,
  );
  assert.throws(
    () => buildLatestJson({ version: 'not-a-version', signature: SIG, url: URL_HTTPS }),
    /版本号格式/,
  );
});

test('签名内容取自 .sig 文件原文', () => {
  const dir = mkdtempSync(join(tmpdir(), 'helm-latest-'));
  const sigPath = join(dir, 'app.exe.sig');
  writeFileSync(sigPath, `${SIG}\n`, 'utf8');
  const json = buildLatestJson({
    version: '0.6.4',
    signature: readFileSync(sigPath, 'utf8'),
    url: URL_HTTPS,
    pubDate: '2026-09-26T03:00:00.000Z',
  });
  assert.equal(json.platforms['windows-x86_64'].signature, SIG);
});
