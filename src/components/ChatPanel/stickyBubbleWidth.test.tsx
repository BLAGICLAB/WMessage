import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { act, render, screen } from "@testing-library/react";
import { MessageList } from "./MessageList";
import { BUBBLE_MAX_W_PCT } from "./constants";
import { availWidth, stickyFloor } from "./stickyBubbleWidth";
import type { Msg } from "./types";

// 头像本体（资料加载 / 兜底首字母）由 ActorAvatar.test 覆盖，这里只看宽度下限
vi.mock("../ActorAvatar", () => ({
  ActorAvatar: ({ bot, size }: { bot: boolean; size?: number }) => (
    <span data-testid="avatar" data-bot={String(bot)} data-size={String(size)} />
  ),
}));

/** jsdom 没有 ResizeObserver：替身记下实例，由测试自己触发观测回调 */
class FakeResizeObserver {
  static instances: FakeResizeObserver[] = [];
  #cb: ResizeObserverCallback;
  constructor(cb: ResizeObserverCallback) {
    this.#cb = cb;
    FakeResizeObserver.instances.push(this);
  }
  observe() {}
  unobserve() {}
  disconnect() {}
  trigger() {
    this.#cb([], this as unknown as ResizeObserver);
  }
}

function rect(left: number, right: number): DOMRect {
  return {
    left,
    right,
    width: right - left,
    height: 0,
    top: 0,
    bottom: 0,
    x: left,
    y: 0,
    toJSON: () => ({}),
  } as DOMRect;
}

/** 行/列/气泡的测量值可变：改 state 再触发观测回调 = 模拟宽度变化 */
type MeasureState = { rowRight: number; colLeft: number; bubbleWidth: number };

function installRects(
  row: HTMLElement,
  col: HTMLElement,
  bubble: HTMLElement,
  state: MeasureState,
) {
  row.getBoundingClientRect = () => rect(0, state.rowRight);
  col.getBoundingClientRect = () =>
    rect(state.colLeft, state.colLeft + state.bubbleWidth);
  bubble.getBoundingClientRect = () =>
    rect(state.colLeft, state.colLeft + state.bubbleWidth);
}

const baseProps = {
  scrollRef: { current: null },
  viewedBusy: false,
  copiedIdx: null,
  verboseLevel: "detailed" as const,
  onCopy: () => {},
  onRemove: () => {},
  onOpenTask: () => {},
  onOpenExecSession: () => {},
  onPickStarter: () => {},
};

function renderAssistant() {
  const messages: Msg[] = [{ role: "assistant", content: "在的" }];
  render(<MessageList messages={messages} {...baseProps} />);
  const avatar = screen.getByTestId("avatar");
  const row = avatar.parentElement as HTMLElement;
  const col = row.children[1] as HTMLElement;
  const bubble = col.querySelector(".chat-bubble") as HTMLElement;
  const state: MeasureState = { rowRight: 760, colLeft: 34, bubbleWidth: 277 };
  installRects(row, col, bubble, state);
  const ro = FakeResizeObserver.instances[0];
  return { col, bubble, state, ro };
}

describe("机器人气泡宽度只增不减", () => {
  beforeEach(() => {
    FakeResizeObserver.instances = [];
    (globalThis as unknown as { ResizeObserver?: unknown }).ResizeObserver =
      FakeResizeObserver;
  });

  afterEach(() => {
    delete (globalThis as unknown as { ResizeObserver?: unknown })
      .ResizeObserver;
  });

  it("下限计算：只增不减", () => {
    expect(stickyFloor(277, 137)).toBe(277);
    expect(stickyFloor(137, 277)).toBe(277);
    expect(stickyFloor(277, 277)).toBe(277);
  });

  it("可用宽度：行右缘减去列左缘，容器隐藏时为 0", () => {
    expect(availWidth(760, 34)).toBe(726);
    expect(availWidth(0, 0)).toBe(0);
    expect(availWidth(10, 40)).toBe(0);
  });

  it("变窄时把宽度写回下限：宽度只增不减", () => {
    const { col, state, ro } = renderAssistant();

    // 工具行出现：气泡此刻就够宽，不必写样式
    act(() => ro.trigger());
    expect(col.style.minWidth).toBe("");

    // 工具跑完，那行变短 → 气泡缩到 137 → 写回见过的最宽值
    state.bubbleWidth = 137;
    act(() => ro.trigger());
    expect(col.style.minWidth).toBe(`min(277px, ${BUBBLE_MAX_W_PCT}%)`);

    // 正文继续变长：气泡自己涨上去，不必改下限
    state.bubbleWidth = 400;
    act(() => ro.trigger());
    expect(col.style.minWidth).toBe(`min(277px, ${BUBBLE_MAX_W_PCT}%)`);

    // 再缩回去：按见过的最宽 400 写回
    state.bubbleWidth = 200;
    act(() => ro.trigger());
    expect(col.style.minWidth).toBe(`min(400px, ${BUBBLE_MAX_W_PCT}%)`);
  });

  it("窗口收窄：下限按新的可用宽度收回，不溢出可用宽度", () => {
    const { col, state, ro } = renderAssistant();

    state.bubbleWidth = 600;
    act(() => ro.trigger());
    state.bubbleWidth = 300;
    act(() => ro.trigger());
    expect(col.style.minWidth).toBe(`min(600px, ${BUBBLE_MAX_W_PCT}%)`);

    // 窗口收窄到可用宽度 400：下限封顶收回
    state.rowRight = 434;
    act(() => ro.trigger());
    expect(col.style.minWidth).toBe(`min(400px, ${BUBBLE_MAX_W_PCT}%)`);
  });

  it("容器隐藏（量到的全是 0）：不动下限", () => {
    const { col, state, ro } = renderAssistant();

    // 首次观测：记下此刻的宽度（真实浏览器里 RO 挂上就会先报一次）
    act(() => ro.trigger());

    // 再建立下限：工具行变短 → 写回见过的最宽值
    state.bubbleWidth = 137;
    act(() => ro.trigger());
    expect(col.style.minWidth).toBe(`min(277px, ${BUBBLE_MAX_W_PCT}%)`);

    // 隐藏态：行与列都量成 0
    state.rowRight = 0;
    state.colLeft = 0;
    state.bubbleWidth = 0;
    act(() => ro.trigger());
    expect(col.style.minWidth).toBe(`min(277px, ${BUBBLE_MAX_W_PCT}%)`);
  });

  it("下限不越上限：粘性下限与限宽上限取小，窗口收窄也顶不出上限", () => {
    // 纯 px 的下限会盖掉列上的百分比上限（min-width 赢过 max-width），
    // 窗口收窄后气泡又被撑到头像底下——下限必须自带同一个上限
    const { col, state, ro } = renderAssistant();

    state.bubbleWidth = 700;
    act(() => ro.trigger());
    state.bubbleWidth = 300;
    act(() => ro.trigger());
    expect(col.style.minWidth).toBe(`min(700px, ${BUBBLE_MAX_W_PCT}%)`);
  });

  it("用户气泡不参与：宽度下限只管机器人气泡", () => {
    const messages: Msg[] = [{ role: "user", content: "在吗" }];
    render(<MessageList messages={messages} {...baseProps} />);
    expect(FakeResizeObserver.instances).toHaveLength(0);
  });
});
