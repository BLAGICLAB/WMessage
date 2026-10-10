import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { TaskDetailPanel } from "./TaskDetailPanel";
import type { Task } from "../../types";

const base: Task = {
  id: "p1",
  title: "排期任务",
  column: "todo",
  planStart: "2026-10-07T10:00",
  planEnd: "2026-10-07T11:00",
  subtasks: [
    { id: "s1", text: "子任务甲", done: false },
    { id: "s2", text: "子任务乙", done: true },
  ],
};

const setup = (over: Partial<Task> = {}) => {
  const onUpdate = vi.fn();
  const onSetColumn = vi.fn();
  const onDelete = vi.fn();
  const onClose = vi.fn();
  render(
    <TaskDetailPanel
      task={{ ...base, ...over }}
      onUpdate={onUpdate}
      onSetColumn={onSetColumn}
      onDelete={onDelete}
      onClose={onClose}
    />,
  );
  return { onUpdate, onSetColumn, onDelete, onClose };
};

describe("TaskDetailPanel", () => {
  it("渲染字段：标题/计划开始/备注/标签/子任务/截止时间", () => {
    setup({ note: "备注内容", tags: ["设计"], due: "2026-10-09T18:00" });
    expect(screen.getByDisplayValue("排期任务")).toBeInTheDocument();
    expect(screen.getByDisplayValue("2026-10-07T10:00")).toBeInTheDocument();
    expect(screen.getByDisplayValue("备注内容")).toBeInTheDocument();
    expect(screen.getByDisplayValue("设计")).toBeInTheDocument();
    expect(screen.getByText("子任务甲")).toBeInTheDocument();
    expect(screen.getByDisplayValue("2026-10-09T18:00")).toBeInTheDocument();
    // 时长与结束角标
    expect(screen.getByText("60 分钟")).toBeInTheDocument();
    expect(screen.getByText("至 三 11:00")).toBeInTheDocument();
  });

  it("标题 blur 提交 patch；Esc 还原不提交", async () => {
    const user = userEvent.setup();
    const { onUpdate } = setup();
    const input = screen.getByLabelText("任务标题");
    await user.clear(input);
    await user.type(input, "新标题");
    await user.tab();
    expect(onUpdate).toHaveBeenCalledWith({ title: "新标题" });
    // Esc 还原
    await user.clear(input);
    await user.type(input, "草稿");
    fireEvent.keyDown(input, { key: "Escape" });
    expect(onUpdate).not.toHaveBeenCalledWith({ title: "草稿" });
  });

  it("时长快捷 30：以现起点重算 planEnd（窗口分钟）", async () => {
    const user = userEvent.setup();
    const { onUpdate } = setup();
    await user.click(screen.getByRole("button", { name: "30" }));
    expect(onUpdate).toHaveBeenCalledWith({
      planStart: "2026-10-07T10:00",
      planEnd: "2026-10-07T10:30",
    });
  });

  it("步进器 +30：跨 18:00 折入次日物理时间", async () => {
    const user = userEvent.setup();
    const { onUpdate } = setup({
      planStart: "2026-10-07T17:30",
      planEnd: "2026-10-07T18:00",
    });
    await user.click(screen.getByRole("button", { name: "时长加 30 分钟" }));
    expect(onUpdate).toHaveBeenCalledWith({
      planStart: "2026-10-07T17:30",
      planEnd: "2026-10-08T08:30",
    });
  });

  it("清除计划：双 null patch", async () => {
    const user = userEvent.setup();
    const { onUpdate } = setup();
    await user.click(screen.getByRole("button", { name: "清除计划" }));
    expect(onUpdate).toHaveBeenCalledWith({ planStart: null, planEnd: null });
  });

  it("改开始时间：保持时长重算结束", () => {
    const { onUpdate } = setup();
    fireEvent.change(screen.getByLabelText("计划开始"), {
      target: { value: "2026-10-08T14:00" },
    });
    expect(onUpdate).toHaveBeenCalledWith({
      planStart: "2026-10-08T14:00",
      planEnd: "2026-10-08T15:00",
    });
  });

  it("标记完成/恢复待办走 onSetColumn；删除走 onDelete", async () => {
    const user = userEvent.setup();
    const h = setup();
    await user.click(screen.getByRole("button", { name: "✓ 标记完成" }));
    expect(h.onSetColumn).toHaveBeenCalledWith("done");
    h.onSetColumn.mockClear();
    cleanup();
    render(
      <TaskDetailPanel
        task={{ ...base, column: "done", completedAt: 1 }}
        onUpdate={h.onUpdate}
        onSetColumn={h.onSetColumn}
        onDelete={h.onDelete}
        onClose={h.onClose}
      />,
    );
    await user.click(screen.getByRole("button", { name: "✓ 恢复待办" }));
    expect(h.onSetColumn).toHaveBeenCalledWith("todo");
    await user.click(screen.getByTitle("删除任务"));
    expect(h.onDelete).toHaveBeenCalled();
  });

  it("子任务勾选切换 + 回车添加", async () => {
    const user = userEvent.setup();
    const { onUpdate } = setup();
    await user.click(screen.getByText("子任务甲"));
    expect(onUpdate).toHaveBeenCalledWith({
      subtasks: [
        { id: "s1", text: "子任务甲", done: true },
        { id: "s2", text: "子任务乙", done: true },
      ],
    });
    const add = screen.getByLabelText("添加子任务");
    await user.type(add, "新子任务{Enter}");
    expect(onUpdate).toHaveBeenCalledWith({
      subtasks: [
        { id: "s1", text: "子任务甲", done: false },
        { id: "s2", text: "子任务乙", done: true },
        expect.objectContaining({ text: "新子任务", done: false }),
      ],
    });
  });

  it("Esc 关面板（window 级）", () => {
    const { onClose } = setup();
    fireEvent.keyDown(window, { key: "Escape" });
    expect(onClose).toHaveBeenCalled();
  });

  it("autoFocusTitle：新建路径聚焦标题并全选", () => {
    render(
      <TaskDetailPanel
        task={{ ...base, title: "新任务" }}
        autoFocusTitle
        onUpdate={vi.fn()}
        onSetColumn={vi.fn()}
        onDelete={vi.fn()}
        onClose={vi.fn()}
      />,
    );
    const input = screen.getByLabelText("任务标题") as HTMLInputElement;
    expect(document.activeElement).toBe(input);
    expect(input.selectionStart).toBe(0);
    expect(input.selectionEnd).toBe("新任务".length);
  });
});
