// 归档规则单元测试：天数钳制 + 内存缓存 + applyArchiveRule 按配置阈值归档
import { describe, it, expect, beforeEach, vi } from "vitest";
import {
  DEFAULT_ARCHIVE_DAYS,
  MIN_ARCHIVE_DAYS,
  MAX_ARCHIVE_DAYS,
  clampArchiveDays,
  getArchiveAfterDays,
  setArchiveAfterDays,
  applyArchiveRule,
} from "./archiveRule";
import type { Task } from "../types";

// invoke 失败路径（loadArchiveDaysFromConfig 的 catch）不打扰断言
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(async () => {
    throw new Error("no backend in unit tests");
  }),
}));

const day = 24 * 60 * 60 * 1000;

describe("clampArchiveDays", () => {
  it("合法值原样（取整）", () => {
    expect(clampArchiveDays(7)).toBe(7);
    expect(clampArchiveDays(30.4)).toBe(30);
  });
  it("越界钳到 1..=365", () => {
    expect(clampArchiveDays(0)).toBe(MIN_ARCHIVE_DAYS);
    expect(clampArchiveDays(-3)).toBe(MIN_ARCHIVE_DAYS);
    expect(clampArchiveDays(1000)).toBe(MAX_ARCHIVE_DAYS);
  });
  it("NaN/Infinity 非有限值回退默认 7", () => {
    expect(clampArchiveDays(NaN)).toBe(DEFAULT_ARCHIVE_DAYS);
    expect(clampArchiveDays(Infinity)).toBe(DEFAULT_ARCHIVE_DAYS);
  });
});

describe("getArchiveAfterDays / setArchiveAfterDays", () => {
  beforeEach(() => {
    setArchiveAfterDays(null); // 复位到默认
  });

  it("默认 7 天", () => {
    expect(getArchiveAfterDays()).toBe(DEFAULT_ARCHIVE_DAYS);
  });

  it("null/undefined → 默认 7；数字 → 钳制后生效", () => {
    setArchiveAfterDays(null);
    expect(getArchiveAfterDays()).toBe(7);
    setArchiveAfterDays(undefined);
    expect(getArchiveAfterDays()).toBe(7);
    setArchiveAfterDays(30);
    expect(getArchiveAfterDays()).toBe(30);
    setArchiveAfterDays(0); // 非法 → 钳到 1
    expect(getArchiveAfterDays()).toBe(1);
  });
});

describe("applyArchiveRule", () => {
  beforeEach(() => {
    setArchiveAfterDays(null);
  });

  const doneTask = (completedAt: number): Task => ({
    id: "t1",
    title: "完成的任务",
    column: "done",
    completedAt,
  });

  it("默认阈值：完成满 7 天 → 归档，不满 → 不归档", () => {
    const now = Date.now();
    const tasks = [doneTask(now - 8 * day), doneTask(now - 6 * day)];
    const next = applyArchiveRule(tasks);
    expect(next[0].archived).toBe(true);
    expect(next[1].archived).toBeUndefined();
  });

  it("阈值改为 30 天：8 天前完成的不再归档（配置即时生效）", () => {
    setArchiveAfterDays(30);
    const now = Date.now();
    const next = applyArchiveRule([doneTask(now - 8 * day)]);
    expect(next[0].archived).toBeUndefined();
    // 31 天前 → 照常归档
    const next2 = applyArchiveRule([doneTask(now - 31 * day)]);
    expect(next2[0].archived).toBe(true);
  });

  it("只动 done 列：todo/doing/已归档/已删除的卡不碰", () => {
    setArchiveAfterDays(1);
    const now = Date.now();
    const tasks: Task[] = [
      { id: "a", title: "待办", column: "todo", completedAt: now - 10 * day },
      {
        id: "b",
        title: "进行中",
        column: "doing",
        completedAt: now - 10 * day,
      },
      {
        id: "c",
        title: "已归档",
        column: "done",
        completedAt: now - 10 * day,
        archived: true,
      },
      {
        id: "d",
        title: "回收站",
        column: "done",
        completedAt: now - 10 * day,
        deletedAt: now,
      },
    ];
    const next = applyArchiveRule(tasks);
    // a/b 未到阈值不动；c 已归档保持；d 回收站不碰
    expect(next.map((t) => t.archived ?? false)).toEqual([
      false,
      false,
      true,
      false,
    ]);
  });

  it("老数据缺 completedAt：补当前时间（不立即归档）", () => {
    setArchiveAfterDays(7);
    const next = applyArchiveRule([
      { id: "old", title: "老卡", column: "done" },
    ]);
    expect(next[0].completedAt).toBeTypeOf("number");
    expect(next[0].archived).toBeUndefined();
  });
});
