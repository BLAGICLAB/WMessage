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
