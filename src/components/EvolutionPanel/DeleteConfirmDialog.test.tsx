import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { DeleteConfirmDialog } from "./DeleteConfirmDialog";
import type { ProposalEntry } from "./types";

const proposal = {
  proposal_id: "p-1",
  suggestion_text: "测试提案",
  related_refs: [],
} as unknown as ProposalEntry;

describe("DeleteConfirmDialog 焦点管理与关闭路径", () => {
  it("挂载后焦点进弹窗（落「取消」钮）；卸载恢复焦点到触发源", () => {
    const trigger = document.createElement("button");
    document.body.appendChild(trigger);
    trigger.focus();
    const { unmount } = render(
      <DeleteConfirmDialog
        proposal={proposal}
        busy={false}
        onConfirm={() => {}}
        onCancel={() => {}}
      />,
    );
    expect(document.activeElement).toBe(screen.getByTestId("delete-cancel"));
    unmount();
    expect(document.activeElement).toBe(trigger);
    document.body.removeChild(trigger);
  });

  it("backdrop 按下调 onCancel；点内卡不调", () => {
    const onCancel = vi.fn();
    render(
      <DeleteConfirmDialog
        proposal={proposal}
        busy={false}
        onConfirm={() => {}}
        onCancel={onCancel}
      />,
    );
    // 内卡点击不外冒（Modal 注入 pointerdown stopProp）；断言按文本
    fireEvent.click(screen.getByText("彻底删除提案"));
    expect(onCancel).not.toHaveBeenCalled();
    // backdrop 关闭走 pointerdown（与多数弹窗一致），不再是 click
    fireEvent.pointerDown(screen.getByTestId("delete-confirm-modal"));
    expect(onCancel).toHaveBeenCalledTimes(1);
  });

  it("busy 时 backdrop 按下与 Esc 都不调 onCancel", async () => {
    const onCancel = vi.fn();
    const user = userEvent.setup();
    render(
      <DeleteConfirmDialog
        proposal={proposal}
        busy={true}
        onConfirm={() => {}}
        onCancel={onCancel}
      />,
    );
    fireEvent.pointerDown(screen.getByTestId("delete-confirm-modal"));
    await user.keyboard("{Escape}");
    expect(onCancel).not.toHaveBeenCalled();
  });

  it("Esc（非 busy）调 onCancel", async () => {
    const onCancel = vi.fn();
    const user = userEvent.setup();
    render(
      <DeleteConfirmDialog
        proposal={proposal}
        busy={false}
        onConfirm={() => {}}
        onCancel={onCancel}
      />,
    );
    await user.keyboard("{Escape}");
    expect(onCancel).toHaveBeenCalledTimes(1);
  });
});
