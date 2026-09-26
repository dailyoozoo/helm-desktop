#!/usr/bin/env node
/**
 * 生成 Tauri v2 自动更新所需的 latest.json，并（可选）上传到 GitHub Release。
 *
 * 作用：让 Helm 的「检查更新」能从「只提示去网页下载」升级为「应用内一键下载安装」。
 * 安装时 Tauri 会用 tauri.conf.json 里的 minisign 公钥校验 signature，
 * 因此 .sig 必须是用与该公钥配对的私钥签出来的。
 *
 * 用法：
 *   node scripts/release-latest-json.mjs \
 *     --version 0.6.4 \
 *     --sig path/to/Helm_0.6.4_x64-setup.exe.sig \
 *     --url https://github.com/dailyoozoo/helm-desktop/releases/download/v0.6.4/Helm_0.6.4_x64-setup.exe \
 *     [--notes "更新说明" | --notes notes.md] [--out latest.json] [--upload] [--repo owner/repo]
 */
import { readFileSync, writeFileSync, existsSync, realpathSync } from 'node:fs';
import { resolve } from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const PLATFORM = 'windows-x86_64';

function parseArgs(argv) {
  const args = {};
  for (let index = 0; index < argv.length; index += 1) {
    const token = argv[index];
    if (!token.startsWith('--')) continue;
    const key = token.slice(2);
    const next = argv[index + 1];
    if (!next || next.startsWith('--')) {
      args[key] = true;
      continue;
    }
    args[key] = next;
    index += 1;
  }
  return args;
}

function versionFromPackageJson(root) {
  const pkg = JSON.parse(readFileSync(resolve(root, 'package.json'), 'utf8'));
  return pkg.version;
}

function fail(message) {
  console.error(`[latest.json] ${message}`);
  process.exit(1);
}

export function buildLatestJson(input) {
  const { version, signature, url, notes, pubDate, platform } = input;
  if (!version) throw new Error('缺少 --version');
  if (!/^\d+\.\d+\.\d+/.test(version)) throw new Error(`版本号格式不对：${version}`);
  if (!signature || !signature.trim()) throw new Error('签名内容为空（--sig 指向的文件无效）');
  if (!url) throw new Error('缺少安装包下载地址 --url');
  if (!/^https:\/\//.test(url)) throw new Error(`下载地址必须是 https：${url}`);
  return {
    version,
    notes: notes ?? '',
    pub_date: pubDate,
    platforms: {
      [platform ?? PLATFORM]: {
        signature: signature.trim(),
        url,
      },
    },
  };
}

export function run(argv = process.argv.slice(2), cwd = process.cwd()) {
  const args = parseArgs(argv);
  const version = args.version || versionFromPackageJson(cwd);
  if (!args.sig) fail('需要 --sig 指向 .sig 签名文件（tauri 构建产物或 tauri signer 生成）');
  if (!args.url) fail('需要 --url 指向安装包下载地址（Release 的 browser_download_url）');
  const sigPath = resolve(cwd, args.sig);
  if (!existsSync(sigPath)) fail(`签名文件不存在：${sigPath}`);

  let notes = '';
  if (typeof args.notes === 'string') {
    const notesPath = resolve(cwd, args.notes);
    notes = existsSync(notesPath) ? readFileSync(notesPath, 'utf8') : args.notes;
  }

  const json = buildLatestJson({
    version,
    signature: readFileSync(sigPath, 'utf8'),
    url: args.url,
    notes,
    pubDate: new Date().toISOString(),
    platform: typeof args.platform === 'string' ? args.platform : PLATFORM,
  });

  const outPath = resolve(cwd, typeof args.out === 'string' ? args.out : 'latest.json');
  writeFileSync(outPath, `${JSON.stringify(json, null, 2)}\n`, 'utf8');
  console.log(`[latest.json] 已生成 ${outPath}`);
  console.log(
    `[latest.json] version=${json.version} platform=${json.platforms && Object.keys(json.platforms)[0]}`,
  );

  if (args.upload) {
    const repo = typeof args.repo === 'string' ? args.repo : 'dailyoozoo/helm-desktop';
    const tag = typeof args.tag === 'string' ? args.tag : `v${version}`;
    const result = spawnSync(
      'gh',
      ['release', 'upload', tag, outPath, '--clobber', '--repo', repo],
      { encoding: 'utf8' },
    );
    if (result.status !== 0) {
      fail(`上传失败：${`${result.stdout || ''}${result.stderr || ''}`.trim()}`);
    }
    console.log(`[latest.json] 已上传到 ${repo} ${tag}`);
  }
  return json;
}

// 直接执行本文件时才跑（Windows 下不能用字符串拼 file:// 比较，需转真实路径）
const invoked = process.argv[1] ? realpathSync(process.argv[1]) : '';
if (invoked && fileURLToPath(import.meta.url) === invoked) {
  run();
}
