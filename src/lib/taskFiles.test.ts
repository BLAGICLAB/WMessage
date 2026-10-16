import { describe, it, expect } from "vitest";
import { taskFiles, filesPatch, mergeFiles, MAX_TASK_FILES } from "./taskFiles";
import type { Task } from "../types";

const base: Task = { id: "t1", title: "x", column: "todo" };

describe("taskFiles", () => {
  it("返回 files 绑定；未绑定为空", () => {
    expect(taskFiles(base)).toEqual([]);
    expect(taskFiles({ ...base, files: [{ path: "/a.pdf", isDir: false }] })).toEqual([
      { path: "/a.pdf", isDir: false },
    ]);
  });
});

describe("filesPatch", () => {
  it("写 files；空列表也整列表提交（后端契约：显式 null/[] = 清空，缺键 = 保留）", () => {
    const p = filesPatch([
      { path: "/a.pdf", isDir: false },
      { path: "/dir", isDir: true },
    ]);
    expect(p.files).toHaveLength(2);

    const empty = filesPatch([]);
    expect(empty.files).toEqual([]);
    // 清空必须显式提交空数组：undefined 键会被 JSON 序列化丢弃，
    // 后端按「未提供」保留旧值 → 已解绑的旧路径复活
    const wire = JSON.parse(JSON.stringify(empty));
    expect(wire).toHaveProperty("files", []);
  });
});

describe("mergeFiles", () => {
  it("文件追加：同路径去重（小写归一），同名不同路径视为不同条目", () => {
    const cur = [{ path: "/a.pdf", isDir: false }];
    const m = mergeFiles(cur, [
      { path: "/a.pdf", isDir: false }, // 同路径 → 去重
      { path: "/A.pdf", isDir: false }, // 小写归一后同路径 → 去重
      { path: "/d/a.pdf", isDir: false }, // 同名不同路径 → 照样新增
      { path: "/b.pdf", isDir: false },
    ]);
    expect(m.applied).toBe(2);
    expect(m.files.map((f) => f.path)).toEqual(["/a.pdf", "/d/a.pdf", "/b.pdf"]);
    expect(m.truncated).toBe(false);
  });

  it("文件夹槽替换：新文件夹占旧文件夹原位，文件条目与顺序不动；同路径重复绑无操作", () => {
    const cur = [
      { path: "/a.pdf", isDir: false },
      { path: "/old/dir", isDir: true },
      { path: "/b.pdf", isDir: false },
    ];
    const m = mergeFiles(cur, [{ path: "/new/dir", isDir: true }]);
    expect(m.applied).toBe(1);
    expect(m.files.map((f) => f.path)).toEqual([
      "/a.pdf",
      "/new/dir",
      "/b.pdf",
    ]);
    // 同路径重复绑 → applied=0，调用方免写库
    const dup = mergeFiles(m.files, [{ path: "/NEW/DIR", isDir: true }]);
    expect(dup.applied).toBe(0);
    expect(dup.files.map((f) => f.path)).toEqual([
      "/a.pdf",
      "/new/dir",
      "/b.pdf",
    ]);
  });

  it("超上限截断并标记 truncated；满员时文件夹替换仍生效（不占新名额）", () => {
    // 上限：9 + 3 → 10，truncated
    const nine = Array.from({ length: 9 }, (_, i) => ({
      path: `/f/${i}.txt`,
      isDir: false,
    }));
    const capped = mergeFiles(nine, [
      { path: "/f/9.txt", isDir: false },
      { path: "/f/10.txt", isDir: false },
      { path: "/f/11.txt", isDir: false },
    ]);
    expect(capped.files).toHaveLength(MAX_TASK_FILES);
    expect(capped.files[9].path).toBe("/f/9.txt");
    expect(capped.truncated).toBe(true);

    // 满员（9 文件 + 1 文件夹）：新文件夹替换旧槽，列表长度不变
    const full = mergeFiles(nine, [{ path: "/old/dir", isDir: true }]);
    expect(full.files).toHaveLength(10);
    const swapped = mergeFiles(full.files, [{ path: "/new/dir", isDir: true }]);
    expect(swapped.applied).toBe(1);
    expect(swapped.files).toHaveLength(10);
    expect(swapped.files[9]).toEqual({ path: "/new/dir", isDir: true });
  });
});

// 跨语言锁步：TS 的 MAX_TASK_FILES 必须与 Rust db::MAX_TASK_FILES 同值——
// 分叉会让 FFI 两侧截断行为静默不一致（一侧截 10 另一侧放 20）。
// 不开 Tauri command（零运行时面）：直接读 Rust 源码正则断言。
describe("MAX_TASK_FILES 跨语言锁步", () => {
  it("TS 常量 == Rust db::MAX_TASK_FILES", async () => {
    const { readFileSync } = await import("node:fs");
    const { resolve } = await import("node:path");
    const rustSrc = readFileSync(
      resolve(__dirname, "../../src-tauri/src/db/tasks.rs"),
      "utf-8",
    );
    const m = rustSrc.match(/MAX_TASK_FILES:\s*usize\s*=\s*(\d+)/);
    expect(m, "tasks.rs 里应能找到 MAX_TASK_FILES 常量定义").toBeTruthy();
    expect(Number(m![1])).toBe(MAX_TASK_FILES);
  });
});
