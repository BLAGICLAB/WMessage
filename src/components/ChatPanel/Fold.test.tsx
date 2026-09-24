import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { Fold } from "./Fold";

describe("Fold 折叠块可达性", () => {
  it("type=button + aria-expanded 翻转 + aria-controls 指向展开面板 id", async () => {
    const user = userEvent.setup();
    render(<Fold title="思考过程">折叠内容</Fold>);
    const btn = screen.getByRole("button", { name: /思考过程/ });
    expect(btn).toHaveAttribute("type", "button");
    expect(btn).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByText("折叠内容")).not.toBeInTheDocument();
    await user.click(btn);
    expect(btn).toHaveAttribute("aria-expanded", "true");
    const panel = screen.getByText("折叠内容");
    expect(panel).toBeInTheDocument();
    // aria-controls 与面板 id 闭合
    expect(btn.getAttribute("aria-controls")).toBe(panel.id);
    await user.click(btn);
    expect(btn).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByText("折叠内容")).not.toBeInTheDocument();
  });
});
