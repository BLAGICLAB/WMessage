import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import { MessageList } from "./MessageList";
import { BUBBLE_MAX_W_PCT } from "./constants";
import type { Msg } from "./types";

// 头像本体（资料加载 / 兜底首字母）由 ActorAvatar.test 覆盖，这里只看
// 「每条消息带谁的头像、在哪一侧」——即消息行自己的布局决策。
vi.mock("../ActorAvatar", () => ({
  ActorAvatar: ({ bot, size }: { bot: boolean; size?: number }) => (
    <span data-testid="avatar" data-bot={String(bot)} data-size={String(size)} />
  ),
}));

const stableCallbacks = {
  onCopy: () => {},
  onRemove: () => {},
  onOpenTask: () => {},
  onOpenExecSession: () => {},
  onPickStarter: () => {},
};

const baseProps = {
  scrollRef: { current: null },
  viewedBusy: false,
  copiedIdx: null,
  verboseLevel: "detailed" as const,
  ...stableCallbacks,
};

/** 消息行：头像挂在行首（DOM 首位），故头像是行的直接子节点 */
function rowOf(avatar: Element): HTMLElement {
  return avatar.parentElement as HTMLElement;
}

/** 行内的列容器：气泡与操作行同属它，头像不属于 */
function colOf(avatar: Element): HTMLElement {
  return rowOf(avatar).children[1] as HTMLElement;
}

describe("消息列表归属头像", () => {
  it("每条消息都带头像：用户头像在右、机器人头像在左", () => {
    const messages: Msg[] = [
      { role: "user", content: "在吗" },
      { role: "assistant", content: "在的" },
    ];
    const { container } = render(
      <MessageList messages={messages} {...baseProps} />,
    );
    const avatars = screen.getAllByTestId("avatar");
    expect(avatars).toHaveLength(2);
    expect(avatars[0]).toHaveAttribute("data-bot", "false");
    expect(avatars[1]).toHaveAttribute("data-bot", "true");

    // 头像是行的首个子元素，左右由行方向决定：row-reverse → 甩到最右
    expect(rowOf(avatars[0]).firstElementChild).toBe(avatars[0]);
    expect(rowOf(avatars[0]).className).toContain("flex-row-reverse");
    expect(rowOf(avatars[1]).firstElementChild).toBe(avatars[1]);
    expect(rowOf(avatars[1]).className).not.toContain("flex-row-reverse");
    expect(container.querySelectorAll(".chat-bubble")).toHaveLength(2);
  });

  it("头像不挤正文：气泡与操作行仍同列", () => {
    const messages: Msg[] = [
      { role: "user", content: "整理周报" },
      { role: "assistant", content: "已完成" },
    ];
    render(<MessageList messages={messages} {...baseProps} />);
    const [, botAvatar] = screen.getAllByTestId("avatar");

    // 操作行（复制/移除）与气泡同属列容器，否则会被排到头像那一列
    expect(colOf(botAvatar)).toContainElement(
      rowOf(botAvatar).querySelector(".chat-bubble") as HTMLElement,
    );
  });

  it("机器人气泡与用户气泡同一条限宽上限：不再通栏伸到头像底下", () => {
    // 回归：机器人列原先满宽（max-w-full），长回复一路伸到用户头像底下，
    // 两边不等宽——用户与机器人的气泡上限必须是同一条
    const messages: Msg[] = [
      { role: "user", content: "整理周报" },
      { role: "assistant", content: "已完成" },
    ];
    render(<MessageList messages={messages} {...baseProps} />);
    const [userAvatar, botAvatar] = screen.getAllByTestId("avatar");

    expect(colOf(botAvatar).style.maxWidth).toBe(colOf(userAvatar).style.maxWidth);
    // 上限挂在列容器上（不是气泡上）：头像不占正文宽度
    expect(colOf(botAvatar).style.maxWidth).toBe(`${BUBBLE_MAX_W_PCT}%`);
    expect(colOf(userAvatar).style.maxWidth).toBe(`${BUBBLE_MAX_W_PCT}%`);
  });
});
