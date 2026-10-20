// 本周计划/完成柱状图：周一..周日两组柱（计划=灰蓝、完成=绿），
// 当天高亮为 accent（参考案例「Camera hours」的当前日高亮语言）。
// 卡头右侧图例 + 当天读数（今天计划 N / 完成 M）。

import { useEffect, useMemo, useRef, useState } from "react";
import {
  Bar,
  BarChart,
  CartesianGrid,
  Cell,
  ReferenceLine,
  Tooltip,
  XAxis,
  YAxis,
  type TooltipPayloadEntry,
} from "recharts";
import type { WeekBucket } from "./homeData";

const FALLBACK_WIDTH = 480;
/** 容器实测尺寸就绪前的兜底（首帧后 RO 立即覆盖） */
const FALLBACK_HEIGHT = 220;
/** 实测高度过小（极端布局）时的下限，保证柱图可读 */
const MIN_CHART_HEIGHT = 140;

/** 容器实测宽高（图表随左列「图+横条」定下的总高自适应，卡内不留空档） */
function useContainerSize(): [
  React.RefObject<HTMLDivElement | null>,
  number,
  number,
] {
  const ref = useRef<HTMLDivElement>(null);
  const [size, setSize] = useState({ w: FALLBACK_WIDTH, h: FALLBACK_HEIGHT });
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const apply = () => {
      const w = el.clientWidth;
      const h = el.clientHeight;
      if (w > 0 && h > 0) setSize({ w, h });
    };
    apply();
    if (typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(apply);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);
  return [ref, size.w, size.h];
}

function WeekTip({
  active,
  payload,
}: {
  active?: boolean;
  payload?: readonly TooltipPayloadEntry[];
}) {
  const p = payload?.[0];
  if (!active || !p) return null;
  const b = p.payload as WeekBucket;
  return (
    <div className="rounded-[var(--r-sm)] border border-[var(--edge)] bg-[var(--surface-raised)] px-2.5 py-1.5 text-xs shadow-[var(--shadow-lg)]">
      <span className="text-[var(--t3)]">{b.date}</span>
      <b className="num ml-2 text-[var(--t1)]">
        计划 {b.planned} · 完成 {b.done}
      </b>
    </div>
  );
}

const axisTick = { fontSize: 11, fill: "var(--t5)" };

/** 受控联动：activeDay 由首页统一下发（周图悬停 / 日程列头 / 任务卡翻页 / 图谱节点共用） */
export function WeeklyChart({
  data,
  activeDay,
  onActiveDay,
}: {
  data: WeekBucket[];
  activeDay: number | null;
  onActiveDay: (day: number | null) => void;
}) {
  const [wrapRef, width, measuredH] = useContainerSize();
  const chartH = Math.max(MIN_CHART_HEIGHT, measuredH);
  const today = useMemo(() => data.findIndex((b) => b.isToday), [data]);
  const cur = activeDay !== null ? data[activeDay] : data[today] ?? data[0];
  const cellOpacity = (i: number) =>
    activeDay === null || activeDay === i ? 1 : 0.25;
  const refLine =
    activeDay !== null ? (
      // 橙色竖向参考线：跨小组件按天对齐的共用信号（用户指定的警示橙）
      <ReferenceLine
        x={data[activeDay].label}
        stroke="var(--plan-amber)"
        strokeWidth={2}
      />
    ) : null;

  return (
    <section
      className="nm-card flex h-full flex-col p-4"
      aria-label="本周计划与完成"
    >
      <div className="flex items-baseline justify-between">
        <div>
          <h3 className="text-[13px] font-semibold text-[var(--t3)]">
            本周计划与完成
          </h3>
          <p className="mt-0.5 text-xs text-[var(--t5)]">
            每日排期任务数与当日完成任务数
          </p>
        </div>
        <div className="text-right">
          <div className="num text-[22px] font-semibold leading-none text-[var(--t1)]">
            {cur.planned}
            <span className="mx-1 text-[var(--t5)]">/</span>
            {cur.done}
          </div>
          <div className="mt-1 flex items-center justify-end gap-3 text-xs text-[var(--t4)]">
            <span>{cur.isToday ? "今天" : cur.date}</span>
            <span className="flex items-center gap-1">
              <span
                aria-hidden
                className="inline-block h-1.5 w-1.5 rounded-full"
                style={{ background: "var(--plan-slate)" }}
              />
              计划
            </span>
            <span className="flex items-center gap-1">
              <span
                aria-hidden
                className="inline-block h-1.5 w-1.5 rounded-full"
                style={{ background: "var(--plan-green)" }}
              />
              完成
            </span>
          </div>
        </div>
      </div>
      {/* 移动端定高可用；桌面端 flex-1 吃满卡内余量（卡高由左列图+横条决定）。
          图表绝对定位铺满——不向网格贡献固有高度，行轨道才不会被 svg 撑高 */}
      <div ref={wrapRef} className="relative mt-3 h-[220px] flex-1 lg:h-auto lg:min-h-0">
        <div className="absolute inset-0">
          <BarChart
            data={data}
            width={width}
            height={chartH}
          margin={{ top: 6, right: 4, left: 0, bottom: 0 }}
          onMouseMove={(s) => {
            const v = s.activeTooltipIndex;
            onActiveDay(typeof v === "number" ? v : null);
          }}
          onMouseLeave={() => onActiveDay(null)}
        >
          <CartesianGrid vertical={false} stroke="var(--edge)" strokeDasharray="3 3" />
          <XAxis dataKey="label" tickLine={false} axisLine={false} tick={axisTick} interval={0} />
          <YAxis width={26} tickLine={false} axisLine={false} tick={axisTick} allowDecimals={false} />
          {/* animationEasing 的类型只收具名曲线与 cubic-bezier(...)，不收 CSS 变量；
              而 recharts 只是把它原样拼进 transition 简写（不参与缓动函数求值），
              所以这里写死曲线值安全。曲线与 --ease-out token 同源由测试锁住。 */}
          <Tooltip
            cursor={false}
            content={(p) => <WeekTip {...p} />}
            animationDuration={150}
            animationEasing="cubic-bezier(0.23,1,0.32,1)"
          />
          {refLine}
          <Bar dataKey="planned" radius={[4, 4, 0, 0]} barSize={11} isAnimationActive={false}>
            {data.map((d, i) => (
              <Cell
                key={d.key}
                data-testid={`week-plan-${i}`}
                fill={d.isToday ? "var(--accent)" : "var(--plan-slate)"}
                style={{ opacity: cellOpacity(i), transition: "opacity var(--dur-card) var(--ease-out)" }}
              />
            ))}
          </Bar>
          <Bar dataKey="done" radius={[4, 4, 0, 0]} barSize={11} isAnimationActive={false}>
            {data.map((d, i) => (
              <Cell
                key={d.key}
                data-testid={`week-done-${i}`}
                fill="var(--plan-green)"
                style={{ opacity: cellOpacity(i), transition: "opacity var(--dur-card) var(--ease-out)" }}
              />
            ))}
          </Bar>
        </BarChart>
        </div>
      </div>
    </section>
  );
}
