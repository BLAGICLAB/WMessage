// 外观刷新的脏标记：收敛之后 pushGeometry 不再被调用，外观全靠 refreshAppearance
// 里的 needsUpdate 上传。它曾经只标了边、漏了星体——切着色维度时连线变色、
// 星体不动，mode 档位看着像坏了。收进 markAppearanceDirty 后，再漏这条单测就红。
import { describe, expect, it } from "vitest";
import * as THREE from "three";
import { markAppearanceDirty } from "./GraphCanvas";

function makeGeos() {
  const nodeGeo = new THREE.BufferGeometry();
  nodeGeo.setAttribute("position", new THREE.BufferAttribute(new Float32Array(9), 3));
  nodeGeo.setAttribute("aColor", new THREE.BufferAttribute(new Float32Array(9), 3));
  nodeGeo.setAttribute("aSize", new THREE.BufferAttribute(new Float32Array(3), 1));
  const edgeGeo = new THREE.BufferGeometry();
  edgeGeo.setAttribute("aColor", new THREE.BufferAttribute(new Float32Array(12), 3));
  edgeGeo.setAttribute("aAlpha", new THREE.BufferAttribute(new Float32Array(4), 1));
  return { nodeGeo, edgeGeo };
}

/** attributes 上的类型是 Buffer | InterleavedBuffer 联合体，这里只会用前者 */
const attr = (geo: THREE.BufferGeometry, name: string) =>
  geo.getAttribute(name) as THREE.BufferAttribute;

describe("外观刷新的脏标记", () => {
  it("星体与星轨的颜色/尺寸属性全部被标脏", () => {
    const { nodeGeo, edgeGeo } = makeGeos();
    markAppearanceDirty(nodeGeo, edgeGeo);
    expect(attr(nodeGeo, "aColor").version).toBeGreaterThan(0);
    expect(attr(nodeGeo, "aSize").version).toBeGreaterThan(0);
    expect(attr(edgeGeo, "aColor").version).toBeGreaterThan(0);
    expect(attr(edgeGeo, "aAlpha").version).toBeGreaterThan(0);
  });

  it("位置属性不归它管（那是 pushGeometry 的活，别顺手标了）", () => {
    const { nodeGeo, edgeGeo } = makeGeos();
    markAppearanceDirty(nodeGeo, edgeGeo);
    expect(attr(nodeGeo, "position").version).toBe(0);
  });
});
