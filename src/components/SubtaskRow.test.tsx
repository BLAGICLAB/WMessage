import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { SubtaskRow } from "./TodoCard/SubtaskRow";

const base = () => ({
  sub: { id: "s1", text: "子任务一", done: false },
  onToggle: vi.fn(),
  onCommit: vi.fn(),
  onDelete: vi.fn(),
});

describe("SubtaskRow readOnly 约束", () => {
  it("readOnly=true：点击文本不进编辑态", async () => {
    const user = userEvent.setup();
    render(<SubtaskRow {...base()} readOnly={true} />);
    await user.click(screen.getByText("子任务一"));
    expect(screen.queryByDisplayValue("子任务一")).not.toBeInTheDocument();
  });

  it("readOnly flip false→true：在飞编辑态退出（input 消失、文本恢复）", async () => {
    const user = userEvent.setup();
    const props = base();
    const { rerender } = render(<SubtaskRow {...props} readOnly={false} />);
    await user.click(screen.getByText("子任务一"));
    expect(screen.getByDisplayValue("子任务一")).toBeInTheDocument();
    rerender(<SubtaskRow {...props} readOnly={true} />);
    expect(screen.queryByDisplayValue("子任务一")).not.toBeInTheDocument();
    expect(screen.getByText("子任务一")).toBeInTheDocument();
  });
});
