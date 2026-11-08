// ChatPanel 子模块：子 agent 单行摘要（ZCode 式 turn 行）。
//
// 「它叫什么 / 在干什么 / 干到哪了 / 花了多久」：状态图标 + 目标 + 轮数进度 + 运行
// 时长，宽时压进一行、窄栏按块换行（见 SubagentRowLine）。聊天内嵌 strip 与右侧栏
// agents 页共用这里的件——两处各写一次就是「改一处漏一处」的起点（工具状态标那回
// 已经踩过）。

import { Check, Loader2, Square, TriangleAlert, X } from "lucide-react";
import type { ReactNode } from "react";
import type { SubagentRun } from "./ContextRail";
import { fmtElapsed, useElapsed } from "./useElapsed";

/** 与首行文字等高的图标槽：外壳行声明 leading-relaxed，1.625em 恰是一行高——进度段
 *  折到标题下方时，状态图标与停止键仍对齐标题那一行，而不是落到几行的中间 */
export const FIRST_LINE_SLOT = "flex h-[1.625em] shrink-0 items-center";

/** 六态 → 图标。取消有自己的 ✕，失败与预算耗尽共用警示图标（同属「没成」，
 *  具体原因看 title 与右侧栏徽标）；都归「非运行」会让用户分不清是跑完了还是
 *  被撤了 */
const STATUS_ICON = {
  running: (
    <Loader2
      size={11}
      aria-hidden
      className="shrink-0 wmsg-activity text-[var(--brand)]"
    />
  ),
  done: (
    <Check
      size={11}
      aria-hidden
      className="shrink-0 text-[var(--success)]"
    />
  ),
  failed: (
    <TriangleAlert
      size={11}
      aria-hidden
      className="shrink-0 text-[var(--danger)]"
    />
  ),
  cancelled: (
    <X size={11} aria-hidden className="shrink-0 text-[var(--t5)]" />
  ),
} as const;

function statusOf(s: SubagentRun): keyof typeof STATUS_ICON {
  if (s.status === "cancelled") return "cancelled";
  if (s.status === "failed" || (s.error && !s.running)) return "failed";
  if (s.running) return "running";
  return "done";
}

/** 进度文案：只报已跑数——分母与主会话共用一套安全网，子 agent 行不单报。
 *  空（终态：守卫注销后读不到仪表）就不显示这一栏。 */
function roundsText(s: SubagentRun): string | null {
  return s.roundsUsed == null ? null : `${s.roundsUsed} 轮`;
}

function toolCallsText(s: SubagentRun): string | null {
  return s.toolCallsUsed == null ? null : `${s.toolCallsUsed} 次`;
}

/** 运行时长。在途每秒刷新，终态定死在 finishedAt-startedAt——
 *  终态还跳秒数等于让用户以为它还在跑。排队期没 startedAt，两端都没有就不显示。 */
function useRunSeconds(s: SubagentRun): number | null {
  const live = useElapsed(s.startedAt ?? undefined, s.running);
  const settled =
    s.finishedAt != null && s.startedAt != null
      ? Math.max(0, Math.floor((s.finishedAt - s.startedAt) / 1000))
      : null;
  return live ?? settled;
}

/** 进度段：轮数 + 工具调用数 + 运行时长。聊天内嵌行与详情页头共用这一段
 *  （两处各算一遍轮数文案，早晚有一边忘加分母）；右侧栏列表行只要用时——
 *  进度细节在详情页看，列表行塞满数字就成了仪表盘。
 *
 *  tail：外壳接在时长后面的尾巴（如「详情 ›」），与时长绑成一块——窄栏折行时
 *  两者一起走，尾巴不会单独落成一行。 */
export function SubagentProgress({
  run: s,
  tail,
}: {
  run: SubagentRun;
  tail?: ReactNode;
}) {
  const rounds = roundsText(s);
  const calls = toolCallsText(s);
  const secs = useRunSeconds(s);
  if (rounds == null && secs == null && calls == null && !tail) return null;
  return (
    <>
      {rounds && (
        <span className="shrink-0 font-mono text-[10px] text-[var(--t5)]">
          {rounds}
        </span>
      )}
      {calls && (
        <span
          className="shrink-0 font-mono text-[10px] text-[var(--t5)]"
          title="工具调用"
        >
          {calls}
        </span>
      )}
      {(secs != null || tail) && (
        <span className="flex shrink-0 items-center gap-1.5">
          {secs != null && (
            <span
              className="font-mono text-[10px] text-[var(--t5)]"
              title="本轮运行时长"
            >
              {fmtElapsed(secs)}
            </span>
          )}
          {tail}
        </span>
      )}
    </>
  );
}

/** 仅运行时长：右侧栏列表行的瘦身版（名称 + 用时 + 停止键），进度与
 *  逐步动作都在详情页。口径与 SubagentProgress 同源（useRunSeconds 一份）。 */
export function SubagentElapsed({ run: s }: { run: SubagentRun }) {
  const secs = useRunSeconds(s);
  if (secs == null) return null;
  return (
    <span
      className="shrink-0 font-mono text-[10px] text-[var(--t5)]"
      title="本轮运行时长"
    >
      {fmtElapsed(secs)}
    </span>
  );
}

/** 一行摘要的内容部分（不含点击/取消外壳）——外壳只管点击与取消，「一行里显示
 *  什么」在这里。children 是外壳追加在进度段末尾的尾巴（如「详情 ›」）。
 *
 *  窄栏放不下时按块换行：标题守住首行（长了截断，全文在外壳 title 里），进度段
 *  （轮数 / 次数 / 时长 + 尾巴）整块折到标题下方，段内再按项换行——否则一行硬塞，
 *  标题被挤成 0 宽、进度溢出卡片、被旁边的停止键压住。 */
export function SubagentRowLine({
  run: s,
  children,
}: {
  run: SubagentRun;
  children?: ReactNode;
}) {
  return (
    <>
      <span className={FIRST_LINE_SLOT}>{STATUS_ICON[statusOf(s)]}</span>
      <span className="flex min-w-0 flex-1 flex-wrap items-center gap-x-1.5 gap-y-0.5">
        <span className="min-w-0 flex-[1_1_8em] truncate text-[11px]">{s.title}</span>
        <span className="flex flex-wrap items-center gap-x-1.5">
          <SubagentProgress run={s} tail={children} />
        </span>
      </span>
    </>
  );
}

/** 在途行的取消钮。两个展示点共用——取消是用户对子 agent 唯一的控制手段，
 *  漏一处就等于那条路彻底够不着了。 */
export function SubagentCancelButton({
  run: s,
  onCancel,
  className = "",
}: {
  run: SubagentRun;
  onCancel: (id: string) => void;
  /** 外壳给的对齐类（聊天条带要钉在首行；右侧栏单行列表不用） */
  className?: string;
}) {
  if (!s.running) return null;
  return (
    <button
      type="button"
      title="取消这个子 agent"
      aria-label={`取消子 agent：${s.title}`}
      className={`shrink-0 text-[var(--t5)] hover:text-[var(--danger)] ${className}`}
      onClick={(e) => {
        // 行内取消不是「看详情」，别顺手把侧栏也切过去
        e.stopPropagation();
        onCancel(s.id);
      }}
    >
      <Square size={10} aria-hidden className="fill-current" />
    </button>
  );
}
