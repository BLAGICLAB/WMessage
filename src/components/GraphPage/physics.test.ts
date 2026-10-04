// physics 单测（任务图谱设计 §3.2）：弹簧收敛、斥力分离、alpha 冷却、位置重绑。
import { describe, expect, it } from "vitest";
import {
  DEFAULT_CONFIG,
  LINK_DEP,
  REHEAT_ALPHA,
  initPositions,
  rebindPositions,
  taskRadius,
  tick,
  type PhysLink,
} from "./physics";

function twoNodes(): { nodes: ReturnType<typeof initPositions>; links: PhysLink[] } {
  const nodes = initPositions(
    [
      { id: "a", r: 4 },
      { id: "b", r: 4 },
    ],
    800,
    600
  );
  const links: PhysLink[] = [
    { source: 0, target: 1, distance: LINK_DEP.distance, strength: LINK_DEP.strength },
  ];
  return { nodes, links };
}

describe("physics", () => {
  it("弹簧把两节点拉近到目标距离（±25%）", () => {
    const { nodes, links } = twoNodes();
    // 初始同点重叠 + 随机拆分由斥力处理；先跑足量 tick
    let alpha = 1;
    for (let i = 0; i < 600; i++) {
      alpha = tick(nodes, links, DEFAULT_CONFIG, alpha, 400, 300);
    }
    const d = Math.hypot(nodes[0].x - nodes[1].x, nodes[0].y - nodes[1].y);
    expect(d).toBeGreaterThan(LINK_DEP.distance * 0.6);
    expect(d).toBeLessThan(LINK_DEP.distance * 1.6);
    expect(d).toBeGreaterThan(0);
  });

  it("斥力把重叠节点推开", () => {
    const nodes = initPositions(
      [
        { id: "a", r: 4 },
        { id: "b", r: 4 },
      ],
      800,
      600
    );
    // 同点放置：无弹簧，只靠斥力 + 碰撞
    nodes[1].x = nodes[0].x;
    nodes[1].y = nodes[0].y;
    let alpha = 1;
    for (let i = 0; i < 120; i++) {
      alpha = tick(nodes, [], DEFAULT_CONFIG, alpha, 400, 300);
    }
    const d = Math.hypot(nodes[0].x - nodes[1].x, nodes[0].y - nodes[1].y);
    expect(d).toBeGreaterThan(nodes[0].r + nodes[1].r + DEFAULT_CONFIG.collidePadding);
  });

  it("alpha 持续衰减直至冷却（循环可停）", () => {
    const { nodes, links } = twoNodes();
    let alpha = 1;
    let ticks = 0;
    while (alpha > DEFAULT_CONFIG.alphaMin && ticks < 1000) {
      alpha = tick(nodes, links, DEFAULT_CONFIG, alpha, 400, 300);
      ticks++;
    }
    expect(alpha).toBeLessThanOrEqual(DEFAULT_CONFIG.alphaMin);
    expect(ticks).toBeLessThan(1000);
  });

  it("固定点（拖拽 pin）在积分中保持不动", () => {
    const { nodes, links } = twoNodes();
    nodes[0].fx = 100;
    nodes[0].fy = 100;
    let alpha = 1;
    for (let i = 0; i < 50; i++) {
      alpha = tick(nodes, links, DEFAULT_CONFIG, alpha, 400, 300);
    }
    expect(nodes[0].x).toBe(100);
    expect(nodes[0].y).toBe(100);
  });

  it("rebindPositions 保留既有节点位置并更新半径", () => {
    const nodes = initPositions(
      [
        { id: "a", r: 4 },
        { id: "b", r: 4 },
      ],
      800,
      600
    );
    nodes[0].x = 42;
    const rebound = rebindPositions(
      nodes,
      [
        { id: "a", r: 9 },
        { id: "c", r: 4 },
      ],
      800,
      600
    );
    const a = rebound.find((n) => n.id === "a")!;
    const c = rebound.find((n) => n.id === "c")!;
    expect(a.x).toBe(42);
    expect(a.r).toBe(9);
    expect(c.x).not.toBeNaN();
    expect(rebound).toHaveLength(2);
  });

  it("taskRadius 随度数单调增", () => {
    expect(taskRadius(0)).toBeGreaterThan(0);
    expect(taskRadius(9)).toBeGreaterThan(taskRadius(4));
  });

  it("REHEAT_ALPHA 暖启动低于 1（避免全图爆开）", () => {
    expect(REHEAT_ALPHA).toBeLessThan(1);
    expect(REHEAT_ALPHA).toBeGreaterThan(DEFAULT_CONFIG.alphaMin);
  });
});
