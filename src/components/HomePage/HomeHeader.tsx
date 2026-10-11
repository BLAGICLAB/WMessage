// 吸顶收缩头部（Sticky Compact Header）：滚动进度驱动字号与间距，收成
// 单行后钉在滚动容器顶栏，反向滚动经 spring 连续还原。
// 进度源 = main 滚动容器的 scrollY（0 → COLLAPSE_PX 线性映射 + spring 平滑）；
// 颜色不参与插值（motion 无法混 CSS 变量），分隔线用独立元素的透明度渐入。
// 欢迎区：姓名取设置里个人资料的用户名；小字只统计今日计划/今日完成，
// 其下进度条 = 今日完成率（参考案例的分段条语言：实心段 + 斜纹余量）。

import {
  motion,
  useReducedMotion,
  useScroll,
  useSpring,
  useTransform,
} from "framer-motion";
import { greetingOf, type PlanPhases, type PoolCounts } from "./homeData";

/** 从顶部滚动多少 px 完成收缩 */
const COLLAPSE_PX = 150;

const WEEKDAY = ["周日", "周一", "周二", "周三", "周四", "周五", "周六"];

function dateChipOf(now: Date): string {
  return `${now.getMonth() + 1}月${now.getDate()}日 ${WEEKDAY[now.getDay()]}`;
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
  now,
  scrollRef,
}: {
  name: string;
  phases: PlanPhases;
  pools: PoolCounts;
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
  const kpiSize = useTransform(progress, [0, 1], [26, 16]);
  const kpiGap = useTransform(progress, [0, 1], [26, 14]);
  const subOpacity = useTransform(progress, [0, 1], [1, 0]);
  const subHeight = useTransform(progress, [0, 1], [24, 0]);
  const barOpacity = useTransform(progress, [0, 1], [1, 0]);
  // 收口容器 = pt-2(8px) + 条(20px)；高度必须与内容严格一致，否则 overflow 裁条
  const barHeight = useTransform(progress, [0, 1], [28, 0]);
  const labelOpacity = useTransform(progress, [0, 1], [1, 0]);
  const labelHeight = useTransform(progress, [0, 1], [18, 0]);
  const borderOpacity = useTransform(progress, [0, 1], [0, 1]);

  const planned = phases.done + phases.doing + phases.todo;
  const summary = planned
    ? `今日计划 ${planned}，已完成 ${phases.done}`
    : "今天还没有排期，去任务页拖一段上来。";
  const kpis: { value: number; label: string }[] = [
    { value: pools.todo, label: "待办" },
    { value: pools.done, label: "完成" },
    { value: pools.archived, label: "归档" },
  ];

  return (
    // 吸附矩形按规范内缩滚动容器的 padding（main 的 p-6）：top/-mt/pt 三者用
    // 同一 24px 抵消，吸附时头部贴住 main 真顶（无透底缝），静止时视觉位置不变。
    <motion.header className="sticky top-[-24px] -mt-6 z-20 -mx-6 bg-[var(--bg)] px-6 pt-6 pb-2.5">
      <div className="flex items-end justify-between gap-6">
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
          className="flex shrink-0 items-baseline"
          style={{ gap: kpiGap }}
        >
          {kpis.map((k) => (
            <div key={k.label} className="text-right">
              <motion.div
                className="num font-semibold leading-none text-[var(--t1)]"
                style={{ fontSize: kpiSize }}
              >
                {k.value}
              </motion.div>
              <motion.div
                className="overflow-hidden text-xs text-[var(--t4)]"
                style={{ opacity: labelOpacity, height: labelHeight }}
              >
                {k.label}
              </motion.div>
            </div>
          ))}
        </motion.div>
      </div>
      {/* 收缩后浮现的顶栏分隔线（独立元素做透明度，避开 CSS 变量插值） */}
      <motion.div
        aria-hidden
        className="mt-1 h-px bg-[var(--edge)]"
        style={{ opacity: borderOpacity }}
      />
    </motion.header>
  );
}
