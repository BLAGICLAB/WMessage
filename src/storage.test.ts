import { describe, it, expect, vi, beforeEach } from "vitest";
import {
  loadTasksFromDb,
  upsertTasks,
  deleteTaskRows,
  upsertWorkspaceItems,
  deleteWorkspaceRows,
} from "./storage";
import type { Task, WorkspaceItem } from "./types";

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

// E1（2026-08-19）：写路径不再 { silent: true } 静默吞错——
// 失败必须 alert 提示用户并把错误抛给调用方，否则 UI 已改而磁盘没落，UI/DB 永久分叉。
describe("写路径错误传播（E1）", () => {
  const alertMock = vi.fn();
  const task: Task = { id: "t1", title: "x", column: "todo" };
  const item: WorkspaceItem = { id: "w1", title: "w", collapsed: false, links: [] };
  const dbErr = { code: "DB_ERROR", message: "disk full", recoverable: false };

  beforeEach(() => {
    invokeMock.mockReset();
    alertMock.mockClear();
    window.alert = alertMock;
  });

  it("upsertTasks：db_upsert 失败 → alert + 错误传播（不静默吞）", async () => {
    invokeMock.mockRejectedValue(dbErr);
    await expect(upsertTasks([task])).rejects.toBe(dbErr);
    expect(alertMock).toHaveBeenCalled();
  });

  it("deleteTaskRows：db_delete 失败 → alert + 错误传播", async () => {
    invokeMock.mockRejectedValue(dbErr);
    await expect(deleteTaskRows(["t1"])).rejects.toBe(dbErr);
    expect(alertMock).toHaveBeenCalled();
  });

  it("upsertWorkspaceItems：workspace_upsert 失败 → alert + 错误传播", async () => {
    invokeMock.mockRejectedValue(dbErr);
    await expect(upsertWorkspaceItems([item])).rejects.toBe(dbErr);
    expect(alertMock).toHaveBeenCalled();
  });

  it("deleteWorkspaceRows：workspace_delete 失败 → alert + 错误传播", async () => {
    invokeMock.mockRejectedValue(dbErr);
    await expect(deleteWorkspaceRows(["w1"])).rejects.toBe(dbErr);
    expect(alertMock).toHaveBeenCalled();
  });

  it("写成功时不弹 alert、不抛错", async () => {
    invokeMock.mockResolvedValue(null);
    await expect(upsertTasks([task])).resolves.toBeUndefined();
    await expect(deleteTaskRows(["t1"])).resolves.toBeUndefined();
    expect(alertMock).not.toHaveBeenCalled();
  });

  it("空数组直接返回，不触发 invoke", async () => {
    await upsertTasks([]);
    await deleteTaskRows([]);
    await upsertWorkspaceItems([]);
    await deleteWorkspaceRows([]);
    expect(invokeMock).not.toHaveBeenCalled();
  });
});
