// 吸顶收缩头部（Sticky Compact Header）：滚动进度驱动字号与间距，收成
// 单行后钉在滚动容器顶栏，反向滚动经 spring 连续还原。
// 进度源 = main 滚动容器的 scrollY（0 → COLLAPSE_PX 线性映射 + spring 平滑）；
// 颜色不参与插值（motion 无法混 CSS 变量），分隔线用独立元素的透明度渐入。
// 首行 = 问候（姓名/日期/今日摘要/完成率条）｜右侧三池（待办/完成/归档，
// 文字在上数字在下）；第二行 = 自定义 KPI 横排（8 选 4，标签在上、36px 大字）。
// 收缩时 KPI 横排整行压平，首行正中的镜像条渐入承接四个数字（姓名/日期与
// 三池之间，页面居中）；两处数字共用同一尺寸插值，交叉时无大小跳变。
// 镜像条 aria-hidden + pointer-events-none：纯视觉承接，防读屏重复朗读。

import {
  motion,
  useReducedMotion,
  useScroll,
  useSpring,
  useTransform,
  type MotionValue,
} from "framer-motion";
import { TrendingDown, TrendingUp } from "lucide-react";
import {
  greetingOf,
  type PlanPhases,
  type PoolCounts,
} from "./homeData";
import {
  formatKpiValue,
  kpiTrendOf,
  kpiTrendTitle,
  type HomeKpiBlock,
} from "../../lib/homeKpi";

/** 从顶部滚动多少 px 完成收缩 */
const COLLAPSE_PX = 150;

const WEEKDAY = ["周日", "周一", "周二", "周三", "周四", "周五", "周六"];

function dateChipOf(now: Date): string {
  return `${now.getMonth() + 1}月${now.getDate()}日 ${WEEKDAY[now.getDay()]}`;
}

/** 涨跌角标：折线箭头指向上一次变动后的走向，颜色 = 好(绿)/坏(红)/中性(灰)；
 *  无基线或数值未变时不渲染。testId 仅横排行传入（镜像条是 aria-hidden 副本） */
function TrendBadge({
  kpi,
  testId,
}: {
  kpi: HomeKpiBlock;
  testId?: string;
}) {
  const trend = kpiTrendOf(kpi.def, kpi.value, kpi.prev);
  if (!trend || kpi.value == null || kpi.prev == null) return null;
  const tone =
    trend.tone === "good"
      ? "text-[var(--success)]"
      : trend.tone === "bad"
        ? "text-[var(--danger)]"
        : "text-[var(--t5)]";
  const Icon = trend.dir === "up" ? TrendingUp : TrendingDown;
  const text = kpiTrendTitle(kpi.def, kpi.value, kpi.prev);
  return (
    <span
      className={`inline-flex ${tone}`}
      title={text}
      aria-label={text}
      data-testid={testId}
    >
      <Icon size={13} strokeWidth={2.2} aria-hidden />
    </span>
  );
}

/** KPI 数字 + 涨跌角标：横排行与居中镜像条共用，字号由调用方的插值传入 */
function KpiNumber({
  kpi,
  size,
  testIdPrefix,
  badgeTestIdPrefix,
}: {
  kpi: HomeKpiBlock;
  size: MotionValue<number>;
  testIdPrefix: string;
  badgeTestIdPrefix?: string;
}) {
  return (
    <div className="flex items-baseline gap-1.5">
      <motion.div
        className="num font-semibold leading-none text-[var(--t1)]"
        style={{ fontSize: size }}
        data-testid={`${testIdPrefix}-${kpi.def.id}`}
      >
        {formatKpiValue(kpi.def, kpi.value)}
      </motion.div>
      <TrendBadge
        kpi={kpi}
        testId={
          badgeTestIdPrefix ? `${badgeTestIdPrefix}-${kpi.def.id}` : undefined
        }
      />
    </div>
  );
}

/** 今日完成率进度条（实心段内嵌百分比，余量斜纹）；参考案例的同款语言 */
function ProgressBar({ done, planned }: { done: number; planned: number }) {
  const reduceMotion = useReducedMotion();
  const pct = planned ? Math.round((done / planned) * 100) : 0;
  return (
    <div
      className="home-progress"
      role="progressbar"
      aria-label="今日完成率"
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={pct}
    >
      <motion.div
        className="home-progress-fill"
        initial={false}
        animate={{ width: `${pct}%` }}
        transition={reduceMotion ? { duration: 0 } : { type: "spring", stiffness: 120, damping: 22 }}
      >
        {pct >= 18 && <span className="num">{pct}%</span>}
      </motion.div>
    </div>
  );
}

export function HomeHeader({
  name,
  phases,
  pools,
  kpis,
  now,
  scrollRef,
}: {
  name: string;
  phases: PlanPhases;
  pools: PoolCounts;
  kpis: HomeKpiBlock[];
  now: Date;
  scrollRef: React.RefObject<HTMLElement | null>;
}) {
  const reduceMotion = useReducedMotion();
  const { scrollY } = useScroll({ container: scrollRef });
  const raw = useTransform(scrollY, [0, COLLAPSE_PX], [0, 1]);
  const spring = useSpring(raw, { stiffness: 320, damping: 34 });
  // 减少动效：跳过 spring，跟随滚动即时到值（still 连续可逆，只是不缓动）
  const progress = reduceMotion ? raw : spring;

  const titleSize = useTransform(progress, [0, 1], [24, 14.5]);
  const poolSize = useTransform(progress, [0, 1], [26, 16]);
  const poolGap = useTransform(progress, [0, 1], [26, 14]);
  const kpiBigSize = useTransform(progress, [0, 1], [36, 15.5]);
  const subOpacity = useTransform(progress, [0, 1], [1, 0]);
  const subHeight = useTransform(progress, [0, 1], [24, 0]);
  const barOpacity = useTransform(progress, [0, 1], [1, 0]);
  // 收口容器 = pt-2(8px) + 条(20px)；高度必须与内容严格一致，否则 overflow 裁条
  const barHeight = useTransform(progress, [0, 1], [28, 0]);
  const labelOpacity = useTransform(progress, [0, 1], [1, 0]);
  const labelHeight = useTransform(progress, [0, 1], [18, 0]);
  // KPI 横排行收口高度 = pt-3(12) + 标签行 16 + mt-1(4) + 数字行 36 = 68，
  // 必须与内容严格一致，否则 overflow 裁字/留白
  const bandHeight = useTransform(progress, [0, 1], [68, 0]);
  const bandOpacity = useTransform(progress, [0, 1], [1, 0]);
  // 居中镜像条在收缩后段渐入（0.55 → 1），与横排行收口交叉衔接
  const stripOpacity = useTransform(progress, [0.55, 1], [0, 1]);
  const borderOpacity = useTransform(progress, [0, 1], [0, 1]);

  const planned = phases.done + phases.doing + phases.todo;
  const summary = planned
    ? `今日计划 ${planned}，已完成 ${phases.done}`
    : "今天还没有排期，去任务页拖一段上来。";
  const poolBlocks = [
    { id: "todo", label: "待办", value: pools.todo },
    { id: "done", label: "完成", value: pools.done },
    { id: "archived", label: "归档", value: pools.archived },
  ];

  return (
    // 吸附矩形按规范内缩滚动容器的 padding（main 的 p-6）：top/-mt/pt 三者用
    // 同一 24px 抵消，吸附时头部贴住 main 真顶（无透底缝），静止时视觉位置不变。
    <motion.header className="sticky top-[-24px] -mt-6 z-20 -mx-6 bg-[var(--bg)] px-6 pt-6 pb-2.5">
      <div className="relative flex items-end justify-between gap-6">
        {/* 收缩后段显形的居中镜像条：四个数字落在姓名/日期与三池之间的正中 */}
        <motion.div
          aria-hidden
          className="pointer-events-none absolute inset-x-0 top-1/2 flex -translate-y-1/2 items-baseline justify-center gap-4"
          style={{ opacity: stripOpacity }}
        >
          {kpis.map((k) => (
            <KpiNumber key={k.def.id} kpi={k} size={kpiBigSize} testIdPrefix="kpi-strip" />
          ))}
        </motion.div>

        <div className="min-w-0 flex-1">
          <div className="flex items-baseline gap-2.5">
            <motion.h1
              className="whitespace-nowrap font-semibold tracking-tight text-[var(--t1)]"
              style={{ fontSize: titleSize }}
            >
              {greetingOf(now)}
              {name ? `，${name}` : ""}
            </motion.h1>
            <span className="whitespace-nowrap text-xs text-[var(--t5)] num">
              {dateChipOf(now)}
            </span>
          </div>
          <motion.p
            className="overflow-hidden text-[13.5px] leading-6 text-[var(--t3)]"
            style={{ opacity: subOpacity, height: subHeight }}
          >
            {summary}
          </motion.p>
          <motion.div
            className="overflow-hidden"
            style={{ opacity: barOpacity, height: barHeight }}
          >
            {planned > 0 && (
              <div className="pt-2">
                <ProgressBar done={phases.done} planned={planned} />
              </div>
            )}
          </motion.div>
        </div>

        <motion.div
          className="flex shrink-0 items-end"
          style={{ gap: poolGap }}
        >
          {poolBlocks.map((p) => (
            <div key={p.id} className="text-right">
              <motion.div
                className="overflow-hidden text-xs text-[var(--t4)]"
                style={{ opacity: labelOpacity, height: labelHeight }}
              >
                {p.label}
              </motion.div>
              <motion.div
                className="num font-semibold leading-none text-[var(--t1)]"
                style={{ fontSize: poolSize }}
                data-testid={`pool-${p.id}`}
              >
                {p.value}
              </motion.div>
            </div>
          ))}
        </motion.div>
      </div>

      {/* KPI 横排行：初始全宽展开，滚动时整行压平收进上方居中镜像条 */}
      <motion.div
        className="overflow-hidden"
        style={{ opacity: bandOpacity, height: bandHeight }}
      >
        <div className="flex items-end justify-between pt-3">
          {kpis.map((k) => (
            <div key={k.def.id}>
              <div className="text-xs text-[var(--t4)]">{k.def.name}</div>
              <div className="mt-1">
                <KpiNumber
                  kpi={k}
                  size={kpiBigSize}
                  testIdPrefix="kpi-value"
                  badgeTestIdPrefix="kpi-trend"
                />
              </div>
            </div>
          ))}
        </div>
      </motion.div>

      {/* 收缩后浮现的顶栏分隔线（独立元素做透明度，避开 CSS 变量插值） */}
      <motion.div
        aria-hidden
        className="mt-1 h-px bg-[var(--edge)]"
        style={{ opacity: borderOpacity }}
      />
    </motion.header>
  );
}
