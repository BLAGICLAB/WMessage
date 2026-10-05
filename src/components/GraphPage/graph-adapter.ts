// graphology 适配层（G3-SIGMA 迁移，设计 docs/TASK-GRAPH-DESIGN-2026-10-05.md §3）：
// graph-build 的渲染无关建图结果 → graphology 图 + Sigma 节点属性。
// 纯函数，无 React/DOM 依赖（单测锚点）。着色只产出语义键（status/owner），
// 具体色值由 GraphCanvas 从 CSS 变量运行时解析后经 Sigma reducer 下发。

import Graph from "graphology";
import { inferSettings } from "graphology-layout-forceatlas2";
import type { Attributes } from "graphology-types";
import {
  SELF_OWNER,
  type BuiltGraph,
  type GraphColorMode,
  type GraphNode,
  type OwnerChip,
} from "./graph-build";

/** Sigma 节点属性（nodeProgram 渲染需要的全部字段） */
export interface SigmaNodeAttrs extends Attributes {
  /** 语义着色键：色值在 reducer 里解析（双主题自动跟随） */
  colorKey: string;
  /** 渲染半径（世界单位；Sigma 里 size 与 camera 缩放相关，直接给半径） */
  size: number;
  label: string;
  /** 节点语义类型（详情面板/hover 逻辑用；Sigma 只认约定属性，多余字段无害） */
  kind: "task" | "hub";
  status?: string;
  /** done 淡化 / hover 邻域淡出在 reducer 层用 forceLabel/highlight 组合表达 */
  hidden?: boolean;
}

/** ownerKey → 调色盘下标（owner 着色模式；本人 = "self"） */
export function ownerColorKey(
  node: GraphNode,
  ownerOrder: Map<string, number>
): string {
  const owner = node.owner ?? SELF_OWNER;
  return owner === SELF_OWNER ? "self" : `owner:${ownerOrder.get(owner) ?? 0}`;
}

/**
 * 建图结果 → graphology 图（Sigma 直渲染）。
 * 节点初始位置：按度数分组的确定性螺旋（FA2 无所谓初值，但要非重叠可收敛）；
 * FA2 supervisor 会逐步覆写 x/y。
 */
export function toGraphologyGraph(
  built: BuiltGraph,
  colorMode: GraphColorMode,
  ownerChips: OwnerChip[]
): Graph<SigmaNodeAttrs> {
  const graph = new Graph<SigmaNodeAttrs>({ multi: false, type: "directed" });
  // owner 着色序：本人 0，外来按 chips 顺序（稳定着色）
  const ownerOrder = new Map<string, number>();
  let next = 1;
  for (const c of ownerChips) {
    if (!c.isSelf) ownerOrder.set(c.id, next++);
  }
  const golden = Math.PI * (3 - Math.sqrt(5));
  built.nodes.forEach((n, i) => {
    const radius = 10 * Math.sqrt(i + 1);
    graph.addNode(n.id, {
      colorKey:
        colorMode === "owner"
          ? ownerColorKey(n, ownerOrder)
          : n.kind === "hub"
            ? "hub"
            : (n.status ?? "todo"),
      size: n.kind === "hub" ? 7 + Math.sqrt(n.degree) : 3 + Math.sqrt(n.degree) * 2,
      label: n.label,
      kind: n.kind,
      status: n.status,
      x: Math.cos(i * golden) * radius,
      y: Math.sin(i * golden) * radius,
    });
  });
  for (const l of built.links) {
    // graph-build 已保证两端存在；addEdge 防御重复边（multi=false 抛错 → 跳过）
    if (!graph.hasEdge(l.source, l.target)) {
      graph.addEdge(l.source, l.target, { kind: l.kind });
    }
  }
  return graph;
}

/** FA2 设置：万节点档的常见调优（barnesHut 省斥力、edgeStrength 关闭防 hub 吸团） */
export function fa2Settings(nodeCount: number) {
  const base = inferSettings(Math.min(nodeCount, 5000));
  return {
    ...base,
    barnesHutOptimize: nodeCount > 1500,
    edgeWeightInfluence: 0,
    // member 边本意是松散聚类，FA2 全局统一弹簧参数——靠低 linLogMode 张力即可
    linLogMode: false,
    outboundAttractionDistribution: false,
    adjustSizes: true, // 碰撞分离（等价旧实现的 collide）
    slowDown: 1 + Math.min(10, nodeCount / 500),
  };
}
