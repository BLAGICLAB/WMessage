import { describe, expect, it } from "vitest";
import { stackSize } from "./PlanBlock";

// 塌缩簇尺寸契约（ACC-K-05 判据 4）：同列 3 卡以上重叠 → 整簇塌缩，
// 块尺寸随块高取 16 / 12 / 10 三档之一。此前该函数是模块内部函数、
// 没有任何用例守着——档位回退只会静默改变观感。
describe("stackSize 塌缩块三档", () => {
  it("长块（≥90 分钟）取 16", () => {
    expect(stackSize(90).size).toBe(16);
    expect(stackSize(180).size).toBe(16);
  });

  it("中块（40–89 分钟）取 12", () => {
    expect(stackSize(40).size).toBe(12);
    expect(stackSize(89).size).toBe(12);
  });

  it("短块（<40 分钟）取 10", () => {
    expect(stackSize(39).size).toBe(10);
    expect(stackSize(0).size).toBe(10);
  });

  it("每档各带一个尺寸类名（渲染靠它，不是靠 size 数字）", () => {
    expect(stackSize(120).cls).toContain("s-lg");
    expect(stackSize(60).cls).toContain("s-md");
    expect(stackSize(20).cls).toContain("s-sm");
  });

  it("三档互不相同（档位塌成同一档就是缺陷）", () => {
    const sizes = new Set([stackSize(120).size, stackSize(60).size, stackSize(20).size]);
    expect(sizes.size).toBe(3);
  });
});
