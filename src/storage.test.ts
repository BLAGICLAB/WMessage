import { describe, it, expect, vi, beforeEach } from "vitest";
import {
  loadTasksFromDb,
  upsertTasks,
  deleteTaskRows,
  upsertWorkspaceItems,
  deleteWorkspaceRows,
  exportTasksToFile,
  importTasksFromFile,
  diffTaskRows,
  assignInsertOrder,
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

// 写路径不再 { silent: true } 静默吞错——
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

// 拖拽排序只改 order，不得刷新 updatedAt——
// 否则多客户端按 updatedAt 合并时排序写互相覆盖，顺序来回乱跳。
describe("diffTaskRows 纯排序保留 updatedAt（P2-20）", () => {
  const base: Task[] = [0, 1, 2, 3].map((i) => ({
    id: `t${i}`,
    title: `任务${i}`,
    column: "todo",
    order: i,
    updatedAt: 1000 + i, // 老时间戳：拖拽后必须原样保留
  }));

  it("4 个 task 拖拽重排后 updatedAt 不变，仅 order 进 upserts", () => {
    // 模拟把 t0 拖到末尾（arrayMove 等价：手动重排 + assignInsertOrder 分配 order）
    const moved = [base[1], base[2], base[3], base[0]];
    const next = assignInsertOrder(moved, "t0");
    const { upserts, deletes } = diffTaskRows(base, next, 999999);
    expect(deletes).toEqual([]);
    // 只有被拖的 t0 order 变了 → 只有它进 upserts
    expect(upserts.map((t) => t.id)).toEqual(["t0"]);
    // 关键断言：纯排序变更不刷新 updatedAt（修复前会被刷成 999999）
    expect(upserts[0].updatedAt).toBe(1000);
    expect(upserts[0].order).toBe(4); // 末尾邻居 3 + 1
    // 未动的行不进 upserts，原数组对象不被修改
    expect(base.every((t, i) => t.updatedAt === 1000 + i)).toBe(true);
  });

  it("间隙耗尽全量整数重排：所有行 order 变化但 updatedAt 全部保留", () => {
    // 构造浮点间隙耗尽：lo/hi 差 ≤ 1e-9 → assignInsertOrder 全量重排
    const tight: Task[] = [
      { ...base[0], order: 0 },
      { ...base[1], order: 1e-10 },
      base[2],
      base[3],
    ];
    // 把 t3 插到 t0/t1 之间（间隙 1e-10 < 1e-9，触发全量重排）
    const moved = [tight[0], tight[3], tight[1], tight[2]];
    const next = assignInsertOrder(moved, "t3");
    const { upserts } = diffTaskRows(tight, next, 999999);
    // 全量重排 → t3/t1/t2 的 order 变了进 upserts（t0 重排后 order 仍为 0，不进）
    expect(upserts.map((t) => t.id).sort()).toEqual(["t1", "t2", "t3"]);
    // 但没有任何一行的 updatedAt 被刷新（修复前全部变 999999 → 多端乱序之源）
    for (const t of upserts) {
      const orig = tight.find((x) => x.id === t.id)!;
      expect(t.updatedAt).toBe(orig.updatedAt);
    }
  });

  it("内容变更照常打 updatedAt=now（不受排序豁免影响）", () => {
    const next = base.map((t) =>
      t.id === "t1" ? { ...t, title: "改名了" } : t
    );
    const { upserts } = diffTaskRows(base, next, 999999);
    expect(upserts).toHaveLength(1);
    expect(upserts[0].updatedAt).toBe(999999);
  });

  it("新增行（无 prev）照常打 updatedAt=now；删除行进 deletes", () => {
    const next = [...base, { id: "t9", title: "新", column: "todo" as const }];
    const { upserts } = diffTaskRows(base, next, 999999);
    expect(upserts.map((t) => t.id)).toEqual(["t9"]);
    expect(upserts[0].updatedAt).toBe(999999);
    const { deletes: del2 } = diffTaskRows(next, base, 999999);
    expect(del2).toEqual(["t9"]);
  });
});

// RMW 写回带基线 expectedUpdatedAt = 快照行 updatedAt——
// 后端 upsert 写前比对现行行，不一致拒写（防两写者读同一快照后交错整行覆盖）。
describe("diffTaskRows 携带 RMW 写回基线（T1-1）", () => {
  const base: Task[] = [
    { id: "t1", title: "甲", column: "todo", order: 0, updatedAt: 1000 },
    { id: "t2", title: "乙", column: "todo", order: 1, updatedAt: 1001 },
  ];

  it("内容变更行：expectedUpdatedAt = prev.updatedAt，updatedAt 刷 now", () => {
    const next = base.map((t) =>
      t.id === "t1" ? { ...t, title: "甲改" } : t
    );
    const { upserts } = diffTaskRows(base, next, 999999);
    expect(upserts).toHaveLength(1);
    expect(upserts[0].expectedUpdatedAt).toBe(1000);
    expect(upserts[0].updatedAt).toBe(999999);
    // prev 快照对象不被基线赋值污染
    expect(base[0].expectedUpdatedAt).toBeUndefined();
  });

  it("纯排序行：updatedAt 保留且同样带基线（排序写撞内容修改也要拒写）", () => {
    const next = assignInsertOrder([base[1], base[0]], "t1");
    const { upserts } = diffTaskRows(base, next, 999999);
    expect(upserts).toHaveLength(1);
    expect(upserts[0].updatedAt).toBe(1000); // 纯排序豁免不破
    expect(upserts[0].expectedUpdatedAt).toBe(1000);
  });

  it("新增行不带基线（无快照可比对，走原时间戳守卫）", () => {
    const next = [...base, { id: "t9", title: "新", column: "todo" as const }];
    const { upserts } = diffTaskRows(base, next, 999999);
    expect(upserts[0].expectedUpdatedAt).toBeUndefined();
  });

  it("prev 残留脏基线不击穿 taskEq 比较（基线是传输元数据非内容）", () => {
    // state 里的 prev 可能带着上次 diff 写入的 expectedUpdatedAt（对象复用）——
    // 不得因此把无变化行误判为变更
    const dirtyPrev: Task[] = base.map((t) => ({ ...t, expectedUpdatedAt: 12345 }));
    const { upserts } = diffTaskRows(dirtyPrev, base.map((t) => ({ ...t })), 999999);
    expect(upserts).toHaveLength(0);
  });
});

// 导出/导入同属写数据类——invoke reject 必须弹 alert，不得静默吞。
describe("导出/导入失败弹 alert（P2-34）", () => {
  const alertMock = vi.fn();
  const ioErr = { code: "IO_ERROR", message: "permission denied", recoverable: false };

  beforeEach(() => {
    invokeMock.mockReset();
    alertMock.mockClear();
    window.alert = alertMock;
  });

  it("tasks_export 失败 → alert 出现", async () => {
    invokeMock.mockRejectedValue(ioErr);
    const n = await exportTasksToFile("/tmp/x.json");
    expect(n).toBe(0);
    expect(alertMock).toHaveBeenCalledTimes(1);
    expect(alertMock.mock.calls[0][0]).toContain("permission denied");
  });

  it("tasks_import 失败 → alert 出现", async () => {
    invokeMock.mockRejectedValue(ioErr);
    const n = await importTasksFromFile("/tmp/x.json");
    expect(n).toBe(0);
    expect(alertMock).toHaveBeenCalledTimes(1);
  });
});
