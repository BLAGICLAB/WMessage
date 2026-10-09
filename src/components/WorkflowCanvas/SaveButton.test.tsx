import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { SaveButton } from "./SaveButton";

beforeEach(() => {
  vi.useFakeTimers();
});
afterEach(() => {
  vi.useRealTimers();
  cleanup();
});

const advance = async (ms: number) => {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(ms);
  });
};

describe("SaveButton 基础态", () => {
  it("idle：渲染保存图标，无「已保存」", () => {
    render(<SaveButton dirty saving={false} onSave={vi.fn()} />);
    const btn = screen.getByTestId("save-btn");
    expect(btn).toHaveAttribute("data-phase", "idle");
    expect(btn.textContent).not.toContain("已保存");
  });

  it("dirty=false → 禁用，无脏点", () => {
    render(<SaveButton dirty={false} saving={false} onSave={vi.fn()} />);
    const btn = screen.getByTestId("save-btn");
    expect(btn).toBeDisabled();
    expect(screen.queryByTestId("save-btn-dot")).toBeNull();
  });

  it("dirty=true → 可用 + 脏点出现", () => {
    render(<SaveButton dirty saving={false} onSave={vi.fn()} />);
    const btn = screen.getByTestId("save-btn");
    expect(btn).not.toBeDisabled();
    expect(screen.getByTestId("save-btn-dot")).toBeTruthy();
  });

  it("saving=true（外部 busy）→ 禁用", () => {
    render(<SaveButton dirty saving onSave={vi.fn()} />);
    expect(screen.getByTestId("save-btn")).toBeDisabled();
  });
});

describe("SaveButton 保存流", () => {
  it("点击 → pressed → done（对勾 + 已保存）", async () => {
    const onSave = vi.fn().mockResolvedValue(true);
    render(<SaveButton dirty saving={false} onSave={onSave} />);
    const btn = screen.getByTestId("save-btn");
    fireEvent.pointerDown(btn, { button: 0, pointerId: 1 });
    expect(btn).toHaveAttribute("data-phase", "pressed");
    fireEvent.pointerUp(btn, { pointerId: 1 });
    await advance(200); // 过 MIN_PRESS_MS，onSave 已 resolve
    expect(onSave).toHaveBeenCalledTimes(1);
    expect(btn).toHaveAttribute("data-phase", "done");
    expect(btn.textContent).toContain("已保存");
  });

  it("onSave 返回 false → 回 idle，不显示已保存", async () => {
    const onSave = vi.fn().mockResolvedValue(false);
    render(<SaveButton dirty saving={false} onSave={onSave} />);
    const btn = screen.getByTestId("save-btn");
    fireEvent.pointerDown(btn, { button: 0, pointerId: 1 });
    fireEvent.pointerUp(btn, { pointerId: 1 });
    await advance(200);
    expect(btn).toHaveAttribute("data-phase", "idle");
    expect(btn.textContent).not.toContain("已保存");
  });

  it("onSave 抛错 → 回 idle（错误由父级兜底）", async () => {
    const onSave = vi.fn().mockRejectedValue(new Error("boom"));
    render(<SaveButton dirty saving={false} onSave={onSave} />);
    const btn = screen.getByTestId("save-btn");
    fireEvent.pointerDown(btn, { button: 0, pointerId: 1 });
    fireEvent.pointerUp(btn, { pointerId: 1 });
    await advance(200);
    expect(btn).toHaveAttribute("data-phase", "idle");
  });

  it("done 后 ~1.4s 回 idle", async () => {
    const onSave = vi.fn().mockResolvedValue(true);
    render(<SaveButton dirty saving={false} onSave={onSave} />);
    const btn = screen.getByTestId("save-btn");
    fireEvent.pointerDown(btn, { button: 0, pointerId: 1 });
    fireEvent.pointerUp(btn, { pointerId: 1 });
    await advance(200);
    expect(btn).toHaveAttribute("data-phase", "done");
    await advance(1500);
    expect(btn).toHaveAttribute("data-phase", "idle");
  });

  it("键盘 Enter 触发保存", async () => {
    const onSave = vi.fn().mockResolvedValue(true);
    render(<SaveButton dirty saving={false} onSave={onSave} />);
    const btn = screen.getByTestId("save-btn");
    btn.focus();
    fireEvent.keyDown(btn, { key: "Enter" });
    await advance(200);
    expect(onSave).toHaveBeenCalledTimes(1);
    expect(btn).toHaveAttribute("data-phase", "done");
  });

  it("按下后 pointerleave 取消，不触发保存", async () => {
    const onSave = vi.fn().mockResolvedValue(true);
    render(<SaveButton dirty saving={false} onSave={onSave} />);
    const btn = screen.getByTestId("save-btn");
    fireEvent.pointerDown(btn, { button: 0, pointerId: 1 });
    fireEvent.pointerLeave(btn, { pointerId: 1 });
    await advance(300);
    expect(onSave).not.toHaveBeenCalled();
    expect(btn).toHaveAttribute("data-phase", "idle");
  });

  it("右键不触发", () => {
    const onSave = vi.fn().mockResolvedValue(true);
    render(<SaveButton dirty saving={false} onSave={onSave} />);
    const btn = screen.getByTestId("save-btn");
    fireEvent.pointerDown(btn, { button: 2, pointerId: 1 });
    expect(btn).toHaveAttribute("data-phase", "idle");
  });

  it("保存中卸载不抛错、不泄漏定时器", async () => {
    const onSave = vi.fn(() => new Promise(() => {})); // 永不 resolve
    const { unmount } = render(<SaveButton dirty saving={false} onSave={onSave} />);
    const btn = screen.getByTestId("save-btn");
    fireEvent.pointerDown(btn, { button: 0, pointerId: 1 });
    fireEvent.pointerUp(btn, { pointerId: 1 });
    await advance(200);
    expect(() => unmount()).not.toThrow();
    expect(() => vi.advanceTimersByTime(3000)).not.toThrow();
  });
});
