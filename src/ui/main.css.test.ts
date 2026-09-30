import { describe, it, expect } from "vitest";

// 悬停过渡对称性：hover 块改了哪些属性，基类 transition 必须同样声明，
// 否则鼠标进入平滑、离开瞬间弹回（视觉跳变）——静态断言锁住两侧一致。
// U1 换扁平材质后 hover 信号 = 背景抬升 + 边框提亮，不再有 transform/shadow 跳变；
// 基类 transition 同时覆盖进出场，hover 块不再重复声明。
describe("main.css nm-card-hover 过渡对称", () => {
  it("基类 transition 声明 hover 变化的属性（background/border），hover 块不引入 transform", async () => {
    const fs = await import("node:fs/promises");
    const path = await import("node:path");
    const url = await import("node:url");
    const cssPath = path.resolve(
      path.dirname(url.fileURLToPath(import.meta.url)),
      "main.css",
    );
    const css = await fs.readFile(cssPath, "utf-8");
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
