import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";

/**
 * 2026-08-28：Windows（WebView2）滚动条颜色适配浅色/深色模式。
 * 原先全局没有任何滚动条样式，深色模式下滚动条仍是浅色原生样式。
 * 修复 = main.css 双机制：color-scheme（原生控件/滚动条随主题）+
 * webkit 细滚动条走主题变量。本测试锁死防回退。
 * （vitest 配置 css:false 会把 .css 导入吞成空串，?raw 也受影响，故直接读文件）
 */
const css = readFileSync("src/ui/main.css", "utf-8");
describe("滚动条主题适配（main.css）", () => {
  it("color-scheme 随深浅主题切换", () => {
    // 浅色根 + .dark 覆盖，二者缺一 WebView2 原生滚动条就不随主题
    expect(css).toMatch(/:root\s*\{[^}]*color-scheme:\s*light/s);
    expect(css).toMatch(/\.dark\s*\{[^}]*color-scheme:\s*dark/s);
  });

  it("webkit 细滚动条走主题变量（不硬编码颜色）", () => {
    expect(css).toContain("::-webkit-scrollbar-thumb");
    expect(css).toMatch(/::-webkit-scrollbar-thumb\s*\{[^}]*background:\s*var\(--t6\)/s);
    // 轨道透明（不遮挡新拟态背景层次）
    expect(css).toMatch(/::-webkit-scrollbar-track\s*\{[^}]*background:\s*transparent/s);
  });
});
