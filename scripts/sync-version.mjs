// P2-29：版本号单一真相源 = src-tauri/Cargo.toml。
// tauri.conf.json 不写 version（tauri 2 构建期回退 CARGO_PKG_VERSION，
// 见 tauri-codegen context.rs）；本脚本把 Cargo.toml version 同步进
// package.json（pnpm build 前经 prebuild 自动执行，也可手动 pnpm run sync-version）。
import { readFileSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const cargo = readFileSync(join(root, 'src-tauri/Cargo.toml'), 'utf8');
const m = cargo.match(/^\[package\][^[]*?^version\s*=\s*"([^"]+)"/ms);
if (!m) {
  console.error('[sync-version] 未在 src-tauri/Cargo.toml 找到 [package] version');
  process.exit(1);
}
const version = m[1];
const pkgPath = join(root, 'package.json');
const pkg = JSON.parse(readFileSync(pkgPath, 'utf8'));
if (pkg.version !== version) {
  pkg.version = version;
  writeFileSync(pkgPath, JSON.stringify(pkg, null, 2) + '\n');
  console.log(`[sync-version] package.json version → ${version}`);
} else {
  console.log(`[sync-version] 已是最新（${version}）`);
}
