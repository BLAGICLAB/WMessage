import { describe, it, expect, beforeEach, vi } from "vitest";
import type { Task } from "../types";
import {
  computeHomeKpis,
  formatKpiDuration,
  formatKpiValue,
  getHomeKpiSelection,
  HOME_KPIS,
  HOME_KPI_DEFAULT_SELECTION,
  kpiDefById,
  kpiTrendOf,
  kpiTrendTitle,
  readKpiSnapshot,
  rollKpiSnapshot,
  setHomeKpiSelection,
  subscribeHomeKpiSelection,
  writeKpiSnapshot,
  type KpiDef,
  type KpiId,
} from "./homeKpi";

// 固定时钟：2026-10-11（周日）15:42
const NOW = new Date(2026, 9, 11, 15, 42);
const H = 3_600_000;
const DAY = 86_400_000;

function t(partial: Partial<Task> & { id: string }): Task {
  return { column: "todo", ...partial } as Task;
}

beforeEach(() => {
  localStorage.clear();
});

describe("computeHomeKpis · 任务完成率 / 大型任务占比", () => {
  it("有效 = 未删（含归档），分母 0 → null", () => {
    const tasks = [
      t({ id: "d1", completedAt: 1 }),
      t({ id: "d2", completedAt: 2, archived: true }),
      t({ id: "open" }),
      t({ id: "del", completedAt: 3, deletedAt: 9 }),
    ];
    const got = computeHomeKpis(tasks, NOW);
    expect(got.completionRate).toBeCloseTo(66.666, 2);
    expect(got.largeCount).toBe(0);
    expect(got.largeRatio).toBe(0); // 有效任务 3 个，0/3 = 0（分母 0 才是 null）
    expect(computeHomeKpis([], NOW).completionRate).toBeNull();
    expect(computeHomeKpis([], NOW).largeRatio).toBeNull();
  });

  it("大型任务：父子/依赖双向都算，多关联只算一次，悬空与已删关联不成边", () => {
    const tasks = [
      t({ id: "mom" }),
      t({ id: "kid", parentId: "mom" }),
      t({ id: "a", dependsOn: ["b"] }),
      t({ id: "b", completedAt: 5, column: "done" }),
      t({ id: "multi", dependsOn: ["b", "a", "ghost"] }),
      t({ id: "dead", dependsOn: ["a"], deletedAt: 5 }),
      t({ id: "lone" }),
    ];
    const got = computeHomeKpis(tasks, NOW);
    // linked = mom,kid,a,b,multi；有效 6 个
    expect(got.largeCount).toBe(5);
    expect(got.largeRatio).toBeCloseTo(83.333, 2);
    expect(got.largeDoneRate).toBe(20);
  });
});

describe("computeHomeKpis · 截止与排期", () => {
  it("按期完成率：纯日期截止按 23:59:59，迟到不算，无截止不入册", () => {
    const tasks = [
      t({ id: "ok", due: "2026-10-10T18:00", completedAt: new Date(2026, 9, 10, 17).getTime() }),
      t({ id: "edge", due: "2026-10-10", completedAt: new Date(2026, 9, 10, 23, 59, 59).getTime() }),
      t({ id: "late", due: "2026-10-10", completedAt: new Date(2026, 9, 11, 10).getTime() }),
      t({ id: "pend", due: "2026-10-10" }),
      t({ id: "future", due: "2026-10-12" }),
      t({ id: "noDue", completedAt: 1 }),
    ];
    // 分母 5（有 due），按期 2
    expect(computeHomeKpis(tasks, NOW).onTimeRate).toBeCloseTo(40, 6);
  });

  it("逾期任务数：过截止且未完成的进行区卡；已归档未完成不算", () => {
    const tasks = [
      t({ id: "pend", due: "2026-10-10" }),
      t({ id: "pendTime", due: "2026-10-11T15:00" }),
      t({ id: "future", due: "2026-10-12" }),
      t({ id: "lateDone", due: "2026-10-09", completedAt: new Date(2026, 9, 10).getTime() }),
      t({ id: "archPend", due: "2026-10-09", archived: true }),
      t({ id: "delPend", due: "2026-10-09", deletedAt: 1 }),
    ];
    expect(computeHomeKpis(tasks, NOW).overdueCount).toBe(2);
  });

  it("计划完成率：须同时有排期起止；完成 ≤ 排期结束才算", () => {
    const tasks = [
      t({ id: "in", planStart: "2026-10-01T09:00", planEnd: "2026-10-11T18:00", completedAt: new Date(2026, 9, 11, 12).getTime() }),
      t({ id: "out", planStart: "2026-10-01T09:00", planEnd: "2026-10-02T09:00", completedAt: new Date(2026, 9, 3, 9).getTime() }),
      t({ id: "noStart", planEnd: "2026-10-06T09:00", completedAt: 1 }),
      t({ id: "open", planStart: "2026-10-01T09:00", planEnd: "2026-10-02T09:00" }),
    ];
    expect(computeHomeKpis(tasks, NOW).planRate).toBeCloseTo(33.333, 2);
  });

  it("平均搁置时长：创建→排期开始的均值；补排期按 0；缺创建时间不入均值", () => {
    const tasks = [
      t({
        id: "wait",
        planStart: "2026-10-01T10:00",
        planEnd: "2026-10-02T10:00",
        createdAt: new Date(2026, 9, 1, 9).getTime(), // 搁置 1h
      }),
      t({
        id: "back",
        planStart: "2026-10-01T09:00",
        planEnd: "2026-10-02T09:00",
        createdAt: new Date(2026, 9, 1, 11).getTime(), // 创建晚于排期开始 → 0
      }),
      t({ id: "noCreated", planStart: "2026-10-01T09:00", planEnd: "2026-10-02T09:00" }),
    ];
    expect(computeHomeKpis(tasks, NOW).shelfDuration).toBe(H / 2);
    expect(computeHomeKpis([t({ id: "x" })], NOW).shelfDuration).toBeNull();
  });
});

describe("formatKpiValue / formatKpiDuration", () => {
  function def(id: KpiId): KpiDef {
    const d = kpiDefById.get(id);
    if (!d) throw new Error(`unknown kpi: ${id}`);
    return d;
  }
  it("pct 一位小数去尾 0；count 整数；null → —", () => {
    expect(formatKpiValue(def("completionRate"), 66.666)).toBe("66.7%");
    expect(formatKpiValue(def("completionRate"), 50)).toBe("50%");
    expect(formatKpiValue(def("completionRate"), null)).toBe("—");
    expect(formatKpiValue(def("largeCount"), null)).toBe("0");
  });
  it("时长：分钟 / 小时 / 天", () => {
    expect(formatKpiDuration(20 * 60_000)).toBe("20 分钟");
    expect(formatKpiDuration(90 * 60_000)).toBe("1.5 小时");
    expect(formatKpiDuration(3 * DAY)).toBe("3 天");
  });
});

describe("kpiTrendOf / kpiTrendTitle", () => {
  function def(id: KpiId): KpiDef {
    const d = kpiDefById.get(id);
    if (!d) throw new Error(`unknown kpi: ${id}`);
    return d;
  }
  it("涨跌只给方向（红涨绿跌在渲染层映射），未变/无基线 → null", () => {
    expect(kpiTrendOf(80, 70)).toEqual({ dir: "up" });
    expect(kpiTrendOf(5, 3)).toEqual({ dir: "up" });
    expect(kpiTrendOf(10, 5)).toEqual({ dir: "up" });
    expect(kpiTrendOf(70, 80)).toEqual({ dir: "down" });
    expect(kpiTrendOf(80, 80)).toBeNull();
    expect(kpiTrendOf(80, null)).toBeNull();
  });
  it("增减量文案带符号与单位", () => {
    expect(kpiTrendTitle(def("completionRate"), 71.428, 70)).toBe("较上次变动 +1.4%");
    expect(kpiTrendTitle(def("overdueCount"), 3, 5)).toBe("较上次变动 −2");
    expect(kpiTrendTitle(def("shelfDuration"), 2 * DAY, DAY)).toBe("较上次变动 +1 天");
  });
});

describe("rollKpiSnapshot", () => {
  const cur = computeHomeKpis([], NOW);
  it("首次记录：无 prev，产出新快照", () => {
    const { prev, next } = rollKpiSnapshot(null, cur);
    expect(prev).toBeNull();
    expect(next).toEqual({ last: cur, prev: null });
  });
  it("数值未变：沿用原快照不重写（next=null）", () => {
    const stored = { last: cur, prev: null };
    const { prev, next } = rollKpiSnapshot(stored, computeHomeKpis([], NOW));
    expect(next).toBeNull();
    expect(prev).toBeNull();
  });
  it("数值变了：旧 last 升格为 prev（上一次变动前）", () => {
    const old = { ...cur, overdueCount: 9 };
    const stored = { last: old, prev: null };
    const { prev, next } = rollKpiSnapshot(stored, cur);
    expect(prev).toEqual(old);
    expect(next).toEqual({ last: cur, prev: old });
  });
});

describe("展示选择（localStorage + 同文档广播）", () => {
  it("未存储时给默认 4 项；保存后读回一致", () => {
    expect(getHomeKpiSelection()).toEqual([...HOME_KPI_DEFAULT_SELECTION]);
    const next = setHomeKpiSelection(["onTimeRate", "overdueCount"]);
    expect(next).toEqual(["onTimeRate", "overdueCount"]);
    expect(getHomeKpiSelection()).toEqual(["onTimeRate", "overdueCount"]);
  });
  it("归一：剔非法、去重、截断到 4；清空回默认", () => {
    expect(setHomeKpiSelection(["onTimeRate", "bogus" as never, "onTimeRate"]).length).toBe(1);
    expect(setHomeKpiSelection(HOME_KPIS.map((d) => d.id)).length).toBe(4);
    expect(setHomeKpiSelection([])).toEqual([...HOME_KPI_DEFAULT_SELECTION]);
  });
  it("保存广播 CHANGE_EVENT，订阅方收到", () => {
    const cb = vi.fn();
    const un = subscribeHomeKpiSelection(cb);
    setHomeKpiSelection(["planRate", "shelfDuration", "onTimeRate", "overdueCount"]);
    expect(cb).toHaveBeenCalledTimes(1);
    un();
    setHomeKpiSelection(["planRate"]);
    expect(cb).toHaveBeenCalledTimes(1);
  });
});

describe("快照读写（脏数据拒收）", () => {
  it("roundtrip + 非法形状返回 null", () => {
    const snap = { last: computeHomeKpis([], NOW), prev: null };
    writeKpiSnapshot(snap);
    expect(readKpiSnapshot()).toEqual(snap);
    localStorage.setItem("wm.home.kpi.snapshot", JSON.stringify({ last: { largeCount: "x" } }));
    expect(readKpiSnapshot()).toBeNull();
  });
});
