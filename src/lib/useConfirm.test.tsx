// useConfirm hook 行为锁：把 plugin-dialog ask 包成「返回 Promise<boolean>」的 hook。
// vitest 用 vi.mock 替换 @tauri-apps/plugin-dialog 拿到的 ask 实现。

import { describe, expect, it, vi, beforeEach } from "vitest";
import { renderHook, act } from "@testing-library/react";

const askMock = vi.fn();
vi.mock("@tauri-apps/plugin-dialog", () => ({
  ask: (...args: unknown[]) => askMock(...args),
}));

import { useConfirm } from "./useConfirm";

describe("useConfirm 默认行为", () => {
  beforeEach(() => {
    askMock.mockReset();
  });

  it("字符串 message 直传：默认 title/kind/按钮文案 + ask 返回值透传", async () => {
    askMock.mockResolvedValueOnce(true);
    const { result } = renderHook(() => useConfirm());
    let ok: boolean | undefined;
    await act(async () => {
      ok = await result.current("删除模板「foo」？");
    });
    expect(ok).toBe(true);
    expect(askMock).toHaveBeenCalledWith("删除模板「foo」？", {
      title: "确认操作",
      kind: "warning",
      okLabel: "确定",
      cancelLabel: "取消",
    });
  });

  it("options 形式：自定义 title/kind/按钮文案全部透传", async () => {
    askMock.mockResolvedValueOnce(false);
    const { result } = renderHook(() => useConfirm());
    let ok: boolean | undefined;
    await act(async () => {
      ok = await result.current({
        message: "确定清空所有工作流的审计记录？",
        title: "清空审计",
        kind: "error",
        okLabel: "清空",
        cancelLabel: "我再想想",
      });
    });
    expect(ok).toBe(false);
    expect(askMock).toHaveBeenCalledWith("确定清空所有工作流的审计记录？", {
      title: "清空审计",
      kind: "error",
      okLabel: "清空",
      cancelLabel: "我再想想",
    });
  });

  it("options 形式：只覆盖部分字段，未覆盖走默认", async () => {
    askMock.mockResolvedValueOnce(true);
    const { result } = renderHook(() => useConfirm());
    await act(async () => {
      await result.current({ message: "删模型？" });
    });
    expect(askMock).toHaveBeenCalledWith("删模型？", {
      title: "确认操作",
      kind: "warning",
      okLabel: "确定",
      cancelLabel: "取消",
    });
  });

  it("ask 抛错（plugin 不可用/无头环境）：返回 false 拒绝（拒绝比误通过安全）", async () => {
    askMock.mockRejectedValueOnce(new Error("dialog not available"));
    const { result } = renderHook(() => useConfirm());
    let ok: boolean | undefined;
    await act(async () => {
      ok = await result.current("不可逆操作？");
    });
    expect(ok).toBe(false);
  });
});
