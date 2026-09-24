import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/window", () => ({
  currentMonitor: vi.fn(async () => null),
}));

import { anchorFromRect, loadAnchor, saveAnchor } from "./storage";
import { POS_KEY } from "./constants";

describe("loadAnchor 形状校验", () => {
  beforeEach(() => {
    localStorage.clear();
  });

  it("无存储值 → null", () => {
    expect(loadAnchor()).toBeNull();
  });

  it("非法 JSON → null", () => {
    localStorage.setItem(POS_KEY, "{not json");
    expect(loadAnchor()).toBeNull();
  });

  it("缺 y → null", () => {
    localStorage.setItem(POS_KEY, JSON.stringify({ x: 100, edge: "right" }));
    expect(loadAnchor()).toBeNull();
  });

  it("x 为字符串 → null", () => {
    localStorage.setItem(
      POS_KEY,
      JSON.stringify({ x: "100", y: 50, edge: "right" })
    );
    expect(loadAnchor()).toBeNull();
  });

  it("坐标为 NaN/Infinity → null", () => {
    localStorage.setItem(POS_KEY, '{"x":1e999,"y":50,"edge":"right"}');
    expect(loadAnchor()).toBeNull();
  });

  it("未知 edge（bottom）→ null", () => {
    localStorage.setItem(
      POS_KEY,
      JSON.stringify({ x: 100, y: 50, edge: "bottom" })
    );
    expect(loadAnchor()).toBeNull();
  });

  it("非 object（数组/数字/字符串）→ null", () => {
    for (const bad of [[1, 2], 42, "anchor"]) {
      localStorage.setItem(POS_KEY, JSON.stringify(bad));
      expect(loadAnchor()).toBeNull();
    }
  });

  it("合法值 → 原样返回（saveAnchor 回读兼容）", () => {
    const a = { x: 123, y: 456, edge: "left" as const };
    saveAnchor(a);
    expect(loadAnchor()).toEqual(a);
  });
});

describe("anchorFromRect 返回扁平 Anchor", () => {
  it("返回顶层 x/y/edge，无 anchor 包装键", () => {
    const a = anchorFromRect(100, 200, 56, 1920);
    expect(a).toHaveProperty("x");
    expect(a).toHaveProperty("y");
    expect(a).toHaveProperty("edge");
    expect(a).not.toHaveProperty("anchor");
  });

  it("右缘吸附（24px 容差）", () => {
    const a = anchorFromRect(1900, 300, 56, 1920);
    expect(a.edge).toBe("right");
    expect(a.y).toBe(300);
  });

  it("左缘吸附", () => {
    expect(anchorFromRect(10, 300, 56, 1920).edge).toBe("left");
  });

  it("顶缘吸附", () => {
    expect(anchorFromRect(500, 5, 56, 1920).edge).toBe("top");
  });

  it("悬浮（不贴边）", () => {
    const a = anchorFromRect(500, 300, 56, 1920);
    expect(a.edge).toBe("float");
    expect(a.x).toBe(500);
    expect(a.y).toBe(300);
  });
});
