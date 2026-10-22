// useTaskFiles 行为锁：合并基线必须是「对话框返回后的最新 files」。
//
// 场景（实机卡 S21）：用户在详情面板点「绑定文件」→ 系统文件对话框弹着，
// 这期间**另一个窗口**给同一张卡绑了文件；用户回来选完 → 两批文件都该在。
// 拿渲染期快照当基线时，后到的那批会把先到的那批**整列覆盖**掉
// （用户看到「另一个窗口绑的文件凭空消失」）。

import { describe, expect, it, vi, beforeEach } from "vitest";
import { renderHook, act } from "@testing-library/react";

const openMock = vi.fn();
vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: (...args: unknown[]) => openMock(...args),
}));

const invokeMock = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

import { useTaskFiles } from "./useTaskFiles";
import type { Task } from "../types";

const taskWith = (paths: string[]): Task => ({
  id: "t1",
  title: "x",
  column: "todo",
  files: paths.map((path) => ({ path, isDir: false })),
});

describe("useTaskFiles 合并基线", () => {
  beforeEach(() => {
    openMock.mockReset();
    invokeMock.mockReset();
  });

  it("对话框挂起期间并发绑进来的那批不被覆盖掉（基线取返回后的最新 files）", async () => {
    let resolveOpen: (v: unknown) => void = () => {};
    openMock.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          resolveOpen = resolve;
        }),
    );
    // 后端判 isDir 后回传
    invokeMock.mockResolvedValueOnce([{ path: "/c.txt", isDir: false }]);

    const commit = vi.fn();
    const { result, rerender } = renderHook(
      ({ t }: { t: Task }) => useTaskFiles(t, commit),
      { initialProps: { t: taskWith(["/a.txt"]) } },
    );

    // 点「绑定文件」→ 系统对话框弹着（pending）
    let pending: Promise<void> = Promise.resolve();
    act(() => {
      pending = result.current.pickFile();
    });

    // 对话框挂着期间，另一个窗口给同一张卡绑了 /b.txt（父组件重渲染出新 task）
    rerender({ t: taskWith(["/a.txt", "/b.txt"]) });

    // 用户选完 /c.txt 回来
    await act(async () => {
      resolveOpen("/c.txt");
      await pending;
    });

    expect(commit).toHaveBeenCalledWith([
      { path: "/a.txt", isDir: false },
      { path: "/b.txt", isDir: false },
      { path: "/c.txt", isDir: false },
    ]);
  });

  it("没有并发改动时，按当前绑定追加（基线就是那份现值）", async () => {
    openMock.mockResolvedValueOnce("/c.txt");
    invokeMock.mockResolvedValueOnce([{ path: "/c.txt", isDir: false }]);

    const commit = vi.fn();
    const { result } = renderHook(() => useTaskFiles(taskWith(["/a.txt"]), commit));

    await act(async () => {
      await result.current.pickFile();
    });

    expect(commit).toHaveBeenCalledWith([
      { path: "/a.txt", isDir: false },
      { path: "/c.txt", isDir: false },
    ]);
  });
});
