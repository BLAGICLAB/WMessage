import { describe, expect, it, vi } from "vitest";
import { render } from "@testing-library/react";
import { TaskPool } from "./TaskPool";

function renderPool(props: { open?: boolean; evaded?: boolean } = {}) {
  const { container } = render(
    <TaskPool
      tasks={[]}
      open={props.open ?? true}
      evaded={props.evaded ?? false}
      onToggle={vi.fn()}
    />,
  );
  const aside = container.querySelector("aside");
  expect(aside).not.toBeNull();
  return aside as HTMLElement;
}

describe("任务池滑出/让位的过渡", () => {
  // 池子滑开是拖拽与折叠的即时反馈。原先退场用 ease-in 家族曲线
  // （cubic-bezier(0.3,0,0.8,0.15)），前 40% 时间只滑了 15% 的路，
  // 看着像卡了一下；进场却是快出曲线，两个方向像两个不同的物体。
  it("进出两个方向用同一条 ease-out 曲线", () => {
    for (const props of [{}, { open: false }, { evaded: true }]) {
      expect(renderPool(props).style.transitionTimingFunction).toBe(
        "var(--ease-out)",
      );
    }
  });

  // 时长的非对称是有意的：退场是「让开视野，别挡路」，要比入场快
  it("时长仍是不对称：退场 150ms / 入场 240ms", () => {
    expect(renderPool({ open: false }).style.transitionDuration).toBe("150ms");
    expect(renderPool({ evaded: true }).style.transitionDuration).toBe("150ms");
    expect(renderPool().style.transitionDuration).toBe("240ms");
  });

  it("过渡的属性是 transform 与 opacity（两个都是合成器属性）", () => {
    expect(renderPool().style.transitionProperty).toBe("transform, opacity");
  });
});
