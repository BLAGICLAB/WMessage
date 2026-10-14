import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { TaskFileChips } from "./TaskFileChips";
import type { Task } from "../../types";

const baseTask: Task = { id: "t1", title: "demo", column: "todo" };

describe("TaskFileChips", () => {
  it("0 绑定文件：返回 null（不渲染任何东西）", () => {
    const { container } = render(
      <TaskFileChips task={baseTask} onOpenPath={vi.fn()} />,
    );
    expect(container.firstChild).toBeNull();
  });

  it("2 个文件：渲染 2 个 chip，每个带打开按钮 + 复制 + ×", () => {
    const onOpenPath = vi.fn();
    const onCopyPath = vi.fn();
    const onRemovePath = vi.fn();
    render(
      <TaskFileChips
        task={{
          ...baseTask,
          files: [
            { path: "/docs/a.pdf", isDir: false },
            { path: "/docs/b.docx", isDir: false },
          ],
        }}
        onOpenPath={onOpenPath}
        onCopyPath={onCopyPath}
        onRemovePath={onRemovePath}
      />,
    );
    expect(screen.getByText("a.pdf")).toBeInTheDocument();
    expect(screen.getByText("b.docx")).toBeInTheDocument();
    // 复制按钮 ×2、×按钮 ×2、打开按钮 ×2
    expect(screen.getAllByText("复制")).toHaveLength(2);
    expect(screen.getAllByTitle("移除该文件")).toHaveLength(2);
  });

  it("6 个文件：默认折叠显示前 5 个 + 「还有 1 个」；点击展开显示全部 6 个", async () => {
    const user = userEvent.setup();
    render(
      <TaskFileChips
        task={{
          ...baseTask,
          files: Array.from({ length: 6 }, (_, i) => ({
            path: `/f/${i}.txt`,
            isDir: false,
          })),
        }}
        onOpenPath={vi.fn()}
      />,
    );
    expect(screen.queryByText("5.txt")).not.toBeInTheDocument();
    expect(screen.getByText("还有 1 个")).toBeInTheDocument();
    await user.click(screen.getByText("还有 1 个"));
    expect(screen.getByText("5.txt")).toBeInTheDocument();
    expect(screen.getByText("收起")).toBeInTheDocument();
  });

  it("只读场景（onRemovePath 不传）：不渲染 × 按钮", () => {
    render(
      <TaskFileChips
        task={{
          ...baseTask,
          files: [
            { path: "/docs/a.pdf", isDir: false },
            { path: "/docs/b.docx", isDir: false },
          ],
        }}
        onOpenPath={vi.fn()}
        onCopyPath={vi.fn()}
        // onRemovePath 故意不传
      />,
    );
    expect(screen.getByText("a.pdf")).toBeInTheDocument();
    expect(screen.queryByTitle("移除该文件")).not.toBeInTheDocument();
    expect(screen.queryByText("×")).not.toBeInTheDocument();
  });

  it("spacious variant 详情面板：emoji 图标 + 复制反馈徽标替代「复制」按钮", () => {
    render(
      <TaskFileChips
        task={{
          ...baseTask,
          files: [{ path: "/docs/a.pdf", isDir: false }],
        }}
        variant="spacious"
        onOpenPath={vi.fn()}
        onCopyPath={vi.fn()}
        onRemovePath={vi.fn()}
        removeTitle="移除绑定"
        copyState={{ "/docs/a.pdf": "ok" }}
      />,
    );
    // emoji + basename 在同一 button 内多个文本节点，用函数匹配器
    expect(
      screen.getByText((_, node) => node?.textContent === "📄 a.pdf"),
    ).toBeInTheDocument();
    // copyState 提供时不渲染「复制」按钮，改显示徽标
    expect(screen.queryByText("复制")).not.toBeInTheDocument();
    expect(screen.getByText("✓ 已复制")).toBeInTheDocument();
    // 详情面板用自定义 removeTitle
    expect(screen.getByTitle("移除绑定")).toBeInTheDocument();
  });

  it("compact variant：5 个文件时仍显示「还有 N 个」（与 spacious 一致）", () => {
    render(
      <TaskFileChips
        task={{
          ...baseTask,
          files: Array.from({ length: 8 }, (_, i) => ({
            path: `/f/${i}.txt`,
            isDir: false,
          })),
        }}
        variant="compact"
        onOpenPath={vi.fn()}
      />,
    );
    expect(screen.getByText("还有 3 个")).toBeInTheDocument();
    expect(screen.queryByText("7.txt")).not.toBeInTheDocument();
  });
});
