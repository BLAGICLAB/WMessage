import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { emit } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { TaskDetailPanel } from "./TaskDetailPanel";
import type { Task } from "../../types";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn().mockResolvedValue(undefined),
}));
vi.mock("@tauri-apps/api/event", () => ({
  emit: vi.fn().mockResolvedValue(undefined),
}));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));
vi.mock("@tauri-apps/api/webviewWindow", () => ({
  WebviewWindow: { getByLabel: vi.fn().mockResolvedValue(null) },
}));

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

  it("文件绑定：chip 显示、移除发 filesPatch([])、无文件时显示绑定按钮", async () => {
    const user = userEvent.setup();
    const { onUpdate } = setup({
      files: [{ path: "/Users/me/桌面/报告.docx", isDir: false }],
    });
    expect(screen.getByText(/报告\.docx/)).toBeInTheDocument();
    await user.click(screen.getByLabelText("移除 报告.docx"));
    // filesPatch 同时清旧单文件字段（filePath/fileIsDir 迁移语义）
    expect(onUpdate).toHaveBeenCalledWith({
      files: [],
      filePath: null,
      fileIsDir: null,
    });
    // 清空后出现绑定入口
    cleanup();
    render(
      <TaskDetailPanel
        task={base}
        onUpdate={onUpdate}
        onSetColumn={vi.fn()}
        onDelete={vi.fn()}
        onClose={vi.fn()}
      />,
    );
    expect(screen.getByText("绑定文件")).toBeInTheDocument();
    expect(screen.getByText("绑定文件夹")).toBeInTheDocument();
  });

  it("交给机器人：发 execute-task 事件（id + 标题）", async () => {
    const user = userEvent.setup();
    setup();
    await user.click(screen.getByRole("button", { name: "交给机器人" }));
    expect(emit).toHaveBeenCalledWith("execute-task", {
      id: base.id,
      title: base.title,
    });
  });

  it("状态行：完成卡显示完成时间；机器人执行中显示对应文案", () => {
    const h = setup({ column: "done", completedAt: 1760000000000 });
    cleanup();
    render(
      <TaskDetailPanel
        task={{ ...base, column: "done", completedAt: 1760000000000 }}
        onUpdate={h.onUpdate}
        onSetColumn={h.onSetColumn}
        onDelete={h.onDelete}
        onClose={h.onClose}
      />,
    );
    expect(screen.getByText(/完成 \d{4}-\d{2}-\d{2} \d{2}:\d{2}/)).toBeInTheDocument();
    cleanup();
    render(
      <TaskDetailPanel
        task={{ ...base, botAssigned: true }}
        onUpdate={h.onUpdate}
        onSetColumn={h.onSetColumn}
        onDelete={h.onDelete}
        onClose={h.onClose}
      />,
    );
    expect(screen.getByText("机器人执行中…")).toBeInTheDocument();
  });

  it("标题自适应高度：textarea 承载全值不截断（长标题换行展示）", () => {
    const long = "这是一个特别特别特别长的任务标题用来验证换行展示而不被截断的用例";
    setup({ title: long });
    const el = screen.getByLabelText("任务标题") as HTMLTextAreaElement;
    expect(el.tagName).toBe("TEXTAREA");
    expect(el.value).toBe(long);
  });

  it("复制反馈：成功换「✓ 已复制」1.6s 还原；失败换「复制失败」", async () => {
    const path = "/Users/me/桌面/报告.docx";
    vi.mocked(invoke).mockResolvedValueOnce(undefined);
    setup({ files: [{ path, isDir: false }] });
    fireEvent.click(screen.getByTitle("复制到剪贴板"));
    expect(await screen.findByText("✓ 已复制")).toBeInTheDocument();
    // ~1.6s 后还原为「复制」
    await vi.waitFor(
      () => expect(screen.getByTitle("复制到剪贴板")).toBeInTheDocument(),
      { timeout: 3000 },
    );
    // 失败态
    vi.mocked(invoke).mockRejectedValueOnce(new Error("denied"));
    fireEvent.click(screen.getByTitle("复制到剪贴板"));
    expect(await screen.findByText("复制失败")).toBeInTheDocument();
  });
});
