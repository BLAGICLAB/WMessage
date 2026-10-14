// 任务可见域谓词单元测试：六口径 × 软删/归档/完成列/完成时间戳的组合边界
import { describe, it, expect } from "vitest";
import {
  isTrashed,
  isLive,
  isArchived,
  isActiveTask,
  isDoneTask,
  isPending,
} from "./taskScope";
import type { Task } from "../types";

function mkTask(over: Partial<Task>): Task {
  return {
    id: "t1",
    title: "测试卡",
    column: "todo",
    ...over,
  } as Task;
}

describe("taskScope", () => {
  it("isTrashed：有 deletedAt 即回收站，无论归档与列", () => {
    expect(isTrashed(mkTask({ deletedAt: 1000 }))).toBe(true);
    expect(isTrashed(mkTask({ deletedAt: 1000, archived: true }))).toBe(true);
    expect(isTrashed(mkTask({}))).toBe(false);
    expect(isTrashed(mkTask({ archived: true }))).toBe(false);
  });

  it("isLive：未删未归档；软删或归档任一命中即出局", () => {
    expect(isLive(mkTask({}))).toBe(true);
    expect(isLive(mkTask({ column: "done" }))).toBe(true);
    expect(isLive(mkTask({ deletedAt: 1000 }))).toBe(false);
    expect(isLive(mkTask({ archived: true }))).toBe(false);
  });

  it("isArchived：归档且未删——软删的归档卡归回收站不归归档区", () => {
    expect(isArchived(mkTask({ archived: true }))).toBe(true);
    expect(isArchived(mkTask({ archived: true, deletedAt: 1000 }))).toBe(false);
    expect(isArchived(mkTask({}))).toBe(false);
  });

  it("isActiveTask：进行区且未进完成列", () => {
    expect(isActiveTask(mkTask({ column: "todo" }))).toBe(true);
    expect(isActiveTask(mkTask({ column: "doing" }))).toBe(true);
    expect(isActiveTask(mkTask({ column: "done" }))).toBe(false);
    expect(isActiveTask(mkTask({ column: "todo", archived: true }))).toBe(false);
    expect(isActiveTask(mkTask({ column: "done", deletedAt: 1 }))).toBe(false);
  });

  it("isDoneTask：进行区且已进完成列（原位保留口径）", () => {
    expect(isDoneTask(mkTask({ column: "done" }))).toBe(true);
    expect(isDoneTask(mkTask({ column: "todo" }))).toBe(false);
    expect(isDoneTask(mkTask({ column: "done", archived: true }))).toBe(false);
  });

  it("isPending：按 completedAt 判活——完成列卡无时间戳仍算可催，有时间戳的待办卡不再催", () => {
    expect(isPending(mkTask({}))).toBe(true);
    expect(isPending(mkTask({ completedAt: 1000 }))).toBe(false);
    expect(isPending(mkTask({ column: "done" }))).toBe(true);
    expect(isPending(mkTask({ archived: true }))).toBe(false);
  });
});
