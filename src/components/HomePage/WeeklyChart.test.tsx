import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";

// 周图 tooltip 的动画参数没配，recharts 3.10.1 的默认值直接生效：
// animationDuration 400 + animationEasing 'ease'，拼成 transition: transform 400ms ease。
// 400ms 是 tooltip 预算（125–200ms）的 2–4 倍，而且它过渡的是 transform——
// tooltip 的位置跟着指针走，400ms 慢慢滑过去，视觉上拖在指针后面。
const src = readFileSync("src/components/HomePage/WeeklyChart.tsx", "utf-8");
const css = readFileSync("src/ui/main.css", "utf-8");
// 闭合的 /> 要锚在行首：content 里的 <WeekTip {...p} /> 也带 />，不锚会提前截断
const tooltip = src.match(/<Tooltip[\s\S]*?^\s*\/>/m)?.[0] ?? "";

describe("周图 tooltip 动画参数", () => {
  it("动画时长在 tooltip 预算内（≤200ms）", () => {
    const dur = Number(tooltip.match(/animationDuration=\{(\d+)\}/)?.[1] ?? 0);
    expect(dur).toBeGreaterThan(0);
    expect(dur).toBeLessThanOrEqual(200);
  });

  it("用强 ease-out 曲线（起步快、落点准，提示框才跟得住指针）", () => {
    expect(tooltip).toMatch(/animationEasing="cubic-bezier\(0\.23,\s*1,\s*0\.32,\s*1\)"/);
  });

  it("曲线与 --ease-out token 同源（类型不收 CSS 变量，只能写死，锁住不漂移）", () => {
    const token = css
      .match(/--ease-out:\s*cubic-bezier\(([^)]+)\)/)?.[1]
      ?.replace(/\s/g, "");
    const literal = tooltip
      .match(/animationEasing="cubic-bezier\(([^)]+)\)"/)?.[1]
      ?.replace(/\s/g, "");
    expect(token).toBeTruthy();
    expect(literal).toBe(token);
  });

  it("两根柱子仍然关掉入场动画（口径一致，tooltip 不是唯一的例外）", () => {
    expect(src.match(/isAnimationActive=\{false\}/g)?.length).toBe(2);
  });
});
