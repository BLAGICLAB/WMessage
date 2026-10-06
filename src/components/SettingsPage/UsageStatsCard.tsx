// 词元统计卡（设置页「tokens」分区）——按「使用统计」参照图重排：
// 汇总指标行（竖线分隔 5 项，含连续天数）→ Token 活动热力图（GitHub contributions 式
// 7×N 周格，每日/每周/累计切换 + 月份标签）→ 时间范围行 + 每日趋势按模型分线 →
// 模型用量榜，各板块独立成卡。
// 数据源 = usage_stats_daily(365) + usage_stats_by_model(365)（Rust 侧升序、缺日补零）
// + usage_stats_daily_by_model(range)（稀疏返回，前端以补零日序列为 x 轴）。
// 不引图表库：曲线是手绘 SVG（Catmull-Rom 平滑），热力图是 div 网格——零依赖纪律。

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { RefreshCw } from "lucide-react";

export type UsageDay = {
  day: string;
  promptTokens: number;
  completionTokens: number;
  runs: number;
  toolCalls: number;
};

export type UsageModelRow = {
  /** 旧行（model 列上线前）/未发请求 = null，展示为「未知模型」 */
  model: string | null;
  promptTokens: number;
  completionTokens: number;
  runs: number;
};

export type UsageDayModelRow = {
  day: string;
  model: string | null;
  promptTokens: number;
  completionTokens: number;
};

/** 中文数量级格式化：≥1亿 → x.x亿；≥1万 → x.x万；≥1000 → x.xK；原值 */
function fmtTokens(n: number): string {
  if (n >= 100_000_000) return `${(n / 100_000_000).toFixed(1)} 亿`;
  if (n >= 10_000) return `${(n / 10_000).toFixed(1)} 万`;
  if (n >= 1000) return `${(n / 1000).toFixed(1)}K`;
  return String(Math.round(n));
}

const dayTotal = (d: UsageDay) => d.promptTokens + d.completionTokens;

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
 *  底部月份标签按周列定位，title 即 tooltip */
function TokenHeatmap({ days, mode }: { days: UsageDay[]; mode: HeatMode }) {
  const firstDowRaw = days.length ? new Date(`${days[0].day}T00:00:00`).getDay() : 0;
  const firstDow = Number.isNaN(firstDowRaw) ? 0 : firstDowRaw;
  const numWeeks = Math.max(1, Math.ceil((firstDow + days.length) / 7));
  const weekOf = (i: number) => Math.floor((firstDow + i) / 7);
  const values = heatValues(days, mode, numWeeks, weekOf);
  const max = Math.max(1, ...values);
  const levelOf = (v: number) => (v <= 0 ? 0 : Math.min(4, Math.ceil((v / max) * 4)));

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

  return (
    <div>
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
          const total = dayTotal(d);
          return (
            <div
              key={d.day}
              className="rounded-[3px]"
              style={{ backgroundColor: heatColor(levelOf(values[i])) }}
              title={`${d.day} · ${fmtTokens(total)} tokens（输入 ${fmtTokens(d.promptTokens)} / 输出 ${fmtTokens(d.completionTokens)}，执行 ${d.runs} 次）`}
            />
          );
        })}
      </div>
      <div className="relative mt-2 h-4 text-[10px] text-[var(--t5)]">
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
    </div>
  );
}

/* ───────────────────────── 每日趋势平滑曲线（按模型） ───────────────────────── */

/** Catmull-Rom → 三次贝塞尔（相邻控制点取 1/6 差分），点少时退化为折线段。
 *  控制点 y 钳制在绘图区内——尖锐谷底（近零日）差分过冲会把曲线顶出 0 线。 */
function smoothPath(pts: Array<[number, number]>): string {
  if (pts.length === 0) return "";
  if (pts.length === 1) return `M ${pts[0][0].toFixed(1)} ${pts[0][1].toFixed(1)}`;
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

const MODEL_COLORS = ["#3563b0", "#35854a", "#b05f2f", "#6d5aa8", "#b04a6a", "#4a5568"];
const TREND_TOP_N = 5;

export type TrendSeries = { key: string; name: string; color: string; values: number[] };

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
  const x = (i: number) => PAD_L + (n <= 1 ? innerW / 2 : (i / (n - 1)) * innerW);
  const maxV = Math.max(1, ...series.flatMap((s) => s.values));
  const y = (v: number) => PAD_T + (1 - v / maxV) * innerH;

  // y 轴 4 条虚线网格（0 / 1/4 / 2/4 / 3/4 / 4/4 峰值），无数值标签
  const yTicks = [0, 1, 2, 3, 4];
  // x 轴日期标签：均匀取 ≤6 个（「9月29日」式中文）
  const labelStep = Math.max(1, Math.ceil(n / 6));
  return (
    <div ref={wrapRef}>
      <svg width={width} height={CHART_H} role="img" aria-label="每日 Token 趋势图">
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
              <path d={smoothPath(pts)} fill="none" stroke={s.color} strokeWidth="2" strokeLinecap="round" />
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

/* ───────────────────────── 模型用量榜 ───────────────────────── */

/** 占比堆叠条 + 图例行（palette 循环取色） */
function ModelUsageList({ rows }: { rows: UsageModelRow[] }) {
  const total = rows.reduce((a, r) => a + r.promptTokens + r.completionTokens, 0) || 1;
  return (
    <div>
      <div className="flex h-2.5 overflow-hidden rounded-full bg-[var(--inset-bg)]">
        {rows.map((r, i) => {
          const t = r.promptTokens + r.completionTokens;
          return (
            <div
              key={i}
              style={{
                width: `${(t / total) * 100}%`,
                backgroundColor: MODEL_COLORS[i % MODEL_COLORS.length],
              }}
              title={`${r.model ?? "未知模型"} · ${fmtTokens(t)} tokens`}
            />
          );
        })}
      </div>
      <div className="mt-2 space-y-1.5">
        {rows.map((r, i) => {
          const t = r.promptTokens + r.completionTokens;
          return (
            <div key={i} className="flex items-center gap-2 text-xs">
              <span
                className="h-2 w-2 shrink-0 rounded-full"
                style={{ backgroundColor: MODEL_COLORS[i % MODEL_COLORS.length] }}
              />
              <span className="min-w-0 flex-1 truncate text-[var(--t2)]">
                {r.model ?? "未知模型（旧数据）"}
              </span>
              <span className="shrink-0 font-mono text-[10px] tabular-nums text-[var(--t4)]">
                {fmtTokens(t)} · {r.runs}次
              </span>
              <span className="w-9 shrink-0 text-right font-mono text-[10px] tabular-nums text-[var(--t5)]">
                {Math.round((t / total) * 100)}%
              </span>
            </div>
          );
        })}
      </div>
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
  const [range, setRange] = useState<7 | 30>(30);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  // 防御归一：mock/异常路径可能给 null（null.map 的 uncaught 会炸掉整个设置页测试宿主）
  const loadOverview = useCallback(() => {
    setLoading(true);
    setError(null);
    Promise.all([
      invoke<UsageDay[]>("usage_stats_daily", { days: 365 }),
      invoke<UsageModelRow[]>("usage_stats_by_model", { days: 365 }),
    ])
      .then(([d, m]) => {
        setDays(Array.isArray(d) ? d : []);
        setModels(Array.isArray(m) ? m : []);
      })
      .catch((e) => setError(String(e)))
      .finally(() => setLoading(false));
  }, []);

  const loadModelDays = useCallback(() => {
    invoke<UsageDayModelRow[]>("usage_stats_daily_by_model", { days: range })
      .then((r) => setModelDays(Array.isArray(r) ? r : []))
      .catch((e) => setError(String(e)));
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
  const peakDay = days.reduce((a, d) => Math.max(a, dayTotal(d)), 0);
  const totalRuns = days.reduce((a, d) => a + d.runs, 0);
  const [curStreak, bestStreak] = useMemo(() => streakDays(days), [days]);
  const trendDays = useMemo(() => days.slice(-range), [days, range]);
  const series = useMemo(() => buildTrendSeries(trendDays, modelDays), [trendDays, modelDays]);
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
            className="shrink-0 min-w-[76px] px-4 py-1.5 text-sm text-[var(--t3)] nm-outset inline-flex items-center justify-center gap-1"
            onClick={refresh}
            disabled={loading}
          >
            <RefreshCw size={12} aria-hidden className={loading ? "animate-spin" : ""} />
            {loading ? "读取中…" : "刷新"}
          </button>
        </div>

        {error && <p className="mt-3 text-xs text-[var(--danger)]">{error}</p>}

        {isEmpty ? (
          <p className="mt-6 text-center text-xs text-[var(--t5)]">
            还没有词元用量——跑一次对话或任务后这里会出现统计。
          </p>
        ) : (
          <div className="mt-4 grid grid-cols-5 divide-x divide-[var(--edge)] text-center">
            {[
              ["累计 Token 数", fmtTokens(totalTokens)],
              ["峰值 Token 数", fmtTokens(peakDay)],
              ["执行次数", String(totalRuns)],
              ["当前连续天数", `${curStreak} 天`],
              ["最长连续天数", `${bestStreak} 天`],
            ].map(([label, value]) => (
              <div key={label} className="px-2">
                <p className="text-lg font-semibold tabular-nums text-[var(--t1)]">{value}</p>
                <p className="mt-0.5 text-[10px] text-[var(--t5)]">{label}</p>
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
            <p className="text-sm font-medium text-[var(--t2)]">每日 Token 趋势图</p>
            <div className="mt-2 flex flex-wrap items-center gap-4 text-[10px] text-[var(--t4)]">
              {series.map((s) => (
                <span key={s.key} className="inline-flex items-center gap-1">
                  <span className="h-2 w-2 rounded-full" style={{ backgroundColor: s.color }} />
                  {s.name}
                </span>
              ))}
            </div>
            {trendDays.length > 0 && series.length > 0 ? (
              <div className="mt-1">
                <TokenTrendChart days={trendDays} series={series} />
              </div>
            ) : (
              <p className="mt-4 text-center text-xs text-[var(--t5)]">窗口内暂无数据</p>
            )}
          </div>

          {/* 卡 D · 模型用量榜 */}
          {models.length > 0 && (
            <div className="nm-card p-5">
              <p className="text-sm font-medium text-[var(--t2)]">模型用量</p>
              <div className="mt-3">
                <ModelUsageList rows={models} />
              </div>
            </div>
          )}

          <p className="px-1 text-[10px] leading-relaxed text-[var(--t5)]">
            数据说明：本地 exec_traces 聚合（热力图窗口 1 年，实际范围受保留期限制）；
            执行次数只计已收尾执行；OpenAI 兼容网关的流式用量暂未接入（仅 Anthropic 协议计统计）。
          </p>
        </>
      )}
    </>
  );
}
