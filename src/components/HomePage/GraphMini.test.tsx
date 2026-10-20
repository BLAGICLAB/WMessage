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

function renderMini(tasks: Task[], dayKeys: string[], activeDay: number | null = 0) {
  return render(
    <GraphMini
      tasks={tasks}
      dayKeys={dayKeys}
      activeDay={activeDay}
      activeTaskId={null}
      onHoverTask={vi.fn()}
      onOpenTask={vi.fn()}
      onMore={vi.fn()}
    />,
  );
}

const opacityOf = (id: string) =>
  document.querySelector(`[data-testid='graph-node-${id}']`)!.getAttribute("opacity");

const fillOf = (id: string) =>
  document.querySelector(`[data-testid='graph-node-${id}']`)!.getAttribute("fill");

afterEach(cleanup);

describe("GraphMini 连线", () => {
  it("父子/依赖关系画出线，同一对关系只画一条", () => {
    renderMini(
      [
        task({ id: "t1", title: "父卡" }),
        task({ id: "t2", title: "子卡", parentId: "t1" }),
        task({ id: "t3", title: "依赖卡", dependsOn: ["t1"] }),
        // 依赖与父子指向同一对：去重后仍只该有一条线
        task({ id: "t4", title: "双重关系", parentId: "t1", dependsOn: ["t1"] }),
      ],
      ["2026-10-10"],
    );
    const lines = document.querySelectorAll('[data-testid="graph-mini"] line');
    expect(lines.length).toBe(3);
  });
});

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

describe("GraphMini 节点状态着色", () => {
  it("无联动时已完成节点填 --success（与图谱页完成色一致），未完成保持 --plan-slate", () => {
    renderMini(
      [
        task({ id: "d1", completedAt: Date.UTC(2026, 9, 10, 12, 0) }),
        task({ id: "w1", column: "todo" }),
      ],
      ["2026-10-10"],
      null,
    );
    expect(fillOf("d1")).toBe("var(--success)");
    expect(fillOf("w1")).toBe("var(--plan-slate)");
  });

  it("联动高亮仍优先于状态色（lit 节点是 accent，不因完成改色）", () => {
    renderMini(
      [task({ id: "d2", completedAt: Date.UTC(2026, 9, 10, 12, 0) })],
      ["2026-10-10"],
      0,
    );
    expect(fillOf("d2")).toBe("var(--accent)");
  });
});

describe("GraphMini 规模上限", () => {
  it("节点超上限只画连接度最高的一批，并说明画的不是全部", () => {
    const tasks: Task[] = [];
    for (let i = 0; i < 320; i++) {
      const t = task({ id: `n${i}`, title: `任务 ${i}` });
      // 前 20 个任务各挂 5 条依赖 → 连接度最高，它们必须留下
      if (i < 20) t.dependsOn = [`n${i + 100}`, `n${i + 101}`, `n${i + 102}`, `n${i + 103}`];
      tasks.push(t);
    }
    const { container } = renderMini(tasks, ["2026-10-11"], null);
    const circles = container.querySelectorAll("circle");
    expect(circles.length).toBe(300);
    // 连接度最高的那批在图里
    expect(container.querySelector("[data-testid='graph-node-n0']")).not.toBeNull();
    // 卡片上说明这是截断后的视图
    expect(container.textContent).toContain("连接度前 300 个");
  });
});
