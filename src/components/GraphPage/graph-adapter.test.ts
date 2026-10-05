// graph-adapter 单测（G3-SIGMA 迁移 + r1 着色统一）：建图 → graphology 图的
// 节点属性、边数守恒、owner 着色双轨键、重复边防御、FA2 设置推导。
import { describe, expect, it } from "vitest";
import type { Task, Workflow } from "../../types";
import { buildTaskGraph, DEFAULT_FILTERS } from "./graph-build";
import {
  fa2Settings,
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
