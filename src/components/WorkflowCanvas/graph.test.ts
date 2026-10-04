import { describe, expect, it } from "vitest";
import {
  wouldCreateCycle,
  layoutGraph,
  draftFromTasks,
  NODE_HEIGHT,
  NODE_WIDTH,
  type CanvasNode,
} from "./graph";

describe("wouldCreateCycle", () => {
  const nodes = [
    { localId: "a", dependsOn: [] },
    { localId: "b", dependsOn: ["a"] },
    { localId: "c", dependsOn: ["b"] },
  ];

  it("拦截自环", () => {
    expect(wouldCreateCycle(nodes, "a", "a")).toBe(true);
  });

  it("拦截间接环（a→b→c 已存在，再加 c→a）", () => {
    // 加边 c→a：a 的 dependsOn 增加 c；从 a（to）可达 c（from）⇒ 成环
    expect(wouldCreateCycle(nodes, "c", "a")).toBe(true);
  });

  it("放行无环连线", () => {
    // a→c：c 的 dependsOn 增加 a；从 c 不可达 a
    expect(wouldCreateCycle(nodes, "a", "c")).toBe(false);
  });
});

describe("layoutGraph", () => {
  it("无依赖节点排在上层、有依赖排在下游（y 递增）", () => {
    const nodes = [
      { localId: "a", dependsOn: [] },
      { localId: "b", dependsOn: [] },
      { localId: "c", dependsOn: ["a", "b"] },
    ];
    const pos = layoutGraph(nodes);
    expect(Object.keys(pos).sort()).toEqual(["a", "b", "c"]);
    expect(pos.c.y).toBeGreaterThan(pos.a.y);
    expect(pos.c.y).toBeGreaterThan(pos.b.y);
    // 同层节点 y 相同、x 错开
    expect(pos.a.y).toBe(pos.b.y);
    expect(pos.a.x).not.toBe(pos.b.x);
  });

  it("悬空依赖引用不炸布局", () => {
    const nodes = [{ localId: "a", dependsOn: ["ghost"] }];
    const pos = layoutGraph(nodes);
    expect(pos.a.x).toBeGreaterThan(0);
  });
});

describe("draftFromTasks", () => {
  it("以真实任务 id 作为 localId 并保留依赖", () => {
    const nodes = draftFromTasks([
      { id: "t1", title: "A", dependsOn: [], workflowId: "w1" },
      {
        id: "t2",
        title: "B",
        dependsOn: ["t1"],
        workflowId: "w1",
        canvasPos: { x: 5, y: 200 },
      },
    ]);
    expect(nodes.map((n) => n.localId)).toEqual(["t1", "t2"]);
    expect(nodes[1].dependsOn).toEqual(["t1"]);
    expect(nodes[1].taskId).toBe("t2");
  });

  it("缺坐标的行补 dagre 布局（无坐标行 y 落在非零布局位）", () => {
    const nodes: CanvasNode[] = draftFromTasks([
      { id: "t1", title: "A", dependsOn: [], workflowId: "w1" },
      { id: "t2", title: "B", dependsOn: ["t1"], workflowId: "w1" },
    ]);
    // t1 无 canvasPos → 全图重排；下游 y > 上游 y
    expect(nodes[1].pos.y).toBeGreaterThan(nodes[0].pos.y);
    expect(nodes[0].pos.x).toBeGreaterThan(-NODE_WIDTH);
    expect(NODE_HEIGHT).toBeGreaterThan(0);
  });

  it("全部有坐标时不重排（保留用户手拖位置）", () => {
    const nodes = draftFromTasks([
      { id: "t1", title: "A", dependsOn: [], canvasPos: { x: 1, y: 2 }, workflowId: "w1" },
      {
        id: "t2",
        title: "B",
        dependsOn: ["t1"],
        canvasPos: { x: 3, y: 4 },
        workflowId: "w1",
      },
    ]);
    expect(nodes[0].pos).toEqual({ x: 1, y: 2 });
    expect(nodes[1].pos).toEqual({ x: 3, y: 4 });
  });
});
