// 任务图谱偏好：纯前端偏好，localStorage 持久化。
// 图谱页只在主窗口存在，且设置页全屏接管时图谱必然已卸载——打开图谱页现读
// 即最新值，无跨窗口同步需求。图谱图例与设置页共用同一读写入口，永远同值。

import type { GraphFilters } from "../components/GraphPage/graph-build";

export type GraphLabelDensity = "sparse" | "standard" | "dense";
export type GraphEdgeWidth = "thin" | "standard" | "thick";
/** 布局松散度（设置页可切）→ 布局外径系数 k（面密度恒定公式 外径 = k·√N 的 k） */
export type GraphLooseness = "tight" | "standard" | "loose";

const LOOSENESS_VALUES = ["tight", "standard", "loose"] as const;

const K = {
  onlyMine: "wm.graph.onlyMine",
  labelDensity: "wm.graph.labelDensity",
  edgeWidth: "wm.graph.edgeWidth",
  autoLayout: "wm.graph.autoLayout",
  looseness: "wm.graph.looseness.v2",
  /** 旧三档（紧凑 20 / 标准 30 / 松散 45）的存档，只在首次读到时搬一次 */
  loosenessV1: "wm.graph.looseness",
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

function readEnum<T extends string>(
  key: string,
  allowed: readonly T[],
  fallback: T,
): T {
  const v = read(key);
  return (allowed as readonly string[]).includes(v ?? "") ? (v as T) : fallback;
}

function readBool(key: string, fallback: boolean): boolean {
  const v = read(key);
  return v === null ? fallback : v === "1";
}

// ── 只看我的任务（默认关）：打开图谱时成员过滤默认只留本人 ──
export const getGraphOnlyMine = () => readBool(K.onlyMine, false);
export const setGraphOnlyMine = (v: boolean) =>
  write(K.onlyMine, v ? "1" : "0");

// ── 标签密度（默认标准）：sparse = 仅 hub+焦点 / dense = 全部 ──
export const getGraphLabelDensity = () =>
  readEnum<GraphLabelDensity>(
    K.labelDensity,
    ["sparse", "standard", "dense"],
    "standard",
  );
export const setGraphLabelDensity = (v: GraphLabelDensity) =>
  write(K.labelDensity, v);

// ── 连线粗细（默认标准） ──
export const getGraphEdgeWidth = () =>
  readEnum<GraphEdgeWidth>(
    K.edgeWidth,
    ["thin", "standard", "thick"],
    "standard",
  );
export const setGraphEdgeWidth = (v: GraphEdgeWidth) => write(K.edgeWidth, v);

// ── 打开时自动播放布局动画（默认开）：关 = 直接给静态三维构图，「重新布局」仍可手动跑 ──
export const getGraphAutoLayout = () => readBool(K.autoLayout, true);
export const setGraphAutoLayout = (v: boolean) =>
  write(K.autoLayout, v ? "1" : "0");

// ── 布局松散度（默认标准）：紧凑 30 / 标准 45 / 宽松 70 ──
// 新旧档位不能共用一个键：「标准」这一档的 k 从 30 变成 45，同一个字符串在两种
// 含义下画出的是两张不同的图，就地改写等于把老用户的选择悄悄换掉。所以读新键、
// 没有就按旧键搬一次，搬完把旧键清掉，不留第二份真值。
const LEGACY_LOOSENESS: Record<string, GraphLooseness> = {
  compact: "tight", // 原「紧凑」(20) 已退役，并入新「紧凑」(30)
  standard: "tight", // 原「标准」(30) → 新「紧凑」(30)：画面不变
  loose: "standard", // 原「松散」(45) → 新「标准」(45)：画面不变
};

/**
 * 读松散度（纯读，不写盘）：新键优先；新键还没有时按旧键的映射给值，
 * 这样旧档位用户读到的仍是搬过去的那个档。
 */
export function getGraphLooseness(): GraphLooseness {
  if (read(K.looseness) !== null) {
    return readEnum<GraphLooseness>(K.looseness, LOOSENESS_VALUES, "standard");
  }
  const old = read(K.loosenessV1);
  const moved = old === null ? undefined : LEGACY_LOOSENESS[old];
  return moved ?? "standard";
}

/**
 * 旧档位搬迁：把旧键的值写进新键，确认写成了才删旧键，不留第二份真值。
 *
 * 显式调一次，不藏在读函数里：读函数在渲染期跑（页面与设置页都在渲染期
 * 读偏好），在那里写 localStorage 是渲染副作用。
 */
export function migrateGraphLooseness(): void {
  if (read(K.looseness) !== null) return;
  const old = read(K.loosenessV1);
  const moved = old === null ? undefined : LEGACY_LOOSENESS[old];
  if (!moved) return;
  write(K.looseness, moved);
  // 确认新值真的落盘才删旧键：write 吞掉配额异常后控制流照常往下走，
  // 先删就等于在写失败时把用户唯一那份存档删掉，且再也搬不回来
  if (read(K.looseness) === moved) {
    try {
      localStorage.removeItem(K.loosenessV1);
    } catch {
      // 删不掉就留着：新键非空时下次直接短路，不会二次搬迁
    }
  }
}
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
  // 形状损坏即自愈：清掉坏键，避免每次打开图谱都重复解析同一份脏数据
  const invalidate = () => {
    try {
      localStorage.removeItem(K.filters);
    } catch {
      // 同上：忽略
    }
  };
  let f: Partial<GraphFilters>;
  try {
    f = JSON.parse(raw) as Partial<GraphFilters>;
  } catch {
    invalidate();
    return null; // JSON 损坏 → 回退默认
  }
  if (
    !f ||
    typeof f !== "object" ||
    !f.status ||
    typeof f.status.todo !== "boolean" ||
    typeof f.status.doing !== "boolean" ||
    typeof f.status.done !== "boolean" ||
    typeof f.includeOrphans !== "boolean"
  ) {
    invalidate();
    return null;
  }
  // 元素级校验：数组元素必须全是 string，脏值降级为 null 不外溢给调用方
  const isStringArray = (x: unknown): x is string[] =>
    Array.isArray(x) && x.every((s) => typeof s === "string");
  return {
    // 逐字段重建：存档里多出来的键不往外带（status 的多余键此前整包透传）
    status: {
      todo: f.status.todo,
      doing: f.status.doing,
      done: f.status.done,
    },
    owners: isStringArray(f.owners) ? f.owners : null,
    tags: isStringArray(f.tags) ? f.tags : null,
    year: typeof f.year === "number" && Number.isFinite(f.year) ? f.year : null,
    includeOrphans: f.includeOrphans,
  };
}

export function saveGraphFilters(f: GraphFilters): void {
  write(K.filters, JSON.stringify(f));
}
