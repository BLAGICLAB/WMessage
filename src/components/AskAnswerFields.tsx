// 提问卡作答区：选项行（单选）+「其他回答」行。聊天提问卡与通知页问题卡共用。
//
// 点选与输入互斥：点选项清空输入，输入即取消点选——「其他回答」行就是最后
// 一个选项，选中态跟着走，互斥看得见。作答取值输入优先（answerText）。

import { useState } from "react";
import { PenLine } from "lucide-react";

export function useAskAnswer() {
  const [picked, setPicked] = useState<string | null>(null);
  const [text, setText] = useState("");
  return {
    picked,
    text,
    answerText: text.trim() || picked,
    pick: (opt: string) => {
      setPicked(opt);
      setText("");
    },
    type: (value: string) => {
      setText(value);
      setPicked(null);
    },
  };
}

type AskAnswer = ReturnType<typeof useAskAnswer>;

// nm-inset = 选中（凹陷）、nm-outset = 未选（抬升，自带 hover）——设计系统的选中语义
const row =
  "flex w-full items-start gap-2 rounded-[var(--r-sm)] px-3 py-1.5 text-left text-xs";
const rowOn = "nm-inset border-[var(--brand)] text-[var(--t1)]";
const rowOff = "nm-outset text-[var(--t2)]";

function RadioDot({ on }: { on: boolean }) {
  return (
    <span
      aria-hidden
      className={`mt-px flex h-3.5 w-3.5 shrink-0 items-center justify-center rounded-full border ${
        on ? "border-[var(--brand)]" : "border-[var(--edge-strong)]"
      }`}
    >
      {on && <span className="h-1.5 w-1.5 rounded-full bg-[var(--brand)]" />}
    </span>
  );
}

export function AskAnswerFields({
  options,
  answer,
  busy,
  onSubmit,
}: {
  options: string[];
  answer: AskAnswer;
  busy: boolean;
  /** 输入行内按 Enter：有可交的回答、且不在输入法组词中才触发 */
  onSubmit: () => void;
}) {
  const typing = answer.text.trim().length > 0;
  return (
    <div className="mt-2 flex flex-col gap-1.5">
      {options.length > 0 && (
        <div role="radiogroup" aria-label="回答选项" className="flex flex-col gap-1.5">
          {options.map((opt) => {
            const on = answer.picked === opt;
            return (
              <button
                key={opt}
                type="button"
                role="radio"
                aria-checked={on}
                className={`${row} ${on ? rowOn : rowOff}`}
                disabled={busy}
                onClick={() => answer.pick(opt)}
              >
                <RadioDot on={on} />
                <span className="min-w-0 break-words">{opt}</span>
              </button>
            );
          })}
        </div>
      )}
      <label
        className={`${row} cursor-text items-center focus-within:[box-shadow:var(--focus-ring)] ${
          typing ? rowOn : rowOff
        }`}
      >
        <PenLine
          size={12}
          aria-hidden
          className={`shrink-0 ${typing ? "text-[var(--brand)]" : "text-[var(--t5)]"}`}
        />
        <input
          aria-label="自定义回答"
          className="min-w-0 flex-1 bg-transparent text-xs text-[var(--t1)] outline-none placeholder:text-[var(--t5)]"
          maxLength={200}
          placeholder={options.length ? "输入其他回答…" : "输入你的回答"}
          value={answer.text}
          disabled={busy}
          onChange={(e) => answer.type(e.target.value)}
          onKeyDown={(e) => {
            if (e.key !== "Enter" || e.nativeEvent.isComposing) return;
            e.preventDefault();
            if (answer.answerText) onSubmit();
          }}
        />
      </label>
    </div>
  );
}
