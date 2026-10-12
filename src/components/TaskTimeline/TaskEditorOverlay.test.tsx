import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { TaskEditorOverlay } from "./TaskEditorOverlay";
import type { Task } from "../../types";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn().mockResolvedValue(undefined),
}));
vi.mock("@tauri-apps/api/event", () => ({
  emit: vi.fn().mockResolvedValue(undefined),
}));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));
vi.mock("@tauri-apps/api/webviewWindow", () => ({
  WebviewWindow: { getByLabel: vi.fn().mockResolvedValue(null) },
}));

const base: Task = { id: "p1", title: "排期任务", column: "todo" };

afterEach(() => {
  cleanup();
});

/** 等一帧 rAF：滑入态（entered=true）在下一帧才生效 */
const nextFrame = () =>
  act(
    () =>
      new Promise<void>((r) => requestAnimationFrame(() => r())),
  );

const setup = (
  over: Partial<Task> = {},
  props: { autoFocusTitle?: boolean; closing?: boolean } = {},
) => {
  const onClosed = vi.fn();
  const onClose = vi.fn();
  render(
    <TaskEditorOverlay
      task={{ ...base, ...over }}
      autoFocusTitle={props.autoFocusTitle}
      closing={props.closing}
      onClosed={onClosed}
      onClose={onClose}
      onUpdate={vi.fn()}
      onSetColumn={vi.fn()}
      onDelete={vi.fn()}
    />,
  );
  return { onClosed, onClose };
};

describe("TaskEditorOverlay", () => {
  it("毛玻璃浮窗渲染面板内容：初始屏外，下一帧滑入（translateX(0)）", async () => {
    setup();
    expect(screen.getByLabelText("任务详情")).toBeInTheDocument();
    const win = screen.getByLabelText("任务详情").parentElement!;
    expect(win.style.transform).toContain("calc(-100% - 212px)");
    await nextFrame();
    expect(win.style.transform).toBe("translateX(0)");
    expect(win.style.opacity).toBe("1");
  });

  it("closing=true 滑出退场，退场完成后回调 onClosed（卸载由接线方执行）", async () => {
    const { onClosed } = setup({}, { closing: true });
    const win = screen.getByLabelText("任务详情").parentElement!;
    expect(win.style.transform).toContain("calc(-100% - 212px)");
    await vi.waitFor(() => expect(onClosed).toHaveBeenCalledTimes(1));
  });

  it("面板 Esc → onClose（接线方据此置 closing），浮窗不自行卸载", async () => {
    const { onClosed, onClose } = setup();
    await nextFrame();
    fireEvent.keyDown(window, { key: "Escape" });
    expect(onClose).toHaveBeenCalledTimes(1);
    expect(onClosed).not.toHaveBeenCalled();
  });

  it("任务切换（closing=false 换 id）：原位换内容不退场，onClosed 不误发", async () => {
    const { rerender } = render(
      <TaskEditorOverlay
        task={base}
        onClosed={vi.fn()}
        onClose={vi.fn()}
        onUpdate={vi.fn()}
        onSetColumn={vi.fn()}
        onDelete={vi.fn()}
      />,
    );
    await nextFrame();
    rerender(
      <TaskEditorOverlay
        task={{ id: "p2", title: "另一张卡", column: "doing" }}
        onClosed={vi.fn()}
        onClose={vi.fn()}
        onUpdate={vi.fn()}
        onSetColumn={vi.fn()}
        onDelete={vi.fn()}
      />,
    );
    expect(screen.getByDisplayValue("另一张卡")).toBeInTheDocument();
    expect(screen.queryByDisplayValue("排期任务")).not.toBeInTheDocument();
  });

  it("autoFocusTitle 透传：新建路径聚焦标题并全选", async () => {
    const user = userEvent.setup();
    setup({ title: "新任务" }, { autoFocusTitle: true });
    const title = screen.getByLabelText("任务标题") as HTMLTextAreaElement;
    expect(document.activeElement).toBe(title);
    await user.keyboard("abc");
    // 全选态下输入直接覆盖旧值
    expect(title.value).toBe("abc");
  });
});
