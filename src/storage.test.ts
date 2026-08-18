import { describe, it, expect, vi, beforeEach } from "vitest";
import { loadTasksFromDb } from "./storage";
import type { Task } from "./types";

const invokeMock = vi.hoisted(() => vi.fn());

vi.mock("@tauri-apps/api/core", () => ({
  invoke: invokeMock,
}));

describe("loadTasksFromDb", () => {
  beforeEach(() => {
    invokeMock.mockReset();
  });

  it("invoke 正常返回时包装为 { ok: true, tasks }", async () => {
    const tasks: Task[] = [{ id: "t1", title: "x", column: "todo" }];
    invokeMock.mockResolvedValue(tasks);
    const res = await loadTasksFromDb();
    expect(res).toEqual({ ok: true, tasks });
  });

  it("invoke 抛错时返回 { ok: false, error }（区分读失败与空库）", async () => {
    const err = new Error("database is locked");
    invokeMock.mockRejectedValue(err);
    const res = await loadTasksFromDb();
    expect(res.ok).toBe(false);
    if (!res.ok) expect(res.error).toBe(err);
  });

  it("空库（invoke 返回 []）返回 ok 载荷而非 error", async () => {
    invokeMock.mockResolvedValue([]);
    const res = await loadTasksFromDb();
    expect(res).toEqual({ ok: true, tasks: [] });
  });
});
