import { describe, it, expect } from "vitest";
import { diffLineClass, gapLabel, parseDiffSegments } from "./diff";

// 真实 hunk 头（git 口径：计数含上下文行），见 bot_fs 的格式契约测试
const TWO_HUNKS = [
  "--- /x.txt",
  "+++ /x.txt",
  "@@ -1,5 +1,5 @@",
  " line1",
  "-line2",
  "+LINE2",
  " line3",
  " line4",
  " line5",
  "@@ -7,6 +7,6 @@",
  " line7",
  " line8",
  " line9",
  "-line10",
  "+LINE10",
  " line11",
  " line12",
].join("\n");

describe("diffLineClass", () => {
  it("文件头弱化 / hunk 头高亮 / 加减行红绿 / 上下文原样", () => {
    expect(diffLineClass("--- /x.txt")).toBe("text-[var(--t5)]");
    expect(diffLineClass("+++ /x.txt")).toBe("text-[var(--t5)]");
    expect(diffLineClass("@@ -1,5 +1,5 @@")).toContain("--info");
    expect(diffLineClass("+LINE2")).toContain("--success");
    expect(diffLineClass("-line2")).toContain("--danger");
    expect(diffLineClass(" line3")).toBe("");
  });
});

describe("parseDiffSegments", () => {
  it("首段从第 1 行开始时无省略段", () => {
    const segs = parseDiffSegments(
      ["--- /x.txt", "+++ /x.txt", "@@ -1,3 +1,3 @@", " a", "-b", "+B", " c"].join("\n"),
    );
    expect(segs).toEqual([
      { type: "lines", lines: ["--- /x.txt", "+++ /x.txt"] },
      { type: "lines", lines: ["@@ -1,3 +1,3 @@", " a", "-b", "+B", " c"] },
    ]);
    expect(segs.some((s) => s.type === "gap")).toBe(false);
  });

  it("首段之前的省略行数 = 起始旧行 − 1", () => {
    const segs = parseDiffSegments(
      ["--- /x.txt", "+++ /x.txt", "@@ -10,3 +10,3 @@", " a", "-b", "+B", " c"].join("\n"),
    );
    const gaps = segs.filter((s) => s.type === "gap");
    expect(gaps).toEqual([{ type: "gap", omitted: 9 }]);
  });

  it("两段之间的省略行数 = 下一段起始旧行 − 本段起始旧行 − 本段计数", () => {
    const segs = parseDiffSegments(TWO_HUNKS);
    // 第 1 段占旧行 1..5，第 2 段从旧行 7 开始 → 中间省略旧行 6
    expect(segs.filter((s) => s.type === "gap")).toEqual([{ type: "gap", omitted: 1 }]);
    expect(segs[0]).toEqual({ type: "lines", lines: ["--- /x.txt", "+++ /x.txt"] });
    expect(segs[1]).toEqual({
      type: "lines",
      lines: ["@@ -1,5 +1,5 @@", " line1", "-line2", "+LINE2", " line3", " line4", " line5"],
    });
    expect(segs[3]).toEqual({
      type: "lines",
      lines: [
        "@@ -7,6 +7,6 @@",
        " line7",
        " line8",
        " line9",
        "-line10",
        "+LINE10",
        " line11",
        " line12",
      ],
    });
  });

  it("hunk 头省略计数时按 1 行算", () => {
    const segs = parseDiffSegments(["@@ -5 +5 @@", "-a", "+A"].join("\n"));
    expect(segs.filter((s) => s.type === "gap")).toEqual([{ type: "gap", omitted: 4 }]);
  });

  it("末段之后不猜省略行数（diff 文本里没有文件总行数）", () => {
    const segs = parseDiffSegments(TWO_HUNKS);
    expect(segs[segs.length - 1].type).toBe("lines");
    expect(segs.filter((s) => s.type === "gap")).toHaveLength(1);
  });

  it("无 hunk 头的文本整体作为一段", () => {
    expect(parseDiffSegments("just text\nmore")).toEqual([
      { type: "lines", lines: ["just text", "more"] },
    ]);
  });

  it("空 diff 无分段", () => {
    expect(parseDiffSegments("")).toEqual([]);
  });
});

describe("gapLabel", () => {
  it("标出行数", () => {
    expect(gapLabel(1)).toBe("⋯ 省略 1 行未改动");
    expect(gapLabel(9)).toBe("⋯ 省略 9 行未改动");
  });
});
