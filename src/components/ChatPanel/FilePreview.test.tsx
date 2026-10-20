import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { FilePreview } from "./FilePreview";

const mocks = vi.hoisted(() => ({
  invokeMock: vi.fn(),
  openUrlMock: vi.fn(async () => {}),
}));

vi.mock("@tauri-apps/api/core", () => ({
  invoke: mocks.invokeMock,
}));

vi.mock("@tauri-apps/plugin-opener", () => ({
  openUrl: mocks.openUrlMock,
}));

const PDF_PATH = "/Users/renshi/Desktop/季度复盘.pdf";
/** 栏内主行动按钮的可访问名（精确匹配）：头部那个 ↗ 的名字带文件名，不会命中 */
const OPEN_EXTERNALLY = "用系统默认程序打开";

/** 让 bot_preview_text 回指定载荷（其余命令一律 null） */
function previewReturns(payload: unknown) {
  mocks.invokeMock.mockImplementation(async (cmd: string) =>
    cmd === "bot_preview_text" ? payload : null,
  );
}

beforeEach(() => {
  mocks.invokeMock.mockReset();
  mocks.openUrlMock.mockReset();
});

// 右侧栏「文件」工作面的预览面板：
// 文本 → 就地渲染；渲染不了（图片/PDF/Office 文档等二进制、文件夹）→ 说清原因
// 并给出「用系统默认程序打开」的主行动项；真读不到（文件没了/权限）→ 只报错误。
describe("FilePreview 预览分流", () => {
  it("文本文件：就地渲染内容，不给外部打开主行动项", async () => {
    previewReturns({
      kind: "text",
      path: "/Users/renshi/a.txt",
      content: "hello preview",
      truncated: false,
      totalBytes: 13,
      lines: 1,
    });
    render(<FilePreview path="/Users/renshi/a.txt" onBack={vi.fn()} />);

    expect(await screen.findByText("hello preview")).toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: OPEN_EXTERNALLY }),
    ).not.toBeInTheDocument();
  });

  it("二进制文件：说明不能在栏内预览，并给出「用系统默认程序打开」按钮", async () => {
    const user = userEvent.setup();
    previewReturns({
      kind: "unsupported",
      path: PDF_PATH,
      reason: "这是二进制文件（图片、PDF、Office 文档等），无法在栏内预览内容",
    });
    render(<FilePreview path={PDF_PATH} onBack={vi.fn()} />);

    expect(await screen.findByText(/这是二进制文件/)).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: OPEN_EXTERNALLY }));

    await waitFor(() =>
      expect(mocks.invokeMock).toHaveBeenCalledWith("open_file_path", {
        path: PDF_PATH,
      }),
    );
    expect(mocks.openUrlMock).not.toHaveBeenCalled();
  });

  it("文件夹：说明不能预览内容，同样给外部打开入口（开访达）", async () => {
    const user = userEvent.setup();
    previewReturns({
      kind: "unsupported",
      path: "/Users/renshi/Desktop",
      reason: "这是文件夹，无法在栏内预览内容",
    });
    render(<FilePreview path="/Users/renshi/Desktop" onBack={vi.fn()} />);

    expect(await screen.findByText(/这是文件夹/)).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: OPEN_EXTERNALLY }));
    await waitFor(() =>
      expect(mocks.invokeMock).toHaveBeenCalledWith("open_file_path", {
        path: "/Users/renshi/Desktop",
      }),
    );
  });

  it("读不到文件（IO 错误）：只报错误，不给没用的外部打开入口", async () => {
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd !== "bot_preview_text") return null;
      throw {
        code: "IO_ERROR",
        message: "文件 IO 错误：读取文件信息失败：No such file",
        recoverable: false,
      };
    });
    render(<FilePreview path="/Users/renshi/gone.txt" onBack={vi.fn()} />);

    expect(await screen.findByText(/读取文件信息失败/)).toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: OPEN_EXTERNALLY }),
    ).not.toBeInTheDocument();
  });
});
