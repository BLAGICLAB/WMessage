// ChatPanel 子模块：子 agent 单行摘要（ZCode 式 turn 行）。
//
// 「它叫什么 / 在干什么 / 干到哪了 / 花了多久」压进一行：状态图标 + 目标 + 轮数
// 进度 + 运行时长。聊天内嵌 strip 与右侧栏 agents 页共用这一份组件——两处各写
// 一次就是「改一处漏一处」的起点（工具状态标那回已经踩过）。

import { Check, Loader2, Square, TriangleAlert, X } from "lucide-react";
import type { SubagentRun } from "./ContextRail";
import { fmtElapsed, useElapsed } from "./useElapsed";

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
 *  进度细节在详情页看，列表行塞满数字就成了仪表盘。 */
export function SubagentProgress({ run: s }: { run: SubagentRun }) {
  const rounds = roundsText(s);
  const calls = toolCallsText(s);
  const secs = useRunSeconds(s);
  if (rounds == null && secs == null && calls == null) return null;
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
          title="工具调用 / 预算"
        >
          {calls}
        </span>
      )}
      {secs != null && (
        <span
          className="shrink-0 font-mono text-[10px] text-[var(--t5)]"
          title="本轮运行时长"
        >
          {fmtElapsed(secs)}
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

/** 一行摘要的内容部分（不含点击/取消外壳）——两个展示点各自套自己的外壳，
 *  只共用「一行里显示什么」。 */
export function SubagentRowLine({ run: s }: { run: SubagentRun }) {
  return (
    <>
      {STATUS_ICON[statusOf(s)]}
      <span className="min-w-0 flex-1 truncate text-[11px]">{s.title}</span>
      <SubagentProgress run={s} />
    </>
  );
}

/** 在途行的取消钮。两个展示点共用——取消是用户对子 agent 唯一的控制手段，
 *  漏一处就等于那条路彻底够不着了。 */
export function SubagentCancelButton({
  run: s,
  onCancel,
}: {
  run: SubagentRun;
  onCancel: (id: string) => void;
}) {
  if (!s.running) return null;
  return (
    <button
      type="button"
      title="取消这个子 agent"
      aria-label={`取消子 agent：${s.title}`}
      className="shrink-0 text-[var(--t5)] hover:text-[var(--danger)]"
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
