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
      user: { name: "我", avatarDataUrl: null },
      bot: { name: "机器人", avatarDataUrl: null },
    });
    render(<ActorAvatar bot={false} />);
    await waitFor(() => {
      expect(screen.getByText("我")).toBeInTheDocument();
    });
  });
});
