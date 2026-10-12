import { describe, it, expect, afterEach } from "vitest";
import { cleanup, render } from "@testing-library/react";
import { GraphMini } from "./GraphMini";
import type { Task } from "../../types";

// 完成日匹配必须与 homeData 的本地 dayKey 同口径。本文件固定西五区跑断言：
// 西半球时区下 new Date(dayKey)（UTC 解析）配 toDateString（本地渲染）会整体偏一天，
// 东八区无法复现（UTC 零点 +8 仍是同一天）。
process.env.TZ = "America/New_York";

function task(partial: Partial<Task> & { id: string }): Task {
  return { column: "done", ...partial } as Task;
}

function renderMini(tasks: Task[], dayKeys: string[]) {
  return render(
    <GraphMini
      tasks={tasks}
      dayKeys={dayKeys}
      activeDay={0}
      activeTaskId={null}
      onHoverTask={vi.fn()}
      onOpenTask={vi.fn()}
      onMore={vi.fn()}
    />,
  );
}

const opacityOf = (id: string) =>
  document.querySelector(`[data-testid='graph-node-${id}']`)!.getAttribute("opacity");

afterEach(cleanup);

describe("GraphMini 完成日匹配（本地时区口径）", () => {
  it("本地日界前完成（纽约 10-10 23:00）计入 10-10 当天，不因 UTC 解析偏一天", () => {
    renderMini(
      [task({ id: "t1", completedAt: Date.UTC(2026, 9, 11, 3, 0) })],
      ["2026-10-10"],
    );
    expect(opacityOf("t1")).toBe("1");
  });

  it("别的本地日完成仍降透明（口径不过扩）", () => {
    renderMini(
      [task({ id: "t2", completedAt: Date.UTC(2026, 9, 11, 20, 0) })],
      ["2026-10-10"],
    );
    expect(opacityOf("t2")).toBe("0.22");
  });
});
