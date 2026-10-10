// graphology 适配层（Sigma 迁移）：
// graph-build 的渲染无关建图结果 → graphology 图 + Sigma 节点属性。
// 纯函数，无 React/DOM 依赖（单测锚点）。
//
// 着色双轨：建图时把 ownerKey（self/owner:N）与 statusKey
// （hub/doing/done/todo）**同时**写入节点——着色模式切换不重建图，reducer 按
// 当前模式现场选键。owner 序由调用方注入（GraphPage 基于 chips 全序计算，
// 是 chips 色点与节点色的单一事实源，不随过滤器漂移）。

import Graph from "graphology";
import { inferSettings } from "graphology-layout-forceatlas2";
import type { Attributes } from "graphology-types";
import { SELF_OWNER, type BuiltGraph, type GraphNode } from "./graph-build";

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
export function ownerColorKey(
  node: GraphNode,
  ownerOrder: Map<string, number>,
): string {
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

/** 节点大小语义（图谱图例可切换）：degree = 连接度（√度数，Obsidian 经典隐喻）；
 *  duration = 耗时（完成−创建天数；doing 用已进行天数——拖得越久越大，钉子户可视化） */
export type GraphSizeMode = "degree" | "duration";

/** 布局松散度（设置页可切）→ R_MAX 系数 k（面密度恒定公式 R_MAX = k·√N 的 k） */
export type GraphLooseness = "compact" | "standard" | "loose";
const LOOSENESS_R_MAX: Record<GraphLooseness, number> = {
  compact: 20,
  standard: 30,
  loose: 45,
};

const DAY_MS = 86_400_000;

/** 任务耗时天数：done = 完成−创建；doing = 现在−创建；todo 或缺创建时间（老数据）= null */
export function durationDaysOf(
  n: Pick<GraphNode, "status" | "createdAt" | "completedAt">,
  now: number,
): number | null {
  if (!n.createdAt) return null;
  if (n.status === "done")
    return n.completedAt
      ? Math.max(0, (n.completedAt - n.createdAt) / DAY_MS)
      : null;
  if (n.status === "doing") return Math.max(0, (now - n.createdAt) / DAY_MS);
  return null;
}

/**
 * 节点半径。duration 模式：3 + 1.5·√天数、15 封顶（平方根压缩——当天≈3、
 * 3 天≈5.6、2 周≈8.6、1 月≈11.2、半年起封顶；天/月/年量纲差异大，线性会失控）。
 * 封顶也护住 FA2 adjustSizes：size 参与质量/碰撞，巨点会把周围推开过远。
 */
export function nodeSize(
  n: GraphNode,
  mode: GraphSizeMode,
  now: number,
): number {
  if (n.kind === "hub") return 7 + Math.sqrt(n.degree);
  if (mode === "degree") return 3 + Math.sqrt(n.degree) * 2;
  const days = durationDaysOf(n, now);
  if (days === null) return 3;
  return Math.min(15, 3 + 1.5 * Math.sqrt(days));
}

export interface ColorPalette {
  brand: string;
  t3: string;
  t5: string;
  success: string;
}

/** 双模式取色（唯一入口：GraphPage chips 与 GraphCanvas reducer 共用，保证同色） */
export function resolveOwnerColor(
  ownerKey: string,
  palette: ColorPalette,
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
  palette: ColorPalette,
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
 * 初始布局（分扇区 + 空间自适应）：布局外径随任务量
 * **√N 缩放（面密度恒定）**——千级任务小画布、万级任务大画布，节点密度
 * 不随规模变化，任何档位打开都是铺满视口的完整图。同标签（同义组）节点
 * 铺在专属扇区内形成聚簇初值，FA2（linLog 模式）从分团初值继续分离强化。
 * 无标签节点在扇区之间的外环环绕。锚点 = 扇区中心隐藏质点（弱边维持凝聚）。
 */
export function toGraphologyGraph(
  built: BuiltGraph,
  ownerOrder: Map<string, number>,
  tagGroups?: Map<string, string>,
  sizeMode: GraphSizeMode = "degree",
  looseness: GraphLooseness = "standard",
  now = Date.now(),
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
  const R_MAX = LOOSENESS_R_MAX[looseness] * Math.sqrt(total); // 布局外径：1227→1050，10000→3000（标准档）
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
      const r =
        GROUP_RING * 0.72 * Math.sqrt((freeIdx + 0.6) / Math.max(freeTotal, 1));
      freeIdx++;
      x = Math.cos(angle) * r;
      y = Math.sin(angle) * r;
    }
    graph.addNode(n.id, {
      ownerKey: ownerColorKey(n, ownerOrder),
      statusKey: n.kind === "hub" ? "hub" : (n.status ?? "todo"),
      size: nodeSize(n, sizeMode, now),
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
      // 扇区键来自 groupOfNode（首-match 组）；组若只经次标签入组（不是任何节点
      // 的首-match 组）就没有扇区——锚点落到 angle 0 会叠进第 0 扇区，这类组不建锚点
      if (!sector) continue;
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
          x: Math.cos(sector.angle) * anchorR,
          y: Math.sin(sector.angle) * anchorR,
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
    // 权重参与吸引：dep/member=1 不变，标签锚点强边(3)把同组卡压成团
    edgeWeightInfluence: 1,
    // linLogMode **永久禁用**（两次真机回归定性）：LinLog 能量模型会把连通团
    // 无限坍缩成一个点、无连线孤点推到无穷远——与「所有任务可见铺开」的验收
    // 目标数学上不可调和。团簇由分扇区初值 + 锚点弱边保证，标准模式足够
    linLogMode: false,
    outboundAttractionDistribution: false,
    adjustSizes: true, // 碰撞分离（等价旧实现的 collide）
    slowDown: 1 + Math.min(10, nodeCount / 500),
  };
}

// ── 视野适配（归一化相机空间） ──
// Sigma v3 的相机不在图坐标系工作：节点坐标先经 normalizationFunction 映射到以
// (0.5,0.5) 为中心的归一化空间（nx = 0.5 + (x−cX)/R，R = 参考 bbox 最大跨度），
// 相机 x/y/ratio 都是该空间的值（matrixFromCamera：
// clip = (p−cam)/ratio × 2·(min(w,h)−2·stagePadding)/dim × correctionRatio）。
// 把原始图坐标直接写进相机会在小图上把内容推到视口外数万像素（画布全空）。
// 参考 bbox 由调用方在建图时用 setCustomBBox 钉死（否则 FA2 每批坐标更新都会触发
// sigma process() 用膨胀后的 bbox 重基归一化，参考系持续漂移）。

/** Sigma 默认 stagePadding（autoRescale 下生效） */
const SIGMA_STAGE_PADDING = 30;

/** 归一化参考 bbox（sigma customBBox 的形状） */
export interface NormExtent {
  x: [number, number];
  y: [number, number];
}

/** 图坐标包围盒 */
export interface ContentBBox {
  minX: number;
  maxX: number;
  minY: number;
  maxY: number;
}

/** 图节点坐标 → 参考 bbox（与 sigma graphExtent 同口径：全节点、含锚点） */
export function graphBBox(
  graph: Pick<Graph<SigmaNodeAttrs>, "forEachNode">,
): NormExtent {
  let minX = Infinity,
    maxX = -Infinity,
    minY = Infinity,
    maxY = -Infinity;
  graph.forEachNode((_, a) => {
    if (!Number.isFinite(a.x) || !Number.isFinite(a.y)) return;
    if (a.x < minX) minX = a.x;
    if (a.x > maxX) maxX = a.x;
    if (a.y < minY) minY = a.y;
    if (a.y > maxY) maxY = a.y;
  });
  if (!Number.isFinite(minX)) return { x: [0, 1], y: [0, 1] };
  return { x: [minX, maxX], y: [minY, maxY] };
}

/**
 * 计算「把内容包围盒铺满视口」的相机态（归一化空间）。
 * 返回 null = 视口退化/输入非有限——调用方不得写相机（写 Infinity/NaN ratio
 * 会把全部节点投影到同一屏幕点）。ratio 下限 0.05 防病态过放大。
 */
export function computeCameraFit(
  bbox: ContentBBox,
  norm: NormExtent,
  viewport: { width: number; height: number },
  margin = 1.12,
): { x: number; y: number; ratio: number } | null {
  const { width, height } = viewport;
  const smallest = Math.min(width, height) - 2 * SIGMA_STAGE_PADDING;
  if (!Number.isFinite(width) || !Number.isFinite(height) || smallest <= 0) {
    return null;
  }
  const normR = Math.max(norm.x[1] - norm.x[0], norm.y[1] - norm.y[0]);
  const spanX = bbox.maxX - bbox.minX;
  const spanY = bbox.maxY - bbox.minY;
  const nums = [
    normR,
    spanX,
    spanY,
    bbox.minX,
    bbox.maxX,
    bbox.minY,
    bbox.maxY,
  ];
  if (normR <= 0 || nums.some((v) => !Number.isFinite(v))) return null;
  // correctionRatio：与 sigma getCorrectionRatio 同式，graphDims 取参考 bbox 维度
  const gw = norm.x[1] - norm.x[0] || 1;
  const gh = norm.y[1] - norm.y[0] || 1;
  const viewportRatio = height / width;
  const graphRatio = gh / gw;
  const cr =
    (viewportRatio < 1 && graphRatio > 1) ||
    (viewportRatio > 1 && graphRatio < 1)
      ? 1
      : Math.min(
          Math.max(graphRatio, 1 / graphRatio),
          Math.max(1 / viewportRatio, viewportRatio),
        );
  const ratio = Math.max(
    (spanX / normR) * (smallest / width) * cr,
    (spanY / normR) * (smallest / height) * cr,
  );
  if (!Number.isFinite(ratio)) return null;
  return {
    x:
      0.5 + ((bbox.minX + bbox.maxX) / 2 - (norm.x[0] + norm.x[1]) / 2) / normR,
    y:
      0.5 + ((bbox.minY + bbox.maxY) / 2 - (norm.y[0] + norm.y[1]) / 2) / normR,
    ratio: Math.max(0.05, ratio * margin),
  };
}
