import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render } from "@testing-library/react";

const mockState = vi.hoisted(() => ({ rejectPosition: false }));

vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({
    outerPosition: mockState.rejectPosition
      ? async () => {
          throw new Error("permission denied");
        }
      : async () => ({ x: 100, y: 100 }),
    outerSize: async () => ({ width: 300, height: 200 }),
    scaleFactor: async () => 1,
  }),
  currentMonitor: vi.fn(async () => null),
}));

import { ResizeEdge } from "./ResizeEdge";
import { SplitBar } from "./SplitBar";
import { setWidgetDragActive, widgetDragActive } from "./storage";

const flush = () => new Promise((r) => setTimeout(r, 0));

beforeEach(() => {
  setWidgetDragActive(false);
  mockState.rejectPosition = false;
});

describe("ResizeEdge listener 生命周期", () => {
  it("正常拖动：move 派发 onDelta，pointerup 后复位且不再派发", async () => {
    const onDelta = vi.fn();
    const { container } = render(<ResizeEdge side="e" onDelta={onDelta} />);
    const el = container.firstChild as HTMLElement;
    fireEvent.pointerDown(el, { button: 0, clientX: 10, clientY: 10, pointerId: 1 });
    expect(widgetDragActive).toBe(true);
    await flush(); // Promise.all 落地，startPos 就位
    fireEvent.pointerMove(window, { clientX: 25, clientY: 20 });
    expect(onDelta).toHaveBeenCalledTimes(1);
    fireEvent.pointerUp(window, { pointerId: 1 });
    expect(widgetDragActive).toBe(false);
    fireEvent.pointerMove(window, { clientX: 40, clientY: 40 });
    expect(onDelta).toHaveBeenCalledTimes(1);
  });

  it("mid-drag unmount：widgetDragActive 复位，move 不再派发", async () => {
    const onDelta = vi.fn();
    const { container, unmount } = render(<ResizeEdge side="e" onDelta={onDelta} />);
    const el = container.firstChild as HTMLElement;
    fireEvent.pointerDown(el, { button: 0, clientX: 10, clientY: 10, pointerId: 1 });
    await flush();
    unmount();
    expect(widgetDragActive).toBe(false);
    fireEvent.pointerMove(window, { clientX: 30, clientY: 30 });
    expect(onDelta).not.toHaveBeenCalled();
  });

  it("起始位置读取失败（Promise.all reject）：不炸、move 不派发、pointerup 正常复位", async () => {
    mockState.rejectPosition = true;
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    const onDelta = vi.fn();
    const { container } = render(<ResizeEdge side="e" onDelta={onDelta} />);
    const el = container.firstChild as HTMLElement;
    fireEvent.pointerDown(el, { button: 0, clientX: 10, clientY: 10, pointerId: 1 });
    expect(widgetDragActive).toBe(true);
    await flush(); // Promise.all reject → .catch 分支
    expect(warn).toHaveBeenCalled();
    fireEvent.pointerMove(window, { clientX: 25, clientY: 20 });
    expect(onDelta).not.toHaveBeenCalled(); // startPos 为 null，move 忽略
    fireEvent.pointerUp(window, { pointerId: 1 });
    expect(widgetDragActive).toBe(false);
  });

  it("右键不起拖", () => {
    const onDelta = vi.fn();
    const { container } = render(<ResizeEdge side="n" onDelta={onDelta} />);
    fireEvent.pointerDown(container.firstChild as HTMLElement, {
      button: 2,
      clientX: 5,
      clientY: 5,
      pointerId: 1,
    });
    expect(widgetDragActive).toBe(false);
  });
});

describe("SplitBar listener 生命周期", () => {
  it("正常拖动：move 派发 onSplit，pointerup 后复位且不再派发", () => {
    const onSplit = vi.fn();
    const { container } = render(<SplitBar onSplit={onSplit} onArrow={() => {}} />);
    const el = container.firstChild as HTMLElement;
    fireEvent.pointerDown(el, { button: 0, clientX: 10, clientY: 10, pointerId: 1 });
    expect(widgetDragActive).toBe(true);
    fireEvent.pointerMove(window, { clientX: 10, clientY: 25 });
    expect(onSplit).toHaveBeenCalledWith(15);
    fireEvent.pointerUp(window, { pointerId: 1 });
    expect(widgetDragActive).toBe(false);
    fireEvent.pointerMove(window, { clientX: 10, clientY: 50 });
    expect(onSplit).toHaveBeenCalledTimes(1);
  });

  it("mid-drag unmount：widgetDragActive 复位，move 不再派发", () => {
    const onSplit = vi.fn();
    const { container, unmount } = render(
      <SplitBar onSplit={onSplit} onArrow={() => {}} />
    );
    fireEvent.pointerDown(container.firstChild as HTMLElement, {
      button: 0,
      clientX: 10,
      clientY: 10,
      pointerId: 1,
    });
    unmount();
    expect(widgetDragActive).toBe(false);
    fireEvent.pointerMove(window, { clientX: 10, clientY: 30 });
    expect(onSplit).not.toHaveBeenCalled();
  });

  it("pointerdown 落在 ▲/▼ 按钮上不起拖（保留按钮 onClick 路径）", () => {
    const onSplit = vi.fn();
    const { getByTitle } = render(<SplitBar onSplit={onSplit} onArrow={() => {}} />);
    fireEvent.pointerDown(getByTitle("聊天区变大"), {
      button: 0,
      clientX: 10,
      clientY: 10,
      pointerId: 1,
    });
    expect(widgetDragActive).toBe(false);
    fireEvent.pointerMove(window, { clientX: 10, clientY: 40 });
    expect(onSplit).not.toHaveBeenCalled();
  });
});
