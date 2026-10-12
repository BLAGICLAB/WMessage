import type { ColumnId, Task } from "../../types";
import {
  addDays,
  isSameDay,
  layoutLanes,
  planColorVar,
  segsForWeek,
} from "./week";
import { PlanBlockView } from "./PlanBlock";

const DOW = ["周一", "周二", "周三", "周四", "周五", "周六", "周日"];
const HOURS = Array.from({ length: 11 }, (_, i) => 8 + i); // 8..18 刻度
const GRID_COLS = "52px repeat(7, 1fr)";

/** 已排期任务集：软删/归档不上网格（回收站/归档页看）。
 *  已完成原位保留在原时间区间（降饱和 + 绿勾，无划线，点勾恢复待办）。
 *  plan 字段用真值判断——「拖回池/清除计划」写入的显式 null 不得进网格。 */
function plannedTasks(tasks: Task[]): Task[] {
  return tasks.filter(
    (t) => !!t.planStart && !!t.planEnd && !t.deletedAt && !t.archived,
  );
}

/** 拖拽反馈：吸附落点虚线框（网格坐标） */
export interface SlotHint {
  dayIdx: number;
  startMin: number;
  endMin: number;
}

/** 周时间网格渲染：列=周一..周日，行=8:00–18:00；跨日分段与重叠分栏在纯函数层。
 *  now 由页面注入（单一时间源）；拖拽编排也在页面——本组件只上报表块手势、
 *  渲染吸附反馈并交出网格 body 供指针反解。 */
export function WeekGrid({
  tasks,
  weekStart,
  now,
  slot = null,
  draggingTaskId = null,
  freshTaskId = null,
  bodyRef,
  onBlockPointerDown,
  onBlockDoubleClick,
  onSetColumn,
}: {
  tasks: Task[];
  weekStart: Date;
  now: Date;
  slot?: SlotHint | null;
  /** 拖拽中的原块降透明（占位由 ghost/slot 承担） */
  draggingTaskId?: string | null;
  /** 刚落位的块播弹入动画 */
  freshTaskId?: string | null;
  /** 页面用于指针反解的网格 body 挂载点 */
  bodyRef?: React.Ref<HTMLDivElement>;
  onBlockPointerDown?: (task: Task, kind: "move" | "resize", e: React.PointerEvent) => void;
  /** 双击块身开详情面板（单击/拖拽不再开，防误触） */
  onBlockDoubleClick?: (task: Task) => void;
  /** 块对勾的完成/恢复通道（缺省不渲染对勾） */
  onSetColumn?: (taskId: string, col: ColumnId) => void;
}) {
  const planned = plannedTasks(tasks);
  const nowMin = now.getHours() * 60 + now.getMinutes();
  const nowInWindow = nowMin >= 8 * 60 && nowMin <= 18 * 60;

  return (
    <div className="week-well flex min-h-0 flex-1 flex-col overflow-hidden rounded-2xl border border-[var(--edge)]">
      {/* 列头 */}
      <div
        className="relative z-1 grid px-2 pt-2.5 pb-2"
        style={{ gridTemplateColumns: GRID_COLS }}
      >
        <div />
        {DOW.map((w, i) => {
          const d = addDays(weekStart, i);
          const isToday = isSameDay(d, now);
          const weekend = i >= 5;
          return (
            <div key={w} className="text-center">
              <div
                className={`text-[11px] leading-4 ${
                  isToday ? "font-medium text-[var(--accent)]" : weekend ? "text-[var(--t5)] opacity-80" : "text-[var(--t5)]"
                }`}
              >
                {w}
              </div>
              <div
                className={`num mx-auto mt-0.5 inline-flex h-7 w-7 items-center justify-center rounded-full text-[19px] font-semibold leading-none ${
                  isToday
                    ? "bg-[var(--accent)] text-white dark:text-[#101228]"
                    : weekend
                      ? "text-[var(--t5)] opacity-80"
                      : "text-[var(--t3)]"
                }`}
              >
                {d.getDate()}
              </div>
            </div>
          );
        })}
      </div>

      {/* 网格主体 */}
      <div
        ref={bodyRef}
        className="relative z-1 grid min-h-0 flex-1 pr-2 pb-2"
        style={{ gridTemplateColumns: GRID_COLS }}
      >
        {/* 时刻轴 */}
        <div className="relative">
          {HOURS.map((h) => (
            <span
              key={h}
              className="num absolute right-2.5 -translate-y-1/2 rounded bg-[var(--bg)]/60 px-0.5 py-px text-[10.5px] text-[var(--t5)]"
              style={{ top: `${((h - 8) / 10) * 100}%` }}
            >
              {String(h).padStart(2, "0")}:00
            </span>
          ))}
        </div>
        {/* 虚线刻度层（垫底） */}
        <div className="pointer-events-none absolute inset-y-0 left-[52px] right-2 z-0">
          {HOURS.map((h) => (
            <div key={h} className="week-hline" style={{ top: `${((h - 8) / 10) * 100}%` }} />
          ))}
          {HOURS.slice(0, -1).map((h) => (
            <div
              key={`hh-${h}`}
              className="week-hline week-hline-hh"
              style={{ top: `${((h - 8) / 10 + 1 / 20) * 100}%` }}
            />
          ))}
        </div>
        {/* 7 日列 */}
        {DOW.map((_, i) => {
          const day = addDays(weekStart, i);
          const isToday = isSameDay(day, now);
          const weekend = i >= 5;
          const segs = layoutLanes(
            planned.flatMap((t) =>
              segsForWeek(t.planStart, t.planEnd, weekStart)
                .filter((s) => s.dayIdx === i)
                .map((s) => ({ ...s, task: t })),
            ),
          );
          // 重叠簇内分组：lanes ≥ 3 时整簇塌缩为头像 + 对勾（所有卡），保持各自分栏几何
          const clusters = new Map<number, typeof segs>();
          for (const s of segs) {
            const g = clusters.get(s.cluster);
            if (g) g.push(s);
            else clusters.set(s.cluster, [s]);
          }
          const blk = (s: (typeof segs)[number], collapsed = false) => (
            <PlanBlockView
              key={`${s.task.id}-${s.startMin}`}
              seg={s}
              task={s.task}
              color={planColorVar(s.task.tags)}
              dimmed={draggingTaskId === s.task.id}
              fresh={freshTaskId === s.task.id}
              done={s.task.column === "done"}
              collapsed={collapsed}
              onSetColumn={onSetColumn}
              onPointerDown={
                onBlockPointerDown
                  ? (e) => {
                      if (e.button !== 0) return;
                      const kind = (e.target as HTMLElement).closest("[data-grip]")
                        ? "resize"
                        : "move";
                      onBlockPointerDown(s.task, kind, e);
                    }
                  : undefined
              }
              onDoubleClick={
                onBlockDoubleClick ? () => onBlockDoubleClick(s.task) : undefined
              }
            />
          );
          return (
            <div
              key={day.toDateString()}
              data-day={i}
              className={`relative border-l border-[color-mix(in_srgb,var(--edge)_55%,transparent)] ${
                i === 0 ? "border-l-0" : ""
              }`}
            >
              {weekend && (
                <div className="absolute inset-0 bg-[var(--inset-bg)] opacity-50" aria-hidden />
              )}
              {isToday && (
                <div
                  className="absolute inset-0 bg-[color-mix(in_srgb,var(--accent)_6%,transparent)]"
                  aria-hidden
                />
              )}
              {slot && slot.dayIdx === i && (
                <div
                  className="plan-slot"
                  style={{
                    top: `${((slot.startMin - 480) / 600) * 100}%`,
                    height: `calc(${((slot.endMin - slot.startMin) / 600) * 100}% - 2px)`,
                  }}
                  aria-hidden
                />
              )}
              {[...clusters.values()].flatMap((group) =>
                // lanes ≥ 3：整簇塌缩（所有卡只显头像 + 对勾），不是只有第 3 张
                group.map((s) => blk(s, group[0].lanes >= 3)),
              )}
              {isToday && nowInWindow && (
                <div className="week-nowline" style={{ top: `${((nowMin - 480) / 600) * 100}%` }}>
                  <span className="week-nowlab num">现在 {String(now.getHours()).padStart(2, "0")}:{String(now.getMinutes()).padStart(2, "0")}</span>
                </div>
              )}
            </div>
          );
        })}
      </div>
    </div>
  );
}
