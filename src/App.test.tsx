import { describe, it, expect, vi, beforeEach } from "vitest";
import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import App from "./App";
import type { Task } from "./types";

// vi.mock 工厂会被提升到顶部，因此共享 mock 变量必须用 vi.hoisted 包裹
const mocks = vi.hoisted(() => {
  const invokeMock = vi.fn();
  const listenMock = vi.fn();
  const openMock = vi.fn();
  const saveMock = vi.fn();
  return { invokeMock, listenMock, openMock, saveMock };
});

const defaultInvokeImpl = async (cmd: string) => {
  if (cmd === "db_load") return []; // 空库 → 走 SEED 注入
  if (cmd === "db_upsert") return null;
  if (cmd === "db_delete") return null;
  if (cmd === "workspace_load") return []; // 工作区空列表
  if (cmd === "workspace_upsert") return null;
  if (cmd === "workspace_delete") return null;
  if (cmd === "tasks_export") return 3;
  return null;
};
mocks.invokeMock.mockImplementation(defaultInvokeImpl);
mocks.listenMock.mockImplementation(async () => () => {});
mocks.openMock.mockImplementation(async () => null);
mocks.saveMock.mockImplementation(async () => null);

vi.mock("@tauri-apps/api/core", () => ({
  invoke: mocks.invokeMock,
  convertFileSrc: vi.fn((p: string) => `tauri://localhost/${p}`),
}));

vi.mock("@tauri-apps/api/event", () => ({
  emit: vi.fn(async () => {}),
  listen: mocks.listenMock,
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: mocks.openMock,
  save: mocks.saveMock,
}));

vi.mock("@tauri-apps/plugin-opener", () => ({
  openPath: vi.fn(async () => {}),
}));

// alert / confirm：vitest 用 spy 替身，避免 jsdom 弹原生弹框卡住测试
const alertMock = vi.fn();
const confirmMock = vi.fn(() => true);
beforeEach(() => {
  mocks.invokeMock.mockClear();
  mocks.invokeMock.mockImplementation(defaultInvokeImpl); // 恢复默认实现（个别用例会覆盖）
  mocks.listenMock.mockClear();
  mocks.openMock.mockClear();
  mocks.saveMock.mockClear();
  alertMock.mockClear();
  confirmMock.mockClear();
  // 直接覆盖 window 上的方法（jsdom 已实现 alert/confirm；stub 一下）
  window.alert = alertMock;
  window.confirm = confirmMock;
});

describe("App", () => {
  it("初始渲染：看板视图 + 注入种子任务（3 个 seed）", async () => {
    render(<App />);
    // 等待 db_load → upsertTasks(SEED) → setTasks
    await waitFor(() => {
      expect(mocks.invokeMock).toHaveBeenCalledWith("db_load");
    });
    // 看板三列：待办/今日/完成（KanbanBoard 渲染了三个列头）
    expect(screen.getByText("待办")).toBeInTheDocument();
    expect(screen.getByText("今日")).toBeInTheDocument();
    expect(screen.getByText("完成")).toBeInTheDocument();
    // 种子标题
    expect(screen.getByText("梳理 WMessage 需求清单")).toBeInTheDocument();
    expect(screen.getByText("过一遍新拟态样式细节")).toBeInTheDocument();
    expect(screen.getByText("完成看板拖拽原型")).toBeInTheDocument();
    // 顶栏：首页 / 归档 / 工作区 / 回收站
    expect(screen.getByText("首页")).toBeInTheDocument();
    expect(screen.getByText("归档")).toBeInTheDocument();
    expect(screen.getByText("工作区")).toBeInTheDocument();
    expect(screen.getByText("回收站")).toBeInTheDocument();
  });

  it("点击「归档」按钮：视图切到 ArchivePage（搜索框出现 / 列头消失）", async () => {
    const user = userEvent.setup();
    render(<App />);
    await waitFor(() => {
      expect(screen.getByText("梳理 WMessage 需求清单")).toBeInTheDocument();
    });
    await user.click(screen.getByText("归档"));
    // 归档页：暂无归档内容（seed 任务都没 archived 标记）
    expect(await screen.findByText("暂无归档内容")).toBeInTheDocument();
    // 看板三列头不再显示
    expect(screen.queryByText("待办")).not.toBeInTheDocument();
  });

  it("点击「工作区」：视图切到 WorkspacePage（显示「+ 新建工作区」）", async () => {
    const user = userEvent.setup();
    render(<App />);
    await waitFor(() => {
      expect(screen.getByText("梳理 WMessage 需求清单")).toBeInTheDocument();
    });
    await user.click(screen.getByText("工作区"));
    expect(
      await screen.findByText("+ 新建工作区")
    ).toBeInTheDocument();
    // 看板三列头消失
    expect(screen.queryByText("待办")).not.toBeInTheDocument();
  });

  it("mutate 流程：点击 + 新建任务 → db_upsert 收到新任务 + 标题进入编辑态", async () => {
    const user = userEvent.setup();
    render(<App />);
    await waitFor(() => {
      expect(screen.getByText("梳理 WMessage 需求清单")).toBeInTheDocument();
    });
    const callsBefore = mocks.invokeMock.mock.calls.filter(
      (c) => c[0] === "db_upsert"
    ).length;
    await user.click(screen.getByText("+ 新建任务"));
    // 新建后立即进入编辑态：input value = "新任务"
    const input = await screen.findByDisplayValue("新任务");
    expect(input).toBeInTheDocument();
    // mutate → upsertTasks → db_upsert 被多调用至少一次
    await waitFor(() => {
      const calls = mocks.invokeMock.mock.calls.filter((c) => c[0] === "db_upsert");
      expect(calls.length).toBeGreaterThan(callsBefore);
    });
  });

  it("点击「回收站」：视图切到 TrashPage（默认显示「暂无回收站内容」）", async () => {
    const user = userEvent.setup();
    render(<App />);
    await waitFor(() => {
      expect(screen.getByText("梳理 WMessage 需求清单")).toBeInTheDocument();
    });
    await user.click(screen.getByText("回收站"));
    expect(
      await screen.findByText(/暂无回收站内容|回收站是空的/)
    ).toBeInTheDocument();
    expect(screen.queryByText("待办")).not.toBeInTheDocument();
  });

  it("db_load 读失败：不走种子/迁移分支、不写库、弹告警（error ≠ empty）", async () => {
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "db_load") throw new Error("database is locked");
      return defaultInvokeImpl(cmd);
    });
    render(<App />);
    // 明确告警（沿用 alert 风格）
    await waitFor(() => {
      expect(alertMock).toHaveBeenCalled();
    });
    // 不触发种子/迁移写入，也不删除任何行
    expect(
      mocks.invokeMock.mock.calls.filter((c) => c[0] === "db_upsert")
    ).toHaveLength(0);
    expect(
      mocks.invokeMock.mock.calls.filter((c) => c[0] === "db_delete")
    ).toHaveLength(0);
    // 看板仍渲染（内存空数组），种子标题不出现
    expect(screen.getByText("待办")).toBeInTheDocument();
    expect(screen.queryByText("梳理 WMessage 需求清单")).not.toBeInTheDocument();
  });

  // P2-21（2026-08-19）：mutate 落盘失败必须抛错、tasksRef/state 不得先行更新——
  // 修复前 tasksRef.current = next 在 await 落盘之前，失败时 UI 已更新但磁盘没动
  it("mutate 落盘失败：抛错 + UI 不更新（tasksRef 未先行赋值）", async () => {
    const user = userEvent.setup();
    const dbErr = { code: "DB_ERROR", message: "disk full", recoverable: false };
    let failUpsert = false;
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "db_upsert" && failUpsert) throw dbErr;
      return defaultInvokeImpl(cmd);
    });
    // mutate 抛错 → fire-and-forget 入口（mutateFire）catch 后 console 留痕
    const errSpy = vi.spyOn(console, "error").mockImplementation(() => {});
    try {
      render(<App />);
      await waitFor(() => {
        expect(screen.getByText("梳理 WMessage 需求清单")).toBeInTheDocument();
      });
      failUpsert = true; // 初始化种子落盘成功后，后续写入全部失败
      alertMock.mockClear();
      // emit mock 跨用例共享（beforeEach 不清），先清零再断言「失败不广播」
      const { emit } = await import("@tauri-apps/api/event");
      (emit as ReturnType<typeof vi.fn>).mockClear();
      await user.click(screen.getByText("+ 新建任务"));
      // storage 层 alert 已弹（upsertTasks → handleCommandError）
      await waitFor(() => expect(alertMock).toHaveBeenCalled());
      // mutate 抛错（错误从 upsertTasks 一路抛到 call site 的 catch）
      await waitFor(() =>
        expect(errSpy).toHaveBeenCalledWith("[mutate] persist failed", dbErr)
      );
      // tasksRef 未先行赋值 → 新任务不进 UI（不会出现标题编辑 input）
      expect(screen.queryByDisplayValue("新任务")).not.toBeInTheDocument();
      // 失败不广播（挂件不得读到未落盘的中间态）
      expect(emit).not.toHaveBeenCalledWith("tasks-changed");
    } finally {
      errSpy.mockRestore();
    }
  });

  // P2-22（2026-08-19）：软删除必须清调度字段——否则任务躺在回收站里 schedule
  // 仍到点触发（deletedAt 不挡调度读取）
  it("软删除进回收站：落盘行 schedule/schedLast 清空", async () => {
    const user = userEvent.setup();
    const withSched: Task[] = [
      {
        id: "s1",
        title: "定时任务",
        column: "todo",
        order: 0,
        updatedAt: 1,
        schedule: "daily:09:00",
        schedLast: 123,
      },
    ];
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "db_load") return withSched;
      return defaultInvokeImpl(cmd);
    });
    render(<App />);
    await waitFor(() => {
      expect(screen.getByText("定时任务")).toBeInTheDocument();
    });
    mocks.invokeMock.mockClear();
    await user.click(screen.getByTitle("删除任务"));
    await waitFor(() => {
      const calls = mocks.invokeMock.mock.calls.filter(
        (c) => c[0] === "db_upsert"
      );
      expect(calls.length).toBeGreaterThan(0);
      const written = (calls[0][1] as { tasks: Task[] }).tasks.find(
        (t) => t.id === "s1"
      );
      expect(written?.deletedAt).toBeTruthy();
      expect(written?.schedule).toBeNull();
      expect(written?.schedLast).toBeNull();
    });
  });

  // E2（2026-08-19）：tasks-updated 事件合并后，规则改动（今日归位/超时归档）
  // 必须落盘，否则只改内存 → 重启/挂件读 db 回到原始数据，三端长期不一致
  it("tasks-updated 合并：超时归档的规则改动落盘 db_upsert + console 观测行", async () => {
    const handlers: Record<string, (e: unknown) => Promise<void>> = {};
    mocks.listenMock.mockImplementation(
      async (event: string, cb: (e: unknown) => Promise<void>) => {
        handlers[event] = cb;
        return () => {};
      }
    );
    render(<App />);
    await waitFor(() => {
      expect(screen.getByText("梳理 WMessage 需求清单")).toBeInTheDocument();
    });
    expect(handlers["tasks-updated"]).toBeDefined();
    const infoSpy = vi.spyOn(console, "info").mockImplementation(() => {});
    mocks.invokeMock.mockClear();
    // 挂件上报：完成超过 7 天、未归档的任务 → applyArchiveRule 应标记 archived
    const staleDone: Task = {
      id: "w1",
      title: "挂件完成的老任务",
      column: "done",
      completedAt: Date.now() - 8 * 24 * 60 * 60 * 1000,
      order: 99,
    };
    await act(async () => {
      await handlers["tasks-updated"]({
        payload: { upserts: [staleDone], deletes: [] },
      });
    });
    const upsertCalls = mocks.invokeMock.mock.calls.filter(
      (c) => c[0] === "db_upsert"
    );
    // 第 1 次：事件原始行落盘；第 2 次：归档规则改动落盘（E2 修复点）
    expect(upsertCalls.length).toBe(2);
    const ruleWrite = upsertCalls[1][1] as { tasks: Task[] };
    expect(ruleWrite.tasks).toHaveLength(1);
    expect(ruleWrite.tasks[0].id).toBe("w1");
    expect(ruleWrite.tasks[0].archived).toBe(true);
    // 观测行：merge_tasks | N changed
    expect(infoSpy).toHaveBeenCalledWith("[tasks-updated] merge_tasks | 1 changed");
    infoSpy.mockRestore();
  });

  // E3（2026-08-19）：导入后必须以 DB 为单一真源——importTasksFromFile 成功后
  // 重新 db_load 全量 → setTasks(fresh) → 再广播，而不是清空 UI 直接广播
  // （避免其他窗口读到中间态，制造"数据全丢"假象）。实现已符合，本例为回归测试。
  it("导入任务：tasks_import 成功后重新 db_load，UI 显示 fresh 数据", async () => {
    const user = userEvent.setup();
    const fresh: Task[] = [
      { id: "imp1", title: "导入的 fresh 任务", column: "todo", order: 0, updatedAt: 1 },
    ];
    let dbLoadCount = 0;
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "db_load") {
        dbLoadCount++;
        // 首次：空库 → 走 SEED；导入后：返回合并后的 fresh 数据
        return dbLoadCount === 1 ? [] : fresh;
      }
      if (cmd === "tasks_import") return 2;
      if (cmd === "api_status") return { enabled: false, port: 4763, token: "" };
      if (cmd === "bot_get_config")
        return { baseUrl: "", model: "", hasApiKey: false };
      if (cmd === "bot_get_enabled" || cmd === "py_get_enabled") return false;
      if (cmd === "skills_list") return [];
      if (cmd === "migration_rules_load") return { version: 1, rules: [] };
      return defaultInvokeImpl(cmd);
    });
    mocks.openMock.mockResolvedValue("/tmp/import.json");
    render(<App />);
    await waitFor(() => {
      expect(screen.getByText("梳理 WMessage 需求清单")).toBeInTheDocument();
    });
    // 进设置页点「📥 导入」
    await user.click(screen.getByTitle("设置"));
    const importBtn = await screen.findByRole("button", { name: "📥 导入" });
    await user.click(importBtn);
    // 导入完成 alert（证明 importTasks 全流程走完）
    await waitFor(() => {
      expect(alertMock).toHaveBeenCalledWith("导入完成：本次写入 2 条任务卡");
    });
    // db_load 第二次调用必须发生在 tasks_import 之后（DB 是单一真源）
    const calls = mocks.invokeMock.mock.calls;
    const importOrder = mocks.invokeMock.mock.invocationCallOrder[
      calls.findIndex((c) => c[0] === "tasks_import")
    ];
    const secondLoadIdx = calls.reduce(
      (idx, c, i) => (c[0] === "db_load" && i > 0 ? i : idx),
      -1
    );
    expect(secondLoadIdx).toBeGreaterThan(-1);
    expect(mocks.invokeMock.mock.invocationCallOrder[secondLoadIdx]).toBeGreaterThan(
      importOrder
    );
    // setTasks 收到 fresh 数据：切回看板，fresh 任务在、种子任务不在
    await user.click(screen.getByText("首页"));
    expect(await screen.findByText("导入的 fresh 任务")).toBeInTheDocument();
    expect(screen.queryByText("梳理 WMessage 需求清单")).not.toBeInTheDocument();
  });
});
