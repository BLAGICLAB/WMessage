// 画布指针手势单测：旋转视角不能被当成点选。
// 判据是纯函数（位移阈值），单测直接钉住这条阈值。

import { describe, expect, it } from "vitest";
import { CLICK_SLOP_PX, isClickGesture } from "./GraphCanvas";

describe("点选手势判定", () => {
  it("按下与松开几乎没动 = 点击", () => {
    expect(isClickGesture({ x: 100, y: 100 }, { x: 100, y: 100 })).toBe(true);
    expect(isClickGesture({ x: 100, y: 100 }, { x: 102, y: 101 })).toBe(true);
  });

  it("拖过了阈值 = 拖拽（旋转视角），不算点选", () => {
    expect(isClickGesture({ x: 100, y: 100 }, { x: 100 + CLICK_SLOP_PX + 1, y: 100 })).toBe(false);
    expect(isClickGesture({ x: 100, y: 100 }, { x: 100, y: 100 - CLICK_SLOP_PX - 1 })).toBe(false);
    // 斜着拖也一样：判定看的是位移长度，不是分轴
    expect(isClickGesture({ x: 0, y: 0 }, { x: 6, y: 6 })).toBe(false);
  });
});
