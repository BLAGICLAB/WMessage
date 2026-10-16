import { describe, it, expect, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import { ActorAvatar } from "./ActorAvatar";

const mocks = vi.hoisted(() => ({
  loadProfileMock: vi.fn(),
}));

vi.mock("../profile", () => ({
  getProfileCache: vi.fn(() => null),
  loadProfile: mocks.loadProfileMock,
  subscribeProfile: vi.fn(() => () => {}),
}));

describe("ActorAvatar 资料加载兜底", () => {
  it("loadProfile reject → 不炸、无 unhandled rejection（profile.ts 侧已留痕）", async () => {
    mocks.loadProfileMock.mockRejectedValueOnce(new Error("db down"));
    const consoleSpy = vi.spyOn(console, "error").mockImplementation(() => {});
    render(<ActorAvatar bot={true} />);
    // catch 吞掉 reject：组件照常渲染（botLogo img 兜底），
    // 且本组件不再二次 console.error（留痕在 profile.ts 内）
    await new Promise((r) => setTimeout(r, 20));
    expect(screen.getByRole("img")).toBeInTheDocument();
    expect(consoleSpy).not.toHaveBeenCalled();
    consoleSpy.mockRestore();
  });

  it("loadProfile 成功 → 渲染正常（回归）", async () => {
    mocks.loadProfileMock.mockResolvedValueOnce({
      user: { name: "我", avatarId: null },
      bot: { name: "机器人", avatarId: null },
    });
    render(<ActorAvatar bot={false} />);
    await waitFor(() => {
      expect(screen.getByText("我")).toBeInTheDocument();
    });
  });

  it("avatarId 已设 → 按注册表解析出内置图资源（未知 ID 回退首字母占位）", async () => {
    mocks.loadProfileMock.mockResolvedValueOnce({
      user: { name: "我", avatarId: "avatar-03" },
      bot: { name: "机器人", avatarId: null },
    });
    const { unmount } = render(<ActorAvatar bot={false} />);
    await waitFor(() => {
      expect(screen.getByRole("img")).toHaveAttribute(
        "src",
        expect.stringContaining("avatar-03"),
      );
    });
    unmount();

    mocks.loadProfileMock.mockResolvedValueOnce({
      user: { name: "我", avatarId: "garbage-id" },
      bot: { name: "机器人", avatarId: null },
    });
    render(<ActorAvatar bot={false} />);
    await waitFor(() => {
      // 非法 ID 无资源 → 不渲染 img，回退首字母文本
      expect(screen.queryByRole("img")).not.toBeInTheDocument();
      expect(screen.getByText("我")).toBeInTheDocument();
    });
  });
});
