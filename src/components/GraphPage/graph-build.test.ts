// graph-build 单测（任务图谱设计 §3.1）：建图、丢悬空边、度数、过滤器、
// 排除软删、含归档、孤立节点剪枝、成员/标签/年份聚合。
import { describe, expect, it } from "vitest";
import type { Task } from "../../types";
import {
  SELF_OWNER,
  buildTaskGraph,
  collectOwners,
  collectTags,
  collectYears,
  DEFAULT_FILTERS,
  wouldCreateDepCycle,
} from "./graph-build";

function t(partial: Partial<Task> & { id: string }): Task {
  return { title: `任务-${partial.id}`, column: "todo", ...partial };
}

const SAMPLE: Task[] = [
  t({ id: "a", column: "doing" }), // 本人孤立卡
  t({ id: "b", column: "done", completedAt: new Date("2026-03-01").getTime() }),
  t({ id: "w1", dependsOn: ["w2"] }),
  t({ id: "w2" }),
  t({ id: "w3", dependsOn: ["gone"] }), // 悬空引用
  t({ id: "gone", deletedAt: 1 }), // 回收站：永不出图
  t({
    id: "arch",
    column: "done",
    archived: true,
    completedAt: new Date("2025-11-02").getTime(),
  }),
  t({ id: "zhang", ownerId: "p-zhang" }), // 外来卡
];

describe("buildTaskGraph", () => {
  it("排除回收站、保留归档，依赖边双向成边", () => {
    const g = buildTaskGraph(SAMPLE, DEFAULT_FILTERS);
    const ids = g.nodes.map((n) => n.id);
    expect(ids).not.toContain("gone");
    expect(ids).toContain("arch");
    const dep = g.links.find((l) => l.kind === "dep");
    expect(dep).toEqual({ source: "w2", target: "w1", kind: "dep" });
  });

  it("悬空 dependsOn 丢边不丢节点", () => {
    const g = buildTaskGraph(SAMPLE, DEFAULT_FILTERS);
    expect(g.nodes.map((n) => n.id)).toContain("w3");
    expect(
      g.links.filter((l) => l.kind === "dep" && l.target === "w3"),
    ).toHaveLength(0);
  });

  it("度数 = dep + parent 边数", () => {
    const g = buildTaskGraph(SAMPLE, DEFAULT_FILTERS);
    const w1 = g.nodes.find((n) => n.id === "w1")!;
    expect(w1.degree).toBe(1);
  });

  it("状态过滤器", () => {
    const filtered = buildTaskGraph(SAMPLE, {
      ...DEFAULT_FILTERS,
      status: { todo: false, doing: true, done: false },
    });
    const ids = filtered.nodes.map((n) => n.id);
    expect(ids).toContain("a");
    expect(ids).not.toContain("b");
  });

  it("成员过滤器（本人哨兵 + 外来 pid）", () => {
    const onlySelf = buildTaskGraph(SAMPLE, {
      ...DEFAULT_FILTERS,
      owners: [SELF_OWNER],
    });
    expect(onlySelf.nodes.map((n) => n.id)).not.toContain("zhang");
    const onlyZhang = buildTaskGraph(SAMPLE, {
      ...DEFAULT_FILTERS,
      owners: ["p-zhang"],
    });
    expect(onlyZhang.nodes.map((n) => n.id)).toContain("zhang");
    expect(onlyZhang.nodes).toHaveLength(1);
  });

  it("标签过滤：任一命中保留", () => {
    const tagged = [...SAMPLE, t({ id: "tagged", tags: ["部署", "周报"] })];
    const g = buildTaskGraph(tagged, {
      ...DEFAULT_FILTERS,
      tags: ["部署"],
    });
    const ids = g.nodes.map((n) => n.id);
    expect(ids).toEqual(["tagged"]);
  });

  it("年份过滤按 completedAt", () => {
    const g = buildTaskGraph(SAMPLE, { ...DEFAULT_FILTERS, year: 2025 });
    const ids = g.nodes.map((n) => n.id);
    expect(ids).toEqual(["arch"]);
  });

  it("includeOrphans=false 剪掉无边任务", () => {
    const g = buildTaskGraph(SAMPLE, {
      ...DEFAULT_FILTERS,
      includeOrphans: false,
    });
    const ids = g.nodes.map((n) => n.id);
    expect(ids).not.toContain("a"); // 孤立本人卡被剪
    expect(ids).not.toContain("zhang");
    expect(ids).toContain("w1"); // 有 dep 边保留
    expect(ids).toContain("w2");
  });

  it("子任务卡出身边：母 → 子成 parent 边，计入度数救孤儿", () => {
    const tasks: Task[] = [
      t({ id: "mother" }),
      t({ id: "child", parentId: "mother" }),
    ];
    const g = buildTaskGraph(tasks, DEFAULT_FILTERS);
    expect(g.links).toContainEqual({
      source: "mother",
      target: "child",
      kind: "parent",
    });
    const byId = new Map(g.nodes.map((n) => [n.id, n]));
    expect(byId.get("mother")?.degree).toBe(1);
    expect(byId.get("child")?.degree).toBe(1);
    // parent 边连接的卡不算孤儿
    const pruned = buildTaskGraph(tasks, {
      ...DEFAULT_FILTERS,
      includeOrphans: false,
    });
    expect(pruned.nodes.map((n) => n.id).sort()).toEqual(["child", "mother"]);
  });

  it("子任务卡出身边：母被过滤/回收站 → 丢边不丢子节点", () => {
    const tasks: Task[] = [
      t({ id: "mother", deletedAt: 1 }),
      t({ id: "child", parentId: "mother" }),
      t({ id: "child2", parentId: "dangling" }),
    ];
    const g = buildTaskGraph(tasks, DEFAULT_FILTERS);
    expect(g.links.filter((l) => l.kind === "parent")).toHaveLength(0);
    const ids = g.nodes.map((n) => n.id);
    expect(ids).toContain("child");
    expect(ids).toContain("child2");
  });
});

describe("collect 聚合", () => {
  it("collectOwners：本人在前，外来含未知占位", () => {
    const owners = collectOwners(SAMPLE, [
      { id: SELF_OWNER, name: "我", isSelf: true },
    ]);
    expect(owners[0]).toEqual({ id: SELF_OWNER, name: "我", isSelf: true });
    expect(owners.map((o) => o.id)).toContain("p-zhang");
  });

  it("collectTags 按计数降序", () => {
    const tasks = [
      t({ id: "1", tags: ["周报"] }),
      t({ id: "2", tags: ["周报", "部署"] }),
    ];
    const tags = collectTags(tasks);
    expect(tags[0]).toEqual({ tag: "周报", count: 2 });
    expect(tags[1]).toEqual({ tag: "部署", count: 1 });
  });

  it("collectYears 降序去重", () => {
    expect(collectYears(SAMPLE)).toEqual([2026, 2025]);
  });
});

describe("wouldCreateDepCycle（G5-DEPEDIT）", () => {
  const tasks = [
    t({ id: "self", dependsOn: ["up1"] }),
    t({ id: "up1", dependsOn: ["up2"] }),
    t({ id: "up2" }),
    t({ id: "down1", dependsOn: ["self"] }),
    t({ id: "other" }),
  ];

  it("自环拒绝", () => {
    expect(wouldCreateDepCycle(tasks, "self", "self")).toBe(true);
  });

  it("候选若（传递）依赖自身则拒绝（会闭环）", () => {
    // down1 → self：若 self 再依赖 down1 → self → down1 → self 成环
    expect(wouldCreateDepCycle(tasks, "self", "down1")).toBe(true);
  });

  it("上游方向放行（self → up1 → up2 无环）", () => {
    expect(wouldCreateDepCycle(tasks, "self", "up1")).toBe(false);
    expect(wouldCreateDepCycle(tasks, "self", "up2")).toBe(false);
  });

  it("无关联与悬空依赖放行", () => {
    expect(wouldCreateDepCycle(tasks, "other", "self")).toBe(false);
    expect(wouldCreateDepCycle(tasks, "self", "ghost")).toBe(false);
  });
});
