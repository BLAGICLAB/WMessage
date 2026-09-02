import { describe, it, expect, vi } from "vitest";
import { spliceMove } from "./KanbanBoard";
import type { Task } from "../types";

// —— Tauri mocks（同 TrashPage.test）：KanbanBoard 模块 import TodoCard → 链到 profile/invoke，
// 本文件只测纯函数 spliceMove，但模块加载需要这些依赖不炸 ——
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

const t = (id: string, column: Task["column"]): Task => ({
  id,
  title: `任务${id}`,
  column,
});

// 第二梯队 #8（2026-09-03）：spliceMove 是拖放提交的唯一排序逻辑（handleDragEnd / 跨列 DragOver 共用），
// 此前零覆盖；以下覆盖同列/跨列/列空白区/边界 index。
describe("KanbanBoard.spliceMove", () => {
  it("同列移动 below=true：插到 over 之后，column 不变", () => {
    const flat = [t("a", "todo"), t("b", "todo"), t("c", "todo")];
    const next = spliceMove(flat, "a", "todo", "c", true);
    expect(next.map((x) => x.id)).toEqual(["b", "c", "a"]);
    expect(next.find((x) => x.id === "a")!.column).toBe("todo");
  });

  it("同列移动 below=false：插到 over 之前", () => {
    const flat = [t("a", "todo"), t("b", "todo"), t("c", "todo")];
    const next = spliceMove(flat, "c", "todo", "a", false);
    expect(next.map((x) => x.id)).toEqual(["c", "a", "b"]);
  });

  it("跨列移动：被拖任务 column 改为目标列，其余任务不受影响", () => {
    const flat = [t("a", "todo"), t("b", "todo"), t("d1", "doing")];
    const next = spliceMove(flat, "a", "doing", "d1", false);
    expect(next.map((x) => x.id)).toEqual(["b", "a", "d1"]);
    expect(next[1].column).toBe("doing");
    // 原数组对象不被改写（updated 是新对象）
    expect(flat[0].column).toBe("todo");
  });

  it("落到列空白区（overId 是列 id）：插到该列最后一个任务之后", () => {
    const flat = [t("a", "todo"), t("d1", "doing"), t("d2", "doing"), t("z", "done")];
    const next = spliceMove(flat, "a", "doing", "doing", false);
    // doing 列最后一个任务是 d2 → 插到 d2 后、z 前
    expect(next.map((x) => x.id)).toEqual(["d1", "d2", "a", "z"]);
    expect(next[2].column).toBe("doing");
  });

  it("落到空列（该列无任务）：插到数组末尾", () => {
    const flat = [t("a", "todo"), t("b", "todo")];
    const next = spliceMove(flat, "a", "done", "done", false);
    expect(next.map((x) => x.id)).toEqual(["b", "a"]);
    expect(next[1].column).toBe("done");
  });

  it("activeId 不存在：原样返回（同引用），不改 column", () => {
    const flat = [t("a", "todo")];
    expect(spliceMove(flat, "ghost", "done", "a", true)).toBe(flat);
  });

  it("overId 既非列也非任务（边界）：退化为插到数组末尾", () => {
    const flat = [t("a", "todo"), t("b", "todo")];
    const next = spliceMove(flat, "a", "todo", "ghost", false);
    expect(next.map((x) => x.id)).toEqual(["b", "a"]);
  });

  it("输入数组不被 mutate（返回新数组，原顺序不变）", () => {
    const flat = [t("a", "todo"), t("b", "todo"), t("c", "todo")];
    spliceMove(flat, "a", "todo", "c", true);
    expect(flat.map((x) => x.id)).toEqual(["a", "b", "c"]);
  });
});
