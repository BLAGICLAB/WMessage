import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useInlineEdit } from "./useInlineEdit";

/** 测试线束：input 始终挂载（模拟编辑态由父组件控制、Escape 后 blur 仍触发的场景） */
function Harness({
  value,
  editing,
  onCommit,
  onCancel,
}: {
  value: string;
  editing: boolean;
  onCommit: (d: string) => void;
  onCancel?: () => void;
}) {
  const edit = useInlineEdit({ value, editing, onCommit, onCancel });
  return (
    <input
      data-testid="edit"
      value={edit.draft}
      onChange={(e) => edit.setDraft(e.target.value)}
      onBlur={edit.onBlur}
      onKeyDown={edit.onKeyDown}
    />
  );
}

// useInlineEdit 统一 TaskCardContent / TodoCard 的内联编辑行为，
// 这里锁定全部路径，防两处实现再次 drift（Escape/blur 类 bug 回归）
describe("useInlineEdit（P2-23）", () => {
  it("进入编辑态（false→true）草稿重置为当前已提交值", () => {
    const { rerender } = render(
      <Harness value="原标题" editing={false} onCommit={vi.fn()} />
    );
    const input = screen.getByTestId("edit") as HTMLInputElement;
    expect(input.value).toBe("原标题");
    // 模拟上一轮残留草稿（非编辑态被改写的情况不会发生，这里直接改值制造脏草稿）
    fireEvent.change(input, { target: { value: "脏草稿" } });
    expect(input.value).toBe("脏草稿");
    // false→true：草稿必须回到已提交值
    rerender(<Harness value="原标题" editing={true} onCommit={vi.fn()} />);
    expect(input.value).toBe("原标题");
  });

  it("编辑中外部 value 变化不覆盖用户输入", () => {
    const { rerender } = render(
      <Harness value="v1" editing={true} onCommit={vi.fn()} />
    );
    const input = screen.getByTestId("edit") as HTMLInputElement;
    fireEvent.change(input, { target: { value: "用户输入" } });
    rerender(<Harness value="v2" editing={true} onCommit={vi.fn()} />);
    expect(input.value).toBe("用户输入");
  });

  it("Enter 提交草稿", async () => {
    const user = userEvent.setup();
    const onCommit = vi.fn();
    render(<Harness value="原标题" editing={true} onCommit={onCommit} />);
    const input = screen.getByTestId("edit");
    await user.type(input, "改");
    fireEvent.keyDown(input, { key: "Enter" });
    expect(onCommit).toHaveBeenCalledWith("原标题改");
  });

  it("IME 组词中 Enter 不提交（isComposing）", () => {
    const onCommit = vi.fn();
    render(<Harness value="原标题" editing={true} onCommit={onCommit} />);
    fireEvent.keyDown(screen.getByTestId("edit"), {
      key: "Enter",
      isComposing: true,
    });
    expect(onCommit).not.toHaveBeenCalled();
  });

  it("Escape：调 onCancel + 草稿回滚已提交值，随后 blur 不提交（E4）", async () => {
    const user = userEvent.setup();
    const onCommit = vi.fn();
    const onCancel = vi.fn();
    render(
      <Harness
        value="原标题"
        editing={true}
        onCommit={onCommit}
        onCancel={onCancel}
      />
    );
    const input = screen.getByTestId("edit") as HTMLInputElement;
    await user.type(input, "改");
    fireEvent.keyDown(input, { key: "Escape" });
    expect(onCancel).toHaveBeenCalledTimes(1);
    expect(input.value).toBe("原标题"); // 草稿回滚
    fireEvent.blur(input);
    expect(onCommit).not.toHaveBeenCalled(); // 取消后的 blur 必须跳过提交
  });

  it("取消标记在 blur 跳过后重置：再次 blur 可正常提交", async () => {
    const user = userEvent.setup();
    const onCommit = vi.fn();
    render(
      <Harness
        value="原标题"
        editing={true}
        onCommit={onCommit}
        onCancel={vi.fn()}
      />
    );
    const input = screen.getByTestId("edit");
    await user.type(input, "改");
    fireEvent.keyDown(input, { key: "Escape" });
    fireEvent.blur(input);
    expect(onCommit).not.toHaveBeenCalled();
    // 标记已重置，后续 blur 正常提交当前草稿（Escape 已把草稿回滚为「原标题」）
    fireEvent.blur(input);
    expect(onCommit).toHaveBeenCalledWith("原标题");
  });

  it("未按 Escape 时 blur 正常提交草稿", async () => {
    const user = userEvent.setup();
    const onCommit = vi.fn();
    render(<Harness value="原标题" editing={true} onCommit={onCommit} />);
    const input = screen.getByTestId("edit");
    await user.type(input, "改");
    fireEvent.blur(input);
    expect(onCommit).toHaveBeenCalledWith("原标题改");
  });

  it("Enter 显式提交后跟随的 blur 不重复提交语义由调用方控制（hook 不拦截）", async () => {
    const user = userEvent.setup();
    const onCommit = vi.fn();
    render(<Harness value="原标题" editing={true} onCommit={onCommit} />);
    const input = screen.getByTestId("edit");
    await user.type(input, "改");
    fireEvent.keyDown(input, { key: "Enter" });
    expect(onCommit).toHaveBeenCalledTimes(1);
    // Enter 已重置取消标记，blur 仍会走 onCommit——
    // 生产组件里 Enter 提交会让父组件退出编辑态卸载 input，blur 不会再触发；
    // 若调用方让 input 保持挂载，重复提交是幂等的（父组件 trim/比较后自行决定写不写）
    fireEvent.blur(input);
    expect(onCommit).toHaveBeenCalledTimes(2);
  });

  it("无 onCancel 时 Escape 不崩溃（可选回调）", () => {
    const onCommit = vi.fn();
    render(<Harness value="x" editing={true} onCommit={onCommit} />);
    const input = screen.getByTestId("edit");
    fireEvent.keyDown(input, { key: "Escape" });
    fireEvent.blur(input);
    expect(onCommit).not.toHaveBeenCalled();
  });
});
