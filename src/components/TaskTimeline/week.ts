// 周时间网格的纯换算层：日期/分钟 ↔ 网格坐标、跨日分段、重叠分栏。
// 时间格式契约与 due 一致："YYYY-MM-DDTHH:mm"（本地时区语义，无时区后缀）；
// 解析失败一律返 null 由调用方兜底（同 col/subtasks 的「损坏按空读取」契约）。

export const DAY_START_MIN = 8 * 60;
export const DAY_END_MIN = 18 * 60;
/** 可排期窗口宽度（分钟）：8:00–18:00 */
export const DAY_SPAN_MIN = DAY_END_MIN - DAY_START_MIN;
/** 拖拽吸附粒度（分钟） */
export const SNAP_MIN = 30;
/** 拖上时间轴的默认时长（分钟） */
export const DEFAULT_DURATION_MIN = 60;

/** 一周首日 = 周一（返回当日 00:00 的本地时间副本） */
export function startOfWeek(d: Date): Date {
  const out = new Date(d.getFullYear(), d.getMonth(), d.getDate());
  out.setDate(out.getDate() - ((out.getDay() + 6) % 7));
  return out;
}

export function addDays(d: Date, n: number): Date {
  const out = new Date(d);
  out.setDate(out.getDate() + n);
  return out;
}

export function isSameDay(a: Date, b: Date): boolean {
  return (
    a.getFullYear() === b.getFullYear() &&
    a.getMonth() === b.getMonth() &&
    a.getDate() === b.getDate()
  );
}

/** 30 分钟吸附（就近取整） */
export function snapMin(min: number): number {
  return Math.round(min / SNAP_MIN) * SNAP_MIN;
}

/** 落点起点钳制：夹进 [DAY_START_MIN, DAY_END_MIN − 30]，保证最短一块放得下 */
export function clampWindowMin(min: number): number {
  return Math.min(DAY_END_MIN - SNAP_MIN, Math.max(DAY_START_MIN, min));
}

const pad2 = (n: number) => String(n).padStart(2, "0");

/** 分钟（日内）→ "HH:mm" */
export function fmtMin(min: number): string {
  return `${pad2(Math.floor(min / 60))}:${pad2(min % 60)}`;
}

const PLAN_DT_RE = /^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2})$/;

/** 解析 "YYYY-MM-DDTHH:mm" → 当日本地 Date（00:00 基）+ 日内分钟；非法返 null。
 *  构造后回读校验月/日，防 Date 自动进位把 2026-02-30 悄悄变成 3 月。 */
export function parsePlanDT(s: string): { day: Date; min: number } | null {
  const m = s.match(PLAN_DT_RE);
  if (!m) return null;
  const y = Number(m[1]);
  const mo = Number(m[2]);
  const d = Number(m[3]);
  const h = Number(m[4]);
  const mi = Number(m[5]);
  if (h > 23 || mi > 59) return null;
  const day = new Date(y, mo - 1, d);
  if (
    day.getFullYear() !== y ||
    day.getMonth() !== mo - 1 ||
    day.getDate() !== d
  ) {
    return null;
  }
  return { day, min: h * 60 + mi };
}

/** 日期 + 日内分钟 → "YYYY-MM-DDTHH:mm" */
export function buildPlanDT(day: Date, min: number): string {
  return `${day.getFullYear()}-${pad2(day.getMonth() + 1)}-${pad2(day.getDate())}T${fmtMin(min)}`;
}

/** 计划在某一周内的可视分段：跨日折行 + 8:00–18:00 窗口裁剪。
 *  - 溢出窗口边界的部分贴边截断，head/tail 标记续接（渲染 »« 与开口角）；
 *  - 完全落在窗口外（如 20:00–23:00）的计划产出近端 30 分钟残段（双向 head+tail），
 *    保证「已排期」的任务不至于在网格上隐身；精确时间以详情面板为准；
 *  - 跨周部分不产出分段（翻周后可见），由周末端的 tail 指示延续。 */
export interface PlanSeg {
  /** 0..6 = 周一..周日 */
  dayIdx: number;
  startMin: number;
  endMin: number;
  /** 起点被裁：从上一日/窗口前延续进来（渲染 « 与上开口角） */
  head: boolean;
  /** 终点被裁：延续到下一日/窗口后（渲染 » 与下开口角） */
  tail: boolean;
}

export function segsForWeek(
  planStart: string,
  planEnd: string,
  weekStart: Date,
): PlanSeg[] {
  const from = parsePlanDT(planStart);
  const to = parsePlanDT(planEnd);
  if (!from || !to) return [];
  const startMs = from.day.getTime() + from.min * 60_000;
  const endMs = to.day.getTime() + to.min * 60_000;
  if (endMs <= startMs) return [];
  const segs: PlanSeg[] = [];
  for (let i = 0; i < 7; i++) {
    const day = addDays(weekStart, i);
    const winStart = day.getTime() + DAY_START_MIN * 60_000;
    const winEnd = day.getTime() + DAY_END_MIN * 60_000;
    const s = Math.max(startMs, winStart);
    const e = Math.min(endMs, winEnd);
    if (e <= s) continue;
    segs.push({
      dayIdx: i,
      startMin: Math.round((s - day.getTime()) / 60_000),
      endMin: Math.round((e - day.getTime()) / 60_000),
      head: startMs < s,
      tail: endMs > e,
    });
  }
  if (segs.length === 0) {
    // 完全在窗口外：落到计划起点所在那天（若在显示周内），近端贴 30 分钟残段
    for (let i = 0; i < 7; i++) {
      const day = addDays(weekStart, i);
      if (!isSameDay(day, from.day)) continue;
      const after = startMs >= day.getTime() + DAY_END_MIN * 60_000;
      const endMin = after ? DAY_END_MIN : DAY_START_MIN + SNAP_MIN;
      const startMin = after ? DAY_END_MIN - SNAP_MIN : DAY_START_MIN;
      segs.push({ dayIdx: i, startMin, endMin, head: true, tail: true });
      break;
    }
  }
  return segs;
}

/** 同日分段的重叠分栏（Google Calendar 式）：贪心分配 lane，重叠簇内
 *  lanes = 簇内最大 lane + 1（传递闭包：链式重叠也算同簇，栏数对齐）。 */
export function layoutLanes<T extends { startMin: number; endMin: number }>(
  segs: T[],
): Array<T & { lane: number; lanes: number }> {
  const sorted = [...segs].sort((a, b) => a.startMin - b.startMin);
  const laneEnds: number[] = [];
  const lane = sorted.map((s) => {
    let l = laneEnds.findIndex((end) => end <= s.startMin);
    if (l === -1) {
      laneEnds.push(s.endMin);
      return laneEnds.length - 1;
    }
    laneEnds[l] = s.endMin;
    return l;
  });
  // 传递重叠簇：并查集
  const parent = sorted.map((_, i) => i);
  const find = (x: number): number =>
    parent[x] === x ? x : (parent[x] = find(parent[x]));
  const union = (a: number, b: number) => {
    parent[find(a)] = find(b);
  };
  for (let i = 0; i < sorted.length; i++) {
    for (let j = i + 1; j < sorted.length; j++) {
      if (sorted[j].startMin < sorted[i].endMin) union(i, j);
    }
  }
  const clusterLanes = new Map<number, number>();
  lane.forEach((l, i) => {
    const root = find(i);
    clusterLanes.set(root, Math.max(clusterLanes.get(root) ?? 0, l + 1));
  });
  return sorted.map((s, i) => ({
    ...s,
    lane: lane[i],
    lanes: clusterLanes.get(find(i)) ?? 1,
  }));
}

/** 计划块颜色：首个标签稳定哈希 → 六色板；无标签用中性 slate。
 *  返回 CSS 变量引用（--plan-*），主题切换自动跟随。 */
const PLAN_COLORS = [
  "--plan-blue",
  "--plan-teal",
  "--plan-green",
  "--plan-amber",
  "--plan-violet",
];
export function planColorVar(tags: string[] | undefined): string {
  const first = tags?.[0];
  if (!first) return "var(--plan-slate)";
  let h = 0;
  for (let i = 0; i < first.length; i++) h = (h * 31 + first.charCodeAt(i)) >>> 0;
  return `var(${PLAN_COLORS[h % PLAN_COLORS.length]})`;
}

/** 池内时间角标文案：planStart → "三 09:30"（周一=0） */
export function planChip(planStart: string): string {
  const p = parsePlanDT(planStart);
  if (!p) return "";
  return `${"一二三四五六日"[(p.day.getDay() + 6) % 7]} ${fmtMin(p.min)}`;
}

/** 从 (day, min) 起沿「日窗口」推进 durMin 分钟：溢出 18:00 折入次日 8:00。
 *  这是「穿透日分割线」的写入侧数学——planEnd 恒为物理墙钟时间
 *  （如周三 17:30 + 60min = 周四 08:30），渲染层按窗口裁剪自然分成两段。 */
export function addWindowMinutes(
  day: Date,
  min: number,
  durMin: number,
): { day: Date; min: number } {
  let d = new Date(day);
  let cur = min;
  let rest = durMin;
  // 防脏数据死循环：窗口分钟上限 = 366 天
  for (let guard = 0; guard < 366 && rest > 0; guard++) {
    const room = DAY_END_MIN - cur;
    if (rest <= room) {
      cur += rest;
      rest = 0;
      break;
    }
    rest -= room;
    d = addDays(d, 1);
    cur = DAY_START_MIN;
  }
  return { day: d, min: cur };
}

/** 两个 planDT 之间落在日窗口内的分钟数（移动既有计划时保持「窗口时长」用）。
 *  脏数据（end <= start / 解析失败）返 null，调用方兜底 60。 */
export function windowMinutesBetween(
  planStart: string,
  planEnd: string,
): number | null {
  const from = parsePlanDT(planStart);
  const to = parsePlanDT(planEnd);
  if (!from || !to) return null;
  let startMs = from.day.getTime() + from.min * 60_000;
  const endMs = to.day.getTime() + to.min * 60_000;
  if (endMs <= startMs) return null;
  let total = 0;
  let day = from.day;
  // 单调推进日窗口，最多走 366 天（防脏数据死循环）
  for (let guard = 0; guard < 366 && startMs < endMs; guard++) {
    const winStart = day.getTime() + DAY_START_MIN * 60_000;
    const winEnd = day.getTime() + DAY_END_MIN * 60_000;
    const s = Math.max(startMs, winStart);
    const e = Math.min(endMs, winEnd);
    if (e > s) total += Math.round((e - s) / 60_000);
    startMs = Math.max(startMs, winEnd);
    day = addDays(day, 1);
  }
  return total;
}

/** 拖拽落点的最小 patch 值（纯函数，UI 只管把结果发 task_patch）。
 *  - schedule：落点起 + durMin（默认 60，跨 18:00 自动折日）；
 *  - move：保持原计划的窗口时长；
 *  - resize：end = 指针物理时刻，但不得短于起点 + 30 窗口分钟。 */
export function planPatchForDrop(
  kind: "schedule" | "move" | "resize",
  opts: {
    day: Date;
    min: number;
    durMin?: number;
    prevPlanStart?: string | null;
    prevPlanEnd?: string | null;
  },
): { planStart: string; planEnd: string } {
  const startMin = clampWindowMin(snapMin(opts.min));
  const durMin =
    opts.durMin ??
    (kind === "schedule"
      ? DEFAULT_DURATION_MIN
      : (opts.prevPlanStart && opts.prevPlanEnd
          ? windowMinutesBetween(opts.prevPlanStart, opts.prevPlanEnd)
          : null) ?? DEFAULT_DURATION_MIN);
  if (kind === "resize" && opts.prevPlanStart) {
    const from = parsePlanDT(opts.prevPlanStart);
    if (from) {
      // 指针相对起点的「窗口分钟」数（跨夜部分不计时长），钳到最短 30
      const between = windowMinutesBetween(
        opts.prevPlanStart,
        buildPlanDT(opts.day, opts.min),
      );
      const winDur = between !== null ? Math.max(SNAP_MIN, snapMin(between)) : SNAP_MIN;
      const end = addWindowMinutes(from.day, from.min, winDur);
      return { planStart: opts.prevPlanStart, planEnd: buildPlanDT(end.day, end.min) };
    }
  }
  const end = addWindowMinutes(opts.day, startMin, durMin);
  return { planStart: buildPlanDT(opts.day, startMin), planEnd: buildPlanDT(end.day, end.min) };
}
