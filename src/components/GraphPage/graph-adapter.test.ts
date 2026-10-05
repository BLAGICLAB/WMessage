// graph-adapter 单测（G3-SIGMA 迁移 + r1 着色统一）：建图 → graphology 图的
// 节点属性、边数守恒、owner 着色双轨键、重复边防御、FA2 设置推导。
import { describe, expect, it } from "vitest";
import type { Task, Workflow } from "../../types";
import { buildTaskGraph, DEFAULT_FILTERS } from "./graph-build";
import {
  computeCameraFit,
  fa2Settings,
  graphBBox,
  ownerColorKey,
  resolveOwnerColor,
  toGraphologyGraph,
} from "./graph-adapter";

const WFS: Workflow[] = [
  { id: "wf1", name: "周报流水线", goal: "出周报" },
];

function t(partial: Partial<Task> & { id: string }): Task {
  return { title: `任务-${partial.id}`, column: "todo", ...partial };
}

const SAMPLE: Task[] = [
  t({ id: "a", column: "doing" }),
  t({ id: "b", column: "done", completedAt: Date.now(), tags: ["周报"] }),
  t({ id: "w1", column: "doing", origin: "workflow", workflowId: "wf1", dependsOn: ["w2"], tags: ["周报"] }),
  t({ id: "w2", column: "done", origin: "workflow", workflowId: "wf1", tags: ["周报"] }),
  t({ id: "z", ownerId: "p-1" }),
  t({ id: "l", ownerId: "p-2" }),
];

// owner 注入序（GraphPage 单一事实源的等价物）：张三 0、李四 1
const ORDER = new Map([
  ["p-1", 0],
  ["p-2", 1],
]);

describe("toGraphologyGraph", () => {
  it("节点/边数与建图结果守恒，属性齐备", () => {
    const built = buildTaskGraph(SAMPLE, WFS, DEFAULT_FILTERS);
    const g = toGraphologyGraph(built, ORDER);
    expect(g.order).toBe(built.nodes.length);
    expect(g.size).toBe(built.links.length);
    const w1 = g.getNodeAttributes("w1");
    expect(w1.kind).toBe("task");
    expect(w1.ownerKey).toBe("self");
    expect(w1.statusKey).toBe("doing");
    expect(w1.size).toBeGreaterThan(0);
    expect(g.getNodeAttribute("wf:wf1", "kind")).toBe("hub");
    expect(g.getNodeAttribute("wf:wf1", "statusKey")).toBe("hub");
    // dep 边方向：w2 → w1
    expect(g.hasEdge("w2", "w1")).toBe(true);
  });

  it("owner 着色双轨键：外来按注入序，状态键独立", () => {
    const built = buildTaskGraph(SAMPLE, WFS, DEFAULT_FILTERS);
    const g = toGraphologyGraph(built, ORDER);
    // 双键共存：任一模式下另一维度的键不丢
    expect(g.getNodeAttribute("z", "ownerKey")).toBe("owner:0");
    expect(g.getNodeAttribute("z", "statusKey")).toBe("todo");
    expect(g.getNodeAttribute("l", "ownerKey")).toBe("owner:1");
    expect(g.getNodeAttribute("a", "ownerKey")).toBe("self");
  });

  it("重复边防御：同 source-target 只留一条", () => {
    const built = buildTaskGraph(SAMPLE, WFS, DEFAULT_FILTERS);
    const g = toGraphologyGraph(built, ORDER);
    expect(g.size).toBe(built.links.length);
  });

  it("标签锚点（G4-CLUSTER）：组内 ≥2 卡建锚点 + 弱边，锚点 hidden", () => {
    const built = buildTaskGraph(SAMPLE, WFS, DEFAULT_FILTERS);
    const groups = new Map([
      ["周报", "周报"],
      ["客户", "客户"],
    ]);
    const g = toGraphologyGraph(built, ORDER, groups);
    const anchorId = "taggrp:周报";
    expect(g.hasNode(anchorId)).toBe(true);
    expect(g.getNodeAttribute(anchorId, "kind")).toBe("anchor");
    expect(g.getNodeAttribute(anchorId, "hidden")).toBe(true);
    // 带周报标签的成员卡连锚点，weight = TAG_EDGE_WEIGHT（3：强拉力聚簇）
    let tagEdges = 0;
    g.forEachEdge((_, attrs, s, t) => {
      if (s === anchorId || t === anchorId) {
        tagEdges++;
        expect(attrs.weight).toBe(3);
      }
    });
    expect(tagEdges).toBeGreaterThanOrEqual(2);
    // 单卡标签（客户仅 z? z 无标签——demo 内客户的卡不在 SAMPLE）不建锚点
    // 客户标签在 SAMPLE 中无带卡 → 无锚点
    expect(g.hasNode("taggrp:客户")).toBe(false);
  });

  it("无 tagGroups 时不建锚点（兼容旧调用）", () => {
    const built = buildTaskGraph(SAMPLE, WFS, DEFAULT_FILTERS);
    const g = toGraphologyGraph(built, ORDER);
    g.forEachNode((_, attrs) => {
      expect(attrs.kind).not.toBe("anchor");
    });
  });
});

describe("owner 着色解析（chips 与节点共用）", () => {
  const palette = { brand: "#brand", t3: "#t3", t5: "#t5", success: "#success" };

  it("ownerColorKey：本人 self，外来按注入序（缺失序号归 0）", () => {
    const zNode = { id: "z", owner: "p-1", degree: 0, kind: "task" as const, label: "" };
    const aNode = { id: "a", owner: undefined, degree: 0, kind: "task" as const, label: "" };
    expect(ownerColorKey(zNode, ORDER)).toBe("owner:0");
    expect(ownerColorKey(aNode, ORDER)).toBe("self");
  });

  it("resolveOwnerColor 与 chips 同源：序 N → PALETTE[N]，self → brand", () => {
    expect(resolveOwnerColor("owner:0", palette)).toBe("#7c6bd6");
    expect(resolveOwnerColor("owner:1", palette)).toBe("#c2711d");
    expect(resolveOwnerColor("self", palette)).toBe("#brand");
  });

  it("键不匹配时安全兜底", () => {
    expect(resolveOwnerColor("bogus", palette)).toBe("#t5");
  });
});

describe("fa2Settings", () => {
  it("大图开启 barnesHut 并增大 slowDown", () => {
    const small = fa2Settings(100);
    const large = fa2Settings(3000);
    expect(small.barnesHutOptimize).toBe(false);
    expect(large.barnesHutOptimize).toBe(true);
    expect(large.slowDown).toBeGreaterThan(small.slowDown);
    expect(large.adjustSizes).toBe(true);
  });
});

describe("computeCameraFit（归一化相机空间适配）", () => {
  // 1024×811 视口 + 对称方形参考 bbox：内容=参考系时 ratio 应恰好铺满
  const NORM = { x: [-100, 100] as [number, number], y: [-100, 100] as [number, number] };
  const VIEW = { width: 1024, height: 811 };

  it("内容充满参考系：相机居中 (0.5,0.5)，ratio 铺满留边距", () => {
    const fit = computeCameraFit({ minX: -100, maxX: 100, minY: -100, maxY: 100 }, NORM, VIEW);
    expect(fit).not.toBeNull();
    expect(fit!.x).toBeCloseTo(0.5, 6);
    expect(fit!.y).toBeCloseTo(0.5, 6);
    // 手算：cr=1（graphRatio=1, viewportRatio<1 同向），smallest=811−60=751，
    // ratio = max(1×751/1024, 1×751/811) × 1.12 = (751/811)×1.12
    expect(fit!.ratio).toBeCloseTo((751 / 811) * 1.12, 6);
  });

  it("规模不变性（坍缩 bug 回归锁）：30 节点与 1227 节点的 ratio 同量级", () => {
    // 旧实现 ratio = spanRaw/像素——规模越小 ratio 越小（过放大推出视口）。
    // 归一化空间下内容相对参考系的占比与绝对规模无关：
    const small = computeCameraFit(
      { minX: -70, maxX: 65, minY: -68, maxY: 61 },
      { x: [-70, 65], y: [-68, 61] },
      VIEW
    );
    const large = computeCameraFit(
      { minX: -1050, maxX: 1050, minY: -1050, maxY: 1050 },
      { x: [-1050, 1050], y: [-1050, 1050] },
      VIEW
    );
    expect(small!.ratio).toBeCloseTo(large!.ratio, 1);
    expect(small!.x).toBeCloseTo(large!.x, 1);
  });

  it("内容偏离参考系中心：相机中心跟随（归一化平移）", () => {
    const fit = computeCameraFit({ minX: 100, maxX: 300, minY: -100, maxY: 100 }, NORM, VIEW);
    // 内容中心 (200, 0) → 归一化 0.5 + 200/200 = 1.5
    expect(fit!.x).toBeCloseTo(1.5, 6);
    expect(fit!.y).toBeCloseTo(0.5, 6);
  });

  it("视口退化/输入非有限 → null（调用方不得写相机）", () => {
    const bbox = { minX: -100, maxX: 100, minY: -100, maxY: 100 };
    expect(computeCameraFit(bbox, NORM, { width: 0, height: 0 })).toBeNull();
    expect(computeCameraFit(bbox, NORM, { width: 50, height: 50 })).toBeNull();
    expect(computeCameraFit({ ...bbox, minX: -Infinity }, NORM, VIEW)).toBeNull();
    expect(computeCameraFit(bbox, { x: [3, 3], y: [3, 3] }, VIEW)).toBeNull();
  });

  it("graphBBox 与图坐标一致，空图/非有限兜底 [0,1]", () => {
    const built = buildTaskGraph(SAMPLE, WFS, DEFAULT_FILTERS);
    const g = toGraphologyGraph(built, ORDER);
    const bb = graphBBox(g);
    let minX = Infinity, maxX = -Infinity, minY = Infinity, maxY = -Infinity;
    g.forEachNode((_, a) => {
      minX = Math.min(minX, a.x); maxX = Math.max(maxX, a.x);
      minY = Math.min(minY, a.y); maxY = Math.max(maxY, a.y);
    });
    expect(bb.x).toEqual([minX, maxX]);
    expect(bb.y).toEqual([minY, maxY]);
  });
});
