// P2-29：版本号单一真相源 = src-tauri/Cargo.toml。
// tauri.conf.json 不写 version（tauri 2 构建期回退 CARGO_PKG_VERSION，
// 见 tauri-codegen context.rs）；本脚本把 Cargo.toml version 同步进
// package.json（pnpm build 前经 prebuild 自动执行，也可手动 pnpm run sync-version）。
import { readFileSync, writeFileSync, renameSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const cargo = readFileSync(join(root, 'src-tauri/Cargo.toml'), 'utf8');
// 只在 [package] 节内找 version（切到下一个节头为止）：
// 跨节正则会被节内含 [ 的注释截断而漏报
const pkgStart = cargo.indexOf('[package]');
const pkgEnd = pkgStart >= 0 ? cargo.indexOf('\n[', pkgStart) : -1;
const pkgSection =
  pkgStart >= 0 ? cargo.slice(pkgStart, pkgEnd < 0 ? cargo.length : pkgEnd) : '';
const m = pkgSection.match(/^version\s*=\s*"([^"]+)"/m);
if (!m) {
  console.error('[sync-version] 未在 src-tauri/Cargo.toml 找到 [package] version');
  process.exit(1);
}
const version = m[1];
// 形状校验：手滑写错的版本（缺位 / 带 v 前缀）不外溢进 package.json
if (!/^\d+\.\d+\.\d+([-+].*)?$/.test(version)) {
  console.error(`[sync-version] Cargo.toml version "${version}" 不是合法的 semver，请先修正`);
  process.exit(1);
}
const pkgPath = join(root, 'package.json');
const pkg = JSON.parse(readFileSync(pkgPath, 'utf8'));
if (pkg.version !== version) {
  pkg.version = version;
  // 写临时文件后 rename 原子替换：中断/盘满不会留下截断的 package.json
  const tmpPath = pkgPath + '.tmp';
  writeFileSync(tmpPath, JSON.stringify(pkg, null, 2) + '\n');
  renameSync(tmpPath, pkgPath);
  console.log(`[sync-version] package.json version → ${version}`);
} else {
  console.log(`[sync-version] 已是最新（${version}）`);
}
