// 会话内提问卡（ask_user）：聊天与任务执行会话共用。
//
// 状态源是 askStore（bot-ask / bot-ask-resolved 事件，实时与回放同源）；
// 本组件只负责呈现与应答——invoke `ask_respond`（与通知页问题卡同一条命令，
// 后端引擎同一把 oneshot 通道），终态由 bot-ask-resolved 事件驱动：
// answer（用户作答）/ assume|dismiss（按假设继续）/ expired（超时或停止未答）。
//
// 红线与通知页一致：忽略问题执行也能走——「按假设继续」是一等按钮，不是暗箱兜底。
// 答完收成「问题 → 答案」两行留到回合收尾，一轮多问时停靠区不被整卡堆高。

import { useState, useSyncExternalStore } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  Check,
  Clock,
  Loader2,
  MessageCircleQuestionMark,
  Plug,
  SkipForward,
  X,
} from "lucide-react";
import type { LucideIcon } from "lucide-react";
import * as askStore from "./askStore";
import type { AskState } from "./askStore";
import { AskAnswerFields, useAskAnswer } from "../AskAnswerFields";
import { respondTaskQuestion } from "../../lib/taskAsk";
import { handleCommandError } from "../../lib/errorHandler";

/** 订阅回调（模块级，引用稳定）：store 变了就通知 React，不反复重订阅 */
function subscribeStore(cb: () => void): () => void {
  return askStore.subscribeAsks(() => cb());
}

const EMPTY: AskState[] = [];

/** 本会话的提问卡列表（bot-ask / bot-ask-resolved 经 useChatStream 落 store） */
export function useAskCards(sessionId: string | null): AskState[] {
  return useSyncExternalStore(
    subscribeStore,
    () => (sessionId ? askStore.asksOf(sessionId) : EMPTY),
  );
}

const btnGhost =
  "rounded-[var(--r-sm)] px-3 py-1.5 text-xs text-[var(--t3)] hover:bg-[var(--hover-bg)] disabled:opacity-50";

/** 终态的图标与结论行。MCP 输入请求没有「假设」：未答一律按拒绝处理。 */
function resolvedView(
  ask: AskState,
  isElicit: boolean,
): { Icon: LucideIcon; tone: string; text: string } {
  const r = ask.resolved;
  const assumed = ask.assumption ? `：${ask.assumption}` : "";
  if (r?.action === "answer") {
    return {
      Icon: Check,
      tone: "text-[var(--success)]",
      text: isElicit ? "已提交（应答值不回显）" : r.answer || "已回答",
    };
  }
  if (r?.action === "expired") {
    return {
      Icon: Clock,
      tone: "text-[var(--t4)]",
      text: isElicit ? "未获应答，已按拒绝处理" : `未获回答，已按假设继续${assumed}`,
    };
  }
  if (isElicit) {
    return { Icon: X, tone: "text-[var(--t4)]", text: "已拒绝该输入请求" };
  }
  return { Icon: SkipForward, tone: "text-[var(--t4)]", text: `已按假设继续${assumed}` };
}

function ResolvedCard({ ask, isElicit }: { ask: AskState; isElicit: boolean }) {
  const { Icon, tone, text } = resolvedView(ask, isElicit);
  return (
    <div className="rounded-[var(--r-md)] px-3 py-1.5 nm-inset">
      <p className="flex items-start gap-1.5 text-xs text-[var(--t4)]">
        <Icon size={12} aria-hidden className={`mt-0.5 shrink-0 ${tone}`} />
        <span className="line-clamp-2 min-w-0 break-words" title={ask.question}>
          {ask.question}
        </span>
      </p>
      <p className="mt-0.5 break-words pl-[18px] text-xs text-[var(--t2)]">→ {text}</p>
    </div>
  );
}

/** order：一轮多问时的序号（单卡不传，不显示） */
export function AskCard({ ask, order }: { ask: AskState; order?: number }) {
  const answer = useAskAnswer();
  const [busy, setBusy] = useState(false);
  const isElicit = ask.source === "mcp_elicit";
  if (ask.resolved) return <ResolvedCard ask={ask} isElicit={isElicit} />;
  const answerText = answer.answerText;
  const respond = async (action: "answer" | "assume") => {
    setBusy(true);
    try {
      // MCP elicitation 桥接卡：应答走独立命令（不落 wfq 通知、无「按假设继续」
      // 语义——assume 分支传空文本 = 后端按拒绝处理）
      if (isElicit) {
        await invoke("elicit_respond", {
          qid: ask.questionId,
          text: action === "answer" ? answerText : "",
        });
        return;
      }
      await respondTaskQuestion(
        ask.questionId,
        action,
        action === "answer" ? (answerText || undefined) : undefined,
      );
    } catch (e) {
      handleCommandError(e, isElicit ? "应答 MCP 输入请求" : "回答提问");
    } finally {
      setBusy(false);
    }
  };
  const SourceIcon = isElicit ? Plug : MessageCircleQuestionMark;
  return (
    <div className="origin-bottom animate-[popover-in_var(--dur-pop)_var(--ease-out)] rounded-[var(--r-md)] px-3 py-2.5 nm-inset">
      <div className="flex items-center gap-1.5 text-[11px] text-[var(--t4)]">
        <SourceIcon size={12} aria-hidden className="shrink-0 text-[var(--brand)]" />
        <span>{isElicit ? "MCP 输入请求" : "需要你决定"}</span>
        {order !== undefined && <span className="ml-auto text-[var(--t5)]">第 {order} 问</span>}
      </div>
      <p className="mt-1.5 whitespace-pre-line break-words text-sm font-medium text-[var(--t1)]">
        {ask.question}
      </p>
      {ask.why && (
        <p className="mt-1 text-[11px] text-[var(--t5)]">问这个：{ask.why}</p>
      )}
      {ask.assumption && (
        <p className="mt-1 text-[11px] text-[var(--t5)]">
          未回答时按：{ask.assumption} 继续
        </p>
      )}
      <AskAnswerFields
        options={ask.options}
        answer={answer}
        busy={busy}
        onSubmit={() => void respond("answer")}
      />
      <div className="mt-2.5 flex items-center justify-end gap-2">
        <button
          type="button"
          className={btnGhost}
          disabled={busy}
          title={
            isElicit
              ? "忽略（拒绝该输入请求）"
              : ask.assumption
                ? `不回答，按 AI 假设继续：${ask.assumption}`
                : "不回答，按继续处理"
          }
          onClick={() => void respond("assume")}
        >
          {isElicit ? "忽略" : "按假设继续"}
        </button>
        <button
          type="button"
          className="nm-btn-brand flex items-center gap-1.5 px-3 py-1.5 text-xs font-medium"
          disabled={busy || !answerText}
          onClick={() => void respond("answer")}
        >
          {busy && <Loader2 size={12} aria-hidden className="wmsg-activity" />}
          {busy ? "提交中…" : "回答并继续"}
        </button>
      </div>
    </div>
  );
}
