import { describe, it, expect } from "vitest";
import type { Task } from "../../types";
import type { TraceRow } from "../../lib/trace";
import {
  dailyTrend,
  dayKey,
  dueSoonTasks,
  dueTimeOf,
  greetingOf,
  planPhases,
  todayPlanTasks,
} from "./homeData";

// 固定时钟：2026-10-11（周日）15:42
const NOW = new Date(2026, 9, 11, 15, 42);
const DAY = 86_400_000;

function task(partial: Partial<Task> & { id: string }): Task {
  return { column: "todo", ...partial } as Task;
}

function trace(partial: Partial<TraceRow> & { id: number }): TraceRow {
  return {
    sessionId: "s",
    origin: "manual",
    status: "done",
    startedAt: NOW.getTime(),
    turnCount: 1,
    toolCalls: 0,
    filesChanged: 0,
    promptTokens: 0,
    completionTokens: 0,
    ...partial,
  };
}

describe("dayKey", () => {
  it("按本地时区输出 YYYY-MM-DD", () => {
    expect(dayKey(new Date(2026, 0, 2))).toBe("2026-01-02");
  });
});

describe("todayPlanTasks", () => {
  it("只留今天有排期的任务并按开始时间升序，排除已删", () => {
    const tasks = [
      task({ id: "b", planStart: "2026-10-11T14:00", planEnd: "2026-10-11T15:00" }),
      task({ id: "a", planStart: "2026-10-11T09:00", planEnd: "2026-10-11T10:00" }),
      task({ id: "y", planStart: "2026-10-10T09:00" }),
      task({ id: "del", planStart: "2026-10-11T11:00", deletedAt: 1 }),
    ];
    const got = todayPlanTasks(tasks, NOW);
    expect(got.map((t) => t.id)).toEqual(["a", "b"]);
  });
});

describe("planPhases", () => {
  it("按 completedAt/doing 列分三态", () => {
    const tasks = [
      task({ id: "1", planStart: "2026-10-11T09:00", completedAt: 1 }),
      task({ id: "2", planStart: "2026-10-11T10:00", column: "doing" }),
      task({ id: "3", planStart: "2026-10-11T11:00" }),
      task({ id: "4", planStart: "2026-10-12T11:00" }),
    ];
    expect(planPhases(tasks, NOW)).toEqual({ done: 1, doing: 1, todo: 1 });
  });
});

describe("dueTimeOf", () => {
  it("纯日期按当天 23:59，日期时间精确到分，无 due 为 null", () => {
    expect(dueTimeOf(task({ id: "1", due: "2026-10-11" }))).toBe(
      new Date(2026, 9, 11, 23, 59, 59).getTime(),
    );
    expect(dueTimeOf(task({ id: "1", due: "2026-10-11T18:00" }))!).toBe(
      new Date(2026, 9, 11, 18, 0).getTime(),
    );
    expect(dueTimeOf(task({ id: "1" }))).toBeNull();
  });
});

describe("dueSoonTasks", () => {
  it("含逾期与 7 天内截止，排除已完成/归档/已删，按截止升序", () => {
    const tasks = [
      task({ id: "far", due: "2026-10-25" }),
      task({ id: "fri", due: "2026-10-16" }),
      task({ id: "over", due: "2026-10-10" }),
      task({ id: "done", due: "2026-10-12", completedAt: 1 }),
      task({ id: "arch", due: "2026-10-12", archived: true }),
      task({ id: "del", due: "2026-10-12", deletedAt: 1 }),
    ];
    const got = dueSoonTasks(tasks, NOW);
    expect(got.map((t) => t.id)).toEqual(["over", "fri"]);
    expect(
      dueSoonTasks(tasks, new Date(NOW.getTime() + 20 * DAY)).map((t) => t.id),
    ).toEqual(["over", "fri", "far"]);
  });
});

describe("dailyTrend", () => {
  it("7 个桶：今天标「今天」，其余按星期；完成与机器人执行按天计数", () => {
    const tasks = [
      task({ id: "1", completedAt: NOW.getTime() }),
      task({ id: "2", completedAt: NOW.getTime() - 1 * DAY }),
      task({ id: "3", completedAt: NOW.getTime() - 20 * DAY }),
    ];
    const traces = [
      trace({ id: 1, startedAt: NOW.getTime() }),
      trace({ id: 2, startedAt: NOW.getTime() }),
      trace({ id: 3, startedAt: NOW.getTime() - 2 * DAY }),
    ];
    const got = dailyTrend(tasks, traces, NOW, 7);
    expect(got).toHaveLength(7);
    expect(got[6].label).toBe("今天");
    expect(got[5].label).toBe("周六");
    expect(got[6].completed).toBe(1);
    expect(got[5].completed).toBe(1);
    expect(got[4].botRuns).toBe(1);
    expect(got[6].botRuns).toBe(2);
  });
});

describe("greetingOf", () => {
  it("按小时分段", () => {
    expect(greetingOf(new Date(2026, 9, 11, 4, 0))).toBe("夜深了");
    expect(greetingOf(new Date(2026, 9, 11, 8, 0))).toBe("早上好");
    expect(greetingOf(new Date(2026, 9, 11, 10, 0))).toBe("上午好");
    expect(greetingOf(new Date(2026, 9, 11, 13, 0))).toBe("中午好");
    expect(greetingOf(new Date(2026, 9, 11, 15, 42))).toBe("下午好");
    expect(greetingOf(new Date(2026, 9, 11, 21, 0))).toBe("晚上好");
  });
});
