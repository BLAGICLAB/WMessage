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

  it("点绑定文件名直接打开（2026-08-26 起不再有 📂 多选列表）", async () => {
    const user = userEvent.setup();
    const onOpenFilePath = vi.fn();
    render(
      <TaskCardContent
        task={multiTask}
        onOpenFilePath={onOpenFilePath}
      />
    );
    await user.click(screen.getByText("📎 b.docx"));
    expect(onOpenFilePath).toHaveBeenCalledWith("/docs/b.docx");
  });

  it("chip 内「复制」字样回调 onCopyFilePath（在解绑 × 前）", async () => {
    const user = userEvent.setup();
    const onCopyFilePath = vi.fn();
    const onRemoveFile = vi.fn();
    render(
      <TaskCardContent
        task={multiTask}
        onCopyFilePath={onCopyFilePath}
        onRemoveFile={onRemoveFile}
      />
    );
    const copies = screen.getAllByText("复制");
    expect(copies).toHaveLength(2);
    await user.click(copies[1]);
    expect(onCopyFilePath).toHaveBeenCalledWith("/docs/b.docx");
  });
});

// 2026-09-02 一致性修复：挂件卡片此前不显示完成时间，主窗口 TodoCard 显示
describe("TaskCardContent 完成时间显示（与主窗口一致）", () => {
  it("完成列任务显示「完成 MM-DD HH:mm」", () => {
    render(
      <TaskCardContent
        task={{ id: "t1", title: "x", column: "done", completedAt: new Date(2026, 8, 1, 18, 30).getTime() }}
      />
    );
    expect(screen.getByText("完成 2026-09-01 18:30")).toBeInTheDocument();
  });

  it("未完成 / 无完成时间不显示", () => {
    const { container } = render(
      <TaskCardContent task={{ id: "t1", title: "x", column: "todo" }} />
    );
    expect(container.textContent).not.toContain("完成 ");
  });
});
