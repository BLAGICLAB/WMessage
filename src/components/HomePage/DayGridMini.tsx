// 日程迷你卡：映射任务页的周时间网格——同一 segsForWeek/layoutLanes 数据源，
// 7 列 × 8:00–18:00，块配色同任务页（planColorVar）。
// 联动：列头 hover/聚焦 = 联动到该天（周图橙色竖线同步落位）；被指向的列画
// 橙色竖向参考线，其余列降透明（Dim）；点块跳任务页编辑态。

import {
  segsForWeek,
  layoutLanes,
  DAY_START_MIN,
  DAY_SPAN_MIN,
  startOfWeek,
  fmtMin,
  planColorVar,
} from "../TaskTimeline/week";
import { plannedTasksByDay } from "./homeData";
import type { Task } from "../../types";

const HEAD_LABELS = ["周一", "周二", "周三", "周四", "周五", "周六", "周日"];

export function DayGridMini({
  tasks,
  now,
  activeDay,
  activeTaskId,
  onActiveDay,
  onHoverTask,
  onOpenTask,
  onMore,
}: {
  tasks: Task[];
  now: Date;
  activeDay: number | null;
  activeTaskId: string | null;
  onActiveDay: (day: number | null) => void;
  onHoverTask: (task: Task | null) => void;
  onOpenTask: (t: Task) => void;
  onMore: () => void;
}) {
  const weekStart = startOfWeek(now);
  const { keys } = plannedTasksByDay(tasks, now);

  const columns = keys.map((key, dayIdx) => {
    const raw: Array<{ task: Task; startMin: number; endMin: number }> = [];
    for (const t of tasks) {
      if (t.deletedAt) continue;
      for (const seg of segsForWeek(t.planStart, t.planEnd, weekStart)) {
        if (seg.dayIdx !== dayIdx) continue;
        raw.push({ task: t, startMin: seg.startMin, endMin: seg.endMin });
      }
    }
    const lanes = layoutLanes(raw);
    return { dayIdx, key, lanes };
  });

  return (
    <section
      className="nm-card flex h-full flex-col p-4"
      aria-label="本周日程"
      onMouseLeave={() => onActiveDay(null)}
      // 焦点真正离开整卡才清联动：列内 blur（如点翻页按钮）不清，避免与接管竞态
      onBlur={(e) => {
        if (!e.currentTarget.contains(e.relatedTarget as Node | null))
          onActiveDay(null);
      }}
    >
      <div className="flex items-center gap-2">
        <span
          aria-hidden
          className="h-[7px] w-[7px] shrink-0 rounded-full"
          style={{ background: "var(--plan-blue)" }}
        />
        <h3 className="text-[13px] font-semibold text-[var(--t3)]">本周日程</h3>
        <button
          onClick={onMore}
          className="ml-auto rounded-md px-2 py-0.5 text-xs text-[var(--t4)] transition-colors hover:bg-[var(--hover-bg)] hover:text-[var(--t2)]"
        >
          任务页
        </button>
      </div>

      <div className="mt-2 flex flex-1 flex-col">
        {/* 列头 = 联动交互面：hover/聚焦指向该天（键盘可达） */}
        <div className="grid grid-cols-7 gap-1">
          {columns.map((col) => (
            <button
              key={col.key}
              type="button"
              data-testid={`daygrid-head-${col.dayIdx}`}
              data-active={activeDay === col.dayIdx || undefined}
              onMouseEnter={() => onActiveDay(col.dayIdx)}
              onFocus={() => onActiveDay(col.dayIdx)}
              onClick={() => onActiveDay(col.dayIdx)}
              className={`rounded-md py-1 text-center text-[11px] transition-colors ${
                activeDay === col.dayIdx
                  ? "bg-[var(--inset-bg)] font-semibold text-[var(--t1)]"
                  : "text-[var(--t4)] hover:bg-[var(--hover-bg)]"
              }`}
            >
              {HEAD_LABELS[col.dayIdx]}
            </button>
          ))}
        </div>

        <div className="relative mt-1.5 min-h-[120px] flex-1">
          <div className="absolute inset-0 grid grid-cols-7 gap-1">
            {columns.map((col) => {
              const dimmed = activeDay !== null && activeDay !== col.dayIdx;
              return (
                <div
                  key={col.key}
                  className="relative border-l border-[var(--edge)] last:border-r"
                  style={{
                    opacity: dimmed ? 0.25 : 1,
                    transition: "opacity .15s ease",
                  }}
                >
                  {[2, 4, 6, 8].map((tick) => (
                    <i
                      key={tick}
                      aria-hidden
                      className="absolute inset-x-0 border-t border-dashed border-[var(--edge)]"
                      style={{ top: `${tick * 10}%` }}
                    />
                  ))}
                  {/* 橙色竖向参考线：落在联动指向的列（与周图共用一天信号） */}
                  {activeDay === col.dayIdx && (
                    <div
                      aria-hidden
                      className="absolute inset-y-0 left-1/2 w-0.5 -translate-x-1/2"
                      style={{ background: "var(--plan-amber)" }}
                    />
                  )}
                  {col.lanes.map((seg, si) => {
                    const top = ((seg.startMin - DAY_START_MIN) / DAY_SPAN_MIN) * 100;
                    const h = ((seg.endMin - seg.startMin) / DAY_SPAN_MIN) * 100;
                    const w = 100 / seg.lanes;
                    const blockActive = activeTaskId === seg.task.id;
                    return (
                      <button
                        key={`${seg.task.id}-${si}`}
                        type="button"
                        data-testid={`daygrid-block-${seg.task.id}`}
                        data-active={blockActive || undefined}
                        className="absolute rounded-md"
                        style={
                          {
                            "--c": planColorVar(seg.task.tags),
                            top: `${top}%`,
                            height: `calc(${h}% - 2px)`,
                            left: `calc(${seg.lane * w}% + 1px)`,
                            width: `calc(${w}% - 3px)`,
                            background:
                              "color-mix(in srgb, var(--c) 14%, var(--surface-raised))",
                            border:
                              "1px solid color-mix(in srgb, var(--c) 26%, transparent)",
                            // 联动指向该任务卡：accent 描边压过日常边框
                            outline: blockActive
                              ? "2px solid var(--accent)"
                              : undefined,
                            outlineOffset: blockActive ? "1px" : undefined,
                            zIndex: blockActive ? 2 : undefined,
                          } as React.CSSProperties
                        }
                        title={`${seg.task.title} ${fmtMin(seg.startMin)}–${fmtMin(seg.endMin)}`}
                        onMouseEnter={() => onHoverTask(seg.task)}
                        onMouseLeave={() => onHoverTask(null)}
                        onClick={() => onOpenTask(seg.task)}
                      />
                    );
                  })}
                </div>
              );
            })}
          </div>
        </div>
      </div>
    </section>
  );
}
