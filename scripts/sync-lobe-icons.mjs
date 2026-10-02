// 把 @lobehub/icons-static-svg 的图标资产拷进 src/assets/providers/lobe/（vendor 模式）。
// 口径：全部 -color.svg（彩色优先）+ 无 color 变体的厂商补一份单色 .svg；*-text.svg 字标不拷。
// 升级 lobehub 包版本后重跑一次即可：node scripts/sync-lobe-icons.mjs

import { cpSync, mkdirSync, readdirSync, rmSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const src = join(root, "node_modules/@lobehub/icons-static-svg/icons");
const dest = join(root, "src/assets/providers/lobe");

// 口径：全部 -color.svg（彩色优先）+ 无 color 变体的厂商补一份单色 .svg；
// 跳过 -text/-text-cn 字标与 -brand 品牌变体。
const all = readdirSync(src).filter((f) => f.endsWith(".svg"));
const colors = all.filter(
  (f) => f.endsWith("-color.svg") && !f.endsWith("-brand-color.svg")
);
const colorSlugs = new Set(colors.map((f) => f.slice(0, -"-color.svg".length)));
// 无 color 变体时的单色兜底
const monos = all.filter(
  (f) =>
    !/-(color|brand)\.svg$/.test(f) &&
    !/-text(-cn)?\.svg$/.test(f) &&
    !colorSlugs.has(f.slice(0, -".svg".length))
);

rmSync(dest, { recursive: true, force: true });
mkdirSync(dest, { recursive: true });
for (const f of [...colors, ...monos]) {
  cpSync(join(src, f), join(dest, f));
}
console.log(
  `sync-lobe-icons: ${colors.length} color + ${monos.length} mono = ${colors.length + monos.length} → ${dest}`
);
