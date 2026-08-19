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
});
