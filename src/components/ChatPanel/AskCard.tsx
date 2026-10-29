// 会话内提问卡（ask_user）：聊天与任务执行会话共用。
//
// 状态源是 askStore（bot-ask / bot-ask-resolved 事件，实时与回放同源）；
// 本组件只负责呈现与应答——invoke `ask_respond`（与通知页问题卡同一条命令，
// 后端引擎同一把 oneshot 通道），终态由 bot-ask-resolved 事件驱动：
// answer（用户作答）/ assume|dismiss（按假设继续）/ expired（超时或停止未答）。
//
// 红线与通知页一致：忽略问题执行也能走——「按假设继续」是一等按钮，不是暗箱兜底。

import { useState, useSyncExternalStore } from "react";
import * as askStore from "./askStore";
import type { AskState } from "./askStore";
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

const btnPrimary =
  "nm-btn rounded-[var(--r-sm)] px-3 py-1.5 text-xs text-[var(--t1)] disabled:opacity-50";
const btnGhost =
  "rounded-[var(--r-sm)] px-3 py-1.5 text-xs text-[var(--t3)] hover:bg-[var(--hover-bg)] disabled:opacity-50";

function ResolvedLine({ ask }: { ask: AskState }) {
  const r = ask.resolved;
  if (!r) return null;
  if (r.action === "answer") {
    return (
      <p className="mt-2 text-xs text-[var(--t2)]">
        ✅ 已回答{r.answer ? `：${r.answer}` : ""}
      </p>
    );
  }
  if (r.action === "expired") {
    return (
      <p className="mt-2 text-xs text-[var(--t4)]">
        未获回答，已按 AI 假设继续
      </p>
    );
  }
  return <p className="mt-2 text-xs text-[var(--t4)]">已按假设继续</p>;
}

export function AskCard({ ask }: { ask: AskState }) {
  const [picked, setPicked] = useState<string | null>(null);
  const [text, setText] = useState("");
  const [busy, setBusy] = useState(false);
  const resolved = ask.resolved;
  if (resolved) {
    return (
      <div className="rounded-[var(--r-md)] border border-[var(--border)] px-3 py-2 nm-inset">
        <p className="text-xs text-[var(--t4)]">{ask.question}</p>
        <ResolvedLine ask={ask} />
      </div>
    );
  }
  const options = ask.options;
  const answerText = text.trim() || picked;
  const respond = async (action: "answer" | "assume") => {
    setBusy(true);
    try {
      await respondTaskQuestion(
        ask.questionId,
        action,
        action === "answer" ? (answerText || undefined) : undefined,
      );
    } catch (e) {
      handleCommandError(e, "回答提问");
    } finally {
      setBusy(false);
    }
  };
  return (
    <div className="rounded-[var(--r-md)] border border-[var(--border)] px-3 py-2 nm-inset">
      <p className="text-sm text-[var(--t1)]">{ask.question}</p>
      {ask.why && (
        <p className="mt-1 text-[11px] text-[var(--t5)]">问这个：{ask.why}</p>
      )}
      {ask.assumption && (
        <p className="mt-1 text-[11px] text-[var(--t5)]">
          未回答时按：{ask.assumption} 继续
        </p>
      )}
      {options.length > 0 && (
        <div className="mt-2 flex flex-wrap gap-1.5">
          {options.map((opt) => (
            <button
              key={opt}
              type="button"
              aria-pressed={picked === opt}
              className={`rounded-full px-3 py-1 text-xs ${
                picked === opt
                  ? "nm-outset text-[var(--t1)]"
                  : "nm-inset text-[var(--t3)]"
              }`}
              disabled={busy}
              onClick={() => {
                setPicked(opt);
                setText("");
              }}
            >
              {opt}
            </button>
          ))}
        </div>
      )}
      <input
        aria-label="自定义回答"
        className="mt-2 w-full rounded-[var(--r-sm)] px-3 py-1.5 text-xs nm-inset text-[var(--t1)] outline-none placeholder:text-[var(--t5)]"
        maxLength={200}
        placeholder={options.length ? "或输入其他回答" : "输入你的回答"}
        value={text}
        disabled={busy}
        onChange={(e) => {
          setText(e.target.value);
          setPicked(null);
        }}
      />
      <div className="mt-2 flex items-center justify-end gap-2">
        <button
          type="button"
          className={btnGhost}
          disabled={busy}
          title={
            ask.assumption
              ? `不回答，按 AI 假设继续：${ask.assumption}`
              : "不回答，按继续处理"
          }
          onClick={() => void respond("assume")}
        >
          按假设继续
        </button>
        <button
          type="button"
          className={btnPrimary}
          disabled={busy || !answerText}
          onClick={() => void respond("answer")}
        >
          {busy ? "提交中…" : "回答并继续"}
        </button>
      </div>
    </div>
  );
}
