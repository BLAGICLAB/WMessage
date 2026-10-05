// 任务图谱偏好（G7-SETTINGS）：纯前端偏好，localStorage 持久化。
// 图谱页只在主窗口存在，且设置页全屏接管时图谱必然已卸载——打开图谱页现读
// 即最新值，无跨 webview 同步需求（workflowVisibility 的 Tauri emit 是为了
// 挂件窗，这里不需要）。sizeMode 键由 G7-SIZEMODE 图谱图例切换先行落地，
// 此处收为同一读写入口（图谱图例与设置页共用，永远同值）。

import type { GraphFilters } from "../components/GraphPage/graph-build";
import type { GraphSizeMode, GraphLooseness } from "../components/GraphPage/graph-adapter";

export type { GraphLooseness };

export type GraphLabelDensity = "sparse" | "standard" | "dense";
export type GraphEdgeWidth = "thin" | "standard" | "thick";

const K = {
  onlyMine: "wm.graph.onlyMine",
  sizeMode: "wm.graph.sizeMode",
  labelDensity: "wm.graph.labelDensity",
  edgeWidth: "wm.graph.edgeWidth",
  autoLayout: "wm.graph.autoLayout",
  looseness: "wm.graph.looseness",
  rememberFilters: "wm.graph.rememberFilters",
  filters: "wm.graph.filters",
} as const;

function read(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null; // 隐私模式等：全部回退默认值
  }
}

function write(key: string, value: string): void {
  try {
    localStorage.setItem(key, value);
  } catch {
    // 写失败 = 会话内生效即可
  }
}

function readEnum<T extends string>(key: string, allowed: readonly T[], fallback: T): T {
  const v = read(key);
  return (allowed as readonly string[]).includes(v ?? "") ? (v as T) : fallback;
}

function readBool(key: string, fallback: boolean): boolean {
  const v = read(key);
  return v === null ? fallback : v === "1";
}

// ── 只看我的任务（默认关）：打开图谱时成员过滤默认只留本人 ──
export const getGraphOnlyMine = () => readBool(K.onlyMine, false);
export const setGraphOnlyMine = (v: boolean) => write(K.onlyMine, v ? "1" : "0");

// ── 节点大小语义（默认连接度） ──
export const getGraphSizeMode = () =>
  readEnum<GraphSizeMode>(K.sizeMode, ["degree", "duration"], "degree");
export const setGraphSizeMode = (v: GraphSizeMode) => write(K.sizeMode, v);

// ── 标签密度（默认标准）：sparse = 仅 hub+焦点 / dense = 全部 ──
export const getGraphLabelDensity = () =>
  readEnum<GraphLabelDensity>(K.labelDensity, ["sparse", "standard", "dense"], "standard");
export const setGraphLabelDensity = (v: GraphLabelDensity) => write(K.labelDensity, v);

// ── 连线粗细（默认标准） ──
export const getGraphEdgeWidth = () =>
  readEnum<GraphEdgeWidth>(K.edgeWidth, ["thin", "standard", "thick"], "standard");
export const setGraphEdgeWidth = (v: GraphEdgeWidth) => write(K.edgeWidth, v);

// ── 打开时自动播放布局动画（默认开）：关 = 静态分扇区布局，「重新布局」仍可手动跑 ──
export const getGraphAutoLayout = () => readBool(K.autoLayout, true);
export const setGraphAutoLayout = (v: boolean) => write(K.autoLayout, v ? "1" : "0");

// ── 布局松散度（默认标准） ──
export const getGraphLooseness = () =>
  readEnum<GraphLooseness>(K.looseness, ["compact", "standard", "loose"], "standard");
export const setGraphLooseness = (v: GraphLooseness) => write(K.looseness, v);

// ── 记住上次的过滤器（默认关）：关掉时清掉已存的过滤器（语义诚实：不再记住） ──
export const getGraphRememberFilters = () => readBool(K.rememberFilters, false);
export function setGraphRememberFilters(v: boolean): void {
  write(K.rememberFilters, v ? "1" : "0");
  if (!v) {
    try {
      localStorage.removeItem(K.filters);
    } catch {
      // 同上：忽略
    }
  }
}

/** 读取记住的过滤器（形状校验：缺关键字段视为无效 → null 回退默认） */
export function loadGraphFilters(): GraphFilters | null {
  const raw = read(K.filters);
  if (!raw) return null;
  try {
    const f = JSON.parse(raw) as Partial<GraphFilters>;
    if (
      !f ||
      typeof f !== "object" ||
      !f.status ||
      typeof f.status.todo !== "boolean" ||
      typeof f.status.doing !== "boolean" ||
      typeof f.status.done !== "boolean" ||
      typeof f.includeOrphans !== "boolean"
    ) {
      return null;
    }
    return {
      status: f.status as GraphFilters["status"],
      owners: Array.isArray(f.owners) ? (f.owners as string[]) : null,
      tags: Array.isArray(f.tags) ? (f.tags as string[]) : null,
      workflowIds: Array.isArray(f.workflowIds) ? (f.workflowIds as string[]) : null,
      year: typeof f.year === "number" ? f.year : null,
      includeOrphans: f.includeOrphans,
    };
  } catch {
    return null; // JSON 损坏 → 回退默认
  }
}

export function saveGraphFilters(f: GraphFilters): void {
  write(K.filters, JSON.stringify(f));
}
