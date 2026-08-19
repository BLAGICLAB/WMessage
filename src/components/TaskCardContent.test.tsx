import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { TaskCardContent } from "./TaskCardContent";
import type { Task } from "../types";

// ActorAvatar → loadProfile 会走 invoke("profile_get")，mock 掉避免 unhandled rejection
const invokeMock = vi.hoisted(() => vi.fn(async () => null));
vi.mock("@tauri-apps/api/core", () => ({ invoke: invokeMock }));

const task: Task = { id: "t1", title: "原始标题", column: "todo" };

// E4（2026-08-19）：Escape 取消内联编辑后，随后的 blur 不得再把草稿提交
// （组件本身不调 invoke，提交走父组件 onCommitTitle → 写库；验证 onCommitTitle
// 不被调用即等价于 db_upsert 不触发）
describe("TaskCardContent 标题编辑 Escape 取消（E4）", () => {
  it("Escape 取消后 blur 不提交草稿", async () => {
    const user = userEvent.setup();
    const onCommitTitle = vi.fn();
    const onCancelTitle = vi.fn();
    render(
      <TaskCardContent
        task={task}
        editingTitle
        onCommitTitle={onCommitTitle}
        onCancelTitle={onCancelTitle}
      />
    );
    const input = screen.getByDisplayValue("原始标题");
    await user.type(input, "改");
    fireEvent.keyDown(input, { key: "Escape" });
    expect(onCancelTitle).toHaveBeenCalledTimes(1);
    fireEvent.blur(input);
    expect(onCommitTitle).not.toHaveBeenCalled();
  });

  it("取消标记在 blur 跳过后重置：再次 blur 可正常提交", async () => {
    const user = userEvent.setup();
    const onCommitTitle = vi.fn();
    render(
      <TaskCardContent
        task={task}
        editingTitle
        onCommitTitle={onCommitTitle}
        onCancelTitle={vi.fn()}
      />
    );
    const input = screen.getByDisplayValue("原始标题");
    await user.type(input, "改");
    fireEvent.keyDown(input, { key: "Escape" });
    fireEvent.blur(input);
    expect(onCommitTitle).not.toHaveBeenCalled();
    // 取消标记已重置，后续 blur 正常提交当前草稿；
    // P2-23：行为与 TodoCard 统一——Escape 已把草稿回滚为已提交值，故提交「原始标题」
    fireEvent.blur(input);
    expect(onCommitTitle).toHaveBeenCalledWith("原始标题");
  });

  it("未按 Escape 时 blur 正常提交草稿", async () => {
    const user = userEvent.setup();
    const onCommitTitle = vi.fn();
    render(
      <TaskCardContent task={task} editingTitle onCommitTitle={onCommitTitle} />
    );
    const input = screen.getByDisplayValue("原始标题");
    await user.type(input, "改");
    fireEvent.blur(input);
    expect(onCommitTitle).toHaveBeenCalledWith("原始标题改");
  });

  it("Enter 正常提交草稿", async () => {
    const user = userEvent.setup();
    const onCommitTitle = vi.fn();
    render(
      <TaskCardContent task={task} editingTitle onCommitTitle={onCommitTitle} />
    );
    const input = screen.getByDisplayValue("原始标题");
    await user.type(input, "改");
    fireEvent.keyDown(input, { key: "Enter" });
    expect(onCommitTitle).toHaveBeenCalledWith("原始标题改");
  });
});

// —— 多文件绑定（2026-08-19）：chip 列表 / 单独移除 / 超 5 折叠 / 多文件打开列表 ——
describe("TaskCardContent 多文件绑定", () => {
  const multiTask: Task = {
    id: "t2",
    title: "多文件任务",
    column: "todo",
    files: [
      { path: "/docs/a.pdf", isDir: false },
      { path: "/docs/b.docx", isDir: false },
    ],
  };

  it("chip 列表渲染 + onRemoveFile 时每个 chip 带 × 回调对应路径", async () => {
    const user = userEvent.setup();
    const onRemoveFile = vi.fn();
    render(<TaskCardContent task={multiTask} onRemoveFile={onRemoveFile} />);
    expect(screen.getByText("📎 a.pdf")).toBeInTheDocument();
    expect(screen.getByText("📎 b.docx")).toBeInTheDocument();
    const removes = screen.getAllByTitle("移除该文件");
    expect(removes).toHaveLength(2);
    await user.click(removes[1]);
    expect(onRemoveFile).toHaveBeenCalledWith("/docs/b.docx");
  });

  it("不传 onRemoveFile 时不渲染 ×（只读场景）", () => {
    render(<TaskCardContent task={multiTask} />);
    expect(screen.getByText("📎 a.pdf")).toBeInTheDocument();
    expect(screen.queryByTitle("移除该文件")).not.toBeInTheDocument();
  });

  it("超过 5 个折叠为「还有 N 个」，点击展开", async () => {
    const user = userEvent.setup();
    const seven: Task = {
      id: "t3",
      title: "七文件",
      column: "todo",
      files: Array.from({ length: 7 }, (_, i) => ({
        path: `/f/${i}.txt`,
        isDir: false,
      })),
    };
    render(<TaskCardContent task={seven} />);
    expect(screen.queryByText("📎 5.txt")).not.toBeInTheDocument();
    await user.click(screen.getByText("还有 2 个"));
    expect(screen.getByText("📎 5.txt")).toBeInTheDocument();
    expect(screen.getByText("📎 6.txt")).toBeInTheDocument();
  });

  it("多文件打开：📂 弹选择列表，点条目回调 onOpenFilePath", async () => {
    const user = userEvent.setup();
    const onOpenFile = vi.fn();
    const onOpenFilePath = vi.fn();
    render(
      <TaskCardContent
        task={multiTask}
        onOpenFile={onOpenFile}
        onOpenFilePath={onOpenFilePath}
      />
    );
    await user.click(screen.getByTitle("打开文件（多选列表）"));
    expect(onOpenFile).not.toHaveBeenCalled();
    // chip 行与选择列表条目文本相同，取按钮（选择列表条目是 button）
    const items = screen.getAllByText("📎 b.docx");
    await user.click(items[items.length - 1]);
    expect(onOpenFilePath).toHaveBeenCalledWith("/docs/b.docx");
  });
});
