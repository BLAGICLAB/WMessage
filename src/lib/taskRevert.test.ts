import { describe, expect, it } from "vitest";
import { revertPatch } from "./taskRevert";
import type { Task } from "../types";

const base: Task = {
  id: "t1",
  title: "原标题",
  column: "todo",
  note: "原备注",
  tags: ["设计"],
  subtasks: [{ id: "s1", text: "子任务甲", done: false }],
  files: [{ path: "/tmp/a.docx", isDir: false }],
  planStart: "2026-10-07T10:00",
  planEnd: "2026-10-07T11:00",
  due: "2026-10-09T18:00",
};

describe("revertPatch", () => {
  it("没改过任何字段 → 空补丁（不必回写）", () => {
    expect(revertPatch(base, { ...base })).toEqual({});
  });

  it("改了标题 → 只回写标题，且值取打开浮窗时的", () => {
    const after = { ...base, title: "改过的标题" };
    expect(revertPatch(base, after)).toEqual({ title: "原标题" });
  });

  it("改了多字段 → 只回写被动过的字段", () => {
    const after = {
      ...base,
      title: "改过的标题",
      planStart: "2026-10-08T14:00",
      planEnd: "2026-10-08T15:00",
    };
    expect(revertPatch(base, after)).toEqual({
      title: "原标题",
      planStart: "2026-10-07T10:00",
      planEnd: "2026-10-07T11:00",
    });
  });

  it("用户清掉的字段回写成打开时的值", () => {
    const after = { ...base, note: undefined, tags: undefined };
    const patch = revertPatch(base, after);
    expect(patch.note).toBe("原备注");
    expect(patch.tags).toEqual(["设计"]);
  });

  it("打开时本就为空的字段 → 回写 undefined（后端 null = 清空）", () => {
    const empty: Task = { id: "t2", title: "无备注卡", column: "todo" };
    const after = { ...empty, note: "临时写了点", tags: ["x"] };
    const patch = revertPatch(empty, after);
    expect("note" in patch).toBe(true);
    expect(patch.note).toBeUndefined();
    expect("tags" in patch).toBe(true);
    expect(patch.tags).toBeUndefined();
  });

  it("标记完成：column/completedAt/archived 一起回滚", () => {
    const after = {
      ...base,
      column: "done" as const,
      completedAt: 1760000000000,
      archived: false,
    };
    expect(revertPatch(base, after)).toEqual({
      column: "todo",
      completedAt: undefined,
      archived: undefined,
    });
  });

  it("botAssigned 不回滚：交给机器人是外发动作，机器人已经在跑", () => {
    const after = { ...base, botAssigned: true };
    expect(revertPatch(base, after)).toEqual({});
  });

  it("子任务/文件的差异按整体比对（顺序变化算改动）", () => {
    const after = {
      ...base,
      subtasks: [{ id: "s1", text: "子任务甲", done: true }],
    };
    expect(revertPatch(base, after)).toEqual({
      subtasks: [{ id: "s1", text: "子任务甲", done: false }],
    });
  });
});
