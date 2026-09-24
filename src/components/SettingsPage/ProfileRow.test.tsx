import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor, act, fireEvent } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { ProfileRow } from "./ProfileRow";

const mocks = vi.hoisted(() => ({
  setProfileNameMock: vi.fn(async () => null),
}));

vi.mock("../../profile", () => ({
  setProfileName: mocks.setProfileNameMock,
  setProfileAvatar: vi.fn(async () => null),
  removeProfileAvatar: vi.fn(async () => null),
}));

vi.mock("../ActorAvatar", () => ({
  useProfile: () => ({
    user: { name: "我", avatarDataUrl: null },
    bot: { name: "机器人", avatarDataUrl: null },
  }),
}));

beforeEach(() => {
  mocks.setProfileNameMock.mockClear();
});

/** fake timers 下 findBy/waitFor 不自前进（testing-library 只认 jest fake），
 *  故初始填充用真 timers + userEvent，开 fake 后只用 fireEvent + act 刷微任务 */
async function fillAndGetInput() {
  const user = userEvent.setup();
  const view = render(<ProfileRow kind="user" label="用户" defaultName="我" />);
  const input = await screen.findByDisplayValue("我");
  await user.clear(input);
  await user.type(input, "新名字");
  return { input, unmount: view.unmount };
}

async function flush() {
  await act(async () => {});
}

describe("ProfileRow 保存计时器与 busy 契约", () => {
  it("保存成功 → 「已保存 ✓」出现；1500ms 后自动消失；连续保存重置计时", async () => {
    await fillAndGetInput();
    vi.useFakeTimers();
    try {
      fireEvent.click(screen.getByText("保存"));
      await flush();
      expect(screen.getByText("已保存 ✓")).toBeInTheDocument();
      // 1400ms 后仍在
      act(() => {
        vi.advanceTimersByTime(1400);
      });
      expect(screen.getByText("已保存 ✓")).toBeInTheDocument();
      // 再保存一次 → 计时器重置（从第二次起算 1500ms）
      fireEvent.click(screen.getByText("已保存 ✓"));
      await flush();
      act(() => {
        vi.advanceTimersByTime(1400);
      });
      expect(screen.getByText("已保存 ✓")).toBeInTheDocument();
      act(() => {
        vi.advanceTimersByTime(200);
      });
      expect(screen.queryByText("已保存 ✓")).not.toBeInTheDocument();
      expect(mocks.setProfileNameMock).toHaveBeenCalledTimes(2);
    } finally {
      vi.useRealTimers();
    }
  });

  it("卸载后计时器不触发（cleanup clearTimeout）", async () => {
    const { unmount } = await fillAndGetInput();
    vi.useFakeTimers();
    try {
      fireEvent.click(screen.getByText("保存"));
      await flush();
      expect(screen.getByText("已保存 ✓")).toBeInTheDocument();
      unmount();
      // 前进超过 1500ms：回调已被清理，无残留计时器
      act(() => {
        vi.advanceTimersByTime(3000);
      });
      expect(vi.getTimerCount()).toBe(0);
    } finally {
      vi.useRealTimers();
    }
  });

  it("保存在飞期间 input disabled（堵 busy 期输入被旧值覆盖的数据丢失路径）", async () => {
    let release: (() => void) | null = null;
    mocks.setProfileNameMock.mockImplementationOnce(
      () =>
        new Promise<null>((res) => {
          release = () => res(null);
        })
    );
    const { input } = await fillAndGetInput();
    fireEvent.click(screen.getByText("保存"));
    await waitFor(() => {
      expect(input).toBeDisabled();
    });
    release!();
    await waitFor(() => {
      expect(input).not.toBeDisabled();
    });
  });
});
