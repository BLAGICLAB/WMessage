import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";

// main.css 静态结构锁（vitest 配置 css:false 会把 .css 导入吞成空串，直接读文件）。
//
// 1) 滚动条主题适配：color-scheme（原生控件/滚动条随主题）+ webkit 细滚动条走
//    主题变量。原先全局没有滚动条样式，深色模式下滚动条仍是浅色原生样式——锁死防回退。
// 2) nm-card-hover 过渡对称：hover 块改了哪些属性，基类 transition 必须同样声明，
//    否则鼠标进入平滑、离开瞬间弹回。 换扁平材质后 hover 信号 = 背景抬升 +
//    边框提亮，不再有 transform/shadow 跳变；基类 transition 同时覆盖进出场。
const css = readFileSync("src/ui/main.css", "utf-8");

describe("滚动条主题适配（main.css）", () => {
  it("color-scheme 随深浅主题切换", () => {
    // 浅色根 + .dark 覆盖，二者缺一 WebView2 原生滚动条就不随主题
    expect(css).toMatch(/:root\s*\{[^}]*color-scheme:\s*light/s);
    expect(css).toMatch(/\.dark\s*\{[^}]*color-scheme:\s*dark/s);
  });

  it("webkit 细滚动条走主题变量（不硬编码颜色）", () => {
    expect(css).toContain("::-webkit-scrollbar-thumb");
    expect(css).toMatch(
      /::-webkit-scrollbar-thumb\s*\{[^}]*background:\s*var\(--t6\)/s,
    );
    // 轨道透明（不遮挡新拟态背景层次）
    expect(css).toMatch(
      /::-webkit-scrollbar-track\s*\{[^}]*background:\s*transparent/s,
    );
  });
});

describe("main.css nm-card-hover 过渡对称", () => {
  it("基类 transition 声明 hover 变化的属性（background/border），hover 块不引入 transform", () => {
    // 基类 .nm-card, .nm-card-hover 块：transition 覆盖 hover 全部变化属性
    expect(css).toMatch(
      /transition:\s*background-color 0\.15s ease,\s*border-color 0\.15s ease;[\s\S]*?\.nm-card-hover:hover/,
    );
    // hover 块只改背景与边框（扁平悬停），不得引入 transform/shadow 跳变
    const hoverBlock = css.match(/\.nm-card-hover:hover\s*\{([^}]*?)\}/);
    expect(hoverBlock).not.toBeNull();
    const body = hoverBlock?.[1] ?? "";
    expect(body).toContain("background-color");
    expect(body).toContain("border-color");
    expect(body).not.toContain("transform");
    expect(body).not.toContain("box-shadow");
    expect(body).not.toContain("transition");
  });
});
