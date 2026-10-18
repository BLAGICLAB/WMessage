// 图谱着色（标签 / 成员 / 状态三选一）：三套色板 + 唯一取色入口。
// 图例色点、筛选 chip 色点、画布节点共用同一入口——「点色块 = 点颜色」
// 必须永远成立，否则图例和画布会说两套话。
//
// 色值取自星云方案（真空黑底）：发光色在黑底上都能分辨，相互不撞色。
// 标签色板 10 色按色相均分，超过 10 个标签按注入序取模——与外来成员色板
// 同样的处理：宁可两个标签撞色，也不让同一标签在不同会话里换色。

import type { ColumnId } from "../../types";
import { SELF_OWNER, type GraphColorMode, type GraphNode } from "./graph-build";

/** 本人（成员着色下的专属色；不占 OWNER_PALETTE 下标，chips 里本人恒最前） */
export const SELF_COLOR = "#5fd3c4";

/** 外来成员调色盘（下标 = ownerOrder 注入序，GraphPage 基于 chips 全序计算） */
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

/** 标签调色盘（下标 = tagOrder 注入序，按标签使用数降序） */
export const TAG_PALETTE = [
  "#d08a2c",
  "#2a9d8f",
  "#7c6bd6",
  "#c05a9e",
  "#6b8f3a",
  "#d2564f",
  "#3f8fd0",
  "#9aa7bd",
  "#b8894a",
  "#4fb583",
];

/** 状态色：进行中发青、待办发粉、已完成压成低亮石板灰（完成了就不该抢注意力） */
export const STATUS_PALETTE: Record<ColumnId, string> = {
  todo: "#ff4d8d",
  doing: "#35d6b5",
  done: "#6b7d95",
};

/** 无标签任务的中性色（不给色板位，避免和真标签抢读） */
export const NO_TAG_COLOR = "#7d8598";

/** 钉子户：进行中且已历此天数以上——逾了期还没推动，值得单独喊一声 */
export const STALL_DAYS = 14;
export const STALL_COLOR = "#e8a33d";

/** 结构锚点（成员核 / 标签核 / 状态核）的中性色，不参与任务着色 */
export const HUB_COLOR = "#8f9bb2";

/** 成员语义键（ownerOrder 为调用方注入的单一事实源；本人不进 map → "self"） */
export function ownerColorKey(
  node: Pick<GraphNode, "owner">,
  ownerOrder: Map<string, number>,
): string {
  const owner = node.owner ?? SELF_OWNER;
  return owner === SELF_OWNER ? "self" : `owner:${ownerOrder.get(owner) ?? 0}`;
}

/**
 * 标签语义键（取首个标签）：任务标签数组的顺序即主次。多标签任务按首个标签
 * 定色，和「点标签筛选」看的是同一组标签——用户按一个色块找任务时不必先猜
 * 这张卡挂了几个标签。无标签返回 null（走中性色，不占色板位）。
 */
export function tagColorKey(
  node: GraphNode,
  tagOrder: Map<string, number>,
): number | null {
  const tag = node.tags?.[0];
  if (!tag) return null;
  return tagOrder.get(tag) ?? 0;
}

/** 节点着色所需的语义键（建图时固化，切换着色模式不重建布局） */
export interface ColorKeys {
  ownerKey: string;
  tagIdx: number | null;
  statusKey: ColumnId;
  stalled: boolean;
}

/** 三模式取色（唯一入口） */
export function resolveNodeColor(
  mode: GraphColorMode,
  k: ColorKeys,
): string {
  if (mode === "owner") {
    if (k.ownerKey === "self") return SELF_COLOR;
    const idx = Number(k.ownerKey.slice(6)) % OWNER_PALETTE.length;
    return OWNER_PALETTE[idx];
  }
  if (mode === "tag") {
    return k.tagIdx === null
      ? NO_TAG_COLOR
      : TAG_PALETTE[k.tagIdx % TAG_PALETTE.length];
  }
  return k.stalled ? STALL_COLOR : STATUS_PALETTE[k.statusKey];
}

/** 图例/筛选 chip 的成员色点（chips 全序 → 与节点同色） */
export function ownerChipColor(
  id: string,
  order: Map<string, number>,
): string {
  if (id === SELF_OWNER) return SELF_COLOR;
  return OWNER_PALETTE[(order.get(id) ?? 0) % OWNER_PALETTE.length];
}

/** 图例/筛选 chip 的标签色点（按使用数降序的注入序 → 与节点同色） */
export function tagChipColor(idx: number): string {
  return TAG_PALETTE[idx % TAG_PALETTE.length];
}
