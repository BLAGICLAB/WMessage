import { act, fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { useHoverTitleTip } from "./HoverTitle";

function Probe({ text }: { text: string }) {
  const tip = useHoverTitleTip();
  return (
    <div>
      <div
        data-testid="probe"
        onPointerEnter={tip.show(text)}
        onPointerLeave={tip.hide}
        onPointerDown={tip.hide}
      >
        截断的标题…
      </div>
      {tip.chip}
    </div>
  );
}

describe("useHoverTitleTip", () => {
  it("悬停静置 350ms 显示完整标题，移开消失", () => {
    vi.useFakeTimers();
    try {
      render(<Probe text="这是一个很长很长不会被截断显示的完整任务标题" />);
      fireEvent.pointerEnter(screen.getByTestId("probe"));
      // 未到延时：不显示
      act(() => {
        vi.advanceTimersByTime(200);
      });
      expect(screen.queryByText(/完整任务标题/)).not.toBeInTheDocument();
      // 350ms 到：portal 显示全文
      act(() => {
        vi.advanceTimersByTime(200);
      });
      expect(
        screen.getByText("这是一个很长很长不会被截断显示的完整任务标题"),
      ).toBeInTheDocument();
      // 移开即收
      fireEvent.pointerLeave(screen.getByTestId("probe"));
      expect(screen.queryByText(/完整任务标题/)).not.toBeInTheDocument();
    } finally {
      vi.useRealTimers();
    }
  });

  it("按下（拖拽开始）即隐藏，不再弹出", () => {
    vi.useFakeTimers();
    try {
      render(<Probe text="拖拽中的标题" />);
      const probe = screen.getByTestId("probe");
      fireEvent.pointerEnter(probe);
      fireEvent.pointerDown(probe);
      act(() => {
        vi.advanceTimersByTime(500);
      });
      expect(screen.queryByText("拖拽中的标题")).not.toBeInTheDocument();
    } finally {
      vi.useRealTimers();
    }
  });
});
