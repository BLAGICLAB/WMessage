import { useEffect, useRef, useState } from "react";
import type { ColumnId, Task } from "../../types";
import {
  DAY_END_MIN,
  DAY_START_MIN,
  addWindowMinutes,
  fmtMin,
  parsePlanDT,
  windowMinutesBetween,
} from "./week";

const DOW1 = "一二三四五六日";
const QUICK_DUR = [30, 60, 90, 120];

/** 计划时长（窗口分钟）：脏数据兜底 60 */
function planDuration(task: Task): number {
  if (!task.planStart || !task.planEnd) return 60;
  return windowMinutesBetween(task.planStart, task.planEnd) ?? 60;
}

/** 任务卡详情面板（毛玻璃，左侧文档流列）：标题/计划时间/备注/标签/子任务/截止
 *  + 完成（服务端 set_task_column 语义）+ 软删。所有编辑走最小 task_patch。 */
export function TaskDetailPanel({
  task,
  autoFocusTitle = false,
  onUpdate,
  onSetColumn,
  onDelete,
  onClose,
}: {
  task: Task;
  /** 新建任务打开时聚焦标题并全选（自动进入编辑态） */
  autoFocusTitle?: boolean;
  onUpdate: (patch: Partial<Task>) => void;
  onSetColumn: (col: ColumnId) => void;
  onDelete: () => void;
  onClose: () => void;
}) {
  // 标题本地草稿：任务切换/外部改名时回填，blur/Enter 提交，Esc 还原
  const [titleDraft, setTitleDraft] = useState(task.title);
  // Esc 还原路径：setState 异步而 blur 同步读旧草稿——用标志跳过一次提交
  const skipTitleCommit = useRef(false);
  useEffect(() => {
    setTitleDraft(task.title);
  }, [task.id, task.title]);
  // 备注/标签同样走草稿（避免每键一次 task_patch）
  const [noteDraft, setNoteDraft] = useState(task.note ?? "");
  useEffect(() => {
    setNoteDraft(task.note ?? "");
  }, [task.id, task.note]);
  const [tagsDraft, setTagsDraft] = useState((task.tags ?? []).join("，"));
  useEffect(() => {
    setTagsDraft((task.tags ?? []).join("，"));
  }, [task.id, task.tags]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && !e.isComposing) onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  const dur = planDuration(task);
  const startVal = task.planStart ?? "";
  const dueVal =
    task.due && parsePlanDT(task.due) ? task.due : task.due && /^\d{4}-\d{2}-\d{2}$/.test(task.due) ? `${task.due}T09:00` : "";

  const setPlan = (start: { day: Date; min: number } | null, duration = dur) => {
    if (!start) {
      onUpdate({ planStart: null, planEnd: null });
      return;
    }
    const end = addWindowMinutes(start.day, start.min, duration);
    onUpdate({
      planStart: build(start.day, start.min),
      planEnd: build(end.day, end.min),
    });
  };
  const build = (day: Date, min: number) => {
    const p = `${day.getFullYear()}-${String(day.getMonth() + 1).padStart(2, "0")}-${String(day.getDate()).padStart(2, "0")}T${fmtMin(min)}`;
    return p;
  };

  const commitTitle = () => {
    if (skipTitleCommit.current) {
      skipTitleCommit.current = false;
      setTitleDraft(task.title);
      return;
    }
    const t = titleDraft.trim();
    if (!t || t === task.title) {
      setTitleDraft(task.title);
      return;
    }
    onUpdate({ title: t });
  };

  const toggleSubtask = (sid: string) => {
    const next = (task.subtasks ?? []).map((s) =>
      s.id === sid ? { ...s, done: !s.done } : s,
    );
    onUpdate({ subtasks: next });
  };

  const addSubtask = (text: string) => {
    const t = text.trim();
    if (!t) return;
    onUpdate({
      subtasks: [
        ...(task.subtasks ?? []),
        { id: crypto.randomUUID(), text: t, done: false },
      ],
    });
  };

  return (
    <aside
      aria-label="任务详情"
      className="glass detail-panel relative z-10 w-[292px] shrink-0 overflow-y-auto p-4"
    >
      <button
        type="button"
        aria-label="关闭详情"
        onClick={onClose}
        className="nm-icon-btn absolute top-3 right-3 h-6 w-6 text-xs"
      >
        ✕
      </button>
      <input
        className="w-[calc(100%-30px)] border-none bg-transparent text-base font-semibold text-[var(--t1)] outline-none"
        value={titleDraft}
        autoFocus={autoFocusTitle}
        onFocus={(e) => autoFocusTitle && e.target.select()}
        onChange={(e) => setTitleDraft(e.target.value)}
        onBlur={commitTitle}
        onKeyDown={(e) => {
          if (e.key === "Enter") (e.target as HTMLInputElement).blur();
          if (e.key === "Escape") {
            skipTitleCommit.current = true;
            setTitleDraft(task.title);
            (e.target as HTMLInputElement).blur();
          }
        }}
        aria-label="任务标题"
      />

      <div className="mt-4 mb-2 text-[11px] font-medium tracking-wide text-[var(--t5)]">
        计划时间
      </div>
      <div className="rounded-xl border border-[var(--edge)] bg-[var(--surface)] p-3">
        <input
          type="datetime-local"
          className="num w-full rounded-lg border border-[var(--edge)] bg-[var(--input-bg)] px-2 py-1.5 text-xs text-[var(--t2)] outline-none focus:border-[var(--brand)]"
          value={startVal}
          onChange={(e) => {
            const v = e.target.value;
            const p = v ? parsePlanDT(v) : null;
            setPlan(p ? { day: p.day, min: Math.min(Math.max(p.min, DAY_START_MIN), DAY_END_MIN - 30) } : null);
          }}
          aria-label="计划开始"
        />
        <div className="mt-2.5 flex items-center">
          <button
            type="button"
            aria-label="时长减 30 分钟"
            className="h-6 w-6 rounded-l-lg border border-[var(--edge)] text-sm text-[var(--t3)] hover:bg-[var(--inset-bg)]"
            onClick={() => {
              const p = task.planStart ? parsePlanDT(task.planStart) : null;
              if (p) setPlan({ day: p.day, min: p.min }, Math.max(30, dur - 30));
            }}
          >
            −
          </button>
          <span className="num border-y border-[var(--edge)] px-2 py-1 text-center text-xs font-medium text-[var(--t1)]" style={{ minWidth: 62 }}>
            {dur} 分钟
          </span>
          <button
            type="button"
            aria-label="时长加 30 分钟"
            className="h-6 w-6 rounded-r-lg border border-[var(--edge)] text-sm text-[var(--t3)] hover:bg-[var(--inset-bg)]"
            onClick={() => {
              const p = task.planStart ? parsePlanDT(task.planStart) : null;
              if (p) setPlan({ day: p.day, min: p.min }, dur + 30);
            }}
          >
            ＋
          </button>
          <span className="num ml-auto text-[10px] text-[var(--t5)]">
            {task.planStart && task.planEnd
              ? `至 ${(() => {
                  const end = parsePlanDT(task.planEnd!);
                  return end
                    ? `${DOW1[(end.day.getDay() + 6) % 7]} ${fmtMin(end.min)}`
                    : "";
                })()}`
              : "未排期"}
          </span>
        </div>
        <div className="mt-2 flex gap-1.5">
          {QUICK_DUR.map((d) => (
            <button
              key={d}
              type="button"
              className={`num rounded-full border px-2 py-0.5 text-[11px] ${
                dur === d
                  ? "border-[var(--accent)] font-medium text-[var(--accent)]"
                  : "border-[var(--edge)] text-[var(--t4)]"
              }`}
              onClick={() => {
                const p = task.planStart ? parsePlanDT(task.planStart) : null;
                if (p) setPlan({ day: p.day, min: p.min }, d);
              }}
            >
              {d}
            </button>
          ))}
          <button
            type="button"
            className="ml-auto text-[11px] text-[var(--t5)] hover:text-[var(--t3)]"
            onClick={() => setPlan(null)}
          >
            清除计划
          </button>
        </div>
      </div>

      <div className="mt-4 mb-1.5 text-[11px] font-medium tracking-wide text-[var(--t5)]">
        备注
      </div>
      <textarea
        className="w-full resize-none rounded-lg border border-[var(--edge)] bg-[var(--input-bg)] px-2 py-1.5 text-xs leading-relaxed text-[var(--t2)] outline-none focus:border-[var(--brand)]"
        rows={3}
        value={noteDraft}
        placeholder="补充说明…"
        onChange={(e) => setNoteDraft(e.target.value)}
        onBlur={() => {
          const v = noteDraft.trim();
          if (v !== (task.note ?? "")) onUpdate({ note: v || undefined });
        }}
      />

      <div className="mt-3 mb-1.5 text-[11px] font-medium tracking-wide text-[var(--t5)]">
        标签
      </div>
      <input
        className="w-full rounded-lg border border-[var(--edge)] bg-[var(--input-bg)] px-2 py-1.5 text-xs text-[var(--t2)] outline-none focus:border-[var(--brand)]"
        value={tagsDraft}
        placeholder="逗号分隔，如：设计，改造"
        onChange={(e) => setTagsDraft(e.target.value)}
        onBlur={() => {
          const tags = tagsDraft
            .split(/[，,]/)
            .map((s) => s.trim())
            .filter(Boolean);
          if (tagsDraft !== (task.tags ?? []).join("，")) onUpdate({ tags });
        }}
        aria-label="标签"
      />

      <div className="mt-3 mb-1.5 text-[11px] font-medium tracking-wide text-[var(--t5)]">
        子任务
      </div>
      <div className="flex flex-col">
        {(task.subtasks ?? []).map((s) => (
          <button
            key={s.id}
            type="button"
            className={`flex items-center gap-2 py-1 text-left text-xs ${
              s.done ? "text-[var(--t5)] line-through" : "text-[var(--t3)]"
            }`}
            onClick={() => toggleSubtask(s.id)}
          >
            <span
              className={`h-3.5 w-3.5 shrink-0 rounded border ${
                s.done ? "border-[var(--success)] bg-[var(--success)]" : "border-[var(--edge-strong)]"
              }`}
              aria-hidden
            />
            {s.text}
          </button>
        ))}
        <input
          className="mt-1 w-full bg-transparent text-xs text-[var(--t2)] outline-none placeholder:text-[var(--t6)]"
          placeholder="+ 子任务，回车添加"
        onKeyDown={(e) => {
          if (e.key === "Enter" && !e.nativeEvent.isComposing) {
            addSubtask((e.target as HTMLInputElement).value);
            (e.target as HTMLInputElement).value = "";
          }
        }}
          aria-label="添加子任务"
        />
      </div>

      <div className="mt-3 mb-1.5 text-[11px] font-medium tracking-wide text-[var(--t5)]">
        截止时间
      </div>
      <input
        type="datetime-local"
        className="num w-full rounded-lg border border-[var(--edge)] bg-[var(--input-bg)] px-2 py-1.5 text-xs text-[var(--t2)] outline-none focus:border-[var(--brand)]"
        value={dueVal}
        onChange={(e) => onUpdate({ due: e.target.value || undefined })}
        aria-label="截止时间"
      />

      <div className="mt-5 flex gap-2">
        <button
          type="button"
          className="flex-1 rounded-lg bg-[var(--accent)] py-2 text-[13px] font-medium text-white dark:text-[#101228]"
          onClick={() => onSetColumn(task.column === "done" ? "todo" : "done")}
        >
          ✓ {task.column === "done" ? "恢复待办" : "标记完成"}
        </button>
        <button
          type="button"
          aria-label="删除任务"
          title="删除任务"
          className="nm-icon-btn w-9 border border-[var(--edge)] text-sm text-[var(--t5)] hover:text-[var(--danger)]"
          onClick={onDelete}
        >
          <svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
            <path d="M4 6.5h16M9.5 6.5V4.8a1.3 1.3 0 0 1 1.3-1.3h2.4a1.3 1.3 0 0 1 1.3 1.3v1.7M6.5 6.5l.9 13a1.5 1.5 0 0 0 1.5 1.4h6.2a1.5 1.5 0 0 0 1.5-1.4l.9-13" />
          </svg>
        </button>
      </div>
    </aside>
  );
}
