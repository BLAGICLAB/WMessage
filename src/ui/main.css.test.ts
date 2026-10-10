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

// 修过的主题按钮按压态：特异性必须与 hover 同级或更高，hover 规则不能盖过按压底色
// 主题液面罐渐变：必须跟随 --success token（浅/深色双变体），不得硬编码颜色
// 主题斜纹流光：默认不播，仅在 .has-liquid 出现时启动
describe("main.css 交互态特异性 + 主题色绑定", () => {
  it("save-btn.is-pressed 特异性 ≥ save-btn:hover（按压期间 hover 规则不得盖过）", () => {
    // 选择器权重：.save-btn.is-pressed(0,2,0) < .save-btn:hover:not(:disabled)(0,3,0)
    // 修法：写为 .save-btn.save-btn.is-pressed(0,3,0) 提升到 (0,3,0) 与 hover 同级，
    // 文件后置顺序盖过 hover 块的 background-color/color；锁定这一写法防回退
    const block = css.match(/\.save-btn\.save-btn\.is-pressed\s*\{([^}]*)\}/);
    expect(block, ".save-btn.save-btn.is-pressed 规则未按提升特异性写法落地").not.toBeNull();
    const body = block?.[1] ?? "";
    expect(body).toContain("background-color");
    expect(body).toContain("box-shadow");
    expect(body).toContain("transform");
  });

  it("hold-confirm.is-active 特异性 ≥ hold-confirm:hover（长按期间 hover 不得盖过激活边色）", () => {
    // 选择器 .hold-confirm.hold-confirm(0,2,1) 与 .hold-confirm:hover:not(:disabled)(0,2,2)
    // 不平级——CSS 规则文件位置晚即可胜出；锁定双重 .hold-confirm 写法防回退
    const block = css.match(
      /\.hold-confirm\.is-active\.hold-confirm\s*\{([^}]*)\}/,
    );
    expect(
      block,
      ".hold-confirm.is-active.hold-confirm 规则未按提升特异性写法落地",
    ).not.toBeNull();
    const body = block?.[1] ?? "";
    expect(body).toContain("border-color");
  });

  it("exec-bar__liquid 渐变跟随 --success token（不硬编码 #16a34a/#4ade80 等具体色）", () => {
    const block = css.match(/\.exec-bar__liquid\s*\{([^}]*)\}/);
    expect(block).not.toBeNull();
    const body = block?.[1] ?? "";
    // 必须用 var(--success) + color-mix，不允许硬编码 #rrggbb
    expect(body).toContain("var(--success)");
    expect(body).toMatch(/color-mix\(in srgb, var\(--success\)/);
    expect(body).not.toMatch(/#[0-9a-fA-F]{3,6}/);
  });

  it("exec-bar__flow 仅在 .has-liquid 出现时启动动画（无液面时不播）", () => {
    // 基类 .exec-bar__flow 不得带 animation（默认 0 GPU 占用）
    const base = css.match(/\.exec-bar__flow\s*\{([^}]*)\}/);
    expect(base).not.toBeNull();
    expect(
      base?.[1] ?? "",
      "基类 .exec-bar__flow 不得默认带 animation——空态不播",
    ).not.toMatch(/animation:/);
    // 条件类 .exec-bar.has-liquid .exec-bar__flow 带 animation: exec-flow
    const cond = css.match(
      /\.exec-bar\.has-liquid\s+\.exec-bar__flow\s*\{([^}]*)\}/,
    );
    expect(
      cond,
      ".exec-bar.has-liquid .exec-bar__flow 条件类缺失",
    ).not.toBeNull();
    expect(cond?.[1] ?? "").toMatch(/animation:\s*exec-flow/);
  });
});
