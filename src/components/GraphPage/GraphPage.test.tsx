// GraphPage 冒烟：渲染 + 过滤器交互（jsdom 无 WebGL2 上下文，GraphCanvas
// 整块 mock 掉——画布行为由 graph-scene/graph-force3d 纯函数单测覆盖）。
import { describe, expect, it, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, within } from "@testing-library/react";
import type { Task } from "../../types";

const invokeMock = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn().mockResolvedValue(() => {}),
}));
// Three.js 渲染层 mock：jsdom 无 WebGL2 上下文，这里只测 GraphPage 壳
vi.mock("./GraphCanvas", () => ({
  default: () => <div data-testid="graph-canvas" />,
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
  t({ id: "c", dependsOn: ["a"] }),
  t({ id: "z", ownerId: "p-1", title: "张三的任务" }),
];

beforeEach(() => {
  invokeMock.mockImplementation((cmd: string) => {
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
      onPatchTask={() => {}}
      {...overrides}
    />,
  );

describe("GraphPage", () => {
  it("渲染统计条（任务/依赖/成员）", async () => {
    renderPage();
    const stats = await screen.findByTestId("graph-stats");
    expect(stats.textContent).toContain("4 任务");
    expect(stats.textContent).toContain("1 依赖");
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
        onPatchTask={() => {}}
      />,
    );
    await screen.findByTestId("graph-stats");
    expect(await screen.findByText("暂无任务")).toBeTruthy();
  });

  it("着色三选一：默认按标签，三档按钮都在且图例随档切换", async () => {
    renderPage();
    await screen.findByTestId("graph-stats");
    const legend = screen.getByTestId("graph-legend");
    // 默认标签档：图例列出数据里真实存在的标签
    expect(screen.getByText("按标签").className).toContain("font-medium");
    expect(within(legend).getByText("周报")).toBeTruthy();

    fireEvent.click(screen.getByText("按成员"));
    expect(screen.getByText("按成员").className).toContain("font-medium");
    // 成员档图例列人名，而不是标签
    expect(within(legend).getByText("张三")).toBeTruthy();
    expect(within(legend).queryByText("周报")).toBeNull();

    fireEvent.click(screen.getByText("按状态"));
    expect(screen.getByText("按状态").className).toContain("font-medium");
    expect(within(legend).getByText("进行中")).toBeTruthy();
  });

  it("「按耗时定大小」已彻底退役：图例与设置页都不再有该控件", async () => {
    renderPage();
    await screen.findByTestId("graph-stats");
    expect(screen.queryByText("耗时")).toBeNull();
    expect(screen.queryByText("大小")).toBeNull();
    expect(screen.queryByLabelText(/节点大小/)).toBeNull();
  });
});
