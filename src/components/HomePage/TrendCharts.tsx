// 近 7 天联动图表（Linked Hover）：悬停任一图表的数据点或底部日期签，
// 所有图表同步高亮同一天、其余数据降透明（Dim），并共用一条竖向参考线，
// 实现跨图表按天对齐。联动走共享 activeIdx state 而非 recharts syncId——
// 高亮/降透明/参考线都要从同一状态推导，且日期签 hover 与键盘聚焦也进同一交互面。
// 卡头大数字 = 当前指向天的值（Switchyard「Camera hours」语言），悬停前显示今天。

import { useEffect, useRef, useState } from "react";
import {
  Area,
  AreaChart,
  Bar,
  BarChart,
  CartesianGrid,
  Cell,
  ReferenceLine,
  Tooltip,
  XAxis,
  YAxis,
  type DotItemDotProps,
  type MouseHandlerDataParam,
  type TooltipPayloadEntry,
} from "recharts";
import type { DayBucket } from "./homeData";

/** jsdom 无 ResizeObserver 时按此宽渲染（生产由 useContainerWidth 实测覆盖） */
const FALLBACK_WIDTH = 560;
const CHART_HEIGHT = 150;

/** 容器实测宽度（UsageStatsCard 同款配方；jsdom 降级固定宽） */
function useContainerWidth(): [React.RefObject<HTMLDivElement | null>, number] {
  const ref = useRef<HTMLDivElement>(null);
  const [w, setW] = useState(FALLBACK_WIDTH);
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const apply = () => {
      const cw = el.clientWidth;
      if (cw > 0) setW(cw);
    };
    apply();
    if (typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(apply);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);
  return [ref, w];
}

function idxOfNext(s: MouseHandlerDataParam): number | null {
  const v = s.activeTooltipIndex;
  if (typeof v === "number") return v;
  if (typeof v === "string" && v !== "" && !Number.isNaN(Number(v)))
    return Number(v);
  return null;
}

/** 面卡骨架：标题 + 大数字读数（随联动天变化）+ 图表 */
function TrendCard({
  title,
  unit,
  value,
  sub,
  children,
}: {
  title: string;
  unit: string;
  value: number;
  sub: string;
  children: React.ReactNode;
}) {
  return (
    <div className="nm-card p-4">
      <div className="flex items-baseline justify-between">
        <h3 className="text-[13px] font-semibold text-[var(--t3)]">{title}</h3>
        <div className="text-right">
          <div className="num text-[22px] font-semibold leading-none text-[var(--t1)]">
            {value}
            <span className="ml-1 text-xs font-normal text-[var(--t4)]">
              {unit}
            </span>
          </div>
          <div className="mt-1 text-xs text-[var(--t5)]">{sub}</div>
        </div>
      </div>
      <div className="mt-3">{children}</div>
    </div>
  );
}

function TrendTip({
  unit,
  active,
  payload,
}: {
  unit: string;
  active?: boolean;
  payload?: readonly TooltipPayloadEntry[];
}) {
  const p = payload?.[0];
  if (!active || !p) return null;
  const bucket = p.payload as DayBucket;
  return (
    <div className="rounded-[var(--r-sm)] border border-[var(--edge)] bg-[var(--surface-raised)] px-2.5 py-1.5 text-xs shadow-[var(--shadow-lg)]">
      <span className="text-[var(--t3)]">{bucket.date}</span>
      <b className="num ml-2 text-[var(--t1)]">
        {String(p.value)} {unit}
      </b>
    </div>
  );
}

export function TrendCharts({ data }: { data: DayBucket[] }) {
  const [active, setActive] = useState<number | null>(null);
  const [wrapRef, width] = useContainerWidth();
  const latest = data.length - 1;
  const cur = active !== null ? data[active] : data[latest];
  const totalDone = data.reduce((s, d) => s + d.completed, 0);
  const totalRuns = data.reduce((s, d) => s + d.botRuns, 0);

  const handleMove = (s: MouseHandlerDataParam) =>
    setActive(idxOfNext(s));
  const clear = () => setActive(null);
  const refLine =
    active !== null ? (
      // 参考线用 accent（选中语义）：红色保留给现在线/逾期，全页唯一活体信号不稀释
      <ReferenceLine x={data[active].label} stroke="var(--accent)" strokeWidth={1.5} />
    ) : null;

  const axisTick = { fontSize: 11, fill: "var(--t5)" };

  return (
    <section aria-label="近 7 天联动图表" className="grid grid-cols-1 gap-3.5 xl:grid-cols-2">
      <TrendCard
        title="每日完成"
        unit="项"
        value={cur.completed}
        sub={active !== null ? cur.date : `本周合计 ${totalDone} 项`}
      >
        <div ref={wrapRef}>
          <BarChart
            data={data}
            width={width}
            height={CHART_HEIGHT}
            margin={{ top: 6, right: 4, left: 0, bottom: 0 }}
            onMouseMove={handleMove}
            onMouseLeave={clear}
          >
            <CartesianGrid vertical={false} stroke="var(--edge)" strokeDasharray="3 3" />
            <XAxis
              dataKey="label"
              tickLine={false}
              axisLine={false}
              tick={axisTick}
              interval={0}
            />
            <YAxis
              width={26}
              tickLine={false}
              axisLine={false}
              tick={axisTick}
              allowDecimals={false}
            />
            <Tooltip cursor={false} content={(p) => <TrendTip {...p} unit="项" />} />
            {refLine}
            <Bar
              dataKey="completed"
              fill="var(--plan-blue)"
              radius={[5, 5, 0, 0]}
              barSize={20}
              isAnimationActive={false}
            >
              {data.map((d, i) => (
                <Cell
                  key={d.key}
                  data-testid={`trend-bar-${i}`}
                  data-active={active === i || undefined}
                  style={{
                    opacity: active === null || active === i ? 1 : 0.22,
                    transition: "opacity .15s ease",
                  }}
                />
              ))}
            </Bar>
          </BarChart>
        </div>
      </TrendCard>

      <TrendCard
        title="每日机器人执行"
        unit="次"
        value={cur.botRuns}
        sub={active !== null ? cur.date : `本周合计 ${totalRuns} 次`}
      >
        <div>
          <AreaChart
            data={data}
            width={width}
            height={CHART_HEIGHT}
            margin={{ top: 6, right: 14, left: 0, bottom: 0 }}
            onMouseMove={handleMove}
            onMouseLeave={clear}
          >
            <defs>
              <linearGradient id="home-bot-fill" x1="0" y1="0" x2="0" y2="1">
                <stop offset="0%" stopColor="var(--plan-green)" stopOpacity={0.26} />
                <stop offset="100%" stopColor="var(--plan-green)" stopOpacity={0.02} />
              </linearGradient>
            </defs>
            <CartesianGrid vertical={false} stroke="var(--edge)" strokeDasharray="3 3" />
            <XAxis
              dataKey="label"
              tickLine={false}
              axisLine={false}
              tick={axisTick}
              interval={0}
            />
            <YAxis
              width={26}
              tickLine={false}
              axisLine={false}
              tick={axisTick}
              allowDecimals={false}
            />
            <Tooltip cursor={false} content={(p) => <TrendTip {...p} unit="次" />} />
            {refLine}
            <Area
              type="monotone"
              dataKey="botRuns"
              stroke="var(--plan-green)"
              strokeWidth={2}
              fill="url(#home-bot-fill)"
              strokeOpacity={active !== null ? 0.35 : 1}
              fillOpacity={active !== null ? 0.4 : 1}
              isAnimationActive={false}
              activeDot={false}
              dot={(p: DotItemDotProps) => {
                const isActive = active === p.index;
                return (
                  <circle
                    key={p.index}
                    cx={p.cx}
                    cy={p.cy}
                    r={isActive ? 4.5 : 2.5}
                    fill="var(--plan-green)"
                    stroke="var(--surface)"
                    strokeWidth={isActive ? 2 : 0}
                    opacity={active === null || isActive ? 1 : 0.3}
                    data-testid={`trend-dot-${p.index}`}
                    data-active={isActive || undefined}
                    style={{ transition: "opacity .15s ease" }}
                  />
                );
              }}
            />
          </AreaChart>
        </div>
      </TrendCard>

      {/* 日期签：图表 hover 的等价交互面（可点、可聚焦），焦点=高亮同一天 */}
      <div
        className="xl:col-span-2 flex justify-center gap-1.5"
        role="group"
        aria-label="按天联动选择"
        onMouseLeave={clear}
      >
        {data.map((d, i) => (
          <button
            key={d.key}
            type="button"
            data-testid={`trend-chip-${i}`}
            data-active={active === i || undefined}
            onMouseEnter={() => setActive(i)}
            onFocus={() => setActive(i)}
            onBlur={clear}
            onClick={() => setActive(i)}
            className={`rounded-md px-2.5 py-1 text-xs transition-colors ${
              active === i
                ? "bg-[var(--inset-bg)] font-semibold text-[var(--t1)]"
                : "text-[var(--t4)] hover:bg-[var(--hover-bg)]"
            }`}
          >
            {d.label}
          </button>
        ))}
      </div>
    </section>
  );
}
