import { useState } from "react";
import { Check, Loader2, Pencil, X } from "lucide-react";
import type { Clarification, ClarifyQuestion } from "../../lib/workflowAsk";

/** 澄清卡组（ 设计 §3.4，）：hero 态内联问答。 *  红线：**「开始拆解」永远可点**——未答题自动落 AI 假设（q.default），
 *  忽略问题也能走；已答卡折叠为摘要行可重开编辑。 */
export function ClarifyCard({
  goal,
  questions,
  previousAnswers,
  busy,
  onSubmit,
  onCancel,
}: {
  goal: string;
  questions: ClarifyQuestion[];
  /** 重拆预填：按问题文本匹配上次的回答 */
  previousAnswers?: Record<string, string>;
  busy: boolean;
  /** 提交澄清（已答 + 未答落假设）；clarifications 空 = 跳过，按 AI 假设拆解 */
  onSubmit: (clarifications: Clarification[]) => void;
  onCancel: () => void;
}) {
  const [answers, setAnswers] = useState<Record<string, string>>(() => {
    const init: Record<string, string> = {};
    for (const q of questions) {
      const prev = previousAnswers?.[q.question];
      if (prev) init[q.id] = prev;
    }
    return init;
  });
  const [editing, setEditing] = useState<Record<string, boolean>>({});
  const unanswered = questions.filter((q) => !answers[q.id]?.trim());

  const submit = () => {
    const clarifications: Clarification[] = [];
    for (const q of questions) {
      const a = answers[q.id]?.trim() || q.default;
      if (a) clarifications.push({ question: q.question, answer: a });
    }
    onSubmit(clarifications);
  };

  return (
    <div className="flex h-full items-center justify-center">
      <div className="nm-card w-full max-w-xl p-6">
        <h2 className="text-base font-semibold text-[var(--t1)]">拆解前确认</h2>
        <p className="mt-1 text-xs text-[var(--t5)]">
          AI 有 {questions.length} 个问题，回答后拆解更准。目标：
          <span className="text-[var(--t3)]">{goal}</span>
        </p>
        <div className="mt-4 space-y-3">
          {questions.map((q) => (
            <QuestionRow
              key={q.id}
              q={q}
              answer={answers[q.id] ?? ""}
              collapsed={!!answers[q.id]?.trim() && !editing[q.id]}
              onAnswer={(a) => setAnswers((prev) => ({ ...prev, [q.id]: a }))}
              onToggleEdit={() => setEditing((prev) => ({ ...prev, [q.id]: true }))}
            />
          ))}
        </div>
        <div className="mt-5 flex items-center justify-end gap-2">
          {busy ? (
            <span className="flex items-center gap-1.5 text-xs text-[var(--t5)]">
              <Loader2 size={13} className="animate-spin" aria-hidden /> AI 拆解中…
            </span>
          ) : (
            <>
              <button
                className="rounded-[var(--r-sm)] px-3 py-1.5 text-xs text-[var(--t5)] hover:text-[var(--t3)]"
                onClick={onCancel}
              >
                取消
              </button>
              <button
                className="nm-outset rounded-[var(--r-sm)] px-3 py-1.5 text-xs text-[var(--t3)]"
                title="不回答任何问题，全部按 AI 的假设拆解"
                onClick={() => onSubmit([])}
              >
                跳过，按 AI 假设拆解
              </button>
              <button
                className="nm-inset rounded-[var(--r-sm)] px-4 py-1.5 text-sm text-[var(--t1)]"
                title="未回答的问题将采用 AI 假设"
                onClick={submit}
              >
                开始拆解 →
              </button>
            </>
          )}
        </div>
        {!busy && unanswered.length > 0 && (
          <p className="mt-2 text-right text-[11px] text-[var(--t5)]">
            {unanswered.length} 题未答，将采用 AI 假设
          </p>
        )}
      </div>
    </div>
  );
}

/** 单题四件套：问题文本 / why（小灰字）/ 选项 chips + 自由输入 / 「用 AI 的假设」 */
function QuestionRow({
  q,
  answer,
  collapsed,
  onAnswer,
  onToggleEdit,
}: {
  q: ClarifyQuestion;
  answer: string;
  collapsed: boolean;
  onAnswer: (a: string) => void;
  onToggleEdit: () => void;
}) {
  const [customOpen, setCustomOpen] = useState(false);
  if (collapsed) {
    return (
      <div className="flex items-center gap-2 rounded-[var(--r-sm)] nm-inset px-3 py-2 text-xs">
        <Check size={13} className="shrink-0 text-[var(--ok,#16a34a)]" aria-hidden />
        <span className="truncate text-[var(--t3)]">
          {q.question} → <span className="text-[var(--t1)]">{answer}</span>
        </span>
        <button
          aria-label={`修改回答：${q.question}`}
          className="ml-auto shrink-0 text-[var(--t5)] hover:text-[var(--t1)]"
          onClick={onToggleEdit}
        >
          <Pencil size={12} aria-hidden />
        </button>
      </div>
    );
  }
  const has = !!answer.trim();
  return (
    <div className="rounded-[var(--r-sm)] nm-inset p-3">
      <p className="text-sm text-[var(--t1)]">{q.question}</p>
      {q.why && <p className="mt-0.5 text-[11px] text-[var(--t5)]">问这个：{q.why}</p>}
      {q.options.length > 0 && (
        <div className="mt-2 flex flex-wrap gap-1.5">
          {q.options.map((opt) => (
            <button
              key={opt}
              aria-pressed={answer === opt}
              className={`rounded-full px-3 py-1 text-xs ${
                answer === opt
                  ? "nm-outset text-[var(--t1)]"
                  : "text-[var(--t3)] opacity-70 hover:opacity-100"
              }`}
              onClick={() => onAnswer(opt)}
            >
              {opt}
            </button>
          ))}
          {!customOpen && (
            <button
              className="rounded-full px-3 py-1 text-xs text-[var(--t5)] hover:text-[var(--t3)]"
              onClick={() => setCustomOpen(true)}
            >
              其他…
            </button>
          )}
        </div>
      )}
      {(customOpen || q.options.length === 0) && (
        <input
          aria-label={`自定义回答：${q.question}`}
          className="mt-2 w-full rounded-[var(--r-sm)] px-3 py-1.5 text-xs nm-inset text-[var(--t1)] outline-none placeholder:text-[var(--t5)]"
          maxLength={200}
          placeholder="输入你的回答"
          value={answer}
          onChange={(e) => onAnswer(e.target.value)}
        />
      )}
      <div className="mt-2 flex items-center justify-between text-[11px]">
        {q.default ? (
          <button
            className="text-[var(--t5)] hover:text-[var(--t1)]"
            title="采用 AI 的推荐假设作为回答"
            onClick={() => onAnswer(q.default as string)}
          >
            ↳ 用 AI 的假设：{q.default}
          </button>
        ) : (
          <span />
        )}
        {has && (
          <span className="text-[var(--ok,#16a34a)]">
            已回答
            <X
              size={11}
              aria-hidden
              className="ml-1 inline cursor-pointer text-[var(--t5)] hover:text-[var(--danger,#ef4444)]"
              role="button"
              aria-label="清除回答"
              onClick={() => {
                onAnswer("");
                setCustomOpen(false);
              }}
            />
          </span>
        )}
      </div>
    </div>
  );
}
