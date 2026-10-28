// useFlashMsg：自动消退提示的时序契约（重置旧计时器 / 卸载清理 / 驻留档）。
// 此前八个手写副本里两处漏了卸载清理（卸载后 setState + 定时器泄漏），此测试钉住。
import { act, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useFlashMsg } from "./useFlashMsg";

describe("useFlashMsg", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it("flash 后按默认时长自动清空；重设会清旧计时器", () => {
    const { result } = renderHook(() => useFlashMsg(3000));
    const [, flash] = result.current;
    act(() => flash("第一条"));
    expect(result.current[0]).toBe("第一条");
    act(() => vi.advanceTimersByTime(2000));
    act(() => flash("第二条"));
    act(() => vi.advanceTimersByTime(2999));
    expect(result.current[0]).toBe("第二条");
    act(() => vi.advanceTimersByTime(1));
    expect(result.current[0]).toBe("");
  });

  it("自定义时长生效；ms=0 表示持续显示不自动清", () => {
    const { result } = renderHook(() => useFlashMsg(3000));
    const [, flash] = result.current;
    act(() => flash("五秒档", 5000));
    act(() => vi.advanceTimersByTime(4999));
    expect(result.current[0]).toBe("五秒档");
    act(() => vi.advanceTimersByTime(1));
    expect(result.current[0]).toBe("");

    act(() => flash("驻留", 0));
    act(() => vi.advanceTimersByTime(60_000));
    expect(result.current[0]).toBe("驻留");
  });

  it("卸载后清掉挂起计时器，不产生迟到 setState", () => {
    const { result, unmount } = renderHook(() => useFlashMsg(3000));
    const [, flash] = result.current;
    act(() => flash("即将卸载"));
    unmount();
    expect(() => vi.advanceTimersByTime(10_000)).not.toThrow();
  });
});
