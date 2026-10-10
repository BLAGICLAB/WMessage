import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
} from "@testing-library/react";
import { HoldToConfirmDelete, HOLD_MS } from "./HoldToConfirmDelete";

beforeEach(() => {
  // rAF / performance.now / setTimeout 全部进 fake 域
  // （vi.useFakeTimers 默认 toFake 含 requestAnimationFrame + performance.now）
  vi.useFakeTimers();
});

afterEach(() => {
  vi.useRealTimers();
  cleanup();
});

/** 推进时间并 flush React 状态。组件内部用 rAF 推进 + setTimeout 触发
 *  shake/done，无 act 包裹时 React 19 的并发更新不会落到 DOM 上。
 *  模 ProfileRow 范式：act 包 vi.advanceTimersByTime */
const advance = async (ms: number) => {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(ms);
  });
};

describe("HoldToConfirmDelete 基础形态", () => {
  it("挂载后渲染 trash 图标、phase=idle、aria-label 透传", () => {
    const onConfirm = vi.fn();
    render(<HoldToConfirmDelete onConfirm={onConfirm} ariaLabel="删除 X" />);
    const btn = screen.getByTestId("hold-confirm-delete");
    expect(btn).toBeInstanceOf(HTMLButtonElement);
    expect(btn).toHaveAttribute("data-phase", "idle");
    expect(btn).toHaveAttribute("data-progress", "0.000");
    expect(btn).toHaveAttribute("aria-label", "删除 X");
    expect(btn).toHaveAttribute("aria-busy", "false");
    expect(btn.querySelector(".lucide-trash-2")).toBeTruthy();
    expect(btn.textContent).not.toContain("已删除");
  });

  it("disabled 时不响应 pointerdown", () => {
    const onConfirm = vi.fn();
    render(<HoldToConfirmDelete onConfirm={onConfirm} disabled />);
    const btn = screen.getByTestId("hold-confirm-delete");
    fireEvent.pointerDown(btn, { button: 0, pointerId: 1 });
    expect(btn).toHaveAttribute("data-phase", "idle");
  });

  it("右键 pointerdown 不触发", () => {
    const onConfirm = vi.fn();
    render(<HoldToConfirmDelete onConfirm={onConfirm} />);
    const btn = screen.getByTestId("hold-confirm-delete");
    // button !== 0 一律忽略（鼠标右键/中间键；触摸/笔恒为 0）
    fireEvent.pointerDown(btn, { button: 2, pointerId: 1 });
    expect(btn).toHaveAttribute("data-phase", "idle");
  });
});

describe("HoldToConfirmDelete 长按流程", () => {
  it("pointerdown 后 phase=pressing、progress 持续推进", async () => {
    const onConfirm = vi.fn();
    render(<HoldToConfirmDelete onConfirm={onConfirm} />);
    const btn = screen.getByTestId("hold-confirm-delete");
    fireEvent.pointerDown(btn, { button: 0, pointerId: 1 });
    expect(btn).toHaveAttribute("data-phase", "pressing");
    await advance(HOLD_MS / 2);
    // 进度约 0.5（允许小误差）
    const p = parseFloat(btn.getAttribute("data-progress") ?? "0");
    expect(p).toBeGreaterThan(0.3);
    expect(p).toBeLessThan(0.7);
  });

  it("中途松手 → 进度倒退归零 → 回到 idle", async () => {
    const onConfirm = vi.fn();
    render(<HoldToConfirmDelete onConfirm={onConfirm} />);
    const btn = screen.getByTestId("hold-confirm-delete");
    fireEvent.pointerDown(btn, { button: 0, pointerId: 1 });
    // 推进到约 50% 进度
    await advance(HOLD_MS / 2);
    const before = parseFloat(btn.getAttribute("data-progress") ?? "0");
    expect(before).toBeGreaterThan(0);
    // 松手：触发反向
    fireEvent.pointerUp(btn, { pointerId: 1 });
    expect(btn).toHaveAttribute("data-phase", "releasing");
    // 倒退用满 HOLD_MS（从 0.5 退到 0 用 0.5 * HOLD_MS）
    await advance(HOLD_MS + 50);
    expect(btn).toHaveAttribute("data-phase", "idle");
    expect(btn).toHaveAttribute("data-progress", "0.000");
    expect(onConfirm).not.toHaveBeenCalled();
  });

  it("pointerleave 等价于 pointerup（中途退出视口同样倒退）", async () => {
    const onConfirm = vi.fn();
    render(<HoldToConfirmDelete onConfirm={onConfirm} />);
    const btn = screen.getByTestId("hold-confirm-delete");
    fireEvent.pointerDown(btn, { button: 0, pointerId: 1 });
    await advance(HOLD_MS / 4);
    fireEvent.pointerLeave(btn, { pointerId: 1 });
    expect(btn).toHaveAttribute("data-phase", "releasing");
    await advance(HOLD_MS);
    expect(btn).toHaveAttribute("data-phase", "idle");
    expect(onConfirm).not.toHaveBeenCalled();
  });

  it("走满瞬间类名含 shake-once（震动已触发）", async () => {
    const onConfirm = vi.fn();
    render(<HoldToConfirmDelete onConfirm={onConfirm} />);
    const btn = screen.getByTestId("hold-confirm-delete");
    fireEvent.pointerDown(btn, { button: 0, pointerId: 1 });
    // 走满后 50ms——还在震动态，shake class 应在
    await advance(HOLD_MS + 50);
    expect(btn.className).toContain("hold-confirm-shake-once");
  });

  it("按满 HOLD_MS → 触发 onConfirm、phase=done、显示对勾 + 「已删除」", async () => {
    const onConfirm = vi.fn();
    render(<HoldToConfirmDelete onConfirm={onConfirm} />);
    const btn = screen.getByTestId("hold-confirm-delete");
    fireEvent.pointerDown(btn, { button: 0, pointerId: 1 });
    // 走满 → 震动 → done + onConfirm：HOLD_MS + SHAKE_MS(360) + buffer
    await advance(HOLD_MS + 360 + 80);
    expect(btn).toHaveAttribute("data-phase", "done");
    expect(btn).toHaveAttribute("data-progress", "1.000");
    expect(btn.className).toContain("is-done");
    expect(btn.textContent).toContain("已删除");
    expect(btn.querySelector(".lucide-check")).toBeTruthy();
    expect(btn).toHaveAttribute("aria-busy", "true");
    expect(onConfirm).toHaveBeenCalledTimes(1);
  });

  it("done 态 ~1.2s 后自动复位到 idle（不依赖父组件卸载）", async () => {
    const onConfirm = vi.fn();
    render(<HoldToConfirmDelete onConfirm={onConfirm} />);
    const btn = screen.getByTestId("hold-confirm-delete");
    fireEvent.pointerDown(btn, { button: 0, pointerId: 1 });
    // 拆分多次 advance：单次大跳跃 + act 嵌套在 rAF/setTimeout 链下
    // 偶发丢最终 setState（react-19 自动批处理跨 timer 不收敛）
    await advance(HOLD_MS); // 走满 + 震动态
    await advance(360 + 80); // 震动结束 → done + onConfirm
    expect(btn).toHaveAttribute("data-phase", "done");
    await advance(1300); // DONE_HOLD_MS 展示 + 复位
    expect(btn).toHaveAttribute("data-phase", "idle");
    expect(btn).toHaveAttribute("data-progress", "0.000");
  });

  it("done 期间再次 pointerdown 不开新一轮（避免重复触发）", async () => {
    const onConfirm = vi.fn();
    render(<HoldToConfirmDelete onConfirm={onConfirm} />);
    const btn = screen.getByTestId("hold-confirm-delete");
    fireEvent.pointerDown(btn, { button: 0, pointerId: 1 });
    await advance(HOLD_MS + 360 + 80);
    expect(btn).toHaveAttribute("data-phase", "done");
    // 再 pointerdown：组件拒绝，phase 不变
    fireEvent.pointerDown(btn, { button: 0, pointerId: 2 });
    expect(btn).toHaveAttribute("data-phase", "done");
    // onConfirm 仍只触发一次
    expect(onConfirm).toHaveBeenCalledTimes(1);
  });
});

describe("HoldToConfirmDelete 键盘可达", () => {
  it("Enter 单次直接确认（无 hold、无震动）", async () => {
    const onConfirm = vi.fn();
    render(<HoldToConfirmDelete onConfirm={onConfirm} />);
    const btn = screen.getByTestId("hold-confirm-delete");
    btn.focus();
    fireEvent.keyDown(btn, { key: "Enter" });
    expect(btn).toHaveAttribute("data-phase", "done");
    expect(btn).toHaveAttribute("data-progress", "1.000");
    // 80ms 后 onConfirm 触发
    await advance(120);
    expect(onConfirm).toHaveBeenCalledTimes(1);
  });

  it("Space 与 Enter 等价", () => {
    const onConfirm = vi.fn();
    render(<HoldToConfirmDelete onConfirm={onConfirm} />);
    const btn = screen.getByTestId("hold-confirm-delete");
    btn.focus();
    fireEvent.keyDown(btn, { key: " " });
    expect(btn).toHaveAttribute("data-phase", "done");
  });

  it("其他键不触发", () => {
    const onConfirm = vi.fn();
    render(<HoldToConfirmDelete onConfirm={onConfirm} />);
    const btn = screen.getByTestId("hold-confirm-delete");
    fireEvent.keyDown(btn, { key: "a" });
    expect(btn).toHaveAttribute("data-phase", "idle");
  });

  it("disabled 时键盘不响应", () => {
    const onConfirm = vi.fn();
    render(<HoldToConfirmDelete onConfirm={onConfirm} disabled />);
    const btn = screen.getByTestId("hold-confirm-delete");
    fireEvent.keyDown(btn, { key: "Enter" });
    expect(btn).toHaveAttribute("data-phase", "idle");
    expect(onConfirm).not.toHaveBeenCalled();
  });
});

describe("HoldToConfirmDelete 生命周期", () => {
  it("按下中卸载：清理 rAF、setTimeout，不抛错、不触发 onConfirm", async () => {
    const onConfirm = vi.fn();
    const { unmount } = render(<HoldToConfirmDelete onConfirm={onConfirm} />);
    const btn = screen.getByTestId("hold-confirm-delete");
    fireEvent.pointerDown(btn, { button: 0, pointerId: 1 });
    await advance(200);
    expect(() => unmount()).not.toThrow();
    // 推进时间也不应触发任何残留
    await advance(HOLD_MS * 2);
    expect(onConfirm).not.toHaveBeenCalled();
  });

  it("走满瞬间卸载：shake/done 计时器清理，不触发 onConfirm", async () => {
    const onConfirm = vi.fn();
    const { unmount } = render(<HoldToConfirmDelete onConfirm={onConfirm} />);
    const btn = screen.getByTestId("hold-confirm-delete");
    fireEvent.pointerDown(btn, { button: 0, pointerId: 1 });
    // 走满：shake/done 计时器刚排进队列
    await advance(HOLD_MS + 5);
    expect(() => unmount()).not.toThrow();
    await advance(500);
    expect(onConfirm).not.toHaveBeenCalled();
  });
});
