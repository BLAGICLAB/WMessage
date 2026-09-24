import { describe, it, expect } from "vitest";

// 悬停过渡对称性：hover 块给 transform 加了过渡，基类若不同步，
// 鼠标进入平滑上浮、离开瞬间弹回（视觉跳变）——静态断言锁住两侧一致
describe("main.css nm-card-hover 过渡对称", () => {
  it("基类与 hover 块的 transition 都含 transform 0.2s ease", async () => {
    const fs = await import("node:fs/promises");
    const path = await import("node:path");
    const url = await import("node:url");
    const cssPath = path.resolve(
      path.dirname(url.fileURLToPath(import.meta.url)),
      "main.css",
    );
    const css = await fs.readFile(cssPath, "utf-8");
    // 基类 .nm-card, .nm-card-hover 块（transform 由 hover 块引入，基类必须同样过渡）
    expect(css).toMatch(
      /transition:\s*background-color 0\.28s ease,\s*box-shadow 0\.2s ease,\s*transform 0\.2s ease;[\s\S]*?\.nm-card-hover:hover/,
    );
    // hover 块自身
    expect(css).toMatch(
      /\.nm-card-hover:hover\s*\{[\s\S]*?transition:[^}]*transform 0\.2s ease/,
    );
  });
});
