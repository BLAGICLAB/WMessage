import { fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { TaskTimelinePage } from "./TaskTimelinePage";
import { startOfWeek } from "./week";
import type { Task } from "../../types";

const MON = startOfWeek(new Date());
const day = (i: number) => {
  const d = new Date(MON);
  d.setDate(d.getDate() + i);
  return d;
};
const dt = (d: Date, hm: string) =>
  `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}T${hm}`;

const mk = (over: Partial<Task> & { id: string; title: string }): Task => ({
  column: "todo",
  updatedAt: 1000,
  ...over,
});

const TASKS: Task[] = [
  mk({ id: "t1", title: "已排期任务甲", tags: ["设计"], planStart: dt(day(2), "09:00"), planEnd: dt(day(2), "10:00") }),
  mk({ id: "t2", title: "未排期任务乙", updatedAt: 2000 }),
  mk({ id: "t3", title: "已完成任务丙", column: "done", completedAt: 1 }),
  mk({ id: "t4", title: "已删除任务丁", deletedAt: 5 }),
  mk({ id: "t5", title: "已归档任务戊", archived: true }),
  mk({ id: "t6", title: "已排期任务己", tags: ["开发"], planStart: dt(day(0), "14:00"), planEnd: dt(day(0), "15:00") }),
  mk({ id: "t7", title: "已归档排期任务庚", archived: true, planStart: dt(day(3), "09:00"), planEnd: dt(day(3), "10:30") }),
];

beforeEach(() => {
  localStorage.clear();
});

describe("TaskTimelinePage", () => {
  it("渲染周网格骨架：周一~周日列头、8:00/18:00 刻度、任务池", () => {
    render(<TaskTimelinePage tasks={TASKS} />);
    expect(screen.getByText("周一")).toBeInTheDocument();
    expect(screen.getByText("周日")).toBeInTheDocument();
    expect(screen.getByText("08:00")).toBeInTheDocument();
    expect(screen.getByText("18:00")).toBeInTheDocument();
    expect(screen.getByLabelText("任务池")).toBeInTheDocument();
  });

  it("池列出全部未完成（含已排期），done/软删/归档不上池", () => {
    render(<TaskTimelinePage tasks={TASKS} />);
    const pool = within(screen.getByLabelText("任务池"));
    expect(pool.getByText("未排期任务乙")).toBeInTheDocument();
    expect(pool.getByText("已排期任务甲")).toBeInTheDocument();
    expect(pool.queryByText("已完成任务丙")).not.toBeInTheDocument();
    expect(pool.queryByText("已删除任务丁")).not.toBeInTheDocument();
    expect(pool.queryByText("已归档任务戊")).not.toBeInTheDocument();
    // 已排期带时间角标
    expect(pool.getByText("三 09:00")).toBeInTheDocument();
  });

  it("池排序：已排期按 planStart 升序在前，未排期按 updatedAt 降序在后", () => {
    render(<TaskTimelinePage tasks={TASKS} />);
    const pool = within(screen.getByLabelText("任务池"));
    const titles = pool
      .getAllByText(/已排期任务|未排期任务/)
      .map((el) => el.textContent);
    expect(titles).toEqual(["已排期任务己", "已排期任务甲", "未排期任务乙"]);
  });

  it("已排期任务在网格上渲染计划块（池与网格各出现一次）", () => {
    render(<TaskTimelinePage tasks={TASKS} />);
    expect(screen.getAllByText("已排期任务甲").length).toBe(2);
  });

  it("块首行：头像最左 + 完成对勾最右；点对勾标记完成且不开浮窗", async () => {
    const user = userEvent.setup();
    const onSetColumn = vi.fn();
    const onOpenTask = vi.fn();
    render(
      <TaskTimelinePage
        tasks={TASKS}
        onUpdate={vi.fn()}
        onSetColumn={onSetColumn}
        onOpenTask={onOpenTask}
      />,
    );
    const blk = screen
      .getAllByText("已排期任务甲")
      .map((el) => el.closest(".plan-blk"))
      .find(Boolean)! as HTMLElement;
    const bar = blk.querySelector(".plan-topbar")!;
    // 头像在首行最左、对勾最右
    expect(bar.firstElementChild).not.toBeNull();
    const check = within(blk).getByRole("button", { name: "标记完成" });
    expect(bar.contains(check)).toBe(true);
    await user.click(check);
    expect(onSetColumn).toHaveBeenCalledWith("t1", "done");
    expect(onOpenTask).not.toHaveBeenCalled();
  });

  it("标题行数与时间格式：90 分钟 2 行 + 时长「1.5 小时」，3 小时长卡、30 分钟矮块同行退化", () => {
    render(
      <TaskTimelinePage
        tasks={[
          mk({ id: "v1", title: "一个半小时任务", planStart: dt(day(1), "09:00"), planEnd: dt(day(1), "10:30") }),
          mk({ id: "v2", title: "三小时长任务标题很长很长很长很长很长很长很长很长很长很长很长很长", planStart: dt(day(1), "11:00"), planEnd: dt(day(1), "14:00") }),
          mk({ id: "v3", title: "半小时任务", planStart: dt(day(1), "15:00"), planEnd: dt(day(1), "15:30") }),
        ]}
      />,
    );
    const blkOf = (title: string) =>
      screen
        .getAllByText(title)
        .map((el) => el.closest(".plan-blk"))
        .find(Boolean)!;
    const half = blkOf("一个半小时任务") as HTMLElement;
    expect(half.querySelector(".plan-blk-title")!.textContent).toContain(
      "一个半小时任务",
    );
    // 时间行只显时长（小时），不显开始时间
    expect(half.textContent).toContain("1.5 小时");
    expect(half.textContent).not.toContain("09:00");
    const longTitle = "三小时长任务标题很长很长很长很长很长很长很长很长很长很长很长很长";
    const long = blkOf(longTitle);
    expect(long.querySelector(".plan-blk-title")).toBeTruthy();
    expect(long.textContent).toContain("3 小时");
    const thin = blkOf("半小时任务") as HTMLElement;
    expect(thin.className).toContain("thin");
    expect(thin.querySelector(".plan-topbar")).toBeNull();
    expect(thin.textContent).not.toContain("0.5 小时");
  });

  it("双卡重叠：两张都是完整卡（半区并显，不塌缩）", () => {
    render(
      <TaskTimelinePage
        tasks={[
          mk({ id: "p1", title: "双卡甲", planStart: dt(day(4), "10:00"), planEnd: dt(day(4), "11:00") }),
          mk({ id: "p2", title: "双卡乙", planStart: dt(day(4), "10:30"), planEnd: dt(day(4), "11:30") }),
        ]}
      />,
    );
    const jia = screen
      .getAllByText("双卡甲")
      .map((el) => el.closest(".plan-blk"))
      .find(Boolean)! as HTMLElement;
    const yi = screen
      .getAllByText("双卡乙")
      .map((el) => el.closest(".plan-blk"))
      .find(Boolean)! as HTMLElement;
    expect(jia.className).not.toContain("plan-stack");
    expect(yi.className).not.toContain("plan-stack");
    expect(jia.className).toContain("narrow");
    expect(yi.className).toContain("narrow");
    expect(jia.textContent).toContain("1 小时");
  });

  it("同簇 3+ 重叠：整簇塌缩（所有卡只显头像 + 对勾，无标题无时间），保持分栏几何", () => {
    const four = [
      mk({ id: "s1", title: "重叠甲", planStart: dt(day(3), "10:00"), planEnd: dt(day(3), "11:00") }),
      mk({ id: "s2", title: "重叠乙", planStart: dt(day(3), "10:00"), planEnd: dt(day(3), "11:00") }),
      mk({ id: "s3", title: "重叠丙", planStart: dt(day(3), "10:00"), planEnd: dt(day(3), "11:00") }),
      mk({ id: "s4", title: "重叠丁", planStart: dt(day(3), "10:00"), planEnd: dt(day(3), "11:00") }),
    ];
    render(<TaskTimelinePage tasks={four} />);
    const gridText = [...document.querySelectorAll(".plan-blk")]
      .map((el) => el.textContent)
      .join();
    expect(gridText).not.toContain("重叠甲");
    expect(gridText).not.toContain("重叠乙");
    expect(gridText).not.toContain("重叠丙");
    expect(gridText).not.toContain("重叠丁");
    // 全部 4 张都塌缩：头像 + 对勾上下排（plan-stack-bar），无标题无时间
    const stacks = document.querySelectorAll(".plan-blk.plan-stack");
    expect(stacks.length).toBe(4);
    for (const st of stacks) {
      expect(st.querySelector(".plan-stack-bar")).not.toBeNull();
      expect(st.querySelector(".plan-blk-title")).toBeNull();
      expect(st.textContent).not.toContain("小时");
    }
  });

  it("已完成任务保留网格原位（无划线，对勾恢复待办），池收进已完成折叠区", async () => {
    const user = userEvent.setup();
    const onSetColumn = vi.fn();
    render(
      <TaskTimelinePage
        tasks={[
          mk({
            id: "d1",
            title: "完成也留位",
            column: "done",
            completedAt: 1,
            tags: ["设计"],
            planStart: dt(day(1), "09:00"),
            planEnd: dt(day(1), "10:00"),
          }),
        ]}
        onUpdate={vi.fn()}
        onSetColumn={onSetColumn}
      />,
    );
    const blk = screen
      .getAllByText("完成也留位")
      .map((el) => el.closest(".plan-blk"))
      .find(Boolean)! as HTMLElement;
    expect(blk.className).toContain("is-done");
    // 完成语义 = 绿勾；不划线（无 line-through 类）
    expect(blk.querySelector(".line-through")).toBeNull();
    expect(blk.textContent).toContain("完成也留位");
    await user.click(within(blk).getByRole("button", { name: "恢复待办" }));
    expect(onSetColumn).toHaveBeenCalledWith("d1", "todo");
    const pool = within(screen.getByLabelText("任务池"));
    expect(pool.getByText(/已完成 1/)).toBeInTheDocument();
  });

  it("任务池开合：页头开关切换，池滑出/滑入带过渡类，状态记忆进 localStorage", async () => {
    const user = userEvent.setup();
    render(
      <TaskTimelinePage
        tasks={TASKS}
        onUpdate={vi.fn()}
        onSetColumn={vi.fn()}
      />,
    );
    const pool = screen.getByLabelText("任务池");
    expect(pool.className).toContain("opacity-100");
    // 页头开关（与新建任务同行）：点后池滑出（transform + opacity 过渡态）
    await user.click(screen.getByRole("button", { name: /任务池\d/ }));
    expect(pool.className).toContain("opacity-0");
    expect(pool.style.transform).toContain("translateX");
    expect(localStorage.getItem("wm-task-pool-open")).toBe("0");
    // 再点滑回
    await user.click(screen.getByRole("button", { name: /任务池\d/ }));
    expect(pool.className).toContain("opacity-100");
    expect(localStorage.getItem("wm-task-pool-open")).toBe("1");
    // 池内 ⇥ 也能折叠（与页头开关同状态）
    await user.click(within(pool).getByRole("button", { name: "折叠任务池" }));
    expect(screen.getByLabelText("任务池").className).toContain("opacity-0");
  });

  it("周导航：‹ 翻到上周出现「今天」，点「今天」回当前周；当前周不显示「今天」", async () => {
    const user = userEvent.setup();
    render(<TaskTimelinePage tasks={[]} />);
    const label = () => screen.getByText(/月.*日 –/);
    const initial = label().textContent;
    expect(screen.queryByText("今天")).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "上一周" }));
    expect(label().textContent).not.toBe(initial);
    expect(screen.getByText("今天")).toBeInTheDocument();
    await user.click(screen.getByText("今天"));
    expect(label().textContent).toBe(initial);
    expect(screen.queryByText("今天")).not.toBeInTheDocument();
  });

  it("空任务：页面骨架照常渲染（网格 + 空池提示）", () => {
    render(<TaskTimelinePage tasks={[]} />);
    expect(screen.getByLabelText("任务池")).toBeInTheDocument();
    expect(screen.getByText(/没有未完成的任务/)).toBeInTheDocument();
  });

  it("显式 null 计划字段（拖回池/清除计划写入）不上网格、不炸渲染", () => {
    render(
      <TaskTimelinePage
        tasks={[
          mk({ id: "n1", title: "已清除计划的任务", planStart: null, planEnd: null }),
        ]}
        onUpdate={vi.fn()}
        onSetColumn={vi.fn()}
      />,
    );
    // 池里照常可见（未完成），网格无块（只有池 + 面板外共 1 处文本）
    expect(screen.getAllByText("已清除计划的任务")).toHaveLength(1);
  });

  it("onNewTask：页头新建按钮透传回调", async () => {
    const user = userEvent.setup();
    let called = 0;
    render(<TaskTimelinePage tasks={[]} onNewTask={() => called++} />);
    await user.click(screen.getByRole("button", { name: "＋ 新建任务" }));
    expect(called).toBe(1);
  });

  it("池块单击纯拖拽不开浮窗（防误触）；双击池项回传 onOpenTask；圆圈快速打勾", async () => {
    const user = userEvent.setup();
    const onSetColumn = vi.fn();
    const onOpenTask = vi.fn();
    render(
      <TaskTimelinePage
        tasks={TASKS}
        onUpdate={vi.fn()}
        onSetColumn={onSetColumn}
        onOpenTask={onOpenTask}
      />,
    );
    // 原位按下松开 = 单击 → 不开浮窗
    fireEvent.pointerDown(screen.getByText("未排期任务乙"), { button: 0 });
    fireEvent.pointerUp(screen.getByText("未排期任务乙"));
    expect(onOpenTask).not.toHaveBeenCalled();
    // 双击池项 → 打开浮窗（App 侧按 id 解析）
    await user.dblClick(screen.getByText("未排期任务乙"));
    expect(onOpenTask).toHaveBeenCalledWith("t2");
    // 完成圆圈（该块尾部）→ set_column done
    const row = screen.getByText("未排期任务乙").closest("div")!;
    await user.click(
      within(row as HTMLElement).getByRole("button", { name: "标记完成" }),
    );
    expect(onSetColumn).toHaveBeenCalledWith("t2", "done");
  });

  it("网格块双击回传 onOpenTask；单击与对勾双击都不回传", async () => {
    const user = userEvent.setup();
    const onOpenTask = vi.fn();
    render(
      <TaskTimelinePage
        tasks={TASKS}
        onUpdate={vi.fn()}
        onSetColumn={vi.fn()}
        onOpenTask={onOpenTask}
      />,
    );
    const blk = screen
      .getAllByText("已排期任务己")
      .map((el) => el.closest(".plan-blk"))
      .find(Boolean)!;
    // 单击（原位按下松开）→ 不开浮窗
    fireEvent.pointerDown(blk, { button: 0 });
    fireEvent.pointerUp(window);
    expect(onOpenTask).not.toHaveBeenCalled();
    // 双击块身 → 打开浮窗
    await user.dblClick(blk);
    expect(onOpenTask).toHaveBeenCalledWith("t6");
  });

  it("已归档排期卡原位保留在网格：封存态（印章、无对勾、无拖拽把手），不进池", () => {
    render(<TaskTimelinePage tasks={TASKS} />);
    const blk = screen
      .getAllByText("已归档排期任务庚")
      .map((el) => el.closest(".plan-blk"))
      .find(Boolean) as HTMLElement;
    // 原位保留（网格上渲染）+ 封存态标识
    expect(blk).toHaveClass("is-archived");
    expect(blk.querySelector(".plan-stamp")?.textContent).toBe("已归档");
    // 不可操作面：无对勾按钮、无拖拽把手
    expect(within(blk).queryByRole("button", { name: /标记完成|恢复待办/ })).toBeNull();
    expect(blk.querySelector(".plan-grip")).toBeNull();
    // 不进任务池
    expect(within(screen.getByLabelText("任务池")).queryByText("已归档排期任务庚")).toBeNull();
  });

  it("已归档块仅双击跳归档区：双击回传 onOpenArchived，单击/拖拽面不回传", () => {
    const onOpenArchived = vi.fn();
    const onOpenTask = vi.fn();
    render(
      <TaskTimelinePage
        tasks={TASKS}
        onUpdate={vi.fn()}
        onOpenTask={onOpenTask}
        onOpenArchived={onOpenArchived}
      />,
    );
    const blk = screen
      .getAllByText("已归档排期任务庚")
      .map((el) => el.closest(".plan-blk"))
      .find(Boolean)!;
    // 单击（原位按下松开）→ 不跳转
    fireEvent.pointerDown(blk, { button: 0 });
    fireEvent.pointerUp(window);
    expect(onOpenArchived).not.toHaveBeenCalled();
    // 双击 → 跳归档区（且不误触 onOpenTask 编辑浮窗）
    fireEvent.doubleClick(blk);
    expect(onOpenArchived).toHaveBeenCalledWith("t7");
    expect(onOpenTask).not.toHaveBeenCalled();
  });

  it("已完成区打勾形式（无划线）：圆圈恢复待办，双击回传 onOpenTask", async () => {
    const user = userEvent.setup();
    const onSetColumn = vi.fn();
    const onOpenTask = vi.fn();
    render(
      <TaskTimelinePage
        tasks={[...TASKS, mk({ id: "t9", title: "完成任务庚", column: "done", completedAt: 1 })]}
        onUpdate={vi.fn()}
        onSetColumn={onSetColumn}
        onOpenTask={onOpenTask}
      />,
    );
    const pool = within(screen.getByLabelText("任务池"));
    await user.click(pool.getByText(/已完成/));
    const row = pool.getByText("完成任务庚").closest("div")!;
    // 无划线（打勾形式承载完成语义）
    expect(row.querySelector(".line-through")).toBeNull();
    await user.click(within(row as HTMLElement).getByRole("button", { name: "恢复待办" }));
    expect(onSetColumn).toHaveBeenCalledWith("t9", "todo");
    await user.dblClick(pool.getByText("完成任务庚"));
    expect(onOpenTask).toHaveBeenCalledWith("t9");
  });

  it("从池拖出：源块池内悬浮，离池整池滑出（translateX 让出视野）", () => {
    render(
      <TaskTimelinePage
        tasks={TASKS}
        onUpdate={vi.fn()}
        onSetColumn={vi.fn()}
      />,
    );
    const pool = screen.getByLabelText("任务池");
    expect(pool.style.transform).toBe("translateX(0)");
    // 按住源块（悬浮）→ 拖离池（jsdom 无布局，池矩形恒 0 → 移动即离池）→ 池滑出
    const poolItem = within(pool).getByText("未排期任务乙");
    fireEvent.pointerDown(poolItem, { button: 0 });
    fireEvent.pointerMove(window, { clientX: 1300, clientY: 400 });
    expect(poolItem.closest(".pool-lift")).not.toBeNull();
    expect(pool.style.transform).toContain("translateX(calc(100%");
    // 松手 → 拖拽态复位
    fireEvent.pointerUp(window, { clientX: 1300, clientY: 400 });
    expect(poolItem.closest(".pool-lift")).toBeNull();
    expect(pool.style.transform).toBe("translateX(0)");
  });

  it("拖拽幽灵用 transform 定位，不写 left/top 布局属性", () => {
    render(
      <TaskTimelinePage
        tasks={TASKS}
        onUpdate={vi.fn()}
        onSetColumn={vi.fn()}
      />,
    );
    const poolItem = within(screen.getByLabelText("任务池")).getByText(
      "未排期任务乙",
    );
    fireEvent.pointerDown(poolItem, { button: 0 });
    fireEvent.pointerMove(window, { clientX: 1300, clientY: 400 });
    const ghost = document.querySelector(".drag-ghost") as HTMLElement;
    expect(ghost).not.toBeNull();
    // 位移只由 transform 给：left/top 是布局属性，逐帧写会每帧重排
    expect(ghost.style.transform).toMatch(
      /^translate3d\(-?[\d.]+px, -?[\d.]+px, 0\)$/,
    );
    expect(ghost.style.left).toBe("");
    expect(ghost.style.top).toBe("");
  });

  it("拖拽中卸载组件：window 拖拽监听被清理（pointermove 不再触发处理，不抛错）", () => {
    // 包一层注册：记录真实 onMove 的调用次数（卸载后派发事件不得再进入处理）
    let moveCalls = 0;
    const originalAdd = window.addEventListener;
    const addSpy = vi
      .spyOn(window, "addEventListener")
      .mockImplementation(function (
        this: Window,
        type: string,
        fn: EventListenerOrEventListenerObject | null,
        opts?: boolean | AddEventListenerOptions,
      ) {
        const target =
          type === "pointermove" && fn
            ? function (this: EventTarget, ev: Event) {
                moveCalls++;
                (fn as EventListener).call(this, ev);
              }
            : fn;
        // 空监听按规范本就不注册，跳过
        if (target) originalAdd.call(window, type, target, opts);
      });
    try {
      const { unmount } = render(
        <TaskTimelinePage
          tasks={TASKS}
          onUpdate={vi.fn()}
          onSetColumn={vi.fn()}
        />,
      );
      const poolItem = within(screen.getByLabelText("任务池")).getByText(
        "未排期任务乙",
      );
      fireEvent.pointerDown(poolItem, { button: 0 });
      // 拖拽中：move 进入处理（拖拽幽灵渲染）
      fireEvent.pointerMove(window, { clientX: 1300, clientY: 400 });
      expect(moveCalls).toBe(1);
      expect(document.querySelector(".drag-ghost")).not.toBeNull();
      // 中途卸载 → 捕获的 onMove 不再被调用（监听已清理），派发也不抛错
      unmount();
      expect(() =>
        fireEvent.pointerMove(window, { clientX: 1300, clientY: 400 }),
      ).not.toThrow();
      expect(moveCalls).toBe(1);
    } finally {
      addSpy.mockRestore();
    }
  });
});
