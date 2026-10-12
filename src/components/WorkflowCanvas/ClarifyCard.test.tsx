import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { ClarifyCard } from "./ClarifyCard";
import type { ClarifyQuestion } from "../../lib/workflowAsk";

const Q: ClarifyQuestion = {
  id: "q1",
  question: "数据存哪里？",
  why: null,
  options: ["本地", "云端"],
  default: "本地",
};

const setup = () =>
  render(
    <ClarifyCard
      goal="整理周报"
      questions={[Q]}
      busy={false}
      onSubmit={vi.fn()}
      onCancel={vi.fn()}
    />,
  );

describe("ClarifyCard", () => {
  it("选中选项即作答并折叠为摘要行，摘要行显示已存回答", () => {
    setup();
    // 作答 → editing[q1] 为 false → 折叠成摘要行（问题 → 回答）
    fireEvent.click(screen.getByRole("button", { name: "本地" }));
    expect(screen.getByText("本地")).toBeInTheDocument();
    // 编辑器收起：选项 chips 不在
    expect(screen.queryByRole("button", { name: "云端" })).toBeNull();
  });

  it("✎ 进入编辑态，改答后再次点击收起回到摘要行（编辑态可回收）", () => {
    setup();
    fireEvent.click(screen.getByRole("button", { name: "本地" }));
    // 摘要行 ✎ → 编辑态重新展开
    fireEvent.click(
      screen.getByRole("button", { name: "修改回答：数据存哪里？" }),
    );
    expect(screen.getByRole("button", { name: "本地" })).toBeInTheDocument();
    // 换答案：仍留在编辑态（不自动折叠）
    fireEvent.click(screen.getByRole("button", { name: "云端" }));
    expect(screen.getByRole("button", { name: "云端" })).toBeInTheDocument();
    // 收起 → 回到摘要行，显示改后的已存回答
    fireEvent.click(
      screen.getByRole("button", { name: "收起回答：数据存哪里？" }),
    );
    expect(screen.getByText("云端")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "云端" })).toBeNull();
  });

  it("收起后提交：clarifications 携带摘要行显示的回答", () => {
    const onSubmit = vi.fn();
    render(
      <ClarifyCard
        goal="整理周报"
        questions={[Q]}
        busy={false}
        onSubmit={onSubmit}
        onCancel={vi.fn()}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: "云端" }));
    // ✎ 展开编辑态 → 收起回摘要行（收起入口只在编辑态出现）
    fireEvent.click(
      screen.getByRole("button", { name: "修改回答：数据存哪里？" }),
    );
    fireEvent.click(
      screen.getByRole("button", { name: "收起回答：数据存哪里？" }),
    );
    fireEvent.click(screen.getByText("开始拆解 →"));
    expect(onSubmit).toHaveBeenCalledWith([
      { question: "数据存哪里？", answer: "云端" },
    ]);
  });
});
