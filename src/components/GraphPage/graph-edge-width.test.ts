// 星轨宽度契约：三档设置承诺的像素宽必须真的量在屏幕上。
// 这条曾经整档错位——着色器里少了 NDC↔像素的 2 倍系数，1.0/1.6/2.8 全部
// 只落地一半，再被纵深系数压到亚像素，「细」档等于没有线。
//
// 只断言档位表是不够的：那份表与着色器是两份真值，表再对也不代表画出来对。
// 所以这里同时断言三处——档位表进 uniform 的接线、着色器里插值进去的换算系数、
// 以及纵深系数的 clamp 区间——少任何一处，都会重演「单测全绿、画面全错」。
import { describe, expect, it } from "vitest";
import {
  EDGE_DEPTH_CLAMP_MAX,
  EDGE_DEPTH_CLAMP_MIN,
  EDGE_DEPTH_REF,
  EDGE_NDC_SCALE,
  EDGE_WIDTH,
  createEdgeMaterial,
} from "./GraphCanvas";
import type { GraphEdgeWidth } from "../../lib/graphPrefs";

const PREFS: GraphEdgeWidth[] = ["thin", "standard", "thick"];

/** 与 EDGE_VERT 里那行 clamp 同一个算法：名义像素宽 × 纵深系数 */
function ribbonWidthPx(nominal: number, depth: number): number {
  const k = Math.min(
    Math.max(EDGE_DEPTH_REF / Math.max(depth, 1), EDGE_DEPTH_CLAMP_MIN),
    EDGE_DEPTH_CLAMP_MAX,
  );
  return nominal * k;
}

describe("星轨宽度（设置页「连线粗细」）", () => {
  it("档位值经材质进 uWidth，三档一一对应（不是写死的某档）", () => {
    for (const pref of PREFS) {
      expect(createEdgeMaterial(pref).uniforms.uWidth.value).toBe(EDGE_WIDTH[pref]);
    }
  });

  it("着色器里的像素换算系数就是 EDGE_NDC_SCALE，没有第二份裸字面量", () => {
    for (const pref of PREFS) {
      const vs = createEdgeMaterial(pref).vertexShader;
      expect(vs).toContain(`* ${EDGE_NDC_SCALE.toFixed(1)} / uRes`);
      // 纵深系数同样插值进去：clamp 的区间改了、着色器里的还是旧值就是第二份真值
      expect(vs).toContain(
        `clamp(${EDGE_DEPTH_REF.toFixed(1)} / max(depth, 1.0), ` +
          `${EDGE_DEPTH_CLAMP_MIN}, ${EDGE_DEPTH_CLAMP_MAX})`,
      );
    }
  });

  it("窄带宽度只由 uWidth 一个来源决定（声明 + 一处使用，无额外基准乘数）", () => {
    for (const pref of PREFS) {
      // 注释里也会提到 uWidth，计数前先剥掉
      const code = createEdgeMaterial(pref).vertexShader.replace(/\/\/[^\n]*/g, "");
      expect(code).toContain("uniform float uWidth;");
      // 声明 1 处 + 使用 1 处；曾有过 EDGE_BASE_WIDTH 相乘，名义值被悄悄缩掉
      expect(code.split("uWidth").length - 1).toBe(2);
      expect(code).toContain("aCorner.x * uWidth * 0.5");
    }
  });

  it("参考纵深上量出来的就是名义像素宽", () => {
    expect(ribbonWidthPx(EDGE_WIDTH.thin, EDGE_DEPTH_REF)).toBeCloseTo(1.0, 5);
    expect(ribbonWidthPx(EDGE_WIDTH.standard, EDGE_DEPTH_REF)).toBeCloseTo(1.6, 5);
    expect(ribbonWidthPx(EDGE_WIDTH.thick, EDGE_DEPTH_REF)).toBeCloseTo(2.8, 5);
  });

  it("近景更粗、远景更细，且远到收细下限也不细过 0.8px", () => {
    // 机位距离 2.882R，近侧节点约 0.66 参考纵深、远侧约 1.35
    const near = ribbonWidthPx(EDGE_WIDTH.standard, EDGE_DEPTH_REF * 0.66);
    const far = ribbonWidthPx(EDGE_WIDTH.standard, EDGE_DEPTH_REF * 1.35);
    const floor = ribbonWidthPx(EDGE_WIDTH.standard, EDGE_DEPTH_REF * 100);
    expect(near).toBeGreaterThan(EDGE_WIDTH.standard);
    expect(far).toBeLessThan(EDGE_WIDTH.standard);
    expect(far).toBeCloseTo(EDGE_WIDTH.standard / 1.35, 5);
    expect(floor).toBeGreaterThanOrEqual(0.8);
  });

  it("三档严格递增，且没有额外的基准乘数（数值即像素）", () => {
    expect(EDGE_WIDTH.thin).toBeLessThan(EDGE_WIDTH.standard);
    expect(EDGE_WIDTH.standard).toBeLessThan(EDGE_WIDTH.thick);
    for (const w of Object.values(EDGE_WIDTH)) {
      expect(w).toBeGreaterThanOrEqual(1);
    }
  });
});
