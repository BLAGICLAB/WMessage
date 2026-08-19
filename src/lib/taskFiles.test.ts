import { describe, it, expect } from "vitest";
import { taskFiles, filesPatch, mergeFiles, MAX_TASK_FILES } from "./taskFiles";
import type { Task } from "../types";

const base: Task = { id: "t1", title: "x", column: "todo" };

describe("taskFiles", () => {
  it("files 非空优先；空时回退旧 filePath/fileIsDir（迁移过渡兜底）", () => {
    expect(taskFiles(base)).toEqual([]);
    // 旧数据（未迁移）：单文件
    expect(taskFiles({ ...base, filePath: "/a.pdf" })).toEqual([
      { path: "/a.pdf", isDir: false },
    ]);
    // 旧数据：文件夹
    expect(taskFiles({ ...base, filePath: "/dir", fileIsDir: true })).toEqual([
      { path: "/dir", isDir: true },
    ]);
    // 新字段优先
    expect(
      taskFiles({
        ...base,
        filePath: "/old.pdf",
        files: [{ path: "/new.pdf", isDir: false }],
      })
    ).toEqual([{ path: "/new.pdf", isDir: false }]);
    // files 空数组 → 回退旧字段
    expect(taskFiles({ ...base, files: [], filePath: "/a.pdf" })).toEqual([
      { path: "/a.pdf", isDir: false },
    ]);
  });
});

describe("filesPatch 双写", () => {
  it("写 files 同时同步旧 filePath/fileIsDir 首条；空列表全清", () => {
    const p = filesPatch([
      { path: "/a.pdf", isDir: false },
      { path: "/dir", isDir: true },
    ]);
    expect(p.files).toHaveLength(2);
    expect(p.filePath).toBe("/a.pdf");
    expect(p.fileIsDir).toBe(false);

    const empty = filesPatch([]);
    expect(empty.files).toEqual([]);
    expect(empty.filePath).toBeUndefined();
    expect(empty.fileIsDir).toBeUndefined();
  });
});

describe("mergeFiles", () => {
  it("去重保序追加；超上限截断并标记 truncated", () => {
    const cur = [{ path: "/a.pdf", isDir: false }];
    // 去重保序
    const m = mergeFiles(cur, [
      { path: "/a.pdf", isDir: false }, // 重复
      { path: "/b.pdf", isDir: false },
    ]);
    expect(m.files.map((f) => f.path)).toEqual(["/a.pdf", "/b.pdf"]);
    expect(m.truncated).toBe(false);

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
  });
});
