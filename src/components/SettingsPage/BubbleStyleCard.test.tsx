// BubbleStyleCard 单测：色板/材质点选即生效（DOM 变量 + data 属性 + localStorage 落盘）
// + 恢复默认。卡片纯前端（零 tauri invoke），直接渲染即可。

import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it } from "vitest";
import { BubbleStyleCard } from "./BubbleStyleCard";

const KEY = "wmessage-bubble-style";

beforeEach(() => {
  localStorage.clear();
  document.documentElement.style.removeProperty("--bubble-user-bg");
  document.documentElement.style.removeProperty("--bubble-user-fg");
  delete document.documentElement.dataset.bubbleBot;
});

describe("BubbleStyleCard", () => {
  it("默认态：跟随主题选中（无 CSS 变量）、材质回卡片、预览含真气泡类", () => {
    const { container } = render(<BubbleStyleCard />);
    expect(screen.getByLabelText("用户气泡颜色：跟随主题")).toBeInTheDocument();
    expect(container.querySelector(".chat-bubble-user")).toBeInTheDocument();
    expect(container.querySelector(".chat-bubble-bot")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "卡片" }).className).toContain(
      "nm-inset",
    );
    expect(
      document.documentElement.style.getPropertyValue("--bubble-user-bg"),
    ).toBe("");
  });

  it("点预设色板：CSS 变量即时生效 + 落盘", async () => {
    const user = userEvent.setup();
    render(<BubbleStyleCard />);
    await user.click(screen.getByLabelText("用户气泡颜色：玫红"));
    const root = document.documentElement;
    expect(root.style.getPropertyValue("--bubble-user-bg")).toBe("#b04a6a");
    expect(root.style.getPropertyValue("--bubble-user-fg")).toBe("#ffffff");
    expect(JSON.parse(localStorage.getItem(KEY)!)).toEqual({
      userBg: "#b04a6a",
      botMaterial: "card",
    });
    expect(screen.getByLabelText("用户气泡颜色：玫红").className).toContain(
      "ring-2",
    );
  });

  it("切机器人材质：data-bubble-bot 即时生效 + 落盘", async () => {
    const user = userEvent.setup();
    render(<BubbleStyleCard />);
    await user.click(screen.getByRole("button", { name: "扁平" }));
    expect(document.documentElement.dataset.bubbleBot).toBe("flat");
    expect(JSON.parse(localStorage.getItem(KEY)!)).toEqual({
      userBg: null,
      botMaterial: "flat",
    });
    await user.click(screen.getByRole("button", { name: "凹陷" }));
    expect(document.documentElement.dataset.bubbleBot).toBe("inset");
  });

  it("恢复默认：清 CSS 变量 + 回跟随主题", async () => {
    const user = userEvent.setup();
    render(<BubbleStyleCard />);
    await user.click(screen.getByLabelText("用户气泡颜色：蓝"));
    expect(
      document.documentElement.style.getPropertyValue("--bubble-user-bg"),
    ).toBe("#3563b0");
    await user.click(screen.getByRole("button", { name: "恢复默认" }));
    expect(
      document.documentElement.style.getPropertyValue("--bubble-user-bg"),
    ).toBe("");
    expect(JSON.parse(localStorage.getItem(KEY)!)).toEqual({
      userBg: null,
      botMaterial: "card",
    });
  });

  it("自定义拾色器输入：按亮度自动反白（浅色 → 深字）", async () => {
    const user = userEvent.setup();
    render(<BubbleStyleCard />);
    const input = screen.getByLabelText(
      "用户气泡颜色：自定义",
    ) as HTMLInputElement;
    await user.click(input);
    // React 值跟踪会吞掉直接赋值的 input 事件，需走原生 setter 触发 onChange
    const setNativeValue = Object.getOwnPropertyDescriptor(
      HTMLInputElement.prototype,
      "value",
    )!.set!;
    setNativeValue.call(input, "#ffd9a0");
    input.dispatchEvent(new Event("input", { bubbles: true }));
    expect(
      document.documentElement.style.getPropertyValue("--bubble-user-fg"),
    ).toBe("#171d2b");
    expect(
      document.documentElement.style.getPropertyValue("--bubble-user-bg"),
    ).toBe("#ffd9a0");
  });
});
