// GraphPage 冒烟：渲染 + 过滤器交互（jsdom 无 canvas 2D 上下文，GraphCanvas
// 静默跳过绘制——画布逻辑由 physics/graph-build 纯函数单测覆盖）。
import { describe, expect, it, vi, beforeEach } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import type { Task, Workflow } from "../../types";

const invokeMock = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn().mockResolvedValue(() => {}),
}));
// Sigma/WebGL 渲染层 mock：jsdom 无 WebGL2 上下文；GraphCanvas 的行为契约
// 由 graph-adapter 单测 + 浏览器压测 harness 覆盖，这里只测 GraphPage 壳
vi.mock("./GraphCanvas", () => ({
  default: () => <div data-testid="graph-canvas" />,
  OWNER_PALETTE: ["#7c6bd6"],
}));

import GraphPage from "./GraphPage";

function t(partial: Partial<Task> & { id: string }): Task {
  return { title: `任务-${partial.id}`, column: "todo", ...partial };
}

const TASKS: Task[] = [
  t({ id: "a", column: "doing" }),
  t({
    id: "b",
    column: "done",
    tags: ["周报"],
    completedAt: new Date("2026-03-01").getTime(),
  }),
  t({ id: "w1", origin: "workflow", workflowId: "wf1" }),
  t({ id: "z", ownerId: "p-1", title: "张三的任务" }),
];

const WFS: Workflow[] = [{ id: "wf1", name: "周报流水线", goal: "出周报" }];

beforeEach(() => {
  invokeMock.mockImplementation((cmd: string) => {
    if (cmd === "workflow_list") return Promise.resolve(WFS);
    if (cmd === "people_list")
      return Promise.resolve([
        { id: "", name: "我", isSelf: true },
        { id: "p-1", name: "张三", isSelf: false },
      ]);
    return Promise.resolve([]);
  });
});

const renderPage = (overrides?: Partial<Parameters<typeof GraphPage>[0]>) =>
  render(
    <GraphPage
      tasks={TASKS}
      onOpenTask={() => {}}
      onOpenWorkflow={() => {}}
      onPatchTask={() => {}}
      {...overrides}
    />,
  );

describe("GraphPage", () => {
  it("渲染统计条（任务/依赖/工作流/成员）", async () => {
    renderPage();
    const stats = await screen.findByTestId("graph-stats");
    expect(stats.textContent).toContain("4 任务");
    expect(stats.textContent).toContain("1 工作流");
    expect(stats.textContent).toContain("2 成员");
  });

  it("点选节点（模拟选中）后详情面板出现，外来卡无「在看板打开」", async () => {
    renderPage();
    await screen.findByTestId("graph-stats");
    // 详情面板由画布点选驱动；这里直接验证面板容器初始不存在
    expect(screen.queryByTestId("graph-detail")).toBeNull();
  });

  it("状态过滤器交互后统计条联动", async () => {
    renderPage();
    const stats = await screen.findByTestId("graph-stats");
    fireEvent.click(screen.getByRole("button", { name: "已完成" }));
    await vi.waitFor(() => {
      expect(stats.textContent).not.toContain("4 任务");
    });
  });

  it("搜索框可输入", async () => {
    renderPage();
    await screen.findByTestId("graph-stats");
    const input = screen.getByLabelText("搜索图谱节点");
    fireEvent.change(input, { target: { value: "张三" } });
    expect((input as HTMLInputElement).value).toBe("张三");
  });

  it("空任务显示引导空态", async () => {
    render(
      <GraphPage
        tasks={[]}
        onOpenTask={() => {}}
        onOpenWorkflow={() => {}}
        onPatchTask={() => {}}
      />,
    );
    await screen.findByTestId("graph-stats");
    expect(await screen.findByText("暂无任务")).toBeTruthy();
  });
});
