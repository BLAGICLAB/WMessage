import { useEffect, useMemo, useRef, useState } from "react";
import type { ColumnId, Task } from "../../types";
import {
  DAY_END_MIN,
  DAY_SPAN_MIN,
  DAY_START_MIN,
  addDays,
  clampWindowMin,
  fmtMin,
  parsePlanDT,
  planColorVar,
  planPatchForDrop,
  snapMin,
  startOfWeek,
  windowMinutesBetween,
} from "./week";
import { WeekGrid, type SlotHint } from "./WeekGrid";
import { TaskPool, loadPoolOpen, savePoolOpen } from "./TaskPool";
import { TaskDetailPanel } from "./TaskDetailPanel";

const DOW1 = "一二三四五六日";

type Drag =
  | { kind: "create"; task: Task }
  | { kind: "move"; task: Task }
  | { kind: "resize"; task: Task };

/** 拖拽编排（批 3）+ 详情面板（批 4）：池→网格排期、块移动、底缘拉伸、拖回池清除；
 *  点块/点池项/⌘N 新建 → 左侧毛玻璃详情面板（编辑与完成入口）。
 *  写入恒发最小 patch（planStart/planEnd 或双 null），跨 18:00 折行数学在 week.ts。 */
export function TaskTimelinePage({
  tasks,
  onNewTask,
  onUpdate,
  onSetColumn,
  onDelete,
  editingId = null,
}: {
  tasks: Task[];
  onNewTask?: () => void;
  /** 任务字段定向补丁（task_patch 通道）；拖拽提交的最小写路径 */
  onUpdate?: (taskId: string, patch: Partial<Task>) => void;
  onSetColumn?: (taskId: string, col: ColumnId) => void;
  onDelete?: (taskId: string) => void;
  /** ⌘N/新建任务流程：App 侧 addTask 打上的编辑态 id，页面据此打开面板 */
  editingId?: string | null;
}) {
  const [weekStart, setWeekStart] = useState(() => startOfWeek(new Date()));
  // 「现在」单一时间源：页头今天回跳与网格现在线共用，每分钟校准
  const [now, setNow] = useState(() => new Date());
  useEffect(() => {
    const id = setInterval(() => setNow(new Date()), 60_000);
    return () => clearInterval(id);
  }, []);

  const [slot, setSlot] = useState<SlotHint | null>(null);
  const [ghost, setGhost] = useState<{
    x: number;
    y: number;
    title: string;
    color: string;
    label: string;
  } | null>(null);
  const [dragTaskId, setDragTaskId] = useState<string | null>(null);
  /** 池淡出只跟「从池拖出」（create）走——网格块拖回池时池是落点，必须可见 */
  const [dragFromPool, setDragFromPool] = useState(false);
  const [freshId, setFreshId] = useState<string | null>(null);
  const gridBodyRef = useRef<HTMLDivElement | null>(null);
  const poolRef = useRef<HTMLElement | null>(null);
  const freshTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  // 任务池开合：页头开关控制（折叠把手已移除——把手会挡住网格右下的任务块）
  const [poolOpen, setPoolOpen] = useState(loadPoolOpen);
  const togglePool = () =>
    setPoolOpen((v) => {
      savePoolOpen(!v);
      return !v;
    });

  // 详情面板选中态：autoFocus 只在「新建任务」路径给（点选不抢焦点）。
  // 不做「任务消失自动关面板」——addTask 的入列与 editingId 分两次更新，
  // 中间态会把新选中误杀；面板的关闭走显式路径（Esc/X/删除）。
  const [selected, setSelected] = useState<{ id: string; autoFocus: boolean } | null>(null);
  useEffect(() => {
    if (editingId) setSelected({ id: editingId, autoFocus: true });
  }, [editingId]);
  const selectedTask = selected ? (tasks.find((t) => t.id === selected.id) ?? null) : null;

  const weekEnd = addDays(weekStart, 6);
  const rangeLabel = useMemo(() => {
    const fmt = (d: Date) => `${d.getMonth() + 1}月${d.getDate()}日`;
    return `${fmt(weekStart)} – ${fmt(weekEnd)}`;
  }, [weekStart, weekEnd]);

  // 池口径：全部未完成（含已排期——已排期的带时间角标，双向拖拽）
  const poolTasks = useMemo(
    () => tasks.filter((t) => t.column !== "done" && !t.deletedAt && !t.archived),
    [tasks],
  );
  // 已完成（未归档未软删）：池底折叠区，点开可进面板恢复待办
  const doneTasks = useMemo(
    () => tasks.filter((t) => t.column === "done" && !t.deletedAt && !t.archived),
    [tasks],
  );
  const isCurrentWeek = useMemo(
    () => startOfWeek(now).getTime() === weekStart.getTime(),
    [now, weekStart],
  );

  const markFresh = (id: string) => {
    setFreshId(id);
    if (freshTimer.current) clearTimeout(freshTimer.current);
    freshTimer.current = setTimeout(() => setFreshId(null), 260);
  };

  /** 指针 → (dayIdx, 吸附分钟)；不在任何列内返 null */
  const locate = (x: number, y: number) => {
    const cols = gridBodyRef.current?.querySelectorAll<HTMLElement>("[data-day]");
    if (!cols) return null;
    for (const col of cols) {
      const r = col.getBoundingClientRect();
      if (x >= r.left && x <= r.right && y >= r.top - 24 && y <= r.bottom + 24) {
        const dayIdx = Number(col.dataset.day);
        const min = clampWindowMin(
          snapMin(DAY_START_MIN + ((y - r.top) / r.height) * DAY_SPAN_MIN),
        );
        return { dayIdx, min, day: addDays(weekStart, dayIdx) };
      }
    }
    return null;
  };

  const startDrag = (e: React.PointerEvent, drag: Drag) => {
    e.preventDefault();
    setDragTaskId(drag.task.id);
    setDragFromPool(drag.kind === "create");
    // 点 vs 拖区分：位移 ≤4px 视为点击（move/create → 选中开面板；resize 无点击语义）
    const startX = e.clientX;
    const startY = e.clientY;
    let moved = false;
    const durMin =
      drag.kind === "create"
        ? 60
        : drag.kind === "move" && drag.task.planStart && drag.task.planEnd
          ? (windowMinutesBetween(drag.task.planStart, drag.task.planEnd) ?? 60)
          : 60;
    const color = planColorVar(drag.task.tags);
    let overPool = false;

    const onMove = (ev: PointerEvent) => {
      if (!moved && Math.hypot(ev.clientX - startX, ev.clientY - startY) > 4) {
        moved = true;
      }
      const hit = locate(ev.clientX, ev.clientY);
      overPool =
        drag.kind === "move" &&
        (() => {
          const r = poolRef.current?.getBoundingClientRect();
          return !!r && ev.clientX >= r.left && ev.clientX <= r.right && ev.clientY >= r.top && ev.clientY <= r.bottom;
        })();
      if (!hit || overPool) {
        setSlot(null);
        setGhost(
          overPool
            ? { x: ev.clientX, y: ev.clientY, title: drag.task.title, color, label: "松手清除排期" }
            : null,
        );
        return;
      }
      if (drag.kind === "resize") {
        const patch = planPatchForDrop("resize", {
          day: hit.day,
          min: hit.min,
          prevPlanStart: drag.task.planStart,
        });
        const end = parsePlanDT(patch.planEnd);
        const start = parsePlanDT(patch.planStart)!;
        const sameDay = end!.day.getTime() === start.day.getTime();
        setSlot({
          dayIdx: hit.dayIdx,
          startMin: sameDay ? start.min : DAY_START_MIN,
          endMin: sameDay ? end!.min : DAY_END_MIN,
        });
        setGhost({
          x: ev.clientX,
          y: ev.clientY,
          title: drag.task.title,
          color,
          label: `至 ${patch.planEnd.slice(11)}`,
        });
      } else {
        const patch = planPatchForDrop(drag.kind === "create" ? "schedule" : "move", {
          day: hit.day,
          min: hit.min,
          durMin,
        });
        const start = parsePlanDT(patch.planStart)!;
        const end = parsePlanDT(patch.planEnd)!;
        const sameDay = end.day.getTime() === start.day.getTime();
        setSlot({
          dayIdx: hit.dayIdx,
          startMin: start.min,
          endMin: sameDay ? end.min : DAY_END_MIN,
        });
        const sd = DOW1[(start.day.getDay() + 6) % 7];
        const ed = DOW1[(end.day.getDay() + 6) % 7];
        setGhost({
          x: ev.clientX,
          y: ev.clientY,
          title: drag.task.title,
          color,
          label: sameDay
            ? `${sd} ${fmtMin(start.min)} – ${fmtMin(end.min)}`
            : `${sd} ${fmtMin(start.min)} – ${ed} ${fmtMin(end.min)}`,
        });
      }
    };

    const onUp = (ev: PointerEvent) => {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
      setDragTaskId(null);
      setDragFromPool(false);
      setSlot(null);
      setGhost(null);
      // 点击（未拖动）：选中任务打开详情面板（move/create 同义；resize 无点击语义）
      if (!moved) {
        if (drag.kind !== "resize") setSelected({ id: drag.task.id, autoFocus: false });
        return;
      }
      const hit = locate(ev.clientX, ev.clientY);
      const poolR = poolRef.current?.getBoundingClientRect();
      const droppedOnPool =
        drag.kind === "move" &&
        !!poolR &&
        ev.clientX >= poolR.left &&
        ev.clientX <= poolR.right &&
        ev.clientY >= poolR.top &&
        ev.clientY <= poolR.bottom;
      if (droppedOnPool) {
        onUpdate?.(drag.task.id, { planStart: null, planEnd: null });
        return;
      }
      if (!hit) return;
      if (drag.kind === "resize") {
        const patch = planPatchForDrop("resize", {
          day: hit.day,
          min: hit.min,
          prevPlanStart: drag.task.planStart,
        });
        onUpdate?.(drag.task.id, { planEnd: patch.planEnd });
      } else {
        const patch = planPatchForDrop(drag.kind === "create" ? "schedule" : "move", {
          day: hit.day,
          min: hit.min,
          durMin,
        });
        onUpdate?.(drag.task.id, patch);
        markFresh(drag.task.id);
      }
    };

    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp);
  };

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
        <button
          type="button"
          className={`ml-auto inline-flex items-center gap-2 rounded-full border px-3 py-1 text-xs font-medium ${
            poolOpen
              ? "border-[var(--accent)] text-[var(--accent)]"
              : "border-[var(--edge)] bg-[var(--surface)] text-[var(--t3)] hover:border-[var(--edge-strong)]"
          }`}
          onClick={togglePool}
          aria-pressed={poolOpen}
        >
          <span aria-hidden>▤</span> 任务池
          <span
            className={`num inline-flex h-4 min-w-4 items-center justify-center rounded-full px-1 text-[10px] font-semibold ${
              poolOpen
                ? "bg-[var(--accent)] text-white dark:text-[#101228]"
                : "bg-[var(--inset-bg)] text-[var(--t4)]"
            }`}
          >
            {poolTasks.length}
          </span>
        </button>
      </div>
      <div className="flex min-h-0 flex-1 gap-4">
        {selectedTask && onUpdate && onSetColumn && onDelete && (
          <TaskDetailPanel
            task={selectedTask}
            autoFocusTitle={selected?.autoFocus ?? false}
            onUpdate={(patch) => onUpdate(selectedTask.id, patch)}
            onSetColumn={(col) => onSetColumn(selectedTask.id, col)}
            onDelete={() => {
              setSelected(null);
              onDelete(selectedTask.id);
            }}
            onClose={() => setSelected(null)}
          />
        )}
        <div className="relative flex min-h-0 flex-1">
          <WeekGrid
            tasks={tasks}
            weekStart={weekStart}
            now={now}
            slot={slot}
            draggingTaskId={dragTaskId}
            freshTaskId={freshId}
            bodyRef={gridBodyRef}
            onBlockPointerDown={(task, kind, e) => {
              if (!onUpdate || !task.planStart || !task.planEnd) return;
              startDrag(e, { kind, task });
            }}
          />
          <TaskPool
            tasks={poolTasks}
            doneTasks={doneTasks}
            open={poolOpen}
            onToggle={togglePool}
            innerRef={poolRef}
            faded={dragFromPool}
            onItemPointerDown={(task, e) => {
              if (!onUpdate) return;
              startDrag(e, { kind: "create", task });
            }}
            onTaskClick={(t) => setSelected({ id: t.id, autoFocus: false })}
          />
        </div>
      </div>
      {ghost && (
        <div
          className="drag-ghost"
          style={
            {
              "--c": ghost.color,
              left: ghost.x + 14,
              top: ghost.y - 14,
            } as React.CSSProperties
          }
        >
          <div className="drag-ghost-title">{ghost.title}</div>
          <div className="drag-ghost-label num">{ghost.label}</div>
        </div>
      )}
    </div>
  );
}
