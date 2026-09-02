import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MarkdownText } from "./MarkdownText";

const mocks = vi.hoisted(() => ({
  openUrlMock: vi.fn(async () => {}),
  invokeMock: vi.fn(async () => null),
}));

vi.mock("@tauri-apps/plugin-opener", () => ({
  openUrl: mocks.openUrlMock,
}));

vi.mock("@tauri-apps/api/core", () => ({
  invoke: mocks.invokeMock,
}));

beforeEach(() => {
  mocks.openUrlMock.mockClear();
  mocks.invokeMock.mockClear();
});

// 第二梯队 #8（2026-09-03）：MarkdownText 的链接/路径识别正则此前零覆盖。
// 关键行为：http(s) → openUrl（尾随标点剥离）；白名单绝对路径/Windows 盘符 → open_file_path；
// 其余 href（mailto/锚点/白名单外路径）降级为纯文本 span，不可点。
describe("MarkdownText 链接识别", () => {
  it("http 链接渲染为 <a>，点击调 openUrl；尾随标点被剥离", async () => {
    const user = userEvent.setup();
    render(<MarkdownText text={"[文档](https://example.com/a,)"} />);
    const link = screen.getByRole("link", { name: "文档" });
    expect(link).toHaveAttribute("href", "https://example.com/a,");
    await user.click(link);
    expect(mocks.openUrlMock).toHaveBeenCalledWith("https://example.com/a");
    expect(mocks.invokeMock).not.toHaveBeenCalled();
  });

  it("裸 URL 由 GFM 自动链接成 <a>", async () => {
    render(<MarkdownText text={"详见 https://example.com/page 内"} />);
    expect(
      screen.getByRole("link", { name: "https://example.com/page" })
    ).toBeInTheDocument();
  });

  it("mac 白名单路径链接 → <a>，点击走 open_file_path（不开浏览器）", async () => {
    const user = userEvent.setup();
    render(<MarkdownText text={"[报告](/Users/x/report.pdf)"} />);
    const link = screen.getByRole("link", { name: "报告" });
    expect(link).toHaveAttribute("title", "打开文件/文件夹");
    await user.click(link);
    expect(mocks.invokeMock).toHaveBeenCalledWith("open_file_path", {
      path: "/Users/x/report.pdf",
    });
    expect(mocks.openUrlMock).not.toHaveBeenCalled();
  });

  it("非 URL/白名单路径的 href 降级为纯文本：mailto、锚点、白名单外路径", () => {
    render(
      <MarkdownText
        text={"[邮件](mailto:a@b.com) [锚点](#sec) [系统文件](/etc/passwd)"}
      />
    );
    expect(screen.queryByRole("link")).not.toBeInTheDocument();
    // 降级为 span 但文本保留
    expect(screen.getByText("邮件")).toBeInTheDocument();
    expect(screen.getByText("锚点")).toBeInTheDocument();
    expect(screen.getByText("系统文件")).toBeInTheDocument();
  });

  it("路径白名单 \\b 边界：/tmpfoo 不命中 /tmp 前缀，降级为纯文本", () => {
    render(<MarkdownText text={"[x](/tmpfoo/a.txt)"} />);
    expect(screen.queryByRole("link")).not.toBeInTheDocument();
    expect(screen.getByText("x")).toBeInTheDocument();
  });
});

describe("MarkdownText 行内代码/代码块识别", () => {
  it("行内代码是 URL：渲染为可点 code，点击调 openUrl", async () => {
    const user = userEvent.setup();
    render(<MarkdownText text={"打开 `https://example.com/x` 看看"} />);
    const code = screen.getByText("https://example.com/x");
    expect(code.tagName).toBe("CODE");
    expect(code).toHaveAttribute("title", "在浏览器打开");
    await user.click(code);
    expect(mocks.openUrlMock).toHaveBeenCalledWith("https://example.com/x");
  });

  it("行内代码是 Windows 路径：可点，点击走 open_file_path", async () => {
    const user = userEvent.setup();
    render(<MarkdownText text={"文件在 `C:\\Users\\x\\a.txt`"} />);
    const code = screen.getByText(String.raw`C:\Users\x\a.txt`);
    expect(code).toHaveAttribute("title", "打开文件/文件夹");
    await user.click(code);
    expect(mocks.invokeMock).toHaveBeenCalledWith("open_file_path", {
      path: String.raw`C:\Users\x\a.txt`,
    });
  });

  it("普通行内代码：纯 code，不带打开行为", () => {
    render(<MarkdownText text={"执行 `npm run dev` 启动"} />);
    const code = screen.getByText("npm run dev");
    expect(code.tagName).toBe("CODE");
    expect(code).not.toHaveAttribute("title");
  });

  it("围栏代码块里的 URL 不转链接（带语言标记 → className 判定为非行内）", () => {
    const { container } = render(
      <MarkdownText text={"```text\nhttps://example.com/block\n```"} />
    );
    const blockCode = container.querySelector("pre code");
    expect(blockCode).not.toBeNull();
    expect(blockCode).not.toHaveAttribute("title");
    expect(screen.queryByRole("link")).not.toBeInTheDocument();
  });

  it("已知边界：无语言标记的单行围栏代码块无法与行内代码区分（react-markdown 剥掉尾换行），会按行内代码处理", () => {
    // 记录现状而非修复：isInline = !className && !text.includes("\n") 对该形态误判为行内
    const { container } = render(
      <MarkdownText text={"```\nhttps://example.com/block\n```"} />
    );
    const blockCode = container.querySelector("pre code");
    expect(blockCode).toHaveAttribute("title", "在浏览器打开");
  });
});
