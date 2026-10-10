import { useEffect, useMemo, useState } from "react";
import type { Task } from "../../types";
import { addDays, startOfWeek } from "./week";
import { WeekGrid } from "./WeekGrid";
import { TaskPool } from "./TaskPool";

/** 任务页：周时间网格 + 右侧毛玻璃任务池。
 *  批 2 为只读渲染（拖拽排期在批 3，详情面板在批 4）。 */
export function TaskTimelinePage({
  tasks,
  onNewTask,
}: {
  tasks: Task[];
  onNewTask?: () => void;
}) {
  const [weekStart, setWeekStart] = useState(() => startOfWeek(new Date()));
  // 「现在」单一时间源：页头今天回跳与网格现在线共用，每分钟校准
  const [now, setNow] = useState(() => new Date());
  useEffect(() => {
    const id = setInterval(() => setNow(new Date()), 60_000);
    return () => clearInterval(id);
  }, []);
  const weekEnd = addDays(weekStart, 6);
  const rangeLabel = useMemo(() => {
    const fmt = (d: Date) => `${d.getMonth() + 1}月${d.getDate()}日`;
    return `${fmt(weekStart)} – ${fmt(weekEnd)}`;
  }, [weekStart, weekEnd]);

  // 池口径：全部未完成（含已排期——已排期的带时间角标，双向拖拽在批 3 打通）
  const poolTasks = useMemo(
    () => tasks.filter((t) => t.column !== "done" && !t.deletedAt && !t.archived),
    [tasks],
  );
  const isCurrentWeek = useMemo(
    () => startOfWeek(now).getTime() === weekStart.getTime(),
    [now, weekStart],
  );

  return (
    <div className="flex h-full min-h-0 flex-col" data-view="task-timeline">
      <div className="mb-4 flex items-center gap-3.5">
        <button type="button" className="btn-primary" onClick={onNewTask}>
          ＋ 新建任务
        </button>
        <div className="flex items-center gap-1">
          <button
            type="button"
            aria-label="上一周"
            className="nm-icon-btn h-7 w-7"
            onClick={() => setWeekStart((w) => addDays(w, -7))}
          >
            ‹
          </button>
          <span className="num mx-1.5 text-[15px] font-semibold text-[var(--t1)]">
            {rangeLabel}
          </span>
          <button
            type="button"
            aria-label="下一周"
            className="nm-icon-btn h-7 w-7"
            onClick={() => setWeekStart((w) => addDays(w, 7))}
          >
            ›
          </button>
        </div>
        {!isCurrentWeek && (
          <button
            type="button"
            className="rounded-full border border-[var(--edge)] bg-[var(--surface)] px-3 py-1 text-xs font-medium text-[var(--t3)] hover:border-[var(--edge-strong)]"
            onClick={() => setWeekStart(startOfWeek(new Date()))}
          >
            今天
          </button>
        )}
      </div>
      <div className="relative flex min-h-0 flex-1">
        <WeekGrid tasks={tasks} weekStart={weekStart} now={now} />
        <TaskPool tasks={poolTasks} />
      </div>
    </div>
  );
}
