// graph-build 单测（任务图谱设计 §3.1）：建图、丢悬空边、hub、度数、过滤器、
// 排除软删、含归档、孤立节点剪枝、成员/标签/年份聚合。
import { describe, expect, it } from "vitest";
import type { Task, Workflow } from "../../types";
import {
  SELF_OWNER,
  buildTaskGraph,
  collectOwners,
  collectTags,
  collectYears,
  DEFAULT_FILTERS,
  wouldCreateDepCycle,
} from "./graph-build";

const WFS: Workflow[] = [
  { id: "wf1", name: "周报流水线", goal: "出周报" },
  { id: "wf2", name: "部署流水线", goal: "部署" },
];

function t(partial: Partial<Task> & { id: string }): Task {
  return { title: `任务-${partial.id}`, column: "todo", ...partial };
}

const SAMPLE: Task[] = [
  t({ id: "a", column: "doing" }), // 本人孤立卡
  t({ id: "b", column: "done", completedAt: new Date("2026-03-01").getTime() }),
  t({ id: "w1", origin: "workflow", workflowId: "wf1", dependsOn: ["w2"] }),
  t({ id: "w2", origin: "workflow", workflowId: "wf1" }),
  t({ id: "w3", origin: "workflow", workflowId: "wf2", dependsOn: ["gone"] }), // 悬空引用
  t({ id: "gone", deletedAt: 1 }), // 回收站：永不出图
  t({ id: "arch", column: "done", archived: true, completedAt: new Date("2025-11-02").getTime() }),
  t({ id: "zhang", ownerId: "p-zhang" }), // 外来卡
];

describe("buildTaskGraph", () => {
  it("排除回收站、保留归档，依赖边双向成边", () => {
    const g = buildTaskGraph(SAMPLE, WFS, DEFAULT_FILTERS);
    const ids = g.nodes.map((n) => n.id);
    expect(ids).not.toContain("gone");
    expect(ids).toContain("arch");
    const dep = g.links.find((l) => l.kind === "dep");
    expect(dep).toEqual({ source: "w2", target: "w1", kind: "dep" });
  });

  it("悬空 dependsOn 丢边不丢节点", () => {
    const g = buildTaskGraph(SAMPLE, WFS, DEFAULT_FILTERS);
    expect(g.nodes.map((n) => n.id)).toContain("w3");
    expect(g.links.filter((l) => l.kind === "dep" && l.target === "w3")).toHaveLength(0);
  });

  it("每个有成员的工作流一个 hub，成员边指向 hub", () => {
    const g = buildTaskGraph(SAMPLE, WFS, DEFAULT_FILTERS);
    const hubs = g.nodes.filter((n) => n.kind === "hub");
    expect(hubs.map((h) => h.id).sort()).toEqual(["wf:wf1", "wf:wf2"]);
    expect(hubs[0].label).toBe("周报流水线");
    const member = g.links.filter((l) => l.kind === "member");
    expect(member.filter((l) => l.target === "wf:wf1")).toHaveLength(2);
  });

  it("度数 = dep + member 边数", () => {
    const g = buildTaskGraph(SAMPLE, WFS, DEFAULT_FILTERS);
    const w1 = g.nodes.find((n) => n.id === "w1")!;
    expect(w1.degree).toBe(2); // 1 dep + 1 member
    const hub1 = g.nodes.find((n) => n.id === "wf:wf1")!;
    expect(hub1.degree).toBe(2);
  });

  it("状态过滤器", () => {
    const g = buildTaskGraph(SAMPLE, WFS, {
      ...DEFAULT_FILTERS,
      status: { todo: false, doing: true, done: false },
    });
    const ids = g.nodes.filter((n) => n.kind === "task").map((n) => n.id);
    expect(ids).toContain("a");
    expect(ids).not.toContain("b");
  });

  it("成员过滤器（本人哨兵 + 外来 pid）", () => {
    const onlySelf = buildTaskGraph(SAMPLE, WFS, {
      ...DEFAULT_FILTERS,
      owners: [SELF_OWNER],
    });
    expect(onlySelf.nodes.map((n) => n.id)).not.toContain("zhang");
    const onlyZhang = buildTaskGraph(SAMPLE, WFS, {
      ...DEFAULT_FILTERS,
      owners: ["p-zhang"],
    });
    expect(onlyZhang.nodes.map((n) => n.id)).toContain("zhang");
    expect(onlyZhang.nodes.filter((n) => n.kind === "task")).toHaveLength(1);
  });

  it("标签过滤：任一命中保留", () => {
    const tagged = [...SAMPLE, t({ id: "tagged", tags: ["部署", "周报"] })];
    const g = buildTaskGraph(tagged, WFS, {
      ...DEFAULT_FILTERS,
      tags: ["部署"],
    });
    const ids = g.nodes.filter((n) => n.kind === "task").map((n) => n.id);
    expect(ids).toEqual(["tagged"]);
  });

  it("年份过滤按 completedAt", () => {
    const g = buildTaskGraph(SAMPLE, WFS, { ...DEFAULT_FILTERS, year: 2025 });
    const ids = g.nodes.filter((n) => n.kind === "task").map((n) => n.id);
    expect(ids).toEqual(["arch"]);
  });

  it("includeOrphans=false 剪掉无边任务，保留有成员的 hub", () => {
    const g = buildTaskGraph(SAMPLE, WFS, {
      ...DEFAULT_FILTERS,
      includeOrphans: false,
    });
    const ids = g.nodes.map((n) => n.id);
    expect(ids).not.toContain("a"); // 孤立本人卡被剪
    expect(ids).not.toContain("zhang");
    expect(ids).toContain("wf:wf2"); // w3 仍有 member 边 → hub 保留
    expect(ids).toContain("wf:wf1");
  });

  it("工作流白名单只保留对应 hub 与成员", () => {
    const g = buildTaskGraph(SAMPLE, WFS, {
      ...DEFAULT_FILTERS,
      workflowIds: ["wf1"],
    });
    const hubIds = g.nodes.filter((n) => n.kind === "hub").map((n) => n.id);
    expect(hubIds).toEqual(["wf:wf1"]);
    expect(g.nodes.map((n) => n.id)).not.toContain("w3");
  });
});

describe("collect 聚合", () => {
  it("collectOwners：本人在前，外来含未知占位", () => {
    const owners = collectOwners(SAMPLE, [{ id: SELF_OWNER, name: "我", isSelf: true }]);
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
