// ChatPanel 子模块：子 agent 只读详情（右侧栏「子 Agent」工作面的详情页）。
//
// 只读：执行中的子 agent 边跑边看，不在这里给任何干预入口——在途能做的只有
// 取消，那留在列表行上（贴着状态图标，比藏在详情页深处更够得着）。

import { ArrowLeft, Check, Loader2, X } from "lucide-react";
import type { SubagentRun } from "./ContextRail";
import { SubagentProgress } from "./SubagentRowLine";
import { fmtElapsed, useElapsed } from "./useElapsed";

/** 一段可折叠的原文块。标题固定显示——子 agent 写了什么必须一眼看到，
 *  默认展开还是折叠由长度决定：短的（status/summary）直接摊开，长的
 *  （整份 Markdown 产物）折起来免得把下面的验收标准顶出视野。 */
function DetailBlock({
  label,
  text,
  defaultOpen,
}: {
  label: string;
  text: string;
  defaultOpen?: boolean;
}) {
  const long = text.length > 220;
  return (
    <details open={defaultOpen || !long} className="rounded-md bg-[var(--hover-bg)] px-2 py-1.5">
      <summary className="cursor-pointer select-none text-[10px] font-medium text-[var(--t5)]">
        {label}
      </summary>
      <pre className="mt-1 whitespace-pre-wrap break-words font-mono text-[10px] leading-relaxed text-[var(--t2)]">
        {text}
      </pre>
    </details>
  );
}

/** 把库里那段 JSON 摊成可读文本：对象取它最像结论的那个字段优先，
 *  其余原样 JSON.stringify。摊平是为了让用户不用在花括号里找答案。 */
function resultText(result: SubagentRun["result"]): string | null {
  if (result == null) return null;
  if (typeof result === "string") return result;
  if (Array.isArray(result)) return result.join("\n");
  const rec = result as Record<string, unknown>;
  const summary = rec.summary ?? rec.result ?? rec.answer;
  const rest = { ...rec };
  delete rest.summary;
  delete rest.result;
  delete rest.answer;
  const restKeys = Object.keys(rest);
  if (typeof summary === "string" && restKeys.length === 0) return summary;
  return JSON.stringify(result, null, 2);
}

function acceptanceLines(a: SubagentRun["acceptance"]): string | null {
  if (a == null) return null;
  if (typeof a === "string") return a;
  if (!Array.isArray(a)) return JSON.stringify(a, null, 2);
  if (a.length === 0) return null;
  return a
    .map((item, i) => `${i + 1}. ${typeof item === "string" ? item : JSON.stringify(item)}`)
    .join("\n");
}

function timeText(ms?: number | null): string {
  if (!ms) return "—";
  return new Date(ms).toLocaleString("zh-CN", { hour12: false });
}

/** 执行详情窗：每一步工具调用一行 `工具 · 预览 · 耗时`，窗口定高 5 行、
 *  超出滚动——逐步动作是点进详情才看的东西。数据新在头，最新一步永远
 *  在窗顶，不用拖着滚动条追直播。动作流事件即焚，useChatSubagents 只给
 *  在途行挂数据，终态行没有 activity，整块不渲染（历史回看走结果与 digest）。 */
function SubagentActivityLog({ run: s }: { run: SubagentRun }) {
  if (!s.activity || s.activity.length === 0) return null;
  return (
    <div className="rounded-md bg-[var(--hover-bg)] px-2 py-1.5">
      <p className="text-[10px] font-medium text-[var(--t5)]">执行详情</p>
      {/* max-h-20 = 80px：行高 leading-4（16px）× 5 行，第六行起进滚动 */}
      <div className="mt-1 max-h-20 overflow-y-auto">
        {s.activity.map((a, i) => (
          <div
            key={`${a.toolCallId ?? i}-${a.at}`}
            className="flex items-center gap-1 text-[10px] leading-4"
          >
            {a.ok === null ? (
              <Loader2
                size={9}
                aria-hidden
                className="shrink-0 wmsg-activity text-[var(--brand)]"
              />
            ) : a.ok ? (
              <Check size={9} aria-hidden className="shrink-0 text-[var(--success)]" />
            ) : (
              <X size={9} aria-hidden className="shrink-0 text-[var(--danger)]" />
            )}
            <span className="shrink-0 font-mono text-[var(--t5)]">{a.tool}</span>
            <span className="min-w-0 flex-1 truncate text-[var(--t5)]" title={a.preview}>
              {a.preview}
            </span>
            {a.ms != null && (
              <span className="shrink-0 font-mono text-[var(--t5)]">{a.ms}ms</span>
            )}
          </div>
        ))}
      </div>
    </div>
  );
}

export function SubagentDetailPanel({
  run: s,
  onBack,
}: {
  run: SubagentRun;
  onBack: () => void;
}) {
  const live = useElapsed(s.startedAt ?? undefined, s.running);
  const secs =
    live ??
    (s.finishedAt != null && s.startedAt != null
      ? Math.max(0, Math.floor((s.finishedAt - s.startedAt) / 1000))
      : null);
  const summary = resultText(s.result);
  const acceptance = acceptanceLines(s.acceptance);

  return (
    <div className="flex flex-col gap-1.5">
      <div className="flex items-center gap-1.5 px-1.5">
        <button
          type="button"
          onClick={onBack}
          aria-label="返回子 agent 列表"
          className="shrink-0 text-[var(--t5)] hover:text-[var(--t2)]"
        >
          <ArrowLeft size={13} aria-hidden />
        </button>
        <span className="text-[11px] font-medium text-[var(--t2)]">
          子 Agent 详情
        </span>
        <SubagentProgress run={s} />
      </div>

      <DetailBlock label="目标" text={s.title} defaultOpen />

      <div className="grid grid-cols-2 gap-x-2 gap-y-0.5 px-2 text-[10px] text-[var(--t5)]">
        <span>状态：{s.status}</span>
        <span>profile：{s.profile ?? "—"}</span>
        <span>开始：{timeText(s.startedAt)}</span>
        <span>结束：{timeText(s.finishedAt)}</span>
        <span>用时：{secs != null ? fmtElapsed(secs) : "—"}</span>
        <span className="truncate">trace：{s.traceId ?? "—"}</span>
      </div>

      <SubagentActivityLog run={s} />

      {summary != null && <DetailBlock label="结果" text={summary} defaultOpen />}
      {acceptance != null && <DetailBlock label="验收标准" text={acceptance} />}
      {s.contextSummary && (
        <DetailBlock label="上下文摘要" text={s.contextSummary} />
      )}
      {s.error && (
        <DetailBlock label="错误" text={s.error} defaultOpen />
      )}
    </div>
  );
}
