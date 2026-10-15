// 共用 SubtaskRow 测试：覆盖三个 variant × 五个可选回调的显隐组合。
// 两个 variant：spacious (TodoCard)、detail (DetailPanel)
// 五个回调：onToggle / onCommit / onDelete / onOpenCard / onGenerateCard

import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { SubtaskRow, type Subtask } from "./SubtaskRow";

const baseSub: Subtask = { id: "s1", text: "子任务一", done: false };

describe("SubtaskRow spacious (TodoCard)", () => {
  it("原生 checkbox + 全文显示；点击文本进编辑，Enter 提交", async () => {
    const user = userEvent.setup();
    const onToggle = vi.fn();
    const onCommit = vi.fn();
    render(
      <SubtaskRow
        sub={baseSub}
        variant="spacious"
        onToggle={onToggle}
        onCommit={onCommit}
      />,
    );
    // 文本带 pre-wrap + break-words
    const text = screen.getByText("子任务一");
    expect(text.className).toContain("whitespace-pre-wrap");
    expect(text.className).not.toContain("truncate");
    // 点击 → input
    await user.click(text);
    const input = screen.getByDisplayValue("子任务一");
    await user.clear(input);
    await user.type(input, "改{Enter}");
    expect(onCommit).toHaveBeenCalledWith("改");
  });

  it("readOnly=true：checkbox 禁用；× 不显示；点击文本不进编辑", async () => {
    const user = userEvent.setup();
    const onToggle = vi.fn();
    const onCommit = vi.fn();
    const onDelete = vi.fn();
    render(
      <SubtaskRow
        sub={baseSub}
        variant="spacious"
        readOnly
        onToggle={onToggle}
        onCommit={onCommit}
        onDelete={onDelete}
      />,
    );
    const cb = screen.getByRole("checkbox") as HTMLInputElement;
    expect(cb.disabled).toBe(true);
    await user.click(screen.getByText("子任务一"));
    expect(screen.queryByDisplayValue("子任务一")).not.toBeInTheDocument();
    expect(screen.queryByTitle("删除子任务")).not.toBeInTheDocument();
  });

  it("无 onCommit 时提交草稿是 no-op（点击仍进编辑态，Enter 不抛错）", async () => {
    const user = userEvent.setup();
    render(
      <SubtaskRow
        sub={baseSub}
        variant="spacious"
        onToggle={vi.fn()}
        onDelete={vi.fn()}
      />,
    );
    // 仍可点入编辑态（保留 TodoCard 原行为）
    await user.click(screen.getByText("子任务一"));
    const input = screen.getByDisplayValue("子任务一");
    await user.clear(input);
    await user.type(input, "改{Enter}");
    // onCommit 未提供 → 静默丢弃草稿，回到原文本
    expect(screen.getByText("子任务一")).toBeInTheDocument();
  });

  it("↗ 在 sub.taskId + onOpenCard 时显示；点击回调携带 cardId", async () => {
    const user = userEvent.setup();
    const onOpenCard = vi.fn();
    render(
      <SubtaskRow
        sub={{ ...baseSub, taskId: "child-1" }}
        variant="spacious"
        onToggle={vi.fn()}
        onOpenCard={onOpenCard}
      />,
    );
    await user.click(screen.getByLabelText("打开子任务「子任务一」生成的任务卡"));
    expect(onOpenCard).toHaveBeenCalledWith("child-1");
  });
});


describe("SubtaskRow detail (DetailPanel)", () => {
  it("整行可点切换 done；按钮式勾选（无原生 checkbox）", async () => {
    const user = userEvent.setup();
    const onToggle = vi.fn();
    render(
      <SubtaskRow
        sub={baseSub}
        variant="detail"
        onToggle={onToggle}
        onOpenCard={vi.fn()}
        onDelete={vi.fn()}
        onGenerateCard={vi.fn()}
      />,
    );
    // 无原生 input[type=checkbox]
    expect(screen.queryByRole("checkbox")).not.toBeInTheDocument();
    // 点文本 → 调 onToggle
    await user.click(screen.getByText("子任务一"));
    expect(onToggle).toHaveBeenCalledTimes(1);
  });

  it("无 taskId 时显示 ＋；onGenerateCard 回调被触发", async () => {
    const user = userEvent.setup();
    const onGenerateCard = vi.fn();
    render(
      <SubtaskRow
        sub={baseSub}
        variant="detail"
        onToggle={vi.fn()}
        onGenerateCard={onGenerateCard}
      />,
    );
    await user.click(screen.getByTitle("生成为任务卡"));
    expect(onGenerateCard).toHaveBeenCalledTimes(1);
  });

  it("有 taskId 时显示 ↗；onOpenCard 回调被触发；不显示 ＋", async () => {
    const user = userEvent.setup();
    const onOpenCard = vi.fn();
    const onGenerateCard = vi.fn();
    render(
      <SubtaskRow
        sub={{ ...baseSub, taskId: "child-1" }}
        variant="detail"
        onToggle={vi.fn()}
        onOpenCard={onOpenCard}
        onGenerateCard={onGenerateCard}
      />,
    );
    expect(screen.queryByTitle("生成为任务卡")).not.toBeInTheDocument();
    await user.click(screen.getByTitle("打开生成的任务卡"));
    expect(onOpenCard).toHaveBeenCalledWith("child-1");
    expect(onGenerateCard).not.toHaveBeenCalled();
  });

  it("× aria-label 带行文本；onDelete 触发时携带 sub.id（由父层闭包传入）", async () => {
    const user = userEvent.setup();
    const onDelete = vi.fn();
    render(
      <SubtaskRow
        sub={baseSub}
        variant="detail"
        onToggle={vi.fn()}
        onDelete={onDelete}
      />,
    );
    await user.click(screen.getByLabelText("删除子任务「子任务一」"));
    expect(onDelete).toHaveBeenCalledTimes(1);
  });

  it("未接 onDelete 时不显示 ×；未接 onGenerateCard 时不显示 ＋", () => {
    render(
      <SubtaskRow
        sub={baseSub}
        variant="detail"
        onToggle={vi.fn()}
        onOpenCard={vi.fn()}
      />,
    );
    expect(screen.queryByTitle("删除子任务")).not.toBeInTheDocument();
    expect(screen.queryByTitle("生成为任务卡")).not.toBeInTheDocument();
  });
});

describe("SubtaskRow 全回调缺省", () => {
  it("不传任何回调：spacious 不抛错", () => {
    expect(() => render(<SubtaskRow sub={baseSub} />)).not.toThrow();
  });
  it("不传任何回调：detail 渲染空行（无 ↗/＋/×）", () => {
    render(<SubtaskRow sub={baseSub} variant="detail" />);
    expect(screen.getByText("子任务一")).toBeInTheDocument();
    expect(screen.queryByTitle("生成为任务卡")).not.toBeInTheDocument();
    expect(screen.queryByTitle("打开生成的任务卡")).not.toBeInTheDocument();
    expect(screen.queryByTitle("删除子任务")).not.toBeInTheDocument();
  });
});
