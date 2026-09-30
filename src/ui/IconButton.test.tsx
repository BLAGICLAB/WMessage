import { describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";
import { Trash2 } from "lucide-react";
import { IconButton } from "./IconButton";

describe("IconButton", () => {
  it("渲染为 type=button，带 nm-icon-btn 基材并追加使用处 className", () => {
    render(
      <IconButton className="w-5 h-5 rounded-full" title="删除">
        <Trash2 size={13} aria-hidden />
      </IconButton>,
    );
    const btn = screen.getByTitle("删除");
    expect(btn.tagName).toBe("BUTTON");
    expect(btn.getAttribute("type")).toBe("button");
    expect(btn.className).toContain("nm-icon-btn");
    expect(btn.className).toContain("rounded-full");
  });

  it("转发点击与 disabled", () => {
    const onClick = vi.fn();
    const { rerender } = render(
      <IconButton onClick={onClick} title="删除">
        <Trash2 size={13} aria-hidden />
      </IconButton>,
    );
    fireEvent.click(screen.getByTitle("删除"));
    expect(onClick).toHaveBeenCalledTimes(1);
    rerender(
      <IconButton onClick={onClick} disabled title="删除">
        <Trash2 size={13} aria-hidden />
      </IconButton>,
    );
    expect(screen.getByTitle("删除")).toBeDisabled();
  });
});
