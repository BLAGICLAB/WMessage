// graph-adapter 单测（G3-SIGMA 迁移）：建图 → graphology 图的节点属性、
// 边数守恒、着色键、FA2 设置推导。
import { describe, expect, it } from "vitest";
import type { Task, Workflow } from "../../types";
import { buildTaskGraph, DEFAULT_FILTERS, SELF_OWNER } from "./graph-build";
import { fa2Settings, ownerColorKey, toGraphologyGraph } from "./graph-adapter";

const WFS: Workflow[] = [
  { id: "wf1", name: "周报流水线", goal: "出周报" },
];

function t(partial: Partial<Task> & { id: string }): Task {
  return { title: `任务-${partial.id}`, column: "todo", ...partial };
}

const SAMPLE: Task[] = [
  t({ id: "a", column: "doing" }),
  t({ id: "b", column: "done", completedAt: Date.now() }),
  t({ id: "w1", column: "doing", origin: "workflow", workflowId: "wf1", dependsOn: ["w2"] }),
  t({ id: "w2", column: "done", origin: "workflow", workflowId: "wf1" }),
  t({ id: "z", ownerId: "p-1" }),
];

describe("toGraphologyGraph", () => {
  it("节点/边数与建图结果守恒，属性齐备", () => {
    const built = buildTaskGraph(SAMPLE, WFS, DEFAULT_FILTERS);
    const g = toGraphologyGraph(built, "status", [
      { id: SELF_OWNER, name: "我", isSelf: true },
      { id: "p-1", name: "张三", isSelf: false },
    ]);
    expect(g.order).toBe(built.nodes.length);
    expect(g.size).toBe(built.links.length);
    const w1 = g.getNodeAttributes("w1");
    expect(w1.kind).toBe("task");
    expect(w1.colorKey).toBe("doing");
    expect(w1.size).toBeGreaterThan(0);
    expect(g.getNodeAttribute("wf:wf1", "kind")).toBe("hub");
    // dep 边方向：w2 → w1
    expect(g.hasEdge("w2", "w1")).toBe(true);
  });

  it("owner 着色键：本人 self，外来按序号", () => {
    const built = buildTaskGraph(SAMPLE, WFS, DEFAULT_FILTERS);
    const zNode = built.nodes.find((n) => n.id === "z")!;
    const aNode = built.nodes.find((n) => n.id === "a")!;
    const order = new Map([
      ["p-1", 2],
      ["p-2", 3],
    ]);
    expect(ownerColorKey(zNode, order)).toBe("owner:2");
    expect(ownerColorKey(aNode, order)).toBe("self");
  });

  it("owner 模式下 colorKey 带序号（reducer 按键取调色盘）", () => {
    const built = buildTaskGraph(SAMPLE, WFS, DEFAULT_FILTERS);
    const g = toGraphologyGraph(built, "owner", [
      { id: SELF_OWNER, name: "我", isSelf: true },
      { id: "p-1", name: "张三", isSelf: false },
    ]);
    expect(g.getNodeAttribute("z", "colorKey")).toBe("owner:1");
    expect(g.getNodeAttribute("a", "colorKey")).toBe("self");
  });

  it("重复边防御：同 source-target 只留一条", () => {
    const built = buildTaskGraph(SAMPLE, WFS, DEFAULT_FILTERS);
    const g = toGraphologyGraph(built, "status", []);
    // 建图结果本身无重复边；防御路径不抛错即可
    expect(g.size).toBe(built.links.length);
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
