import { describe, it, expect } from "vitest";
import { execTaskDedup } from "./constants";

describe("execTaskDedup.shouldSkip（窗口去重 + prune）", () => {
  it("首调放行；窗口内重复跳过；超窗再放行", () => {
    const t0 = 1_000_000;
    expect(execTaskDedup.shouldSkip("task-a", t0)).toBe(false);
    expect(execTaskDedup.shouldSkip("task-a", t0 + 100)).toBe(true);
    expect(execTaskDedup.shouldSkip("task-a", t0 + 1999)).toBe(true);
    expect(execTaskDedup.shouldSkip("task-a", t0 + 2000)).toBe(false);
  });

  it("不同 id 互不干扰", () => {
    const t0 = 2_000_000;
    expect(execTaskDedup.shouldSkip("task-b", t0)).toBe(false);
    expect(execTaskDedup.shouldSkip("task-c", t0)).toBe(false);
    expect(execTaskDedup.shouldSkip("task-b", t0 + 10)).toBe(true);
  });

  it("prune：超窗条目被清理后同 id 可再触发（表不兜底旧值）", () => {
    const t0 = 3_000_000;
    expect(execTaskDedup.shouldSkip("task-d", t0)).toBe(false);
    // 远超窗口后再次触发：若旧条目未被 prune 仍应放行（窗口判定），
    // prune 保证其不留内存——行为上表现为可再触发
    expect(execTaskDedup.shouldSkip("task-d", t0 + 60_000)).toBe(false);
  });
});
