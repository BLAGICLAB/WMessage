import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { ArchivePage } from "./ArchivePage";
import type { Task } from "../types";

// —— Tauri mocks（同 TrashPage.test：ArchivePage → TodoCard → ActorAvatar 走 profile/invoke）——
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(async () => null),
  convertFileSrc: vi.fn((p: string) => `tauri://localhost/${p}`),
}));

vi.mock("@tauri-apps/api/event", () => ({
  emit: vi.fn(async () => {}),
  listen: vi.fn(async () => () => {}),
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: vi.fn(async () => null),
}));

vi.mock("@tauri-apps/plugin-opener", () => ({
  openPath: vi.fn(async () => {}),
}));

vi.mock("@tauri-apps/api/webviewWindow", () => ({
  WebviewWindow: { getByLabel: vi.fn(async () => null) },
}));

vi.mock("../profile", () => ({
  getProfileCache: vi.fn(() => ({
    user: { name: "Test", avatarDataUrl: null },
    bot: { name: "Bot", avatarDataUrl: null },
  })),
  loadProfile: vi.fn(async () => ({
    user: { name: "Test", avatarDataUrl: null },
    bot: { name: "Bot", avatarDataUrl: null },
  })),
  subscribeProfile: vi.fn(() => () => {}),
}));

const archivedTask = (
  over: Partial<Task> & { id: string; title: string },
): Task => ({
  column: "done",
  archived: true,
  ...over,
});

const tasks: Task[] = [
  archivedTask({ id: "a", title: "买牛奶", tags: ["生活"], order: 0 }),
  archivedTask({
    id: "b",
    title: "Weekly Report",
    note: "周五截止",
    tags: ["工作"],
    order: 1,
  }),
  archivedTask({
    id: "e",
    title: "牛奶清单",
    tags: ["生活", "购物"],
    order: 2,
    collapsed: false,
  }),
  // 不应出现在归档页：未归档 / 已软删除
  { id: "c", title: "进行中任务", column: "todo", order: 3 },
  archivedTask({ id: "d", title: "已删除任务", deletedAt: 123, order: 4 }),
];

const renderPage = (
  ts: Task[] = tasks,
  props: Partial<Parameters<typeof ArchivePage>[0]> = {},
) => render(<ArchivePage tasks={ts} onUpdate={vi.fn()} onDelete={vi.fn()} {...props} />);

// 归档页过滤：搜索 + 标签 AND 叠加 + archived/deletedAt 前置过滤。
describe("ArchivePage 过滤", () => {
  it("只显示已归档且未删除的任务，按 order 排序", () => {
    renderPage();
    expect(screen.getByText("买牛奶")).toBeInTheDocument();
    expect(screen.getByText("Weekly Report")).toBeInTheDocument();
    expect(screen.getByText("牛奶清单")).toBeInTheDocument();
    expect(screen.queryByText("进行中任务")).not.toBeInTheDocument();
    expect(screen.queryByText("已删除任务")).not.toBeInTheDocument();
  });

  it("搜索过滤：命中标题或备注，大小写不敏感", async () => {
    const user = userEvent.setup();
    renderPage();
    const input = screen.getByPlaceholderText("搜索归档内容…");
    // 小写命中大写标题
    await user.type(input, "weekly");
    expect(screen.getByText("Weekly Report")).toBeInTheDocument();
    expect(screen.queryByText("买牛奶")).not.toBeInTheDocument();
    // 改搜备注内容
    await user.clear(input);
    await user.type(input, "截止");
    expect(screen.getByText("Weekly Report")).toBeInTheDocument();
    expect(screen.queryByText("牛奶清单")).not.toBeInTheDocument();
  });

  it("搜索无匹配：显示「没有匹配的归档」", async () => {
    const user = userEvent.setup();
    renderPage();
    await user.type(
      screen.getByPlaceholderText("搜索归档内容…"),
      "不存在的关键词",
    );
    expect(screen.getByText("没有匹配的归档")).toBeInTheDocument();
  });

  it("标签计数渲染（多标签任务各计一次），点击筛选、再点取消", async () => {
    const user = userEvent.setup();
    renderPage();
    const lifeTag = screen.getByRole("button", { name: /# 生活/ });
    // 生活标签下 a + e 两个任务
    expect(lifeTag.textContent).toContain("2");
    await user.click(lifeTag);
    expect(screen.getByText("买牛奶")).toBeInTheDocument();
    expect(screen.getByText("牛奶清单")).toBeInTheDocument();
    expect(screen.queryByText("Weekly Report")).not.toBeInTheDocument();
    // 再点同一标签取消筛选
    await user.click(screen.getByRole("button", { name: /# 生活/ }));
    expect(screen.getByText("Weekly Report")).toBeInTheDocument();
  });

  it("标签与搜索 AND 叠加：两者同时生效取交集", async () => {
    const user = userEvent.setup();
    renderPage();
    await user.click(screen.getByRole("button", { name: /# 生活/ }));
    await user.type(screen.getByPlaceholderText("搜索归档内容…"), "清单");
    // 生活标签下只有 e 命中「清单」；a 被搜索滤掉，b 被标签滤掉
    expect(screen.getByText("牛奶清单")).toBeInTheDocument();
    expect(screen.queryByText("买牛奶")).not.toBeInTheDocument();
    expect(screen.queryByText("Weekly Report")).not.toBeInTheDocument();
  });

  it("归档卡片默认折叠；collapsed=false 的保持展开（FoldToggle 状态）", () => {
    renderPage();
    // a/b 未设 collapsed → 折叠（▸ 展开）；e 显式 collapsed=false → 展开（▾ 收起）
    expect(screen.getAllByTitle("展开")).toHaveLength(2);
    expect(screen.getAllByTitle("收起")).toHaveLength(1);
  });

  it("无归档任务：显示空态文案，不渲染搜索框", () => {
    renderPage([{ id: "x", title: "未归档", column: "todo" }]);
    expect(screen.getByText("暂无归档内容")).toBeInTheDocument();
    expect(
      screen.queryByPlaceholderText("搜索归档内容…"),
    ).not.toBeInTheDocument();
  });
});

// 时间泳道双击归档卡跳转过来：滚动定位 + 紫边框（归档区不盖章；定位态保留到下一次交互）
describe("ArchivePage 跳转定位", () => {
  beforeEach(() => {
    Element.prototype.scrollIntoView = vi.fn();
  });

  it("定位目标卡带紫边框标记（archive-locate），并触发滚动居中", () => {
    renderPage(tasks, { highlightTaskId: "b" });
    expect(document.querySelector('[data-task-id="b"]')).toHaveClass("archive-locate");
    expect(document.querySelector('[data-task-id="a"]')).not.toHaveClass("archive-locate");
    expect(Element.prototype.scrollIntoView).toHaveBeenCalledWith({
      behavior: "smooth",
      block: "center",
    });
  });

  it("目标卡被当前筛选挡住时自动清筛选，保证卡可见", async () => {
    const user = userEvent.setup();
    const { rerender } = renderPage();
    await user.type(screen.getByPlaceholderText("搜索归档内容…"), "weekly");
    expect(screen.queryByText("买牛奶")).not.toBeInTheDocument();
    // 跳转定位 a（买牛奶）：被搜索词挡住 → 自动清空搜索
    rerender(
      <ArchivePage
        tasks={tasks}
        onUpdate={vi.fn()}
        onDelete={vi.fn()}
        highlightTaskId="a"
      />,
    );
    expect(
      (screen.getByPlaceholderText("搜索归档内容…") as HTMLInputElement).value,
    ).toBe("");
    expect(screen.getByText("买牛奶")).toBeInTheDocument();
  });

  it("任一交互（点卡 / 改筛选）即清除定位态", async () => {
    const user = userEvent.setup();
    const onClearHighlight = vi.fn();
    renderPage(tasks, { highlightTaskId: "b", onClearHighlight });
    // 点定位卡本身
    await user.click(document.querySelector('[data-task-id="b"]')!);
    expect(onClearHighlight).toHaveBeenCalledTimes(1);
    // 改搜索词（逐字符触发 onChange：牛奶 = 2 次）
    await user.type(screen.getByPlaceholderText("搜索归档内容…"), "牛奶");
    expect(onClearHighlight).toHaveBeenCalledTimes(3);
  });
});
