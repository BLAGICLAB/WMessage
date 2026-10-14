// 首页 KPI 指标单源：8 项口径的纯计算 + 涨跌对比（较上一次变动前的本地快照）
// + 展示选择（8 选 4，数组顺序即展示顺序）。
// 数据源 = App 全量下发的 tasks（含归档/回收站）。任务间关联 = parentId（父子）
// 与 dependsOn（依赖），与图谱 buildTaskGraph 同口径：两端都未删才成边；
// 工作流成员关系连的是 hub 不是「别的任务」，不计入大型任务。
// 选择与快照是主窗口本地偏好，localStorage 持久化（同 bubbleStyle 模式），
// 设置页改动经自定义事件即时同步首页。

import type { Task } from "../types";

export type KpiId =
  | "completionRate"
  | "largeCount"
  | "largeRatio"
  | "largeDoneRate"
  | "onTimeRate"
  | "overdueCount"
  | "planRate"
  | "shelfDuration";

export interface KpiDef {
  id: KpiId;
  name: string;
  /** 口径说明（设置面板就地展示） */
  desc: string;
  unit: "pct" | "count" | "duration";
  /** 涨跌的「好方向」：up=涨为绿 / down=跌为绿；null=结构性指标，中性灰 */
  goodWhen: "up" | "down" | null;
}

export const HOME_KPIS: readonly KpiDef[] = [
  {
    id: "completionRate",
    name: "任务完成率",
    desc: "已完成任务 ÷ 全部有效任务（未删，含归档）×100%",
    unit: "pct",
    goodWhen: "up",
  },
  {
    id: "largeCount",
    name: "大型任务数",
    desc: "与其他任务存在关联（依赖 / 父子）的任务个数，有任一关联只算一次",
    unit: "count",
    goodWhen: null,
  },
  {
    id: "largeRatio",
    name: "大型任务占比",
    desc: "大型任务数 ÷ 全部有效任务 ×100%",
    unit: "pct",
    goodWhen: null,
  },
  {
    id: "largeDoneRate",
    name: "大型任务完成率",
    desc: "已完成的大型任务 ÷ 全部大型任务 ×100%",
    unit: "pct",
    goodWhen: "up",
  },
  {
    id: "onTimeRate",
    name: "按期完成率",
    desc: "有截止时间的任务里，完成时间不晚于截止的占比",
    unit: "pct",
    goodWhen: "up",
  },
  {
    id: "overdueCount",
    name: "逾期任务数",
    desc: "已过截止时间仍未完成的任务数（进行区内）",
    unit: "count",
    goodWhen: "down",
  },
  {
    id: "planRate",
    name: "计划完成率",
    desc: "同时有排期开始与结束的任务里，完成时间不晚于排期结束的占比",
    unit: "pct",
    goodWhen: "up",
  },
  {
    id: "shelfDuration",
    name: "平均搁置时长",
    desc: "同时有排期起止的任务，从创建到排期开始的平均等待时长",
    unit: "duration",
    goodWhen: "down",
  },
];

export const kpiDefById: Map<KpiId, KpiDef> = new Map(
  HOME_KPIS.map((d) => [d.id, d]),
);

/** 默认展示 4 项 = 口径清单前四（用户未选择时的初始值） */
export const HOME_KPI_DEFAULT_SELECTION: readonly KpiId[] = [
  "completionRate",
  "largeCount",
  "largeRatio",
  "largeDoneRate",
];

export type KpiValues = Record<KpiId, number | null>;

/** 首页顶栏单个 KPI 块：口径定义 + 当前值 + 涨跌基线（上一次变动前） */
export interface HomeKpiBlock {
  def: KpiDef;
  value: number | null;
  prev: number | null;
}

/** "YYYY-MM-DD" 按当天 23:59、"YYYY-MM-DDTHH:mm" 精确到分（与
 *  homeData.dueTimeOf 同口径；本文件在 lib 层不反向依赖组件目录） */
function parseLocalTime(v: string): number {
  const [date, time] = v.split("T");
  const [y, m, d] = date.split("-").map(Number);
  if (time === undefined) return new Date(y, m - 1, d, 23, 59, 59).getTime();
  const [hh, mm] = time.split(":").map(Number);
  return new Date(y, m - 1, d, hh, mm).getTime();
}

/** 分母为 0 的比率返回 null（前端展示「—」，不参与涨跌） */
function ratio(num: number, den: number): number | null {
  return den > 0 ? (num / den) * 100 : null;
}

export function computeHomeKpis(tasks: Task[], now: Date): KpiValues {
  const valid = tasks.filter((t) => !t.deletedAt);
  const byId = new Map(valid.map((t) => [t.id, t]));
  const completed = valid.filter((t) => t.completedAt != null);

  const completionRate = ratio(completed.length, valid.length);

  // 大型任务：任一有效关联（父子/依赖，双向）命中即记一次
  const linked = new Set<string>();
  for (const t of valid) {
    if (t.parentId && byId.has(t.parentId)) {
      linked.add(t.id);
      linked.add(t.parentId);
    }
    for (const d of t.dependsOn ?? []) {
      if (byId.has(d)) {
        linked.add(t.id);
        linked.add(d);
      }
    }
  }
  const largeCount = linked.size;
  const largeRatio = ratio(largeCount, valid.length);
  const largeDone = completed.filter((t) => linked.has(t.id)).length;
  const largeDoneRate = ratio(largeDone, largeCount);

  // 逾期：进行区内（未完成未归档未删）且截止时刻已过
  const nowMs = now.getTime();
  const overdueCount = valid.filter(
    (t) =>
      !t.completedAt &&
      !t.archived &&
      !!t.due &&
      parseLocalTime(t.due) < nowMs,
  ).length;

  const withDue = valid.filter((t) => !!t.due);
  const onTime = withDue.filter(
    (t) => t.completedAt != null && t.completedAt <= parseLocalTime(t.due!),
  ).length;
  const onTimeRate = ratio(onTime, withDue.length);

  const withPlan = valid.filter((t) => !!t.planStart && !!t.planEnd);
  const planDone = withPlan.filter(
    (t) =>
      t.completedAt != null &&
      t.completedAt <= parseLocalTime(t.planEnd!),
  ).length;
  const planRate = ratio(planDone, withPlan.length);

  // 搁置：创建 → 排期开始的等待；补排期（创建晚于排期开始）按 0 计
  let shelfTotal = 0;
  let shelfN = 0;
  for (const t of withPlan) {
    if (t.createdAt == null) continue;
    shelfTotal += Math.max(0, parseLocalTime(t.planStart!) - t.createdAt);
    shelfN++;
  }
  const shelfDuration = shelfN > 0 ? shelfTotal / shelfN : null;

  return {
    completionRate,
    largeCount,
    largeRatio,
    largeDoneRate,
    onTimeRate,
    overdueCount,
    planRate,
    shelfDuration,
  };
}

function trimOneDecimal(v: number): string {
  const s = v.toFixed(1);
  return s.endsWith(".0") ? s.slice(0, -2) : s;
}

/** 时长人类可读：分钟（<1h）/ 小时（<1d）/ 天 */
export function formatKpiDuration(ms: number): string {
  const min = ms / 60_000;
  if (min < 60) return `${Math.round(min)} 分钟`;
  const h = min / 60;
  if (h < 24) return `${trimOneDecimal(h)} 小时`;
  return `${trimOneDecimal(h / 24)} 天`;
}

export function formatKpiValue(def: KpiDef, v: number | null): string {
  if (def.unit === "count") return String(v ?? 0);
  if (v == null) return "—";
  if (def.unit === "pct") return `${trimOneDecimal(v)}%`;
  return formatKpiDuration(v);
}

export type KpiTrend = {
  dir: "up" | "down";
  tone: "good" | "bad" | "neutral";
};

/** 较上次变动前快照的涨跌；无基线或数值未变 → null（不显示角标） */
export function kpiTrendOf(
  def: KpiDef,
  v: number | null,
  prev: number | null,
): KpiTrend | null {
  if (v == null || prev == null) return null;
  const d = v - prev;
  if (Math.abs(d) < 1e-9) return null;
  const dir = d > 0 ? "up" : "down";
  const tone =
    def.goodWhen === null ? "neutral" : def.goodWhen === dir ? "good" : "bad";
  return { dir, tone };
}

/** 角标 tooltip / 无障碍文案：较上次记录的增减量（带符号与单位） */
export function kpiTrendTitle(def: KpiDef, v: number, prev: number): string {
  const d = v - prev;
  const abs =
    def.unit === "pct"
      ? `${trimOneDecimal(Math.abs(d))}%`
      : def.unit === "count"
        ? String(Math.abs(Math.round(d)))
        : formatKpiDuration(Math.abs(d));
  return `较上次变动 ${d > 0 ? "+" : "−"}${abs}`;
}

// ── 涨跌基线快照：last=最近一次记录，prev=上一次变动前的数值 ──

export interface KpiSnapshot {
  last: KpiValues;
  prev: KpiValues | null;
}

function sameValues(a: KpiValues, b: KpiValues): boolean {
  return HOME_KPIS.every(({ id }) => a[id] === b[id]);
}

/** 滚动快照：数值没变 → 原样沿用（next=null 表示无需写盘）；
 *  变了 → 旧 last 升格为 prev（「上一次变动前」） */
export function rollKpiSnapshot(
  stored: KpiSnapshot | null,
  current: KpiValues,
): { prev: KpiValues | null; next: KpiSnapshot | null } {
  if (stored && sameValues(stored.last, current)) {
    return { prev: stored.prev, next: null };
  }
  const prev = stored?.last ?? null;
  return { prev, next: { last: current, prev } };
}

const SEL_KEY = "wm.home.kpis";
const SNAP_KEY = "wm.home.kpi.snapshot";
/** 同文档（设置页 → 首页）的选择变更广播；storage 事件只跨文档收不到 */
const CHANGE_EVENT = "wmessage:home-kpi-changed";

const ALL_IDS: readonly string[] = HOME_KPIS.map((d) => d.id);

function normalizeSelection(raw: unknown): KpiId[] {
  const arr = Array.isArray(raw)
    ? raw.filter(
        (x): x is KpiId => typeof x === "string" && ALL_IDS.includes(x),
      )
    : [];
  const dedup = [...new Set(arr)];
  return dedup.length ? dedup.slice(0, 4) : [...HOME_KPI_DEFAULT_SELECTION];
}

export function getHomeKpiSelection(): KpiId[] {
  try {
    const raw = localStorage.getItem(SEL_KEY);
    return raw
      ? normalizeSelection(JSON.parse(raw))
      : [...HOME_KPI_DEFAULT_SELECTION];
  } catch {
    return [...HOME_KPI_DEFAULT_SELECTION];
  }
}

/** 保存展示选择（上限 4，非法项剔除），返回归一后的结果并广播首页刷新 */
export function setHomeKpiSelection(sel: KpiId[]): KpiId[] {
  const next = normalizeSelection(sel);
  try {
    localStorage.setItem(SEL_KEY, JSON.stringify(next));
  } catch {
    /* 写失败 = 会话内生效即可 */
  }
  window.dispatchEvent(new Event(CHANGE_EVENT));
  return next;
}

export function subscribeHomeKpiSelection(cb: () => void): () => void {
  window.addEventListener(CHANGE_EVENT, cb);
  return () => window.removeEventListener(CHANGE_EVENT, cb);
}

function coerceValues(v: unknown): KpiValues | null {
  if (!v || typeof v !== "object") return null;
  const o = v as Record<string, unknown>;
  const out = {} as KpiValues;
  for (const { id } of HOME_KPIS) {
    const x = o[id];
    if (x !== null && typeof x !== "number") return null;
    out[id] = typeof x === "number" ? x : null;
  }
  return out;
}

export function readKpiSnapshot(): KpiSnapshot | null {
  try {
    const raw = localStorage.getItem(SNAP_KEY);
    if (!raw) return null;
    const o = JSON.parse(raw) as { last?: unknown; prev?: unknown };
    const last = coerceValues(o.last);
    if (!last) return null;
    return { last, prev: coerceValues(o.prev) };
  } catch {
    return null;
  }
}

export function writeKpiSnapshot(s: KpiSnapshot): void {
  try {
    localStorage.setItem(SNAP_KEY, JSON.stringify(s));
  } catch {
    /* 写失败放弃本次基线，下次变动重记 */
  }
}
