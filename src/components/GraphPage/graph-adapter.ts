// graphology 适配层（G3-SIGMA 迁移，设计 docs/TASK-GRAPH-DESIGN-2026-10-05.md §3）：
// graph-build 的渲染无关建图结果 → graphology 图 + Sigma 节点属性。
// 纯函数，无 React/DOM 依赖（单测锚点）。
//
// 着色双轨（G3-SIGMA r1 修复）：建图时把 ownerKey（self/owner:N）与 statusKey
// （hub/doing/done/todo）**同时**写入节点——着色模式切换不重建图，reducer 按
// 当前模式现场选键。owner 序由调用方注入（GraphPage 基于 chips 全序计算，
// 是 chips 色点与节点色的单一事实源，不随过滤器漂移）。

import Graph from "graphology";
import { inferSettings } from "graphology-layout-forceatlas2";
import type { Attributes } from "graphology-types";
import {
  SELF_OWNER,
  type BuiltGraph,
  type GraphNode,
} from "./graph-build";

/** Sigma 节点属性（nodeProgram 渲染需要的全部字段） */
export interface SigmaNodeAttrs extends Attributes {
  /** owner 语义键："self" | "owner:N"（N = 注入序） */
  ownerKey: string;
  /** 状态语义键："hub" | "doing" | "done" | "todo" */
  statusKey: string;
  /** 渲染半径（世界单位） */
  size: number;
  label: string;
  /** 节点语义类型（详情面板/hover 逻辑用；Sigma 只认约定属性，多余字段无害） */
  kind: "task" | "hub";
  status?: string;
}

/** owner 语义键（ownerOrder 为调用方注入的单一事实源；本人不进 map → "self"） */
export function ownerColorKey(node: GraphNode, ownerOrder: Map<string, number>): string {
  const owner = node.owner ?? SELF_OWNER;
  return owner === SELF_OWNER ? "self" : `owner:${ownerOrder.get(owner) ?? 0}`;
}

/** 外来成员调色盘（下标 = 注入序；与 GraphPage chips 色点共用同一解析） */
export const OWNER_PALETTE = [
  "#7c6bd6",
  "#c2711d",
  "#0e7490",
  "#b04a8f",
  "#5a7a1f",
  "#b8434a",
  "#6b7dd6",
  "#8a6d3b",
];

export interface ColorPalette {
  brand: string;
  t3: string;
  t5: string;
  success: string;
}

/** 双模式取色（唯一入口：GraphPage chips 与 GraphCanvas reducer 共用，保证同色） */
export function resolveOwnerColor(
  ownerKey: string,
  palette: ColorPalette
): string {
  if (ownerKey === "self") return palette.brand;
  if (ownerKey.startsWith("owner:")) {
    const idx = Number(ownerKey.slice(6)) % OWNER_PALETTE.length;
    return OWNER_PALETTE[idx];
  }
  return palette.t5;
}

export function resolveStatusColor(
  statusKey: string,
  palette: ColorPalette
): string {
  switch (statusKey) {
    case "hub":
      return palette.t3;
    case "doing":
      return palette.brand;
    case "done":
      return palette.success;
    default:
      return palette.t5; // todo
  }
}

/**
 * 建图结果 → graphology 图（Sigma 直渲染）。
 * 节点初始位置：按度数分组的确定性螺旋（FA2 无所谓初值，但要非重叠可收敛）；
 * FA2 supervisor 会逐步覆写 x/y。
 */
export function toGraphologyGraph(
  built: BuiltGraph,
  ownerOrder: Map<string, number>
): Graph<SigmaNodeAttrs> {
  const graph = new Graph<SigmaNodeAttrs>({ multi: false, type: "directed" });
  const golden = Math.PI * (3 - Math.sqrt(5));
  built.nodes.forEach((n, i) => {
    const radius = 10 * Math.sqrt(i + 1);
    graph.addNode(n.id, {
      ownerKey: ownerColorKey(n, ownerOrder),
      statusKey: n.kind === "hub" ? "hub" : (n.status ?? "todo"),
      size: n.kind === "hub" ? 7 + Math.sqrt(n.degree) : 3 + Math.sqrt(n.degree) * 2,
      label: n.label,
      kind: n.kind,
      status: n.status,
      x: Math.cos(i * golden) * radius,
      y: Math.sin(i * golden) * radius,
    });
  });
  for (const l of built.links) {
    // graph-build 已保证两端存在；multi=false 下重复边防御
    if (!graph.hasEdge(l.source, l.target)) {
      graph.addEdge(l.source, l.target, { kind: l.kind });
    }
  }
  return graph;
}

/** FA2 设置：万节点档的常见调优（barnesHut 省斥力、edgeWeightInfluence 关闭防 hub 吸团） */
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
