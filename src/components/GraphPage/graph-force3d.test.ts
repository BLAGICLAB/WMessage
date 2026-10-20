// 三维力场单测：位移封顶（温度即位移预算）、锚点回拉、最小间距硬分离。
// 这三条是收敛的结构保证，改动公式时单测要能立刻变红。

import { describe, expect, it } from "vitest";
import type { Task } from "../../types";
import { buildScene, type Scene } from "./graph-scene";
import { Force3D, KINETIC_EPSILON, settle } from "./graph-force3d";
import { buildTaskGraph, DEFAULT_FILTERS } from "./graph-build";

function mkTasks(n: number): Task[] {
  const out: Task[] = [];
  for (let i = 0; i < n; i++) {
    const t: Task = {
      id: `t${i}`,
      title: `任务 ${i}`,
      column: (["todo", "doing", "done"] as const)[i % 3],
      tags: [`标签${i % 5}`],
      createdAt: 1_700_000_000_000,
    };
    if (i >= 2) t.dependsOn = [`t${i - 2}`];
    out.push(t);
  }
  return out;
}

function sceneOf(n: number): Scene {
  const built = buildTaskGraph(mkTasks(n), DEFAULT_FILTERS);
  return buildScene(built, {
    ownerOrder: new Map(),
    tagOrder: new Map(),
    looseness: "standard",
    now: Date.now(),
  });
}

/** 两个节点的裸场景（锚点/边都不要），用来精确控制坐标 */
function bareScene(): Scene {
  return {
    nodes: [
      { id: "a", kind: "task", label: "a", radius: 2.6, degree: 0, ownerKey: "self", tagIdx: null, statusKey: "todo", stalled: false, hint: null },
      { id: "b", kind: "task", label: "b", radius: 2.6, degree: 0, ownerKey: "self", tagIdx: null, statusKey: "todo", stalled: false, hint: null },
    ],
    edges: [],
    indexOf: new Map([["a", 0], ["b", 1]]),
    radius: 100,
  };
}

describe("Force3D", () => {
  it("每步位移都不超过当前温度：锚点也不例外", () => {
    const scene = sceneOf(40);
    const force = new Force3D(scene);
    const prev = new Float32Array(force.pos);
    let sawHub = false;
    for (let s = 0; s < 300; s++) {
      const temp = force.temp;
      prev.set(force.pos);
      force.step();
      for (let i = 0; i < scene.nodes.length; i++) {
        const d = Math.hypot(
          force.pos[i * 3] - prev[i * 3],
          force.pos[i * 3 + 1] - prev[i * 3 + 1],
          force.pos[i * 3 + 2] - prev[i * 3 + 2],
        );
        if (scene.nodes[i].kind === "hub") sawHub = true;
        // 容差给到 1e-4：坐标存的是 float32，位置量级到几千时加减一次就有
        // 1e-5 量级的舍入误差（真越界是 1.7 倍量级，不会被这点误差掩盖）
        expect(d).toBeLessThanOrEqual(temp * 1.0001 + 1e-6);
      }
    }
    expect(sawHub).toBe(true); // 场景里得有锚点，这条用例才真的覆盖了锚点
  });

  it("锚点收敛后仍守在岗位上", () => {
    const scene = sceneOf(40);
    const force = new Force3D(scene);
    settle(force, { maxSteps: 700 });
    scene.nodes.forEach((node, i) => {
      if (!node.hint) return;
      const d = Math.hypot(
        force.pos[i * 3] - node.hint[0],
        force.pos[i * 3 + 1] - node.hint[1],
        force.pos[i * 3 + 2] - node.hint[2],
      );
      expect(d).toBeLessThan(scene.radius * 0.02);
    });
  });

  it("收敛判据成立：动能掉到阈值以下", () => {
    const force = new Force3D(sceneOf(40));
    const steps = settle(force, { maxSteps: 900 });
    expect(steps).toBeLessThan(900);
    expect(force.kinetic()).toBeLessThan(KINETIC_EPSILON);
  });

  it("最小间距硬分离：两点分到指定间距（缺口一次补满）", () => {
    const scene = bareScene();
    const force = new Force3D(scene);
    force.pos[0] = 0; force.pos[1] = 0; force.pos[2] = 0;
    force.pos[3] = 8.373; force.pos[4] = 0; force.pos[5] = 0;
    force.separate(8.89);
    const d = Math.hypot(
      force.pos[3] - force.pos[0],
      force.pos[4] - force.pos[1],
      force.pos[5] - force.pos[2],
    );
    expect(d).toBeCloseTo(8.89, 3);
  });

  it("最小间距硬分离：重合的两点也被推开", () => {
    const scene = bareScene();
    const force = new Force3D(scene);
    // 初始坐标由种子撒开，两点并不重合——要测重合分支得手动摆成同一点
    force.pos.fill(0);
    force.separate(8);
    const d = Math.hypot(
      force.pos[3] - force.pos[0],
      force.pos[4] - force.pos[1],
      force.pos[5] - force.pos[2],
    );
    expect(d).toBeCloseTo(8, 3);
  });
});
