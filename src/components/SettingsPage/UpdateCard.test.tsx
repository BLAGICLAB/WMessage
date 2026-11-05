import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { UpdateCard, isWindowsPlatform } from "./UpdateCard";

const mocks = vi.hoisted(() => ({
  check: vi.fn(),
  getVersion: vi.fn(),
}));

vi.mock("@tauri-apps/plugin-updater", () => ({ check: mocks.check }));
vi.mock("@tauri-apps/api/app", () => ({ getVersion: mocks.getVersion }));

const WIN_UA =
  "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/120.0 Safari/537.36";
const MAC_UA =
  "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 Safari/605.1.15";

/** 造一个假 Update：downloadAndInstall 按序发 Started/Progress/Finished */
function sampleUpdate(over: Record<string, unknown> = {}) {
  return {
    version: "0.2.0",
    body: "修复若干问题",
    downloadAndInstall: vi.fn(
      async (onEvent?: (e: { event: string; data?: { [k: string]: number } }) => void) => {
        onEvent?.({ event: "Started", data: { contentLength: 100 } });
        onEvent?.({ event: "Progress", data: { chunkLength: 60 } });
        onEvent?.({ event: "Progress", data: { chunkLength: 40 } });
        onEvent?.({ event: "Finished" });
      },
    ),
    ...over,
  };
}

beforeEach(() => {
  mocks.check.mockReset();
  mocks.getVersion.mockReset();
  mocks.getVersion.mockResolvedValue("0.1.0");
});

describe("isWindowsPlatform", () => {
  it("Windows UA 命中、macOS UA 不命中", () => {
    expect(isWindowsPlatform(WIN_UA)).toBe(true);
    expect(isWindowsPlatform(MAC_UA)).toBe(false);
  });
});

describe("UpdateCard", () => {
  it("非 Windows 平台不渲染", () => {
    const { container } = render(<UpdateCard ua={MAC_UA} />);
    expect(container.textContent).toBe("");
  });

  it("检查后无新版本 → 已是最新（版本号真值）", async () => {
    mocks.check.mockResolvedValue(null);
    render(<UpdateCard ua={WIN_UA} />);
    fireEvent.click(screen.getByRole("button", { name: "检查更新" }));
    expect(await screen.findByText("✓ 已是最新（v0.1.0）")).toBeTruthy();
  });

  it("发现新版本 → 下载走完进 installed 态（重启由安装器 /R 完成，前端无 relaunch）", async () => {
    const u = sampleUpdate();
    mocks.check.mockResolvedValue(u);
    render(<UpdateCard ua={WIN_UA} />);
    fireEvent.click(screen.getByRole("button", { name: "检查更新" }));
    expect(await screen.findByText("发现新版本 v0.2.0")).toBeTruthy();
    expect(screen.getByText("修复若干问题")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: /下载并安装/ }));
    await waitFor(() => {
      expect(screen.getByText(/安装器接管中/)).toBeTruthy();
    });
    expect(u.downloadAndInstall).toHaveBeenCalledTimes(1);
  });

  it("检查失败 → 显式报错文案，不冒充最新", async () => {
    mocks.check.mockRejectedValue(new Error("网络超时"));
    render(<UpdateCard ua={WIN_UA} />);
    fireEvent.click(screen.getByRole("button", { name: "检查更新" }));
    expect(await screen.findByText(/更新失败：网络超时/)).toBeTruthy();
    expect(screen.queryByText(/已是最新/)).toBeNull();
  });

  it("下载/安装失败 → error 态显式报错，不假成功", async () => {
    const u = sampleUpdate({
      downloadAndInstall: vi.fn(async (onEvent?: (e: { event: string; data?: { contentLength: number } }) => void) => {
        onEvent?.({ event: "Started", data: { contentLength: 10 } });
        throw new Error("签名校验失败");
      }),
    });
    mocks.check.mockResolvedValue(u);
    render(<UpdateCard ua={WIN_UA} />);
    fireEvent.click(screen.getByRole("button", { name: "检查更新" }));
    await screen.findByText("发现新版本 v0.2.0");
    fireEvent.click(screen.getByRole("button", { name: /下载并安装/ }));
    expect(await screen.findByText(/更新失败：签名校验失败/)).toBeTruthy();
    expect(screen.queryByText(/已是最新/)).toBeNull();
  });
});
