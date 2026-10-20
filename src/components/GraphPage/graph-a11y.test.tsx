// 画布可访问性单测：可聚焦、可命名、有操作说明，键盘能走图。
// jsdom 没有 WebGL2 上下文，画布起不来（组件会给出说明而不是崩掉），
// 但键盘游标是在建渲染器之前就挂好的，这里照样能测。
import { describe, expect, it, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import type { Task } from "../../types";
import GraphCanvas from "./GraphCanvas";
import { buildTaskGraph, DEFAULT_FILTERS } from "./graph-build";

function t(partial: Partial<Task> & { id: string }): Task {
  return { title: `任务-${partial.id}`, column: "todo", ...partial };
}

const TASKS: Task[] = [
  t({ id: "a", title: "甲", column: "doing", dependsOn: ["b"] }),
  t({ id: "b", title: "乙", column: "done" }),
  t({ id: "c", title: "丙", tags: ["周报"] }),
];

function renderCanvas(onSelect = vi.fn()) {
  const graph = buildTaskGraph(TASKS, DEFAULT_FILTERS);
  render(
    <GraphCanvas
      graph={graph}
      colorMode="tag"
      labelDensity="standard"
      edgeWidth="standard"
      autoLayout={false}
      looseness="standard"
      ownerOrder={new Map()}
      tagOrder={new Map()}
      relayoutSignal={0}
      selectedId={null}
      hoverId={null}
      searchMatchIds={null}
      onHover={vi.fn()}
      onSelect={onSelect}
      obstacleRects={() => []}
    />,
  );
  return screen.getByTestId("graph-canvas");
}

describe("画布可访问性", () => {
  it("画布可聚焦，有可访问名与操作说明", () => {
    const canvas = renderCanvas();
    expect(canvas).toHaveAttribute("tabindex", "0");
    expect(canvas).toHaveAttribute("role", "application");
    expect(canvas.getAttribute("aria-label")).toContain("任务图谱");
    const hintId = canvas.getAttribute("aria-describedby");
    expect(hintId).toBeTruthy();
    expect(document.getElementById(hintId!)?.textContent).toContain("Esc");
  });

  it("方向键移动游标、回车选中任务、Esc 清除选择", () => {
    const onSelect = vi.fn();
    const canvas = renderCanvas(onSelect);
    fireEvent.keyDown(canvas, { key: "ArrowRight" });
    fireEvent.keyDown(canvas, { key: "Enter" });
    expect(onSelect).toHaveBeenCalledTimes(1);
    const picked = onSelect.mock.calls[0][0] as string;
    expect(picked).toBeTruthy();
    expect(picked.startsWith("hub:")).toBe(false); // 游标只落在任务节点上
    fireEvent.keyDown(canvas, { key: "Escape" });
    expect(onSelect).toHaveBeenLastCalledWith(null);
  });

  it("没有 WebGL2 时给出说明，不静默白屏", () => {
    const canvas = renderCanvas();
    expect(canvas.textContent).toContain("WebGL2");
  });
});
