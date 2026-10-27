// ChatPanel 子模块：执行计划浮层（聊天区右上角开关打开）。
//
// 三段式，各答一个问题，刻意不合并：
// - 计划：模型自报的执行进度（update_plan 工具写入，后端整表替换）
// - 后台任务：子 agent 在途与终态（复用右侧栏同一份运行列表）
// - 环境信息：本会话动过的文件概览（会话口径累计 ±行，不是 git 差异）；
//   完整清单在右侧栏文件页的「改动」子页——同一份数据不画两遍
//
// 「计划」与「后台任务」是两套机制：前者答「做到哪」，后者答「什么还在跑」，
// 混成一段会让人分不清是进度还是并发。

import { useEffect, useRef, useState } from "react";
import { Bot, Check, Circle, Loader2, X } from "lucide-react";
import { formatMonthDayTime } from "../../format";
import { usePlanState } from "./usePlanState";
import type { PlanStepStatus } from "./planStore";
import type { SubagentRun } from "./ContextRail";
import { SUBAGENT_STATUS } from "./constants";
import {
  sessionFileChangeList,
  type SessionFileChangeItem,
} from "../../lib/sessionFileChanges";
import { formatCommandError, handleCommandError } from "../../lib/errorHandler";
import { useTauriListen } from "../../lib/useTauriListen";

type PlanPanelProps = {
  sessionId: string | null;
  /** 子 agent 运行列表（与右侧栏同一份；在途 + 刚结束） */
  subagents: SubagentRun[];
  onClose: () => void;
  /** 更上层浮层开着（命令面板等）：Esc 让位给它，面板不动 */
  escBlocked?: boolean;
  /** 跳到右侧栏文件页的「改动」子页（完整清单在那里，不在浮层里重画） */
  onShowFileChanges?: () => void;
};

export function PlanPanel({
  sessionId,
  subagents,
  onClose,
  escBlocked,
  onShowFileChanges,
}: PlanPanelProps) {
  const { plan, stale } = usePlanState(sessionId);
  /** 文件清单带上会话 id：切会话时靠它派生「不属于当前会话 → 不显示」 */
  const [fileState, setFileState] = useState<{
    sid: string | null;
    items: SessionFileChangeItem[];
    error: string | null;
  }>({ sid: null, items: [], error: null });
  const [fileTick, setFileTick] = useState(0);
  const sessionIdRef = useRef(sessionId);
  useEffect(() => {
    sessionIdRef.current = sessionId;
  }, [sessionId]);
  // 落盘事件 → 清单要跟着长（口径在后端，前端不自己累加）
  useTauriListen<{ sessionId?: string | null }>("bot-file-changed", (payload) => {
    const sid = payload.sessionId ?? null;
    if (sid && sid === sessionIdRef.current) setFileTick((t) => t + 1);
  });
  useEffect(() => {
    if (!sessionId) return;
    let alive = true;
    sessionFileChangeList(sessionId)
      .then((r) => {
        if (!alive) return;
        setFileState({ sid: sessionId, items: r?.items ?? [], error: null });
      })
      .catch((e) => {
        if (!alive) return;
        handleCommandError(e, "读取会话文件改动", { silent: true });
        setFileState({
          sid: sessionId,
          items: [],
          error: formatCommandError(e) || "读取失败",
        });
      });
    return () => {
      alive = false;
    };
  }, [sessionId, fileTick]);

  // Esc 关闭：更上层浮层开着时让位——否则按一次 Esc 会把命令面板一起关掉
  const closeRef = useRef(onClose);
  const escCtxRef = useRef({ escBlocked: !!escBlocked });
  useEffect(() => {
    closeRef.current = onClose;
    escCtxRef.current = { escBlocked: !!escBlocked };
  });
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape" || e.isComposing) return;
      if (escCtxRef.current.escBlocked) return;
      closeRef.current();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  const runningCount = subagents.filter((s) => s.running).length;
  // 别的会话的清单不显示（切会话瞬间旧数据不能闪一下）
  const fileItems = fileState.sid === sessionId ? fileState.items : [];
  const fileError = fileState.sid === sessionId ? fileState.error : null;
  const added = fileItems.reduce((n, i) => n + i.added, 0);
  const deleted = fileItems.reduce((n, i) => n + i.deleted, 0);

  return (
    <aside
      className="glass flex max-h-[70vh] w-[320px] flex-col rounded-xl p-3"
      aria-label="执行计划面板"
    >
      <header className="mb-2 flex shrink-0 items-center justify-between">
        <span className="text-[11px] font-medium text-[var(--t2)]">执行计划</span>
        <button
          type="button"
          className="nm-icon-btn h-5 w-5"
          aria-label="关闭计划面板"
          onClick={onClose}
        >
          <X size={12} aria-hidden />
        </button>
      </header>

      <div className="min-h-0 flex-1 space-y-3 overflow-y-auto">
        {/* 计划 */}
        <section aria-label="计划">
          <div className="mb-1 flex items-center gap-1.5">
            <span className="text-[11px] font-medium text-[var(--t2)]">计划</span>
            {plan && plan.steps.length > 0 && (
              <span className="text-[10px] text-[var(--t4)]">
                {plan.steps.length} 步
              </span>
            )}
          </div>
          {!plan || plan.steps.length === 0 ? (
            <p className="text-[11px] text-[var(--t4)]">
              本会话暂无计划（模型调用 update_plan 后显示）
            </p>
          ) : (
            <>
              <ol className="space-y-1">
                {plan.steps.map((s, i) => (
                  <li
                    // 键带序号防同文步骤撞 key（步骤文本由模型写，不天然唯一；
                    // 整表替换下位置会变，重建行无副作用）
                    // oxlint-disable-next-line react/no-array-index-key
                    key={`${i}-${s.content}`}
                    className="flex items-start gap-1.5"
                  >
                    <StepIcon status={s.status} />
                    <span
                      className={`min-w-0 flex-1 text-xs ${
                        s.status === "completed"
                          ? "text-[var(--t4)] line-through"
                          : s.status === "in_progress"
                            ? "text-[var(--t1)]"
                            : "text-[var(--t3)]"
                      }`}
                    >
                      {s.content}
                    </span>
                  </li>
                ))}
              </ol>
              {stale && (
                <p className="mt-1 text-[10px] text-[var(--plan-amber)]">
                  本轮尚未更新，状态可能已过期
                </p>
              )}
              {plan.explanation && (
                <p className="mt-1 text-[10px] text-[var(--t4)]">
                  变更：{plan.explanation}
                </p>
              )}
              <p className="mt-1 text-[10px] text-[var(--t5)]">
                更新于 {plan.updatedAt ? formatMonthDayTime(plan.updatedAt) : "—"}
              </p>
            </>
          )}
        </section>

        {/* 后台任务 */}
        <section aria-label="后台任务">
          <div className="mb-1 flex items-center gap-1.5">
            <span className="text-[11px] font-medium text-[var(--t2)]">
              后台任务
            </span>
            {runningCount > 0 && (
              <span className="text-[10px] text-[var(--brand)]">
                {runningCount} 个执行中
              </span>
            )}
          </div>
          {subagents.length === 0 ? (
            <p className="text-[11px] text-[var(--t4)]">没有后台任务</p>
          ) : (
            <ul className="space-y-0.5">
              {subagents.map((s) => (
                <li key={s.id} className="flex items-center gap-1.5 text-xs">
                  {s.running ? (
                    <Loader2
                      size={12}
                      aria-hidden
                      className="wmsg-activity shrink-0 text-[var(--brand)]"
                    />
                  ) : (
                    <Bot
                      size={12}
                      aria-hidden
                      className={`shrink-0 ${SUBAGENT_STATUS[s.status].color}`}
                    />
                  )}
                  <span className="min-w-0 flex-1 truncate text-[var(--t3)]">
                    {s.title}
                  </span>
                  <span
                    className={`shrink-0 text-[10px] ${SUBAGENT_STATUS[s.status].color}`}
                  >
                    {SUBAGENT_STATUS[s.status].label}
                  </span>
                </li>
              ))}
            </ul>
          )}
        </section>

        {/* 环境信息：只报概览。完整清单在右侧栏文件页的「改动」子页——
            同一份数据两处渲染，改一处就漏一处 */}
        <section aria-label="环境信息">
          <div className="mb-1 flex items-center gap-1.5">
            <span className="text-[11px] font-medium text-[var(--t2)]">
              环境信息
            </span>
            {fileItems.length > 0 && (
              <span className="text-[10px] text-[var(--t4)]">
                {fileItems.length} 个文件
              </span>
            )}
          </div>
          {fileError ? (
            <p className="text-[11px] text-[var(--danger)]">{fileError}</p>
          ) : fileItems.length === 0 ? (
            <p className="text-[11px] text-[var(--t4)]">本会话没有文件改动</p>
          ) : (
            <>
              <p
                className="text-[11px] text-[var(--t3)]"
                title="会话口径（累计改动，非 git 差异）"
              >
                累计{" "}
                <span className="text-[var(--success)]">+{added}</span> /{" "}
                <span className="text-[var(--danger)]">-{deleted}</span> 行
              </p>
              {onShowFileChanges && (
                <button
                  type="button"
                  className="mt-1 text-[11px] text-[var(--brand)] hover:underline"
                  onClick={onShowFileChanges}
                >
                  在右侧栏看改动清单 →
                </button>
              )}
            </>
          )}
        </section>
      </div>
    </aside>
  );
}

/** 步骤状态图标：完成=勾，进行中=转子，未开始=空心圈 */
function StepIcon({ status }: { status: PlanStepStatus }) {  if (status === "completed") {
    return (
      <Check
        size={12}
        aria-hidden
        className="mt-0.5 shrink-0 text-[var(--plan-green)]"
      />
    );
  }
  if (status === "in_progress") {
    return (
      <Loader2
        size={12}
        aria-hidden
        className="wmsg-activity mt-0.5 shrink-0 text-[var(--brand)]"
      />
    );
  }
  return (
    <Circle size={12} aria-hidden className="mt-0.5 shrink-0 text-[var(--t5)]" />
  );
}
