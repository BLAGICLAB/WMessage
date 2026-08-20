import { describe, it, expect, vi, beforeEach } from "vitest";

const invokeMock = vi.hoisted(() => vi.fn());

vi.mock("@tauri-apps/api/core", () => ({
  invoke: invokeMock,
}));

beforeEach(() => {
  invokeMock.mockReset();
  // 模块级缓存（cache/extSet/maxTaskFiles 活绑定）需每用例重建
  vi.resetModules();
});

describe("loadAppConsts", () => {
  it("invoke 正常返回时缓存后端值，maxTaskFiles 活绑定经 taskFiles 同步", async () => {
    invokeMock.mockResolvedValue({
      max_task_files: 3,
      max_title: 50,
      max_note: 60,
      image_exts: ["png"],
    });
    const consts = await import("./consts");
    const loaded = await consts.loadAppConsts();

    expect(loaded.max_task_files).toBe(3);
    expect(consts.appConsts()).toEqual({
      max_task_files: 3,
      max_title: 50,
      max_note: 60,
      image_exts: ["png"],
    });
    expect(consts.imageExtSet().has("png")).toBe(true);
    expect(consts.imageExtSet().has("jpg")).toBe(false);

    // taskFiles 的 MAX_TASK_FILES 是 consts 活绑定的 re-export，同步更新
    const { MAX_TASK_FILES } = await import("./taskFiles");
    expect(MAX_TASK_FILES).toBe(3);
  });

  it("invoke 失败时回退硬编码值（纯前端测试环境可用）", async () => {
    invokeMock.mockRejectedValue(new Error("no tauri runtime"));
    const consts = await import("./consts");
    const loaded = await consts.loadAppConsts();

    expect(loaded).toEqual(consts.FALLBACK_CONSTS);
    expect(consts.appConsts()).toEqual(consts.FALLBACK_CONSTS);
    expect(consts.imageExtSet()).toEqual(new Set(consts.FALLBACK_CONSTS.image_exts));

    const { MAX_TASK_FILES } = await import("./taskFiles");
    expect(MAX_TASK_FILES).toBe(consts.FALLBACK_CONSTS.max_task_files);
  });
});
