// graph-scene / graph-force3d 单测：场景结构 + 力场收敛。
// 这两层是图谱最容易整帧报废的地方——坐标一旦发散，渲染层按它写顶点缓冲，
// 一个坏节点就清空整帧画布（「一闪而过即空白」）。测试锚在这里，不在画面上。
import { describe, expect, it } from "vitest";
import type { Task } from "../../types";
import type { SceneNode } from "./graph-scene";
import { buildTaskGraph, DEFAULT_FILTERS } from "./graph-build";
import { buildScene, isHub, isStalled, labelDegreeFloor, nodeRadius } from "./graph-scene";
import { Force3D, KINETIC_EPSILON, mulberry32, settle } from "./graph-force3d";

function t(partial: Partial<Task> & { id: string }): Task {
  return { title: `任务-${partial.id}`, column: "todo", ...partial };
}

const NOW = Date.UTC(2026, 9, 18);
const DAY = 86_400_000;

function sceneOf(
  tasks: Task[],
  over: Partial<Parameters<typeof buildScene>[1]> = {},
) {
  return buildScene(buildTaskGraph(tasks, DEFAULT_FILTERS), {
    ownerOrder: new Map(),
    tagOrder: new Map(),
    looseness: "standard",
    now: NOW,
    ...over,
  });
}

describe("nodeRadius", () => {
  it("只跟连接数走，单调递增，零度不炸", () => {
    expect(nodeRadius(0)).toBeCloseTo(nodeRadius(1));
    expect(nodeRadius(2)).toBeLessThan(nodeRadius(9));
    expect(nodeRadius(9)).toBeLessThan(nodeRadius(50));
    expect(Number.isFinite(nodeRadius(0))).toBe(true);
  });
});

describe("isStalled", () => {
  it("进行中且超期才算；已完成/待办/无创建时间都不算", () => {
    const old = { status: "doing", createdAt: NOW - 20 * DAY } as const;
    expect(isStalled(old, NOW)).toBe(true);
    expect(isStalled({ ...old, createdAt: NOW - 13 * DAY }, NOW)).toBe(false);
    expect(isStalled({ ...old, status: "todo" }, NOW)).toBe(false);
    expect(isStalled({ status: "doing" }, NOW)).toBe(false);
  });
});

describe("buildScene", () => {
  it("只为有成员的维度建锚点，空维度不产生空核", () => {
    const scene = sceneOf([
      t({ id: "a", ownerId: "p1", tags: ["发布"], column: "doing" }),
      t({ id: "b", ownerId: "p1", tags: ["发布"], column: "done" }),
    ]);
    const hubs = scene.nodes.filter((n) => n.kind === "hub");
    expect(hubs.filter((h) => h.hubKind === "owner")).toHaveLength(1);
    expect(hubs.filter((h) => h.hubKind === "tag")).toHaveLength(1);
    // 没有 todo 任务 → 不建「待办」核
    expect(hubs.filter((h) => h.hubKind === "status").map((h) => h.label)).toEqual([
      "进行中",
      "已完成",
    ]);
    expect(hubs.every((h) => h.hint !== null)).toBe(true);
  });

  it("单卡标签不建核（一个任务的标签没有聚簇意义）", () => {
    const scene = sceneOf([
      t({ id: "a", tags: ["孤儿标签"] }),
      t({ id: "b", tags: ["成组"] }),
      t({ id: "c", tags: ["成组"] }),
    ]);
    const tags = scene.nodes
      .filter((n) => n.kind === "hub" && n.hubKind === "tag")
      .map((n) => n.label);
    expect(tags).toEqual(["成组"]);
  });

  it("锚点有可解的岗位坐标，且按外径缩放", () => {
    const tight = sceneOf([t({ id: "a" }), t({ id: "b", column: "doing" })], {
      looseness: "compact",
    });
    const loose = sceneOf([t({ id: "a" }), t({ id: "b", column: "doing" })], {
      looseness: "loose",
    });
    const hubOf = (s: ReturnType<typeof sceneOf>) =>
      s.nodes.find((n) => n.kind === "hub" && n.hubKind === "status")!;
    expect(loose.radius).toBeGreaterThan(tight.radius);
    expect(Math.hypot(...hubOf(loose).hint!)).toBeGreaterThan(
      Math.hypot(...hubOf(tight).hint!),
    );
  });

  it("多标签任务按首个标签定色，标签顺序即主次", () => {
    const scene = sceneOf(
      [t({ id: "a", tags: ["主", "次"] }), t({ id: "b", tags: ["次"] }), t({ id: "c", tags: ["次"] })],
      { tagOrder: new Map([["主", 0], ["次", 1]]) },
    );
    const a = scene.nodes.find((n) => n.id === "a")!;
    expect(a.tagIdx).toBe(0);
  });

  it("悬空 dependsOn 不产生指向不存在节点的边", () => {
    const scene = sceneOf([t({ id: "a", dependsOn: ["不存在"] })]);
    const idx = scene.indexOf.get("a")!;
    expect(scene.edges.every((e) => e.a === idx || e.b === idx)).toBe(true);
  });

  it("锚点 id 带 hub: 前缀，isHub 据此把锚点挡在业务交互外", () => {
    const scene = sceneOf([t({ id: "a" }), t({ id: "b" })]);
    expect(scene.nodes.some((n) => isHub(n.id))).toBe(true);
    expect(isHub("a")).toBe(false);
  });

  it("松散度外径随 √N 缩放（面密度恒定）", () => {
    const two = sceneOf([t({ id: "a" }), t({ id: "b" })]);
    const four = sceneOf([
      t({ id: "a" }),
      t({ id: "b" }),
      t({ id: "c" }),
      t({ id: "d" }),
    ]);
    expect(four.radius / two.radius).toBeCloseTo(Math.sqrt(2), 1);
  });

  it("锚点岗位按小样的形状铺：成员核是向后弯的浅弧，状态核是竖直一列", () => {
    const owners = ["u-1", "u-2", "u-3", "u-4"];
    const scene = sceneOf(
      [
        t({ id: "a", ownerId: "u-1", tags: ["周报"], column: "todo" }),
        t({ id: "b", ownerId: "u-2", tags: ["周报"], column: "doing" }),
        t({ id: "c", ownerId: "u-3", tags: ["周报"], column: "done" }),
        t({ id: "d", ownerId: "u-4", tags: ["周报"], column: "doing" }),
      ],
      { ownerOrder: new Map(owners.map((id, i) => [id, i])) },
    );
    const ownerHints = scene.nodes
      .filter((n) => n.hubKind === "owner")
      .map((n) => n.hint!);
    // 弧线：z 随 |x| 递增。压平成 z 常数的话，正对镜头时四枚铭牌叠成一行
    const zs = ownerHints.map((h) => h[2]);
    const extremes = [ownerHints[0], ownerHints[ownerHints.length - 1]];
    expect(Math.max(...zs)).toBeGreaterThan(Math.min(...zs) + 1);
    extremes.forEach((h) => expect(h[2]).toBeGreaterThan(30 * (scene.radius / 258)));

    // 状态核：x 固定成一列，只在 y 上拉开——状态是有序维度，散排就没法扫读
    const statusHints = scene.nodes.filter((n) => n.hubKind === "status").map((n) => n.hint!);
    expect(new Set(statusHints.map((h) => h[0])).size).toBe(1);
    const ys = statusHints.map((h) => h[1]);
    expect(Math.max(...ys) - Math.min(...ys)).toBeGreaterThan(0);
  });

  it("锚点铭牌印人话：状态用中文，成员用显示名，绝不漏内部键", () => {
    const scene = sceneOf(
      [
        t({ id: "a", column: "doing", ownerId: "u-1", tags: ["周报"] }),
        t({ id: "b", column: "doing", ownerId: "u-1", tags: ["周报"] }),
      ],
      { ownerNames: new Map([["owner:0", "张三"]]) },
    );
    const hubs = scene.nodes.filter((n) => n.kind === "hub");
    const labels = hubs.map((h) => h.label);
    expect(labels).toContain("张三");
    expect(labels).toContain("进行中");
    // 内部键（owner:N / 列 id）一个都不许出现在画布文字上
    expect(labels.some((l) => /^owner:\d+$/.test(l) || /^(todo|doing|done)$/.test(l)))
      .toBe(false);
  });
});

describe("Force3D", () => {
  const tasks = Array.from({ length: 40 }, (_, i) =>
    t({
      id: `n${i}`,
      column: (["todo", "doing", "done"] as const)[i % 3],
      ownerId: i % 2 ? "p1" : undefined,
      tags: [`组${i % 4}`],
      dependsOn: i > 0 && i % 5 === 0 ? [`n${i - 1}`] : [],
    }),
  );

  it("收敛后坐标全部有限（NaN 会让整批图元在 GPU 上被丢弃）", () => {
    const scene = sceneOf(tasks);
    const f = new Force3D(scene);
    settle(f, { maxSteps: 250 });
    for (const v of f.pos) expect(Number.isFinite(v)).toBe(true);
    expect(f.repairCount).toBe(0);
  });

  it("收敛：动能降到阈值以下后停住（不是持续震荡）", () => {
    const f = new Force3D(sceneOf(tasks));
    f.step();
    for (let i = 0; i < 100; i++) f.step();
    const mid = f.kinetic();
    expect(mid).toBeGreaterThan(KINETIC_EPSILON);
    // 温度单调下降到 minTemp 后位移封顶跟着降，动能必然跨过收敛判据
    const steps = settle(f);
    expect(steps).toBeLessThanOrEqual(900);
    expect(f.kinetic()).toBeLessThan(KINETIC_EPSILON);
  });

  it("确定性：同种子两次布局坐标逐位相同（构图可回归）", () => {
    const a = new Force3D(sceneOf(tasks));
    const b = new Force3D(sceneOf(tasks));
    for (let i = 0; i < 60; i++) {
      a.step();
      b.step();
    }
    expect(Array.from(a.pos)).toEqual(Array.from(b.pos));
  });

  it("锚点被拉回岗位附近（同族任务因此聚成可辨认的星团）", () => {
    const scene = sceneOf(tasks);
    const f = new Force3D(scene);
    settle(f, { maxSteps: 500 });
    scene.nodes.forEach((node, i) => {
      if (!node.hint) return;
      const d = Math.hypot(
        f.pos[i * 3] - node.hint[0],
        f.pos[i * 3 + 1] - node.hint[1],
        f.pos[i * 3 + 2] - node.hint[2],
      );
      expect(d).toBeLessThan(scene.radius);
    });
  });

  it("坐标消毒：NaN 节点被救回场内，不留在越界位置", () => {
    const scene = sceneOf(tasks);
    const f = new Force3D(scene);
    f.pos[0] = Number.NaN;
    f.pos[1] = Number.POSITIVE_INFINITY;
    f.step();
    expect(Number.isFinite(f.pos[0])).toBe(true);
    expect(Number.isFinite(f.pos[1])).toBe(true);
    expect(f.repairCount).toBeGreaterThan(0);
  });

  it("normalize 把全场缩放到目标外径", () => {
    const scene = sceneOf(tasks);
    const f = new Force3D(scene);
    settle(f, { maxSteps: 200 });
    f.normalize(100);
    let max = 0;
    for (let i = 0; i < f.pos.length; i += 3) {
      max = Math.max(max, Math.hypot(f.pos[i], f.pos[i + 1], f.pos[i + 2]));
    }
    expect(max).toBeCloseTo(100, 0);
  });

  it("任务铺满星云而不是塌成一颗中心核（缺静息长度就会全挤成一团过曝白球）", () => {
    const scene = sceneOf(tasks);
    const f = new Force3D(scene);
    settle(f, { maxSteps: 500 });
    f.normalize(scene.radius);
    const radii = scene.nodes
      .map((n, i) => (n.kind === "task" ? Math.hypot(f.pos[i * 3], f.pos[i * 3 + 1], f.pos[i * 3 + 2]) : -1))
      .filter((r) => r >= 0)
      .sort((a, b) => a - b);
    const median = radii[Math.floor(radii.length / 2)] / scene.radius;
    expect(median).toBeGreaterThan(0.3);
  });

  it("归一化后岗位跟着换尺度：再迭代锚点不会被拽回旧位置跳一次", () => {
    const scene = sceneOf(tasks);
    const f = new Force3D(scene);
    settle(f, { maxSteps: 300 });
    f.normalize(scene.radius);
    const before = Float32Array.from(f.pos);
    f.reheat();
    for (let i = 0; i < 40; i++) f.step();
    scene.nodes.forEach((node, i) => {
      if (!node.hint) return;
      const moved = Math.hypot(
        f.pos[i * 3] - before[i * 3],
        f.pos[i * 3 + 1] - before[i * 3 + 1],
        f.pos[i * 3 + 2] - before[i * 3 + 2],
      );
      expect(moved).toBeLessThan(scene.radius * 0.2);
    });
  });

  it("separate 把贴在一起的节点推开到最小间距之上", () => {
    const scene = sceneOf(tasks);
    const f = new Force3D(scene);
    for (let i = 0; i < scene.nodes.length; i++) {
      f.pos[i * 3] = 0;
      f.pos[i * 3 + 1] = 0;
      f.pos[i * 3 + 2] = 0;
    }
    f.separate(14);
    const a = scene.nodes.length;
    const d = Math.hypot(
      f.pos[0] - f.pos[3],
      f.pos[1] - f.pos[4],
      f.pos[2] - f.pos[5],
    );
    expect(a).toBeGreaterThan(1);
    expect(d).toBeGreaterThan(0);
  });

  it("空场景不炸（空图谱页也会 mount 画布）", () => {
    const scene = sceneOf([]);
    const f = new Force3D(scene);
    expect(() => settle(f, { maxSteps: 10 })).not.toThrow();
    expect(f.kinetic()).toBe(0);
  });
});

describe("mulberry32", () => {
  it("同种子序列一致，落在 [0,1)", () => {
    const a = mulberry32(42);
    const b = mulberry32(42);
    for (let i = 0; i < 20; i++) {
      const v = a();
      expect(v).toBe(b());
      expect(v).toBeGreaterThanOrEqual(0);
      expect(v).toBeLessThan(1);
    }
  });
});

describe("labelDegreeFloor", () => {
  const task = (degree: number): SceneNode => ({
    id: `t${degree}`,
    kind: "task",
    label: "x",
    radius: 2.6,
    degree,
    ownerKey: "self",
    tagIdx: null,
    statusKey: "todo",
    stalled: false,
    hint: null,
  });

  it("稀疏图也留得住最重的几颗（写死阈值会让 40 边图一张标签都不出）", () => {
    const degs = [0, 0, 1, 1, 1, 2, 2, 3, 3, 4];
    const floor = labelDegreeFloor(degs.map(task));
    expect(degs.filter((d) => d >= floor).length).toBeGreaterThanOrEqual(1);
    expect(floor).toBeLessThanOrEqual(Math.max(...degs));
  });

  it("密图只放行头部，不糊成字墙", () => {
    const degs = Array.from({ length: 100 }, (_, i) => Math.floor(i / 8));
    const floor = labelDegreeFloor(degs.map(task));
    expect(degs.filter((d) => d >= floor).length).toBeLessThanOrEqual(15);
  });

  it("只看任务，锚点的成员数不参与分位", () => {
    const hub: SceneNode = { ...task(99), id: "hub:tag:x", kind: "hub", hint: [0, 0, 0] };
    const floor = labelDegreeFloor([...Array.from({ length: 10 }, (_, i) => task(i)), hub]);
    expect(floor).toBeLessThan(99);
  });

  it("空场景返回 1，不返回 NaN", () => {
    expect(labelDegreeFloor([])).toBe(1);
  });
});
