import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { FoldToggle } from "./FoldToggle";

describe("FoldToggle 可达性与表单安全", () => {
  it("type=button + aria-expanded 随 collapsed 翻转 + aria-label", () => {
    const { rerender } = render(
      <FoldToggle collapsed={false} onToggle={() => {}} />
    );
    const btn = screen.getByRole("button", { name: "收起" });
    expect(btn).toHaveAttribute("type", "button");
    expect(btn).toHaveAttribute("aria-expanded", "true");
    rerender(<FoldToggle collapsed={true} onToggle={() => {}} />);
    const btn2 = screen.getByRole("button", { name: "展开" });
    expect(btn2).toHaveAttribute("aria-expanded", "false");
  });

  it("点击调 onToggle（且 stopPropagation 不外冒）", async () => {
    const onToggle = vi.fn();
    const onOuter = vi.fn();
    const user = userEvent.setup();
    render(
      <div onClick={onOuter}>
        <FoldToggle collapsed={false} onToggle={onToggle} />
      </div>
    );
    await user.click(screen.getByRole("button", { name: "收起" }));
    expect(onToggle).toHaveBeenCalledTimes(1);
    expect(onOuter).not.toHaveBeenCalled();
  });
});
