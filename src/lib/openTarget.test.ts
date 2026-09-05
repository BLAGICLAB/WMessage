import { describe, it, expect, vi, beforeEach } from "vitest";

const mocks = vi.hoisted(() => ({
  openUrlMock: vi.fn(async () => {}),
  invokeMock: vi.fn(async () => {}),
}));
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: mocks.openUrlMock }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invokeMock }));
const { openUrlMock, invokeMock } = mocks;

import {
  LINK_OR_PATH_RE,
  extractFilePaths,
  isAbsPath,
  normalizeUrl,
  openTarget,
} from "./openTarget";

// 2026-09-05 回归：聊天下方文档/网址链接「有时打不开、有时显示 Program」
describe("LINK_OR_PATH_RE 带空格路径", () => {
  it("Windows 路径含空格（Program Files）完整匹配，不再截断成 C:\\Program", () => {
    const m = "已生成：C:\\Program Files\\WMessage\\报告.docx 请查收".matchAll(
      new RegExp(LINK_OR_PATH_RE.source, "gi")
    );
    const tokens = [...m].map((x) => x[0]);
    expect(tokens).toEqual(["C:\\Program Files\\WMessage\\报告.docx"]);
  });

  it("文件名含空格（报告 终稿.docx）完整匹配", () => {
    const tokens = [
      ..."产物在 /Users/x/AI_Gen_Files/周报 修订版.docx。".matchAll(
        new RegExp(LINK_OR_PATH_RE.source, "gi")
      ),
    ].map((x) => x[0]);
    expect(tokens).toEqual(["/Users/x/AI_Gen_Files/周报 修订版.docx"]);
  });

  it("URL 与无空格文件夹路径行为不变", () => {
    const tokens = [
      ..."看 https://example.com/a?b=1，目录 /Users/x/docs 和 D:\\资料".matchAll(
        new RegExp(LINK_OR_PATH_RE.source, "gi")
      ),
    ].map((x) => x[0]);
    expect(tokens).toEqual(["https://example.com/a?b=1", "/Users/x/docs", "D:\\资料"]);
  });

  it("带空格但不是已知扩展名的文本不误判为路径", () => {
    const tokens = [
      ..."见 /Users/x/会议纪要 草稿版 已完成".matchAll(
        new RegExp(LINK_OR_PATH_RE.source, "gi")
      ),
    ].map((x) => x[0]);
    // 无扩展名 → 回退无空格分支，截到首个空格（旧行为，文件夹类）
    expect(tokens).toEqual(["/Users/x/会议纪要"]);
  });
});

describe("extractFilePaths", () => {
  it("带空格路径完整提取、URL 跳过、去重保序、反引号穿透", () => {
    const content =
      "已生成修订版 Word：`C:\\Program Files\\WMessage\\周报 修订版.docx`\n" +
      "参考 https://example.com 与 /Users/x/a.pdf，再说一遍 /Users/x/a.pdf";
    expect(extractFilePaths(content)).toEqual([
      "C:\\Program Files\\WMessage\\周报 修订版.docx",
      "/Users/x/a.pdf",
    ]);
  });
});

describe("normalizeUrl", () => {
  it("http(s) 原样、尾随标点剥离、裸域名补 https、垃圾返回 null", () => {
    expect(normalizeUrl("https://example.com/a.")).toBe("https://example.com/a");
    expect(normalizeUrl("www.example.com/x")).toBe("https://www.example.com/x");
    expect(normalizeUrl("example.com")).toBe("https://example.com");
    expect(normalizeUrl("C:\\Program Files\\a.docx")).toBeNull();
    expect(normalizeUrl("随便一句话")).toBeNull();
  });
});

describe("openTarget 统一分发", () => {
  beforeEach(() => {
    openUrlMock.mockClear();
    invokeMock.mockClear();
  });

  it("http URL → openUrl；裸域名补 scheme；绝对路径 → open_file_path", async () => {
    openTarget("https://example.com/a");
    openTarget("www.example.com");
    openTarget("C:\\Program Files\\WMessage\\报告.docx");
    openTarget("/Users/x/周报 修订版.docx");
    expect(openUrlMock).toHaveBeenCalledWith("https://example.com/a");
    expect(openUrlMock).toHaveBeenCalledWith("https://www.example.com");
    expect(invokeMock).toHaveBeenCalledWith("open_file_path", {
      path: "C:\\Program Files\\WMessage\\报告.docx",
    });
    expect(invokeMock).toHaveBeenCalledWith("open_file_path", {
      path: "/Users/x/周报 修订版.docx",
    });
  });

  it("打开失败不静默：弹 alert 提示原因（不再点了没反应）", async () => {
    const alertSpy = vi.fn();
    vi.stubGlobal("alert", alertSpy);
    invokeMock.mockRejectedValueOnce({
      code: "IO_ERROR",
      message: "路径不存在（可能已被移动或删除）：/Users/x/gone.docx",
      recoverable: false,
    });
    openTarget("/Users/x/gone.docx");
    await new Promise((r) => setTimeout(r, 0));
    expect(alertSpy).toHaveBeenCalledOnce();
    expect(alertSpy.mock.calls[0][0]).toContain("路径不存在");
    vi.unstubAllGlobals();
  });
});

describe("isAbsPath", () => {
  it("盘符/常见 unix 根为路径；C: 单字母不被当成 URL scheme 以外的误判", () => {
    expect(isAbsPath("C:\\Program Files\\a.docx")).toBe(true);
    expect(isAbsPath("D:/资料")).toBe(true);
    expect(isAbsPath("/Users/x/a")).toBe(true);
    expect(isAbsPath("https://example.com")).toBe(false);
  });
});
