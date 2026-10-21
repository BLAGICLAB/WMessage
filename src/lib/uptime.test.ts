import { describe, it, expect } from "vitest";
import { formatUptime } from "./uptime";

describe("formatUptime", () => {
  it("不足一分钟按秒", () => {
    expect(formatUptime(0)).toBe("0 秒");
    expect(formatUptime(59)).toBe("59 秒");
  });

  it("不足一小时按分 + 秒", () => {
    expect(formatUptime(60)).toBe("1 分 0 秒");
    expect(formatUptime(125)).toBe("2 分 5 秒");
  });

  it("超过一小时按小时 + 分", () => {
    expect(formatUptime(3600)).toBe("1 小时 0 分");
    expect(formatUptime(3725)).toBe("1 小时 2 分");
  });

  it("负数与小数按 0 起算（不显示负时长）", () => {
    expect(formatUptime(-5)).toBe("0 秒");
    expect(formatUptime(12.9)).toBe("12 秒");
  });
});
