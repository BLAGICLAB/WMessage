import { describe, expect, it, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { Archive } from "lucide-react";
import { EmptyState } from "./EmptyState";

describe("EmptyState", () => {
  it("渲染图标/标题/说明，缺省说明不渲染", () => {
    const { rerender } = render(
      <EmptyState icon={<Archive size={18} aria-hidden />} title="暂无归档内容" />,
    );
    expect(screen.getByText("暂无归档内容")).toBeInTheDocument();
    expect(screen.queryByRole("button")).not.toBeInTheDocument();
    rerender(
      <EmptyState
        icon={<Archive size={18} aria-hidden />}
        title="暂无归档内容"
        description="完成任务 7 天后自动归档"
      />,
    );
    expect(screen.getByText("完成任务 7 天后自动归档")).toBeInTheDocument();
  });

  it("行动按钮：label 渲染 + 点击回调", () => {
    const onClick = vi.fn();
    render(
      <EmptyState
        icon={<Archive size={18} aria-hidden />}
        title="暂无服务器"
        description="接入第一个 MCP 服务器"
        action={{ label: "添加服务器", onClick }}
      />,
    );
    fireEvent.click(screen.getByText("添加服务器"));
    expect(onClick).toHaveBeenCalledTimes(1);
  });
});
