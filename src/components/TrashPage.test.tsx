import { describe, it, expect, vi } from "vitest";
import { render } from "@testing-library/react";
import { TrashPage } from "./TrashPage";
import type { Task } from "../types";

// —— Tauri mocks（同 TodoCard.test：TodoCard → ActorAvatar 会走 profile/invoke）——
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(async () => null),
  convertFileSrc: vi.fn((p: string) => `tauri://localhost/${p}`),
}));

vi.mock("@tauri-apps/api/event", () => ({
  emit: vi.fn(async () => {}),
  listen: vi.fn(async () => () => {}),
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: vi.fn(async () => null),
}));

vi.mock("@tauri-apps/plugin-opener", () => ({
  openPath: vi.fn(async () => {}),
}));

vi.mock("@tauri-apps/api/webviewWindow", () => ({
  WebviewWindow: { getByLabel: vi.fn(async () => null) },
}));

vi.mock("../profile", () => ({
  getProfileCache: vi.fn(() => ({
    user: { name: "Test", avatarDataUrl: null },
    bot: { name: "Bot", avatarDataUrl: null },
  })),
  loadProfile: vi.fn(async () => ({
    user: { name: "Test", avatarDataUrl: null },
    bot: { name: "Bot", avatarDataUrl: null },
  })),
  subscribeProfile: vi.fn(() => () => {}),
}));

// P2-22（2026-08-19）：回收站列表按 updatedAt 倒序（新删的在前）——
// 修复前不排序，展示顺序依赖任务数组原始 order，删除时间与位置对不上
describe("TrashPage 按 updatedAt 倒序（P2-22）", () => {
  it("3 个回收站任务按 updatedAt 倒序渲染", () => {
    const tasks: Task[] = [
      { id: "a", title: "最老删除", column: "todo", deletedAt: 1, updatedAt: 100, order: 0 },
      { id: "b", title: "最新删除", column: "todo", deletedAt: 3, updatedAt: 300, order: 1 },
      { id: "c", title: "中间删除", column: "todo", deletedAt: 2, updatedAt: 200, order: 2 },
    ];
    const { container } = render(
      <TrashPage
        tasks={tasks}
        onUpdate={vi.fn()}
        onDelete={vi.fn()}
      />
    );
    const titles = [...container.querySelectorAll("h3")].map((h) => h.textContent);
    expect(titles).toEqual(["最新删除", "中间删除", "最老删除"]);
  });

  it("updatedAt 缺失按 0 处理（排最后），不崩溃", () => {
    const tasks: Task[] = [
      { id: "a", title: "无时间戳", column: "todo", deletedAt: 1, order: 0 },
      { id: "b", title: "有时间戳", column: "todo", deletedAt: 2, updatedAt: 50, order: 1 },
    ];
    const { container } = render(
      <TrashPage
        tasks={tasks}
        onUpdate={vi.fn()}
        onDelete={vi.fn()}
      />
    );
    const titles = [...container.querySelectorAll("h3")].map((h) => h.textContent);
    expect(titles).toEqual(["有时间戳", "无时间戳"]);
  });
});
