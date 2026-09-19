import { describe, expect, it, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import "@testing-library/jest-dom/vitest";
import { Toggle } from "./Toggle";

describe("Toggle", () => {
  it("checked=true → bg 红 + role=switch aria-checked=true", () => {
    render(<Toggle checked onChange={() => {}} />);
    const btn = screen.getByRole("switch");
    expect(btn).toHaveAttribute("aria-checked", "true");
    expect(btn).toHaveClass("bg-[#ef4444]");
  });

  it("checked=false → bg 灰 + aria-checked=false", () => {
    render(<Toggle checked={false} onChange={() => {}} />);
    const btn = screen.getByRole("switch");
    expect(btn).toHaveAttribute("aria-checked", "false");
    expect(btn.className).toContain("bg-[var(--border)]");
  });

  it("点击切换 onChange(true→false→true)", () => {
    const cb = vi.fn();
    render(<Toggle checked={true} onChange={cb} />);
    fireEvent.click(screen.getByRole("switch"));
    expect(cb).toHaveBeenCalledWith(false);
  });

  it("disabled=true 不触发 onChange", () => {
    const cb = vi.fn();
    render(<Toggle checked={false} onChange={cb} disabled />);
    fireEvent.click(screen.getByRole("switch"));
    expect(cb).not.toHaveBeenCalled();
  });

  it("ariaLabel 透传", () => {
    render(<Toggle checked={false} onChange={() => {}} ariaLabel="启用提案" />);
    expect(screen.getByLabelText("启用提案")).toBeInTheDocument();
  });

  it("size=sm 切换尺寸", () => {
    render(<Toggle checked={false} onChange={() => {}} size="sm" />);
    expect(screen.getByRole("switch").className).toContain("w-9 h-5");
  });
});
