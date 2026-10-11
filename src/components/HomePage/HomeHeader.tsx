// 吸顶收缩头部（Sticky Compact Header）：滚动进度驱动字号与间距，收成
// 单行后钉在滚动容器顶栏，反向滚动经 spring 连续还原。
// 进度源 = main 滚动容器的 scrollY（0 → COLLAPSE_PX 线性映射 + spring 平滑）；
// 颜色不参与插值（motion 无法混 CSS 变量），分隔线用独立元素的透明度渐入。

import {
  motion,
  useReducedMotion,
  useScroll,
  useSpring,
  useTransform,
} from "framer-motion";
import { greetingOf, type PlanPhases } from "./homeData";

/** 从顶部滚动多少 px 完成收缩 */
const COLLAPSE_PX = 150;

const WEEKDAY = ["周日", "周一", "周二", "周三", "周四", "周五", "周六"];

export type HomeKpi = { value: number; label: string; danger?: boolean };

function dateChipOf(now: Date): string {
  return `${now.getMonth() + 1}月${now.getDate()}日 ${WEEKDAY[now.getDay()]}`;
}

/** 阶段条：已完成/进行中/待开始 三段（参考案例的 Prepar/Shooting 语言，色取应用语义色） */
function PhaseBar({ phases }: { phases: PlanPhases }) {
  const total = phases.done + phases.doing + phases.todo;
  const segs = [
    { n: phases.done, color: "var(--success)", label: "已完成" },
    { n: phases.doing, color: "var(--accent)", label: "进行中" },
    { n: phases.todo, color: "var(--plan-slate)", label: "待开始" },
  ];
  return (
    <div>
      <div className="flex gap-4 text-xs text-[var(--t4)]">
        {segs.map((s) => (
          <span key={s.label} className="flex items-center gap-1.5">
            <span
              aria-hidden
              className="inline-block h-1.5 w-1.5 rounded-full"
              style={{ background: s.color }}
            />
            {s.label} <b className="num font-semibold text-[var(--t2)]">{s.n}</b>
          </span>
        ))}
      </div>
      <div className="mt-1.5 flex h-2 gap-0.5 overflow-hidden rounded-full">
        {total === 0 ? (
          <div className="h-full flex-1 rounded-full bg-[var(--inset-bg)]" />
        ) : (
          segs.map((s) =>
            s.n === 0 ? null : (
              <div
                key={s.label}
                className="h-full rounded-full"
                style={{ background: s.color, width: `${(s.n / total) * 100}%` }}
              />
            ),
          )
        )}
      </div>
    </div>
  );
}

export function HomeHeader({
  phases,
  kpis,
  now,
  scrollRef,
  onNewTask,
}: {
  phases: PlanPhases;
  kpis: HomeKpi[];
  now: Date;
  scrollRef: React.RefObject<HTMLElement | null>;
  onNewTask: () => void;
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
  const phaseOpacity = useTransform(progress, [0, 1], [1, 0]);
  const phaseHeight = useTransform(progress, [0, 1], [36, 0]);
  const labelOpacity = useTransform(progress, [0, 1], [1, 0]);
  const labelHeight = useTransform(progress, [0, 1], [18, 0]);
  const borderOpacity = useTransform(progress, [0, 1], [0, 1]);

  const planned = phases.done + phases.doing + phases.todo;
  const dueCount = kpis.find((k) => k.label === "已逾期")?.value ?? 0;
  const botToday = kpis.find((k) => k.label === "机器人执行")?.value ?? 0;
  const summary = planned
    ? `今天排了 ${planned} 段计划、${dueCount} 个截止；机器人跑了 ${botToday} 次。`
    : "今天还没有排期，去任务页拖一段上来。";

  // 吸附矩形按规范内缩滚动容器的 padding（main 的 p-6）：top/-mt/pt 三者用
  // 同一 24px 抵消，吸附时头部贴住 main 真顶（无透底缝），静止时视觉位置不变。
  return (
    <motion.header className="sticky top-[-24px] -mt-6 z-20 -mx-6 bg-[var(--bg)] px-6 pt-6 pb-2.5">
      <div className="flex items-end justify-between gap-6">
        <div className="min-w-0">
          <div className="flex items-baseline gap-2.5">
            <motion.h1
              className="whitespace-nowrap font-semibold tracking-tight text-[var(--t1)]"
              style={{ fontSize: titleSize }}
            >
              {greetingOf(now)}，Blagic
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
            <button
              onClick={onNewTask}
              className="ml-2 text-[var(--brand)] underline-offset-2 hover:underline"
            >
              新建任务
            </button>
          </motion.p>
          <motion.div
            className="overflow-hidden"
            style={{ opacity: phaseOpacity, height: phaseHeight }}
          >
            <div className="pt-2">
              <PhaseBar phases={phases} />
            </div>
          </motion.div>
        </div>

        <motion.div
          className="flex shrink-0 items-baseline"
          style={{ gap: kpiGap }}
        >
          {kpis.map((k) => (
            <div key={k.label} className="text-right">
              <motion.div
                className="num font-semibold leading-none"
                style={{
                  fontSize: kpiSize,
                  color: k.danger && k.value > 0 ? "var(--danger)" : "var(--t1)",
                }}
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
