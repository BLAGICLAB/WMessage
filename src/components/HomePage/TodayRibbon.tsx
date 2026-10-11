// 今日时间带：周时间网格缩成一天的横带（8:00–24:00），块材质与任务页
// plan-blk 同配方（见 main.css .home-ribbon-blk）。现在线画在块下层——
// 任何数据下都不遮块内文字，「当前块」用 accent 描边互证。
// 排期写的是 8:00–18:00 网格，但展示带放宽到 24:00（晚间计划也进带）。

import { planColorVar } from "../TaskTimeline/week";
import type { Task } from "../../types";
import { todayPlanTasks } from "./homeData";

/** 展示带窗口：8:00–24:00（与任务页 8–18 编辑网格刻意不同，见文件头） */
const HOME_DAY_START = 8 * 60;
const HOME_DAY_END = 24 * 60;
const HOME_SPAN = HOME_DAY_END - HOME_DAY_START;
const DEFAULT_DURATION_MIN = 60;

/** "YYYY-MM-DDTHH:mm" → 当日分钟数；纯日期（无时间）排期无法上带，返回 null */
function planMinuteOf(v: string | null | undefined): number | null {
  if (!v || v.length !== 16) return null;
  const h = Number(v.slice(11, 13));
  const m = Number(v.slice(14, 16));
  if (Number.isNaN(h) || Number.isNaN(m)) return null;
  return h * 60 + m;
}

type RibbonBlock = {
  task: Task;
  start: number;
  end: number;
  /** 结束被钳到带右缘（物理溢出 24:00） */
  clipped: boolean;
  done: boolean;
  current: boolean;
};

function buildBlocks(tasks: Task[], now: Date): RibbonBlock[] {
  const nowMin = now.getHours() * 60 + now.getMinutes();
  const blocks: RibbonBlock[] = [];
  for (const task of todayPlanTasks(tasks, now)) {
    const start = planMinuteOf(task.planStart);
    if (start === null) continue;
    const end = Math.min(
      planMinuteOf(task.planEnd) ?? start + DEFAULT_DURATION_MIN,
      HOME_DAY_END,
    );
    if (end <= Math.max(start, HOME_DAY_START)) continue;
    const done = !!task.completedAt;
    blocks.push({
      task,
      start: Math.max(start, HOME_DAY_START),
      end: Math.max(end, HOME_DAY_START + 15),
      clipped: (planMinuteOf(task.planEnd) ?? start + DEFAULT_DURATION_MIN) > HOME_DAY_END,
      done,
      current: !done && start <= nowMin && nowMin < end,
    });
  }
  return blocks.sort((a, b) => a.start - b.start);
}

export function TodayRibbon({
  tasks,
  now,
  onOpenTask,
}: {
  tasks: Task[];
  now: Date;
  onOpenTask: (t: Task) => void;
}) {
  const blocks = buildBlocks(tasks, now);
  const nowMin = now.getHours() * 60 + now.getMinutes();
  const showNow = nowMin >= HOME_DAY_START && nowMin < HOME_DAY_END;
  const doneCount = blocks.filter((b) => b.done).length;

  return (
    <section
      aria-label="今日时间带"
      className="nm-card p-4"
      data-testid="today-ribbon"
    >
      <div className="mb-3 flex items-center gap-2">
        <h3 className="text-[13px] font-semibold text-[var(--t3)]">今日时间带</h3>
        <span className="text-xs text-[var(--t5)] num">
          {doneCount} 已完成 · {blocks.length - doneCount} 待开始
        </span>
      </div>

      {blocks.length === 0 ? (
        <p className="py-8 text-center text-[13px] text-[var(--t5)]">
          今天还没有排期——在任务页把未完成任务拖进网格，这里会出现它的位置。
        </p>
      ) : (
        <>
          <div className="relative h-[128px]" data-testid="ribbon-lane">
            <div aria-hidden className="absolute inset-0 flex">
              {Array.from({ length: 8 }, (_, i) => (
                <i key={i} className="flex-1 border-l border-[var(--edge)] opacity-55 last:border-r" />
              ))}
            </div>
            {showNow && (
              <div
                aria-hidden
                className="home-now-line"
                style={{ left: `${((nowMin - HOME_DAY_START) / HOME_SPAN) * 100}%` }}
              >
                <span className="home-now-chip num">
                  {String(now.getHours()).padStart(2, "0")}:
                  {String(now.getMinutes()).padStart(2, "0")}
                </span>
              </div>
            )}
            {blocks.map((b) => {
              const left = ((b.start - HOME_DAY_START) / HOME_SPAN) * 100;
              const widthPct = ((b.end - b.start) / HOME_SPAN) * 100;
              return (
                <button
                  key={b.task.id}
                  type="button"
                  className={`home-ribbon-blk${b.done ? " done" : ""}${b.current ? " cur" : ""}`}
                  style={
                    {
                      "--c": planColorVar(b.task.tags),
                      left: `${left}%`,
                      width: `calc(${widthPct}% - 4px)`,
                    } as React.CSSProperties
                  }
                  onClick={() => onOpenTask(b.task)}
                  title={b.task.title}
                >
                  <span className="home-ribbon-blk-title">{b.task.title}</span>
                  {/* 窄块放不下完整区间：只显示开始时刻（任务页多车道重叠同口径） */}
                  <span className="home-ribbon-blk-time num">
                    {fmtHM(b.start)}
                    {widthPct >= 8 ? `–${fmtHM(b.end)}` : ""}
                    {b.clipped ? " »" : ""}
                  </span>
                  {b.current && <span className="home-ribbon-blk-cur">进行中</span>}
                </button>
              );
            })}
          </div>
          <div aria-hidden className="mt-1.5 flex text-[11px] text-[var(--t5)] num">
            {Array.from({ length: 8 }, (_, i) => (
              <span key={i} className="flex-1">
                {fmtHM(HOME_DAY_START + i * 120)}
              </span>
            ))}
          </div>
        </>
      )}
    </section>
  );
}

function fmtHM(min: number): string {
  const h = Math.floor(min / 60);
  const m = min % 60;
  return `${String(h).padStart(2, "0")}:${String(m).padStart(2, "0")}`;
}
