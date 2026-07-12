import fs from 'node:fs';
import path from 'node:path';
import process from 'node:process';

const root = process.cwd();
const packageJson = JSON.parse(fs.readFileSync(path.join(root, 'package.json'), 'utf8'));
const tauriConfig = JSON.parse(
  fs.readFileSync(path.join(root, 'src-tauri', 'tauri.conf.json'), 'utf8'),
);
const cargoToml = fs.readFileSync(path.join(root, 'src-tauri', 'Cargo.toml'), 'utf8');
const cargoVersion = cargoToml.match(/^version\s*=\s*"([^"]+)"/m)?.[1];

const versions = {
  'package.json': packageJson.version,
  'src-tauri/tauri.conf.json': tauriConfig.version,
  'src-tauri/Cargo.toml': cargoVersion,
};
const unique = new Set(Object.values(versions));
if (unique.size !== 1 || unique.has(undefined)) {
  process.stderr.write('版本号不一致：\n');
  for (const [file, version] of Object.entries(versions)) {
    process.stderr.write(`- ${file}: ${version ?? '缺失'}\n`);
  }
  process.exit(1);
}

process.stdout.write(`版本号一致：${packageJson.version}\n`);
