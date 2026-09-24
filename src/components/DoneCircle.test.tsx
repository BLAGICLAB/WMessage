import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { DoneCircle } from "./DoneCircle";

describe("DoneCircle", () => {
  it("type=button（form 内不触发 submit）", () => {
    render(<DoneCircle done={false} onToggle={() => {}} />);
    expect(screen.getByTitle("标记完成")).toHaveAttribute("type", "button");
  });

  it("点击调 onToggle 且不外冒", async () => {
    const onToggle = vi.fn();
    const onOuter = vi.fn();
    const user = userEvent.setup();
    render(
      <div onClick={onOuter}>
        <DoneCircle done={false} onToggle={onToggle} />
      </div>
    );
    await user.click(screen.getByTitle("标记完成"));
    expect(onToggle).toHaveBeenCalledTimes(1);
    expect(onOuter).not.toHaveBeenCalled();
  });
});
