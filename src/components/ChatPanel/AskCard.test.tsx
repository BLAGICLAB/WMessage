import { describe, it, expect, vi, beforeEach } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";
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

const elicit: AskState = {
  questionId: "elicit:1",
  question: "[MCP 服务器「demo」请求输入] 选择区域",
  options: ["华东", "华北"],
  source: "mcp_elicit",
};

beforeEach(() => {
  mocks.invokeMock.mockReset();
  mocks.invokeMock.mockResolvedValue(undefined);
});

describe("提问卡：待答呈现", () => {
  it("渲染来源标识、问题、why、假设提示与选项行", () => {
    render(<AskCard ask={pending} />);
    expect(screen.getByText("需要你决定")).toBeTruthy();
    expect(screen.getByText("投放哪个平台？")).toBeTruthy();
    expect(screen.getByText("问这个：各平台排版规范不同")).toBeTruthy();
    expect(screen.getByText("未回答时按：公众号 继续")).toBeTruthy();
    expect(screen.getByRole("radio", { name: "公众号" })).toBeTruthy();
    expect(screen.getByRole("radio", { name: "知乎" })).toBeTruthy();
    expect(screen.getByLabelText("自定义回答")).toBeTruthy();
    expect(screen.queryByText(/^第 \d+ 问$/)).toBeNull();
  });

  it("选中态走设计系统语义：选中凹陷（nm-inset），未选抬升（nm-outset）", async () => {
    const user = userEvent.setup();
    render(<AskCard ask={pending} />);
    const zhihu = screen.getByRole("radio", { name: "知乎" });
    expect(zhihu.getAttribute("aria-checked")).toBe("false");
    expect(zhihu.className).toContain("nm-outset");
    await user.click(zhihu);
    expect(zhihu.getAttribute("aria-checked")).toBe("true");
    expect(zhihu.className).toContain("nm-inset");
    expect(screen.getByRole("radio", { name: "公众号" }).className).toContain(
      "nm-outset",
    );
  });

  it("点选选项后「回答并继续」可用，点击走 answer 且携带所选文本", async () => {
    const user = userEvent.setup();
    render(<AskCard ask={pending} />);
    const primary = screen.getByRole("button", { name: "回答并继续" });
    expect((primary as HTMLButtonElement).disabled).toBe(true);
    await user.click(screen.getByRole("radio", { name: "知乎" }));
    await user.click(primary);
    expect(mocks.invokeMock).toHaveBeenCalledWith("ask_respond", {
      questionId: "q1",
      action: "answer",
      answer: "知乎",
    });
  });

  it("自由输入优先于点选，且输入即取消点选（互斥看得见）", async () => {
    const user = userEvent.setup();
    render(<AskCard ask={pending} />);
    const zhihu = screen.getByRole("radio", { name: "知乎" });
    await user.click(zhihu);
    await user.type(screen.getByLabelText("自定义回答"), "小红书");
    expect(zhihu.getAttribute("aria-checked")).toBe("false");
    await user.click(screen.getByRole("button", { name: "回答并继续" }));
    expect(mocks.invokeMock).toHaveBeenCalledWith("ask_respond", {
      questionId: "q1",
      action: "answer",
      answer: "小红书",
    });
  });

  it("输入行内 Enter 直接提交", async () => {
    const user = userEvent.setup();
    render(<AskCard ask={pending} />);
    await user.type(screen.getByLabelText("自定义回答"), "小红书{Enter}");
    expect(mocks.invokeMock).toHaveBeenCalledWith("ask_respond", {
      questionId: "q1",
      action: "answer",
      answer: "小红书",
    });
  });

  it("输入法组词中的 Enter 不提交；没有可交的回答时 Enter 也不提交", () => {
    render(<AskCard ask={pending} />);
    const input = screen.getByLabelText("自定义回答");
    fireEvent.keyDown(input, { key: "Enter" });
    fireEvent.change(input, { target: { value: "xiao" } });
    fireEvent.keyDown(input, { key: "Enter", isComposing: true });
    expect(mocks.invokeMock).not.toHaveBeenCalled();
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

  it("无选项的问题不渲染选项行，占位文案换成「输入你的回答」", () => {
    render(<AskCard ask={{ ...pending, options: [] }} />);
    expect(screen.queryByRole("radio")).toBeNull();
    expect(
      (screen.getByLabelText("自定义回答") as HTMLInputElement).placeholder,
    ).toBe("输入你的回答");
  });

  it("一轮多问时标序号", () => {
    render(<AskCard ask={pending} order={2} />);
    expect(screen.getByText("第 2 问")).toBeTruthy();
  });

  it("MCP 输入请求：来源标识可辨，「忽略」走 elicit_respond 空文本", async () => {
    const user = userEvent.setup();
    render(<AskCard ask={elicit} />);
    expect(screen.getByText("MCP 输入请求")).toBeTruthy();
    expect(screen.queryByText("需要你决定")).toBeNull();
    await user.click(screen.getByRole("button", { name: "忽略" }));
    expect(mocks.invokeMock).toHaveBeenCalledWith("elicit_respond", {
      qid: "elicit:1",
      text: "",
    });
  });
});

describe("提问卡：终态呈现（bot-ask-resolved 落定后）", () => {
  it("answer 终态收成「问题 → 答案」，不再渲染任何交互件", () => {
    render(
      <AskCard
        ask={{ ...pending, resolved: { action: "answer", answer: "知乎" } }}
      />,
    );
    expect(screen.getByText("投放哪个平台？")).toBeTruthy();
    expect(screen.getByText("→ 知乎")).toBeTruthy();
    expect(screen.queryByRole("button")).toBeNull();
    expect(screen.queryByRole("radio")).toBeNull();
    expect(screen.queryByLabelText("自定义回答")).toBeNull();
  });

  it("assume/dismiss 终态带出所按的假设", () => {
    render(<AskCard ask={{ ...pending, resolved: { action: "dismiss" } }} />);
    expect(screen.getByText("→ 已按假设继续：公众号")).toBeTruthy();
  });

  it("expired 终态（超时/停止未答）显示回落假设", () => {
    render(<AskCard ask={{ ...pending, resolved: { action: "expired" } }} />);
    expect(screen.getByText("→ 未获回答，已按假设继续：公众号")).toBeTruthy();
  });

  it("MCP 输入请求没有假设：未答按拒绝呈现，作答不回显应答值", () => {
    const { rerender } = render(
      <AskCard ask={{ ...elicit, resolved: { action: "dismiss" } }} />,
    );
    expect(screen.getByText("→ 已拒绝该输入请求")).toBeTruthy();
    rerender(<AskCard ask={{ ...elicit, resolved: { action: "expired" } }} />);
    expect(screen.getByText("→ 未获应答，已按拒绝处理")).toBeTruthy();
    rerender(
      <AskCard
        ask={{
          ...elicit,
          resolved: { action: "answer", answer: "[MCP 应答，值不落日志]" },
        }}
      />,
    );
    expect(screen.getByText("→ 已提交（应答值不回显）")).toBeTruthy();
    expect(screen.queryByText(/按假设/)).toBeNull();
  });
});
