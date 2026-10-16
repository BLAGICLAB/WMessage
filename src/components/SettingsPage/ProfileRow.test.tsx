import { describe, it, expect, vi, beforeEach } from "vitest";
import {
  render,
  screen,
  waitFor,
  act,
  fireEvent,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { ProfileRow } from "./ProfileRow";

const mocks = vi.hoisted(() => {
  /** useProfile 返回值：测试可改写（如已设头像态） */
  const profileValue: {
    user: { name: string; avatarId: string | null };
    bot: { name: string; avatarId: string | null };
  } = {
    user: { name: "我", avatarId: null },
    bot: { name: "机器人", avatarId: null },
  };
  return {
    setProfileNameMock: vi.fn(async () => null),
    setProfileAvatarMock: vi.fn(async () => null),
    removeProfileAvatarMock: vi.fn(async () => null),
    profileValue,
  };
});

vi.mock("../../profile", () => ({
  setProfileName: mocks.setProfileNameMock,
  setProfileAvatar: mocks.setProfileAvatarMock,
  removeProfileAvatar: mocks.removeProfileAvatarMock,
}));

vi.mock("../ActorAvatar", () => ({
  useProfile: () => mocks.profileValue,
}));

beforeEach(() => {
  mocks.setProfileNameMock.mockClear();
  mocks.setProfileAvatarMock.mockClear();
  mocks.removeProfileAvatarMock.mockClear();
  mocks.profileValue.user = { name: "我", avatarId: null };
  mocks.profileValue.bot = { name: "机器人", avatarId: null };
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
      expect(screen.getByText("已保存")).toBeInTheDocument();
      // 1400ms 后仍在
      act(() => {
        vi.advanceTimersByTime(1400);
      });
      expect(screen.getByText("已保存")).toBeInTheDocument();
      // 再保存一次 → 计时器重置（从第二次起算 1500ms）
      fireEvent.click(screen.getByText("已保存"));
      await flush();
      act(() => {
        vi.advanceTimersByTime(1400);
      });
      expect(screen.getByText("已保存")).toBeInTheDocument();
      act(() => {
        vi.advanceTimersByTime(200);
      });
      expect(screen.queryByText("已保存")).not.toBeInTheDocument();
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
      expect(screen.getByText("已保存")).toBeInTheDocument();
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
        }),
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

describe("ProfileRow 预设头像选择器", () => {
  it("点头像开浮层 → 16 个选项；选第 3 张调 setProfileAvatar(kind, id) 并收起", async () => {
    const user = userEvent.setup();
    render(<ProfileRow kind="user" label="用户" defaultName="我" />);
    await screen.findByDisplayValue("我");
    await user.click(screen.getByRole("button", { name: "更换用户头像" }));
    const listbox = screen.getByRole("listbox", { name: "用户头像" });
    const options = within(listbox).getAllByRole("option");
    expect(options).toHaveLength(16);
    await user.click(options[2]); // AVATAR_IDS[2] = avatar-03
    expect(mocks.setProfileAvatarMock).toHaveBeenCalledWith(
      "user",
      "avatar-03",
    );
    await waitFor(() => {
      expect(screen.queryByRole("listbox")).not.toBeInTheDocument();
    });
  });

  it("已设头像 → 浮层含「恢复默认头像」，点击调 removeProfileAvatar(kind)", async () => {
    mocks.profileValue.user = { name: "我", avatarId: "avatar-02" };
    const user = userEvent.setup();
    render(<ProfileRow kind="user" label="用户" defaultName="我" />);
    await screen.findByDisplayValue("我");
    await user.click(screen.getByRole("button", { name: "更换用户头像" }));
    await user.click(screen.getByRole("button", { name: "恢复默认头像" }));
    expect(mocks.removeProfileAvatarMock).toHaveBeenCalledWith("user");
    await waitFor(() => {
      expect(screen.queryByRole("listbox")).not.toBeInTheDocument();
    });
  });

  it("未设头像 → 浮层无「恢复默认头像」入口（无可恢复项）", async () => {
    const user = userEvent.setup();
    render(<ProfileRow kind="bot" label="机器人" defaultName="机器人" />);
    await screen.findByDisplayValue("机器人");
    await user.click(screen.getByRole("button", { name: "更换机器人头像" }));
    expect(screen.getByRole("listbox", { name: "机器人头像" })).toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "恢复默认头像" }),
    ).not.toBeInTheDocument();
  });

  it("选择失败 → 错误上浮到行内文案，不炸组件", async () => {
    mocks.setProfileAvatarMock.mockRejectedValueOnce(new Error("disk full"));
    const user = userEvent.setup();
    render(<ProfileRow kind="user" label="用户" defaultName="我" />);
    await screen.findByDisplayValue("我");
    await user.click(screen.getByRole("button", { name: "更换用户头像" }));
    await user.click(within(screen.getByRole("listbox")).getAllByRole("option")[0]);
    await waitFor(() => {
      expect(screen.getByText(/disk full/)).toBeInTheDocument();
    });
  });
});
