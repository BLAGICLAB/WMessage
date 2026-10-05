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
  /** 状态语义键："hub" | "doing" | "done" | "todo" | "anchor" */
  statusKey: string;
  /** 渲染半径（世界单位） */
  size: number;
  label: string;
  /** 节点语义类型（详情面板/hover 逻辑用；Sigma 只认约定属性，多余字段无害） */
  kind: "task" | "hub" | "anchor";
  status?: string;
  /** 建图时的真实度数（不含标签锚点边——reducer 标签资格判定用） */
  degree: number;
  /** 锚点节点恒 hidden（reducer/渲染层都不画） */
  hidden?: boolean;
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

/** 标签锚点边权重（> dep/member 的 1：强拉力把同组卡压过互斥聚成团） */
const TAG_EDGE_WEIGHT = 3;
/** 组内成员不足此数不建锚点（单卡标签无聚簇意义） */
const TAG_ANCHOR_MIN_CARDS = 2;
/** 锚点节点 id 前缀（事件/渲染层隔离用） */
export const TAG_ANCHOR_PREFIX = "taggrp:";

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
 *
 * 初始布局（G4-CLUSTER 分扇区 + G4-G6-r2 空间自适应）：布局外径随任务量
 * **√N 缩放（面密度恒定）**——千级任务小画布、万级任务大画布，节点密度
 * 不随规模变化，任何档位打开都是铺满视口的完整图。同标签（同义组）节点
 * 铺在专属扇区内形成聚簇初值，FA2（linLog 模式）从分团初值继续分离强化。
 * 无标签节点在扇区之间的外环环绕。锚点 = 扇区中心隐藏质点（弱边维持凝聚）。
 */
export function toGraphologyGraph(
  built: BuiltGraph,
  ownerOrder: Map<string, number>,
  tagGroups?: Map<string, string>
): Graph<SigmaNodeAttrs> {
  const graph = new Graph<SigmaNodeAttrs>({ multi: false, type: "directed" });

  // ── 分扇区初值：节点 → 组键（首个命中标签的组）→ 每组一个扇区 ──
  const groupOfNode = new Map<string, string>();
  if (tagGroups && tagGroups.size > 0) {
    for (const n of built.nodes) {
      if (n.kind !== "task") continue;
      for (const tag of n.tags ?? []) {
        const group = tagGroups.get(tag);
        if (group) {
          groupOfNode.set(n.id, group);
          break; // 多标签节点归第一个命中的组
        }
      }
    }
  }
  const groupKeys = [...new Set(groupOfNode.values())].sort();
  const sectorOf = new Map<string, { angle: number; width: number }>();
  groupKeys.forEach((key, gi) => {
    sectorOf.set(key, {
      angle: (gi / groupKeys.length) * Math.PI * 2,
      width: (Math.PI * 2) / groupKeys.length,
    });
  });
  const groupMemberCount = new Map<string, number>();
  for (const key of groupKeys) groupMemberCount.set(key, 0);
  for (const n of built.nodes) {
    const group = groupOfNode.get(n.id);
    if (group) {
      groupMemberCount.set(group, (groupMemberCount.get(group) ?? 0) + 1);
    }
  }

  // ── 空间自适应：布局外径 ∝ √任务量（面密度恒定，万级=千级的等比放大） ──
  const total = Math.max(built.nodes.length, 1);
  const R_MAX = 30 * Math.sqrt(total); // 布局外径：1227→1050，10000→3000
  const GROUP_RING = R_MAX * 0.62; // 有组扇区外径（内圈 62%，外环留给孤点）
  // 无组（孤点）数量与分布：面密度均匀圆盘填充（√ 序号铺半径，黄金角散角度）
  // ——旧公式（固定 0.68~1.0 窄环带 + %400 循环）会把小任务量的节点全挤在一圈
  const freeTotal = built.nodes.filter((n) => !groupOfNode.get(n.id)).length;

  const groupMemberIdx = new Map<string, number>();
  const golden = Math.PI * (3 - Math.sqrt(5));
  let freeIdx = 0;
  built.nodes.forEach((n) => {
    const group = groupOfNode.get(n.id);
    let x: number;
    let y: number;
    if (group) {
      // 扇区内均匀面密度铺开：半径 ∝ √(序/组员数) × 扇区外径
      const sector = sectorOf.get(group)!;
      const size = groupMemberCount.get(group) ?? 1;
      const j = groupMemberIdx.get(group) ?? 0;
      groupMemberIdx.set(group, j + 1);
      const angle = sector.angle + ((j % 12) - 5.5) * (sector.width / 14);
      const r = GROUP_RING * 0.28 * Math.sqrt((j + 0.6) / size);
      x = Math.cos(angle) * r;
      y = Math.sin(angle) * r;
    } else {
      // 无组节点：全域均匀圆盘（r ∝ √序号，黄金角散角度——铺满不挤环）
      const angle = (freeIdx + 0.5) * golden;
      const r = GROUP_RING * 0.72 * Math.sqrt((freeIdx + 0.6) / Math.max(freeTotal, 1));
      freeIdx++;
      x = Math.cos(angle) * r;
      y = Math.sin(angle) * r;
    }
    graph.addNode(n.id, {
      ownerKey: ownerColorKey(n, ownerOrder),
      statusKey: n.kind === "hub" ? "hub" : (n.status ?? "todo"),
      size: n.kind === "hub" ? 7 + Math.sqrt(n.degree) : 3 + Math.sqrt(n.degree) * 2,
      label: n.label,
      kind: n.kind,
      status: n.status,
      degree: n.degree,
      x,
      y,
    });
  });
  for (const l of built.links) {
    // graph-build 已保证两端存在；multi=false 下重复边防御
    if (!graph.hasEdge(l.source, l.target)) {
      graph.addEdge(l.source, l.target, { kind: l.kind, weight: 1 });
    }
  }
  // ── 标签锚点：扇区中心隐藏质点 + 星型弱边（维持团内凝聚） ──
  if (tagGroups && tagGroups.size > 0) {
    const membersByGroup = new Map<string, string[]>();
    for (const n of built.nodes) {
      if (n.kind !== "task") continue;
      for (const tag of n.tags ?? []) {
        const group = tagGroups.get(tag);
        if (!group) continue;
        const list = membersByGroup.get(group) ?? [];
        if (!list.includes(n.id)) list.push(n.id);
        membersByGroup.set(group, list);
      }
    }
    for (const [group, members] of membersByGroup) {
      if (members.length < TAG_ANCHOR_MIN_CARDS) continue;
      const anchorId = `${TAG_ANCHOR_PREFIX}${group}`;
      const sector = sectorOf.get(group);
      const anchorR = GROUP_RING * 0.22;
      if (!graph.hasNode(anchorId)) {
        graph.addNode(anchorId, {
          ownerKey: "self",
          statusKey: "anchor",
          // size 参与 FA2 斥力（质量）：大锚点在团间撑开距离；渲染层 hidden
          size: 8 + members.length * 0.2,
          label: group,
          kind: "anchor",
          degree: members.length,
          hidden: true,
          x: Math.cos(sector?.angle ?? 0) * anchorR,
          y: Math.sin(sector?.angle ?? 0) * anchorR,
        });
      }
      for (const memberId of members) {
        if (!graph.hasEdge(memberId, anchorId)) {
          graph.addEdge(memberId, anchorId, {
            kind: "tag",
            weight: TAG_EDGE_WEIGHT,
          });
        }
      }
    }
  }
  return graph;
}

/** FA2 设置：万节点档的常见调优（barnesHut 省斥力、权重参与吸引防 hub 吸团） */
export function fa2Settings(nodeCount: number) {
  const base = inferSettings(Math.min(nodeCount, 5000));
  return {
    ...base,
    barnesHutOptimize: nodeCount > 1500,
    // 权重参与吸引：dep/member=1 不变，标签锚点强边(3)把同组卡压成团（G4-CLUSTER）
    edgeWeightInfluence: 1,
    // LinLog 模式（Noack 能量模型）：团簇分离最大杠杆（连通团拉紧、团间推远）。
    // r1 曾因「孤点飞散」回退，r2 定位真因是布局空间不随规模缩放（固定小画布
    // 装不下万节点、密度爆炸互相挤出视野）——空间随 √N 自适应后孤点由 gravity
    // 拉向中心，不再飞散
    linLogMode: true,
    outboundAttractionDistribution: false,
    adjustSizes: true, // 碰撞分离（等价旧实现的 collide）
    slowDown: 1 + Math.min(10, nodeCount / 500),
  };
}
