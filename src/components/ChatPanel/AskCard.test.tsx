import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { AskCard } from "./AskCard";
import type { AskState } from "./askStore";

const mocks = vi.hoisted(() => ({ invokeMock: vi.fn() }));

vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invokeMock }));

const pending: AskState = {
  questionId: "q1",
  question: "投放哪个平台？",
  why: "各平台排版规范不同",
  options: ["公众号", "知乎"],
  assumption: "公众号",
};

beforeEach(() => {
  mocks.invokeMock.mockReset();
  mocks.invokeMock.mockResolvedValue(undefined);
});

describe("提问卡：待答呈现", () => {
  it("渲染问题、why、假设提示与选项 chips", () => {
    render(<AskCard ask={pending} />);
    expect(screen.getByText("投放哪个平台？")).toBeTruthy();
    expect(screen.getByText("问这个：各平台排版规范不同")).toBeTruthy();
    expect(screen.getByText("未回答时按：公众号 继续")).toBeTruthy();
    expect(screen.getByRole("button", { name: "公众号" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "知乎" })).toBeTruthy();
    expect(screen.getByLabelText("自定义回答")).toBeTruthy();
  });

  it("点选选项后「回答并继续」可用，点击走 answer 且携带所选文本", async () => {
    const user = userEvent.setup();
    render(<AskCard ask={pending} />);
    const primary = screen.getByRole("button", { name: "回答并继续" });
    expect((primary as HTMLButtonElement).disabled).toBe(true);
    await user.click(screen.getByRole("button", { name: "知乎" }));
    await user.click(primary);
    expect(mocks.invokeMock).toHaveBeenCalledWith("ask_respond", {
      questionId: "q1",
      action: "answer",
      answer: "知乎",
    });
  });

  it("自由输入的「其他回答」优先于点选的选项", async () => {
    const user = userEvent.setup();
    render(<AskCard ask={pending} />);
    await user.click(screen.getByRole("button", { name: "知乎" }));
    await user.type(screen.getByLabelText("自定义回答"), "小红书");
    await user.click(screen.getByRole("button", { name: "回答并继续" }));
    expect(mocks.invokeMock).toHaveBeenCalledWith("ask_respond", {
      questionId: "q1",
      action: "answer",
      answer: "小红书",
    });
  });

  it("「按假设继续」走 assume，不携带回答", async () => {
    const user = userEvent.setup();
    render(<AskCard ask={pending} />);
    await user.click(screen.getByRole("button", { name: "按假设继续" }));
    expect(mocks.invokeMock).toHaveBeenCalledWith("ask_respond", {
      questionId: "q1",
      action: "assume",
      answer: null,
    });
  });

  it("无选项的问题不渲染 chips，占位文案换成「输入你的回答」", () => {
    render(<AskCard ask={{ ...pending, options: [] }} />);
    expect(screen.queryByRole("button", { name: "公众号" })).toBeNull();
    expect(
      (screen.getByLabelText("自定义回答") as HTMLInputElement).placeholder,
    ).toBe("输入你的回答");
  });
});

describe("提问卡：终态呈现（bot-ask-resolved 落定后）", () => {
  it("answer 终态显示回答，不再渲染任何交互件", () => {
    render(
      <AskCard
        ask={{ ...pending, resolved: { action: "answer", answer: "知乎" } }}
      />,
    );
    expect(screen.getByText("✅ 已回答：知乎")).toBeTruthy();
    expect(screen.queryByRole("button", { name: "回答并继续" })).toBeNull();
    expect(screen.queryByRole("button", { name: "按假设继续" })).toBeNull();
    expect(screen.queryByLabelText("自定义回答")).toBeNull();
  });

  it("assume/dismiss 终态显示已按假设继续", () => {
    render(<AskCard ask={{ ...pending, resolved: { action: "assume" } }} />);
    expect(screen.getByText("已按假设继续")).toBeTruthy();
  });

  it("expired 终态（超时/停止未答）显示回落假设", () => {
    render(<AskCard ask={{ ...pending, resolved: { action: "expired" } }} />);
    expect(screen.getByText("未获回答，已按 AI 假设继续")).toBeTruthy();
  });
});
