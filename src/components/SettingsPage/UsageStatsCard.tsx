// 词元统计卡（设置页「tokens」分区）——按「使用统计」参照图重排：
// 汇总指标行（竖线分隔 5 项，含连续天数）→ Token 活动热力图（GitHub contributions 式
// 7×N 周格，每日/每周/累计切换 + 月份标签）→ 时间范围行 + 每日趋势按模型分线 →
// 模型用量 3D 饼图，各板块独立成卡。
// 数据源 = usage_stats_daily(整个可查窗口) + usage_stats_by_model(整个可查窗口)
// （Rust 侧升序、缺日补零）+ usage_stats_daily_by_model(range)（稀疏返回，
// 前端以补零日序列为 x 轴）。窗口宽度由后端给，前端不写死天数。
// 不引图表库：曲线是手绘 SVG（Catmull-Rom 平滑），热力图是 div 网格，饼图是手绘
// 椭圆扇形 + 侧壁——零依赖纪律。

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { RefreshCw } from "lucide-react";

export type UsageDay = {
  day: string;
  promptTokens: number;
  completionTokens: number;
  /** 缓存档（Anthropic 四档计价之二）。不计入 dayTotal——
   *  缓存单价与普通 input 不同，混算会让「用量」面板的总 token 数失真 */
  cacheReadTokens: number;
  cacheWriteTokens: number;
  runs: number;
  toolCalls: number;
};

export type UsageModelRow = {
  /** 旧行（model 列上线前）/未发请求 = null，展示为「未知模型」 */
  model: string | null;
  promptTokens: number;
  completionTokens: number;
  cacheReadTokens: number;
  cacheWriteTokens: number;
  runs: number;
};

export type UsageDayModelRow = {
  day: string;
  model: string | null;
  promptTokens: number;
  completionTokens: number;
  cacheReadTokens: number;
  cacheWriteTokens: number;
};

/** 中文数量级格式化：≥1亿 → x.x亿；≥1万 → x.x万；≥1000 → x.xK；原值 */
function fmtTokens(n: number): string {
  if (n >= 100_000_000) return `${(n / 100_000_000).toFixed(1)} 亿`;
  if (n >= 10_000) return `${(n / 10_000).toFixed(1)} 万`;
  if (n >= 1000) return `${(n / 1000).toFixed(1)}K`;
  return String(Math.round(n));
}

const dayTotal = (d: UsageDay) => d.promptTokens + d.completionTokens;

/** 缓存档合计（读+写）。全程不并入展示口径的 total：缓存读取单价约为普通输入的
 *  十分之一，混算会让「累计 Token 数」既不反映真实规模也不反映真实开销。
 *  数值缺省（老后端/旧行）按 0 处理。 */
const cacheReadOf = (d: { cacheReadTokens?: number }) => d.cacheReadTokens ?? 0;
const cacheWriteOf = (d: { cacheWriteTokens?: number }) => d.cacheWriteTokens ?? 0;
const cacheOf = (d: { cacheReadTokens?: number; cacheWriteTokens?: number }) =>
  cacheReadOf(d) + cacheWriteOf(d);

/** 连续活跃天数（活跃 = 当日 tokens > 0）：
 *  当前 = 从今天（今天不活跃则从昨天）往前连续活跃日数；最长 = 窗口内最长活跃游程 */
export function streakDays(days: UsageDay[]): [number, number] {
  const active = days.map((d) => dayTotal(d) > 0);
  let cur = 0;
  let i = active.length - 1;
  if (i >= 0 && !active[i]) i -= 1;
  for (; i >= 0 && active[i]; i -= 1) cur += 1;
  let best = 0;
  let run = 0;
  for (const a of active) {
    run = a ? run + 1 : 0;
    if (run > best) best = run;
  }
  return [cur, best];
}

/* ───────────────────────── Token 活动热力图 ───────────────────────── */

function heatColor(level: number): string {
  // 0 档 = 空底；1-4 档品牌色按 25/50/75/100% 混合（低饱和主题内的单色阶，
  // 深浅色模式随 --brand/--surface 自动切换）
  if (level <= 0) return "var(--inset-bg)";
  return `color-mix(in srgb, var(--brand) ${level * 25}%, var(--surface))`;
}

export type HeatMode = "daily" | "weekly" | "cumulative";

/** 各档取值：每日 = 当日 tokens；每周 = 同周格按该周总量；累计 = 截至当日累计总量 */
function heatValues(
  days: UsageDay[],
  mode: HeatMode,
  numWeeks: number,
  weekOf: (i: number) => number,
): number[] {
  if (mode === "daily") return days.map(dayTotal);
  if (mode === "weekly") {
    const wk = new Array<number>(numWeeks).fill(0);
    days.forEach((d, i) => {
      wk[weekOf(i)] += dayTotal(d);
    });
    return days.map((_, i) => wk[weekOf(i)]);
  }
  let acc = 0;
  return days.map((d) => (acc += dayTotal(d)));
}

/** GitHub contributions 式年宽热力图：列 = 周（首列对齐周日），每列 7 行圆角方格，
 *  底部月份标签按周列定位。悬浮说明用自绘浮层而非原生 title：
 *  原生 title 不能分行、样式不可控，且会溢出窗口边界（热力图贴右边缘时
 *  提示框被裁掉一半）。 */
function TokenHeatmap({ days, mode }: { days: UsageDay[]; mode: HeatMode }) {
  const firstDowRaw = days.length
    ? new Date(`${days[0].day}T00:00:00`).getDay()
    : 0;
  const firstDow = Number.isNaN(firstDowRaw) ? 0 : firstDowRaw;
  const numWeeks = Math.max(1, Math.ceil((firstDow + days.length) / 7));
  const weekOf = (i: number) => Math.floor((firstDow + i) / 7);
  const values = heatValues(days, mode, numWeeks, weekOf);
  const max = Math.max(1, ...values);
  const levelOf = (v: number) =>
    v <= 0 ? 0 : Math.min(4, Math.ceil((v / max) * 4));

  /** 悬浮中的格子下标；-1 = 无。浮层内容由 hovered 派生，不另存副本 */
  const [hover, setHover] = useState(-1);
  /** 浮层锚点（相对外层容器）：{left, top} + 是否翻到格子左侧 */
  const [tip, setTip] = useState<{ left: number; top: number; flip: boolean }>({
    left: 0,
    top: 0,
    flip: false,
  });
  const wrapRef = useRef<HTMLDivElement>(null);

  const showTip = (i: number, el: HTMLElement) => {
    const wrap = wrapRef.current;
    if (!wrap) return;
    const wr = wrap.getBoundingClientRect();
    const cr = el.getBoundingClientRect();
    // 格子宽 12px、浮层约 210px：靠右时向左翻，避免越过容器右缘被裁
    const flip = cr.right - wr.left > wr.width - 220;
    setTip({
      left: flip ? cr.left - wr.left - 6 : cr.right - wr.left + 6,
      top: cr.top - wr.top + 14,
      flip,
    });
    setHover(i);
  };

  // 月份标签：月首所在周列，与上一标签距离 < 3 列时跳过（避免挤压重叠）
  const labels: Array<{ col: number; text: string }> = [];
  let lastMonth = -1;
  let lastCol = -99;
  days.forEach((d, i) => {
    const m = parseInt(d.day.slice(5, 7), 10);
    const col = weekOf(i);
    if (m !== lastMonth) {
      if (col - lastCol >= 3) labels.push({ col, text: `${m}月` });
      lastMonth = m;
      lastCol = col;
    }
  });

  const hd = hover >= 0 ? days[hover] : null;

  return (
    <div className="relative" ref={wrapRef} onMouseLeave={() => setHover(-1)}>
      <div
        className="grid"
        style={{
          gridTemplateColumns: `repeat(${numWeeks}, 1fr)`,
          gridTemplateRows: "repeat(7, 12px)",
          gridAutoFlow: "column",
          gap: "3px",
        }}
      >
        {Array.from({ length: firstDow }, (_, i) => (
          <div key={`blank-${i}`} />
        ))}
        {days.map((d, i) => {
          return (
            <div
              key={d.day}
              data-day={d.day}
              className="rounded-[3px]"
              style={{ backgroundColor: heatColor(levelOf(values[i])) }}
              onMouseEnter={(e) => showTip(i, e.currentTarget)}
            />
          );
        })}
      </div>
      <div className="relative mt-2 h-4 text-xs text-[var(--t5)]">
        {labels.map((l) => (
          <span
            key={`${l.col}-${l.text}`}
            className="absolute -translate-x-1/2"
            style={{ left: `${((l.col + 0.5) / numWeeks) * 100}%` }}
          >
            {l.text}
          </span>
        ))}
      </div>

      {/* 悬浮说明：六行分层。分组口径——未命中（色阶所用那组）与缓存分开列，
          读缓存单价约为普通输入的十分之一，合成一个「总 token」会两头失真。 */}
      {hd && (
        <div
          role="tooltip"
          className="pointer-events-none absolute z-10 w-[210px] rounded-[var(--r-md)] border border-[var(--edge-strong)] bg-[var(--surface-raised)] px-3 py-2 shadow-[var(--shadow-lg)]"
          style={{
            left: tip.left,
            top: tip.top,
            transform: tip.flip ? "translateX(-100%)" : undefined,
          }}
        >
          <p className="text-xs font-medium text-[var(--t1)]">{hd.day}</p>
          <div className="mt-1.5 space-y-1.5">
            {/* 未命中：热力图色阶就是按这一组取值的 */}
            <div>
              <div className="flex items-baseline justify-between gap-2">
                <span className="text-xs text-[var(--t5)]">缓存未命中</span>
                <span className="font-mono text-xs tabular-nums text-[var(--t2)]">
                  {fmtTokens(dayTotal(hd))}
                </span>
              </div>
              <p className="mt-0.5 text-xs text-[var(--t6)]">
                输入 {fmtTokens(hd.promptTokens)} · 输出{" "}
                {fmtTokens(hd.completionTokens)}
              </p>
            </div>
            {/* 缓存：读 + 写两档分开，OpenAI 协议与未开缓存的日子整组为 0 */}
            <div>
              <div className="flex items-baseline justify-between gap-2">
                <span className="text-xs text-[var(--t5)]">缓存命中</span>
                <span className="font-mono text-xs tabular-nums text-[var(--t2)]">
                  {fmtTokens(cacheOf(hd))}
                </span>
              </div>
              <p className="mt-0.5 text-xs text-[var(--t6)]">
                命中 {fmtTokens(cacheReadOf(hd))} · 写入{" "}
                {fmtTokens(cacheWriteOf(hd))}
              </p>
            </div>
            <div className="flex items-baseline justify-between gap-2 border-t border-[var(--edge)] pt-1.5">
              <span className="text-xs text-[var(--t5)]">轮次</span>
              <span className="font-mono text-xs tabular-nums text-[var(--t2)]">
                {hd.runs}
              </span>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}

/* ───────────────────────── 每日趋势平滑曲线（按模型） ───────────────────────── */

/** Catmull-Rom → 三次贝塞尔（相邻控制点取 1/6 差分），点少时退化为折线段。
 *  控制点 y 钳制在绘图区内——尖锐谷底（近零日）差分过冲会把曲线顶出 0 线。 */
function smoothPath(pts: Array<[number, number]>): string {
  if (pts.length === 0) return "";
  if (pts.length === 1)
    return `M ${pts[0][0].toFixed(1)} ${pts[0][1].toFixed(1)}`;
  const yMin = PAD_T;
  const yMax = CHART_H - PAD_B;
  const clampY = (v: number) => Math.min(yMax, Math.max(yMin, v));
  let d = `M ${pts[0][0].toFixed(1)} ${pts[0][1].toFixed(1)}`;
  for (let i = 0; i < pts.length - 1; i++) {
    const p0 = pts[i - 1] ?? pts[i];
    const p1 = pts[i];
    const p2 = pts[i + 1];
    const p3 = pts[i + 2] ?? p2;
    const c1x = p1[0] + (p2[0] - p0[0]) / 6;
    const c1y = clampY(p1[1] + (p2[1] - p0[1]) / 6);
    const c2x = p2[0] - (p3[0] - p1[0]) / 6;
    const c2y = clampY(p2[1] - (p3[1] - p1[1]) / 6);
    d += ` C ${c1x.toFixed(1)} ${c1y.toFixed(1)}, ${c2x.toFixed(1)} ${c2y.toFixed(1)}, ${p2[0].toFixed(1)} ${p2[1].toFixed(1)}`;
  }
  return d;
}

const CHART_H = 200;
const PAD_L = 16;
const PAD_R = 16;
const PAD_T = 12;
const PAD_B = 24;

const MODEL_COLORS = [
  "#3563b0",
  "#35854a",
  "#b05f2f",
  "#6d5aa8",
  "#b04a6a",
  "#4a5568",
];
const TREND_TOP_N = 5;

export type TrendSeries = {
  key: string;
  name: string;
  color: string;
  values: number[];
};

/** 稀疏 (day, model) 行 → 按模型分线：以补零日序列为 x 轴（缺日记 0），
 *  按窗口内 tokens 降序取前 TREND_TOP_N 条（NULL 模型 = 「未知模型」参与排序） */
export function buildTrendSeries(
  days: UsageDay[],
  rows: UsageDayModelRow[],
  topN = TREND_TOP_N,
): TrendSeries[] {
  const NULL_KEY = "__null__";
  const dayIdx = new Map(days.map((d, i) => [d.day, i]));
  const valuesByKey = new Map<string, number[]>();
  const sumByKey = new Map<string, number>();
  for (const r of rows) {
    const key = r.model ?? NULL_KEY;
    if (!valuesByKey.has(key)) {
      valuesByKey.set(key, new Array<number>(days.length).fill(0));
      sumByKey.set(key, 0);
    }
    const i = dayIdx.get(r.day);
    const t = r.promptTokens + r.completionTokens;
    if (i !== undefined) valuesByKey.get(key)![i] += t;
    sumByKey.set(key, sumByKey.get(key)! + t);
  }
  return [...sumByKey.entries()]
    .sort((a, b) => b[1] - a[1])
    .slice(0, topN)
    .map(([key], si) => ({
      key,
      name: key === NULL_KEY ? "未知模型" : key,
      color: MODEL_COLORS[si % MODEL_COLORS.length],
      values: valuesByKey.get(key)!,
    }));
}

/** 每模型一条平滑曲线：容器实测宽度渲染（ResizeObserver），jsdom 降级固定宽。
 *  无 y 轴数字（仅虚线网格）；曲线无可见数据点，透明命中区承载逐点 tooltip */
function TokenTrendChart({
  days,
  series,
}: {
  days: UsageDay[];
  series: TrendSeries[];
}) {
  const wrapRef = useRef<HTMLDivElement | null>(null);
  const [width, setWidth] = useState(600);
  useEffect(() => {
    const el = wrapRef.current;
    if (!el || typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver((entries) => {
      const w = entries[0]?.contentRect.width;
      if (w && w > 0) setWidth(w);
    });
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  const n = days.length;
  const innerW = Math.max(10, width - PAD_L - PAD_R);
  const innerH = CHART_H - PAD_T - PAD_B;
  const x = (i: number) =>
    PAD_L + (n <= 1 ? innerW / 2 : (i / (n - 1)) * innerW);
  const maxV = Math.max(1, ...series.flatMap((s) => s.values));
  const y = (v: number) => PAD_T + (1 - v / maxV) * innerH;

  // y 轴 4 条虚线网格（0 / 1/4 / 2/4 / 3/4 / 4/4 峰值），无数值标签
  const yTicks = [0, 1, 2, 3, 4];
  // x 轴日期标签：均匀取 ≤6 个（「9月29日」式中文）
  const labelStep = Math.max(1, Math.ceil(n / 6));
  return (
    <div ref={wrapRef}>
      <svg
        width={width}
        height={CHART_H}
        role="img"
        aria-label="每日 Token 趋势图"
      >
        {yTicks.map((k) => (
          <line
            key={k}
            x1={PAD_L}
            x2={width - PAD_R}
            y1={y((maxV * k) / 4)}
            y2={y((maxV * k) / 4)}
            stroke="var(--edge)"
            strokeDasharray="3 4"
            strokeWidth="1"
          />
        ))}
        {series.map((s) => {
          const pts = s.values.map((v, i) => [x(i), y(v)] as [number, number]);
          return (
            <g key={s.key}>
              <path
                d={smoothPath(pts)}
                fill="none"
                stroke={s.color}
                strokeWidth="2"
                strokeLinecap="round"
              />
              {pts.map(([px, py], i) => (
                <circle
                  key={i}
                  cx={px}
                  cy={py}
                  r="3"
                  fill="transparent"
                  stroke="transparent"
                  strokeWidth="8"
                >
                  <title>{`${days[i].day} ${s.name} ${fmtTokens(s.values[i])} tokens`}</title>
                </circle>
              ))}
            </g>
          );
        })}
        {days.map((d, i) =>
          i % labelStep === 0 || i === n - 1 ? (
            <text
              key={d.day}
              x={x(i)}
              y={CHART_H - 6}
              textAnchor="middle"
              fontSize="9"
              fill="var(--t5)"
            >
              {(() => {
                const [, m, dd] = d.day.split("-");
                return `${parseInt(m, 10)}月${parseInt(dd, 10)}日`;
              })()}
            </text>
          ) : null,
        )}
      </svg>
    </div>
  );
}

/* ───────────────────────── 模型用量 3D 饼图 ───────────────────────── */

/** 密集图例行的次级数字档：比正文低一档字号。
 *  text-[10px] 仍在 [data-font-size] 覆盖表内（standard/large/xlarge → 12/13/14px），
 *  四档字号照常生效——它与设置行的 text-xs 分工不同（设置行是主阅读面，
 *  这里是三级元数据），不是任意值混排。 */
const META_NUM = "text-[10px] font-mono tabular-nums text-[var(--t5)]";

/** 立体饼的三个投影常量：顶面椭圆半径、俯视压扁比、侧壁厚度。压扁 + 竖直拉伸
 *  这两步就是 Excel 3D 饼的画法——俯视看到的顶面是椭圆，朝向观众的那半圈侧壁
 *  向下露出来；背面那半圈侧壁落在椭圆内部，被顶面盖掉，不用做背面剔除。 */
const PIE_R = 76;
const PIE_FLAT = 0.55;
const PIE_DEPTH = 20;
const PIE_RY = PIE_R * PIE_FLAT;
const PIE_PAD = 4;
const PIE_W = PIE_R * 2 + PIE_PAD * 2;
const PIE_H = PIE_RY * 2 + PIE_DEPTH + PIE_PAD * 2;
const PIE_CX = PIE_W / 2;
const PIE_CY = PIE_RY + PIE_PAD;

const UNKNOWN_MODEL = "未知模型（旧数据）";

/** 角度参数化：0 在 12 点方向、顺时针为正。屏幕 y 轴向下，故 θ 增大在视觉上是
 *  顺时针，与 SVG 弧的 sweep=1 同向——两侧接缝才不会错开。 */
function piePt(th: number): [number, number] {
  return [PIE_CX + PIE_R * Math.sin(th), PIE_CY - PIE_RY * Math.cos(th)];
}

const n2 = (v: number) => v.toFixed(1);

/** 一段椭圆弧的 A 命令。dy > 0 画在顶面下方 PIE_DEPTH 处的底缘上（侧壁折回时用）。
 *  整圆必须拆成两个半弧：首尾落在同一点的单条 A 弧画不出圆。 */
function arcChain(a1: number, a2: number, sweep: 1 | 0, dy = 0): string {
  const seg = (from: number, to: number) => {
    const [x, y] = piePt(to);
    const large = Math.abs(to - from) > Math.PI ? 1 : 0;
    return `A ${PIE_R} ${n2(PIE_RY)} 0 ${large} ${sweep} ${n2(x)} ${n2(y + dy)}`;
  };
  return Math.abs(a2 - a1) >= Math.PI * 2 - 1e-6
    ? `${seg(a1, a1 + Math.PI)} ${seg(a1 + Math.PI, a2)}`
    : seg(a1, a2);
}

/** 顶面 = 椭圆扇形（整圆时退化为整个椭圆，不从圆心起笔） */
function pieTopPath(a1: number, a2: number): string {
  const [sx, sy] = piePt(a1);
  const lead =
    a2 - a1 >= Math.PI * 2 - 1e-6
      ? `M ${n2(sx)} ${n2(sy)}`
      : `M ${PIE_CX} ${PIE_CY} L ${n2(sx)} ${n2(sy)}`;
  return `${lead} ${arcChain(a1, a2, 1)} Z`;
}

/** 侧壁 = 顶面那段弧沿竖直拉伸 PIE_DEPTH 后折回（折回那一段必须走底缘坐标，
 *  否则下边缘会收敛回顶面，整条带子塌成零厚度） */
function pieWallPath(a1: number, a2: number): string {
  const [sx, sy] = piePt(a1);
  const [ex, ey] = piePt(a2);
  return `M ${n2(sx)} ${n2(sy)} ${arcChain(a1, a2, 1)} L ${n2(ex)} ${n2(
    ey + PIE_DEPTH,
  )} ${arcChain(a2, a1, 0, PIE_DEPTH)} Z`;
}

export type PieSlice = {
  key: string;
  name: string;
  /** 扇形取值 = 缓存未命中口径（输入+输出） */
  value: number;
  share: number;
  a1: number;
  a2: number;
  color: string;
};

/** 占比堆叠条 → 3D 饼图。扇形角度按「缓存未命中」口径（输入+输出）分配，与
 *  usage_stats_by_model 的 ORDER BY 同一口径——名次和图形算两套数会让
 *  「第一名」的扇形明显偏小。零占比的模型分不到角度（没有扇形可画），但仍留在
 *  图例里，悬浮照样给它的累计数。 */
export function pieSlices(rows: UsageModelRow[]): PieSlice[] {
  const value = (r: UsageModelRow) => r.promptTokens + r.completionTokens;
  const total = rows.reduce((a, r) => a + value(r), 0);
  if (total <= 0) return [];
  let acc = 0;
  return rows
    .filter((r) => value(r) > 0)
    .map((r, i) => {
      const v = value(r);
      const a1 = (acc / total) * Math.PI * 2;
      acc += v;
      return {
        key: r.model ?? UNKNOWN_MODEL,
        name: r.model ?? UNKNOWN_MODEL,
        value: v,
        share: v / total,
        a1,
        a2: (acc / total) * Math.PI * 2,
        color: MODEL_COLORS[i % MODEL_COLORS.length],
      };
    });
}

/** 3D 饼图 + 图例行。悬浮扇形或图例行都出累计数浮层（模型名 + 未命中/缓存
 *  两组各带一行拆分 + 轮次）；侧壁不单独挂事件，它只是同一片扇形的立体投影，
 *  顶面已经盖住整个命中区。 */
function ModelUsagePie({ rows }: { rows: UsageModelRow[] }) {
  const slices = pieSlices(rows);
  const total =
    rows.reduce((a, r) => a + r.promptTokens + r.completionTokens, 0) || 1;
  const [hover, setHover] = useState<string | null>(null);
  const [tip, setTip] = useState({ left: 0, top: 0, flip: false });
  const wrapRef = useRef<HTMLDivElement>(null);

  const showTip = (key: string, el: Element) => {
    const wrap = wrapRef.current;
    if (!wrap) return;
    const wr = wrap.getBoundingClientRect();
    const cr = el.getBoundingClientRect();
    // 浮层宽 210px：靠右的扇形/图例行向左翻，避免越过容器右缘被裁
    const flip = cr.right - wr.left > wr.width - 220;
    setTip({
      left: flip ? cr.left - wr.left - 6 : cr.right - wr.left + 6,
      top: cr.top - wr.top + 12,
      flip,
    });
    setHover(key);
  };

  const hov = rows.find((r) => (r.model ?? UNKNOWN_MODEL) === hover) ?? null;

  /** 图例行兼作零占比模型的唯一入口（没有扇形可悬浮），所以它也挂同一套浮层 */
  const legendRow = (r: UsageModelRow) => {
    const key = r.model ?? UNKNOWN_MODEL;
    const v = r.promptTokens + r.completionTokens;
    return (
      <div
        key={key}
        data-model={key}
        className="flex items-center gap-2 text-xs"
        onMouseEnter={(e) => showTip(key, e.currentTarget)}
      >
        <span
          className="h-2 w-2 shrink-0 rounded-full"
          style={{
            backgroundColor:
              slices.find((s) => s.key === key)?.color ?? "var(--edge-strong)",
          }}
        />
        <span className="min-w-0 flex-1 truncate text-[var(--t2)]">
          {r.model ?? UNKNOWN_MODEL}
        </span>
        <span className={`shrink-0 ${META_NUM} text-[var(--t4)]`}>
          缓存未命中 {fmtTokens(v)}
        </span>
        <span className={`shrink-0 ${META_NUM} w-9 text-right`}>
          {Math.round((v / total) * 100)}%
        </span>
      </div>
    );
  };

  return (
    <div
      className="relative"
      ref={wrapRef}
      onMouseLeave={() => setHover(null)}
    >
      <div className="flex flex-wrap items-center gap-x-8 gap-y-4">
        <svg
          width={PIE_W}
          height={PIE_H}
          role="img"
          aria-label="模型用量 3D 饼图"
          style={{ overflow: "visible" }}
        >
          {/* 侧壁先于顶面画：背面那半圈落在椭圆内部，顶面一盖就看不见了 */}
          {slices.map((s) => (
            <path
              key={`wall-${s.key}`}
              d={pieWallPath(s.a1, s.a2)}
              fill={`color-mix(in srgb, ${s.color} 62%, #000)`}
            />
          ))}
          {slices.length === 0 ? (
            /* 全零数据：留一块中性底坯，图例仍可读 */
            <ellipse
              cx={PIE_CX}
              cy={PIE_CY}
              rx={PIE_R}
              ry={PIE_RY}
              fill="var(--inset-bg)"
            />
          ) : (
            /* 悬浮不做变暗：顶面一透明，底下那条深色侧壁就原形毕露，读起来像一圈
               投影（越悬浮越明显）。反馈交给浮层——它就贴着光标出现，扇面保持
               常态即可。 */
            slices.map((s) => (
              <g
                key={s.key}
                data-model={s.key}
                data-share={s.share.toFixed(4)}
                onMouseEnter={(e) => showTip(s.key, e.currentTarget)}
                style={{ cursor: "pointer" }}
              >
                <path d={pieTopPath(s.a1, s.a2)} fill={s.color} />
              </g>
            ))
          )}
        </svg>

        <div className="min-w-[240px] flex-1 space-y-1.5">
          {rows.map((r) => legendRow(r))}
        </div>
      </div>

      <p className="mt-2 text-[10px] text-[var(--t6)]">
        扇形角度与占比按「缓存未命中」口径；缓存命中的 token
        单独计价，不计入扇形。悬浮扇形或图例行看该模型的累计数。
      </p>

      {/* 悬浮说明：五行——未命中合计 / 其输入输出拆分 / 缓存合计 / 其命中写入
          拆分 / 轮次。分组口径同热力图：缓存读单价约为普通输入的十分之一，
          合成一个「总 token」会让规模与开销两头失真。 */}
      {hov && (
        <div
          role="tooltip"
          className="pointer-events-none absolute z-10 w-[210px] rounded-[var(--r-md)] border border-[var(--edge-strong)] bg-[var(--surface-raised)] px-3 py-2 shadow-[var(--shadow-lg)]"
          style={{
            left: tip.left,
            top: tip.top,
            transform: tip.flip ? "translateX(-100%)" : undefined,
          }}
        >
          <p className="text-xs font-medium text-[var(--t1)]">
            {hov.model ?? UNKNOWN_MODEL}
          </p>
          <div className="mt-1.5 space-y-1.5">
            <div>
              <div className="flex items-baseline justify-between gap-2">
                <span className="text-xs text-[var(--t5)]">累计缓存未命中</span>
                <span className="font-mono text-xs tabular-nums text-[var(--t2)]">
                  {fmtTokens(hov.promptTokens + hov.completionTokens)}
                </span>
              </div>
              <p className="mt-0.5 text-xs text-[var(--t6)]">
                输入 {fmtTokens(hov.promptTokens)} · 输出{" "}
                {fmtTokens(hov.completionTokens)}
              </p>
            </div>
            <div>
              <div className="flex items-baseline justify-between gap-2">
                <span className="text-xs text-[var(--t5)]">累计缓存命中</span>
                <span className="font-mono text-xs tabular-nums text-[var(--t2)]">
                  {fmtTokens(cacheOf(hov))}
                </span>
              </div>
              <p className="mt-0.5 text-xs text-[var(--t6)]">
                命中 {fmtTokens(cacheReadOf(hov))} · 写入{" "}
                {fmtTokens(cacheWriteOf(hov))}
              </p>
            </div>
            <div className="flex items-baseline justify-between gap-2 border-t border-[var(--edge)] pt-1.5">
              <span className="text-xs text-[var(--t5)]">轮次</span>
              <span className="font-mono text-xs tabular-nums text-[var(--t2)]">
                {hov.runs}
              </span>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}

/* ───────────────────────── 卡片组装 ───────────────────────── */

const HEAT_MODES: Array<[HeatMode, string]> = [
  ["daily", "每日"],
  ["weekly", "每周"],
  ["cumulative", "累计"],
];

export function UsageStatsCard() {
  const [days, setDays] = useState<UsageDay[]>([]);
  const [models, setModels] = useState<UsageModelRow[]>([]);
  const [modelDays, setModelDays] = useState<UsageDayModelRow[]>([]);
  const [heatMode, setHeatMode] = useState<HeatMode>("daily");
  // 趋势图的视图档位：用户点「近7日 / 近30日」选的是**看多久**，不是后端
  // 限制的副本——后端接受任意 1..=窗口上限的天数，这里是产品选的两档快捷方式
  const [range, setRange] = useState<7 | 30>(30);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  // 防御归一：mock/异常路径可能给 null（null.map 的 uncaught 会炸掉整个设置页测试宿主）
  const loadOverview = useCallback(() => {
    setLoading(true);
    setError(null);
    Promise.all([
      // days 传 null = 要后端能给的整个窗口（热力图按返回的天数铺开）。
      // 前端写死 365 的话，后端一旦改了可查窗口上限，这里就静默少一段历史
      // （图还是按实际返回的天数渲染，只是窄了，不报错）。
      invoke<UsageDay[]>("usage_stats_daily", { days: null }),
      invoke<UsageModelRow[]>("usage_stats_by_model", { days: null }),
    ])
      .then(([d, m]) => {
        setDays(Array.isArray(d) ? d : []);
        setModels(Array.isArray(m) ? m : []);
      })
      .catch((e) => setError(String(e)))
      .finally(() => setLoading(false));
  }, []);

  // 竞态令牌：快速切 7↔30（或连点刷新）时，慢的旧档位响应不得覆盖新档位数据
  const modelDaysSeqRef = useRef(0);
  const loadModelDays = useCallback(() => {
    const seq = ++modelDaysSeqRef.current;
    invoke<UsageDayModelRow[]>("usage_stats_daily_by_model", { days: range })
      .then((r) => {
        if (seq !== modelDaysSeqRef.current) return;
        setModelDays(Array.isArray(r) ? r : []);
      })
      .catch((e) => {
        if (seq !== modelDaysSeqRef.current) return;
        setError(String(e));
      });
  }, [range]);

  // 挂载期拉一次；range 变化只重拉按模型序列（setState 在 await 后，lint 显式豁免——项目既有模式）
  useEffect(() => {
    loadOverview();
  }, [loadOverview]);
  useEffect(() => {
    loadModelDays();
  }, [loadModelDays]);

  const refresh = useCallback(() => {
    loadOverview();
    loadModelDays();
  }, [loadOverview, loadModelDays]);

  const totalTokens = days.reduce((a, d) => a + dayTotal(d), 0);
  const totalCache = days.reduce((a, d) => a + cacheOf(d), 0);
  const peakDay = days.reduce((a, d) => Math.max(a, dayTotal(d)), 0);
  const totalRuns = days.reduce((a, d) => a + d.runs, 0);
  const [curStreak, bestStreak] = useMemo(() => streakDays(days), [days]);
  const trendDays = useMemo(() => days.slice(-range), [days, range]);
  const series = useMemo(
    () => buildTrendSeries(trendDays, modelDays),
    [trendDays, modelDays],
  );
  const isEmpty = !loading && !error && totalTokens === 0 && totalRuns === 0;

  const rangeBtn = (r: 7 | 30, label: string) => (
    <button
      key={r}
      className={`px-3 py-1 text-xs text-[var(--t3)] ${range === r ? "nm-inset" : "nm-outset"}`}
      onClick={() => setRange(r)}
    >
      {label}
    </button>
  );

  return (
    <>
      {/* 卡 A · 汇总指标行（参照图：一卡 5 项、竖线分隔） */}
      <div className="nm-card p-5">
        <div className="flex items-center justify-between gap-4">
          <div>
            <h2 className="text-lg font-semibold text-[var(--t1)]">词元统计</h2>
            <p className="mt-1 text-xs text-[var(--t5)]">
              本地词元用量（含聊天与任务执行；不涉任何上报）
            </p>
          </div>
          <button
            className="btn-sm-action nm-outset"
            onClick={refresh}
            disabled={loading}
          >
            <RefreshCw
              size={12}
              aria-hidden
              className={loading ? "animate-spin" : ""}
            />
            {loading ? "读取中…" : "刷新"}
          </button>
        </div>

        {error && <p className="mt-3 text-xs text-[var(--danger)]">{error}</p>}

        {isEmpty ? (
          <p className="mt-6 text-center text-xs text-[var(--t5)]">
            还没有词元用量——跑一次对话或任务后这里会出现统计。
          </p>
        ) : (
          /* 五格：合并「当前/最长连续」腾出一格给缓存，两档 token 并列。
             开着 prompt caching 时 prompt_tokens 只承载缓存未命中的部分，所以
             两个 token 格都必须写明是未命中还是命中，否则会低估真实消耗。 */
          <div className="mt-4 grid grid-cols-5 divide-x divide-[var(--edge)] text-center">
            {(
              [
                ["累计缓存未命中", fmtTokens(totalTokens)],
                [
                  "累计缓存命中",
                  // 缓存仅 Anthropic 协议上报；恒零时用「—」而非 0，
                  // 0 读起来像「测过是零」，— 才是「这一档没数据」
                  totalCache === 0 ? "—" : fmtTokens(totalCache),
                ],
                ["峰值缓存未命中", fmtTokens(peakDay)],
                ["执行次数", String(totalRuns)],
                ["当前 / 最长连续", `${curStreak} / ${bestStreak} 天`],
              ] as const
            ).map(([label, value]) => (
              <div key={label} className="px-2">
                <p className="text-lg font-semibold tabular-nums text-[var(--t1)]">
                  {value}
                </p>
                <p className="mt-0.5 text-xs text-[var(--t5)]">{label}</p>
              </div>
            ))}
          </div>
        )}
      </div>

      {!isEmpty && (
        <>
          {/* 卡 B · Token 活动热力图 */}
          <div className="nm-card p-5">
            <div className="flex items-center justify-between gap-4">
              <p className="text-sm font-medium text-[var(--t2)]">Token 活动</p>
              <div className="flex gap-1">
                {HEAT_MODES.map(([m, label]) => (
                  <button
                    key={m}
                    className={`px-3 py-1 text-xs text-[var(--t3)] ${
                      heatMode === m ? "nm-inset" : "nm-outset"
                    }`}
                    onClick={() => setHeatMode(m)}
                  >
                    {label}
                  </button>
                ))}
              </div>
            </div>
            <div className="mt-3">
              <TokenHeatmap days={days} mode={heatMode} />
            </div>
          </div>

          {/* 时间范围行（参照图：卡外标题 + 右侧切换） */}
          <div className="flex items-center justify-between gap-4 px-1">
            <p className="text-sm font-medium text-[var(--t2)]">时间范围</p>
            <div className="flex gap-1">
              {rangeBtn(7, "近7日")}
              {rangeBtn(30, "近30日")}
            </div>
          </div>

          {/* 卡 C · 每日 Token 趋势图（按模型分线） */}
          <div className="nm-card p-5">
            <p className="text-sm font-medium text-[var(--t2)]">
              每日 Token 趋势图
            </p>
            <div className="mt-2 flex flex-wrap items-center gap-4 text-xs text-[var(--t4)]">
              {series.map((s) => (
                <span key={s.key} className="inline-flex items-center gap-1">
                  <span
                    className="h-2 w-2 rounded-full"
                    style={{ backgroundColor: s.color }}
                  />
                  {s.name}
                </span>
              ))}
            </div>
            {trendDays.length > 0 && series.length > 0 ? (
              <div className="mt-1">
                <TokenTrendChart days={trendDays} series={series} />
              </div>
            ) : (
              <p className="mt-4 text-center text-xs text-[var(--t5)]">
                窗口内暂无数据
              </p>
            )}
          </div>

          {/* 卡 D · 模型用量 3D 饼图 */}
          {models.length > 0 && (
            <div className="nm-card p-5">
              <p className="text-sm font-medium text-[var(--t2)]">模型用量</p>
              <div className="mt-3">
                <ModelUsagePie rows={models} />
              </div>
            </div>
          )}

          <p className="px-1 text-xs leading-relaxed text-[var(--t5)]">
            数据说明：本地 exec_traces 聚合（热力图窗口 1
            年，实际范围受保留期限制）； 执行次数只计已收尾执行。Anthropic
            与 OpenAI 两种协议的用量都在统计——OpenAI 的
            <code className="mx-1">prompt_tokens</code> 含
            <code className="mx-1">cached_tokens</code>，已减去后再落库，两种
            协议的「普通输入」口径一致。OpenAI 兼容形状没有缓存写入字段，
            该档恒 0；网关不支持
            <code className="mx-1">stream_options</code> 时该次请求不记用量。
          </p>
        </>
      )}
    </>
  );
}
