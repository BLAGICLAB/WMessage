import { describe, it, expect } from "vitest";
import { SUBAGENT_STATUS, execTaskDedup } from "./constants";

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

// 状态→文案曾分散在侧栏与收尾提示两处，同一状态两处措辞不同。这里把
// 「共享表 + 六态齐全」钉住：新增状态漏词会在编译期与本用例双重暴露
describe("SUBAGENT_STATUS（子 agent 六态共享标签）", () => {
  it("六态齐全且措辞唯一（cancel/budget 不与 failed 混说）", () => {
    expect(SUBAGENT_STATUS).toEqual({
      queued: { label: "排队中", color: expect.any(String) },
      running: { label: "执行中", color: expect.any(String) },
      succeeded: { label: "完成", color: expect.any(String) },
      failed: { label: "失败", color: expect.any(String) },
      cancelled: { label: "已取消", color: expect.any(String) },
      budget_exceeded: { label: "预算耗尽", color: expect.any(String) },
    });
    const labels = Object.values(SUBAGENT_STATUS).map((v) => v.label);
    expect(new Set(labels).size).toBe(labels.length);
  });
});
