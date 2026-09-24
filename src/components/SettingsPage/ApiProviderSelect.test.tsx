import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { ApiProviderSelect } from "./ApiProviderSelect";

describe("ApiProviderSelect listbox 语义与键盘导航", () => {
  it("触发钮 aria-haspopup/aria-expanded；浮层 listbox；选项 option/aria-selected", async () => {
    const user = userEvent.setup();
    render(<ApiProviderSelect value="openai" onChange={() => {}} />);
    const trigger = screen.getByRole("button", { name: /OpenAI 兼容/ });
    expect(trigger).toHaveAttribute("aria-haspopup", "listbox");
    expect(trigger).toHaveAttribute("aria-expanded", "false");
    await user.click(trigger);
    expect(trigger).toHaveAttribute("aria-expanded", "true");
    expect(trigger).toHaveAttribute("aria-controls");
    const listbox = screen.getByRole("listbox");
    expect(listbox.id).toBe(trigger.getAttribute("aria-controls"));
    const options = screen.getAllByRole("option");
    expect(options.length).toBeGreaterThanOrEqual(2);
    expect(options[0]).toHaveAttribute("aria-selected", "true");
    expect(options[1]).toHaveAttribute("aria-selected", "false");
    // aria-activedescendant 挂在持有焦点的触发钮上（active-descendant 模式）
    expect(trigger.getAttribute("aria-activedescendant")).toBe(options[0].id);
  });

  it("关闭态方向键直接展开（对齐原生 select）", async () => {
    render(<ApiProviderSelect value="openai" onChange={() => {}} />);
    const trigger = screen.getByRole("button", { name: /OpenAI 兼容/ });
    trigger.focus();
    await userEvent.setup().keyboard("{ArrowDown}");
    expect(screen.getByRole("listbox")).toBeInTheDocument();
  });

  it("Tab 离开时收起浮层；Enter 提交后焦点归还触发钮", async () => {
    const onChange = vi.fn();
    const user = userEvent.setup();
    render(<ApiProviderSelect value="openai" onChange={onChange} />);
    const trigger = screen.getByRole("button", { name: /OpenAI 兼容/ });
    await user.click(trigger);
    await user.keyboard("{Tab}");
    expect(screen.queryByRole("listbox")).not.toBeInTheDocument();
    await user.click(trigger);
    await user.keyboard("{ArrowDown}{Enter}");
    expect(onChange).toHaveBeenCalledWith("anthropic");
    expect(screen.queryByRole("listbox")).not.toBeInTheDocument();
    expect(document.activeElement).toBe(trigger);
  });

  it("方向键移动高亮 + Enter 提交 onChange；Escape 收起", async () => {
    const onChange = vi.fn();
    const user = userEvent.setup();
    render(<ApiProviderSelect value="openai" onChange={onChange} />);
    const trigger = screen.getByRole("button", { name: /OpenAI 兼容/ });
    await user.click(trigger);
    await user.keyboard("{ArrowDown}");
    const options = screen.getAllByRole("option");
    expect(trigger.getAttribute("aria-activedescendant")).toBe(options[1].id);
    await user.keyboard("{Enter}");
    expect(onChange).toHaveBeenCalledWith(options[1].id.replace(/.*-opt-/, ""));
    expect(screen.queryByRole("listbox")).not.toBeInTheDocument();
    // 再开 → Escape 收起
    await user.click(trigger);
    expect(screen.getByRole("listbox")).toBeInTheDocument();
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("listbox")).not.toBeInTheDocument();
  });

  it("点击选项即选即收（既有行为保持）", async () => {
    const onChange = vi.fn();
    const user = userEvent.setup();
    render(<ApiProviderSelect value="openai" onChange={onChange} />);
    await user.click(screen.getByRole("button", { name: /OpenAI 兼容/ }));
    await user.click(screen.getByRole("option", { name: /Anthropic 兼容/ }));
    expect(onChange).toHaveBeenCalledWith("anthropic");
    expect(screen.queryByRole("listbox")).not.toBeInTheDocument();
  });
});
