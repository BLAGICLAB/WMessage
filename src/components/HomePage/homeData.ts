// 首页仪表盘的数据聚合（纯函数，不碰 invoke/state，方便单测与复用）。
// 数据源：任务（主窗口 tasks 全量，含归档）+ 执行痕迹（trace_list）。
// 口径与周时间网格对齐：planStart/planEnd 本地时区、日界 8:00–24:00。

import type { Task } from "../../types";
import type { TraceRow } from "../../lib/trace";

/** 本地日期键 YYYY-MM-DD（与 App.tsx 的 localDateStr 同口径） */
export function dayKey(d: Date): string {
  const m = String(d.getMonth() + 1).padStart(2, "0");
  const day = String(d.getDate()).padStart(2, "0");
  return `${d.getFullYear()}-${m}-${day}`;
}

/** planStart/planEnd 的日期部分（格式 "YYYY-MM-DD" 或 "YYYY-MM-DDTHH:mm"） */
function planDayOf(v: string): string {
  return v.slice(0, 10);
}

/** 今天有排期的任务（排除已删；按开始时间升序） */
export function todayPlanTasks(tasks: Task[], now: Date): Task[] {
  const today = dayKey(now);
  return tasks
    .filter((t) => !t.deletedAt && t.planStart && planDayOf(t.planStart) === today)
    .sort((a, b) => (a.planStart! < b.planStart! ? -1 : 1));
}

export type PlanPhases = { done: number; doing: number; todo: number };

/** 今日排期三态：已完成（有 completedAt）/ 进行中（doing 列）/ 待开始 */
export function planPhases(tasks: Task[], now: Date): PlanPhases {
  const phases: PlanPhases = { done: 0, doing: 0, todo: 0 };
  for (const t of todayPlanTasks(tasks, now)) {
    if (t.completedAt) phases.done++;
    else if (t.column === "doing") phases.doing++;
    else phases.todo++;
  }
  return phases;
}

export type PoolCounts = { todo: number; done: number; archived: number };

/** 三个池子的存量：待办（未完成未归档）/ 完成（已完成未归档）/ 归档 */
export function poolCounts(tasks: Task[]): PoolCounts {
  const c: PoolCounts = { todo: 0, done: 0, archived: 0 };
  for (const t of tasks) {
    if (t.deletedAt) continue;
    if (t.archived) c.archived++;
    else if (t.completedAt) c.done++;
    else c.todo++;
  }
  return c;
}

/** 截止时间戳：纯日期按当天 23:59 计，日期时间精确到分；无 due 返回 null */
export function dueTimeOf(t: Task): number | null {
  if (!t.due) return null;
  const [date, time] = t.due.split("T");
  const [y, m, d] = date.split("-").map(Number);
  if (time === undefined) return new Date(y, m - 1, d, 23, 59, 59).getTime();
  const [hh, mm] = time.split(":").map(Number);
  return new Date(y, m - 1, d, hh, mm).getTime();
}

/** 即将到期的时间窗（含逾期）：7 天内会撞线的事 */
const DUE_HORIZON_DAYS = 7;

/** 即将到期：未完成、未归档未删，截止在今天以前 ~ 未来 horizon 天内，按截止升序 */
export function dueSoonTasks(tasks: Task[], now: Date): Task[] {
  const horizon = now.getTime() + DUE_HORIZON_DAYS * 86_400_000;
  return tasks
    .filter((t) => !t.completedAt && !t.archived && !t.deletedAt)
    .map((t) => ({ t, due: dueTimeOf(t) }))
    .filter((x): x is { t: Task; due: number } => x.due !== null)
    .filter((x) => x.due <= horizon)
    .sort((a, b) => a.due - b.due)
    .map((x) => x.t);
}

export type DayBucket = {
  /** 本地日期键（桶定位用） */
  key: string;
  /** 图表 x 轴短标：今天 / 周一… */
  label: string;
  /** tooltip 用完整日期（M月D日） */
  date: string;
  completed: number;
  botRuns: number;
};

const WEEKDAY = ["周日", "周一", "周二", "周三", "周四", "周五", "周六"];

/** 近 n 天趋势桶：每日完成任务数（completedAt）+ 机器人执行次数（全部来源 trace） */
export function dailyTrend(
  tasks: Task[],
  traces: TraceRow[],
  now: Date,
  days = 7,
): DayBucket[] {
  const start = new Date(now);
  start.setHours(0, 0, 0, 0);
  start.setDate(start.getDate() - (days - 1));
  const buckets: DayBucket[] = [];
  const idxOf = new Map<string, number>();
  for (let i = 0; i < days; i++) {
    const d = new Date(start);
    d.setDate(d.getDate() + i);
    const key = dayKey(d);
    idxOf.set(key, i);
    buckets.push({
      key,
      label: key === dayKey(now) ? "今天" : WEEKDAY[d.getDay()],
      date: `${d.getMonth() + 1}月${d.getDate()}日`,
      completed: 0,
      botRuns: 0,
    });
  }
  const countAt = (ms: number, field: "completed" | "botRuns") => {
    const i = idxOf.get(dayKey(new Date(ms)));
    if (i !== undefined) buckets[i][field]++;
  };
  for (const t of tasks) {
    if (t.completedAt) countAt(t.completedAt, "completed");
  }
  for (const tr of traces) {
    countAt(tr.startedAt, "botRuns");
  }
  return buckets;
}

/** 按时段问候（凌晨/早上/上午/中午/下午/晚上） */
export type WeekBucket = {
  key: string;
  /** 周一..周日 */
  label: string;
  /** tooltip 用完整日期（M月D日） */
  date: string;
  /** 当日排期数（planStart 落在当天，未删） */
  planned: number;
  /** 当日完成数（completedAt 落在当天） */
  done: number;
  isToday: boolean;
};

/** 本周（周一为起点）逐日「计划/完成」桶，供首页周柱状图 */
export function weeklyBuckets(tasks: Task[], now: Date): WeekBucket[] {
  const start = new Date(now);
  start.setHours(0, 0, 0, 0);
  start.setDate(start.getDate() - ((start.getDay() + 6) % 7));
  const todayKey = dayKey(now);
  const buckets: WeekBucket[] = [];
  for (let i = 0; i < 7; i++) {
    const d = new Date(start);
    d.setDate(d.getDate() + i);
    buckets.push({
      key: dayKey(d),
      label: WEEKDAY[d.getDay()],
      date: `${d.getMonth() + 1}月${d.getDate()}日`,
      planned: 0,
      done: 0,
      isToday: dayKey(d) === todayKey,
    });
  }
  const idxOf = new Map(buckets.map((b, i) => [b.key, i]));
  for (const t of tasks) {
    if (t.deletedAt) continue;
    if (t.planStart) {
      const i = idxOf.get(planDayOf(t.planStart));
      if (i !== undefined) buckets[i].planned++;
    }
    if (t.completedAt) {
      const i = idxOf.get(dayKey(new Date(t.completedAt)));
      if (i !== undefined) buckets[i].done++;
    }
  }
  return buckets;
}

export function greetingOf(now: Date): string {
  const h = now.getHours();
  if (h < 5) return "夜深了";
  if (h < 9) return "早上好";
  if (h < 12) return "上午好";
  if (h < 14) return "中午好";
  if (h < 18) return "下午好";
  return "晚上好";
}
