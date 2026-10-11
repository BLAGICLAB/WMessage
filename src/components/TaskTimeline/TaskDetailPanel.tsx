import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { emit } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import { WebviewWindow } from "@tauri-apps/api/webviewWindow";
import { Bot, FolderOpen, History, Paperclip } from "lucide-react";
import type { ColumnId, Task } from "../../types";
import {
  MAX_TASK_FILES,
  filesPatch,
  mergeFiles,
  taskFiles,
} from "../../lib/taskFiles";
import { formatCompletedAt } from "../../format";
import { handleCommandError } from "../../lib/errorHandler";
import { ActorAvatar } from "../ActorAvatar";
import { TracePanel } from "../TracePanel";
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

const pad2 = (n: number) => String(n).padStart(2, "0");
const toLocalDT = (day: Date, min: number) =>
  `${day.getFullYear()}-${pad2(day.getMonth() + 1)}-${pad2(day.getDate())}T${fmtMin(min)}`;

/** 任务卡详情面板（毛玻璃，左侧文档流列）。
 *  字段与旧看板卡（TodoCard）能力对齐：标题/备注/标签/子任务/文件绑定（多文件+
 *  文件夹，点开/复制/移除）/🤖 交给机器人/执行详情/截止时间/完成状态 + 本页新增的
 *  计划时间编辑与完成入口。所有编辑走最小 task_patch。 */
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
    task.due && parsePlanDT(task.due)
      ? task.due
      : task.due && /^\d{4}-\d{2}-\d{2}$/.test(task.due)
        ? `${task.due}T09:00`
        : "";

  const setPlan = (start: { day: Date; min: number } | null, duration = dur) => {
    if (!start) {
      onUpdate({ planStart: null, planEnd: null });
      return;
    }
    const end = addWindowMinutes(start.day, start.min, duration);
    onUpdate({
      planStart: toLocalDT(start.day, start.min),
      planEnd: toLocalDT(end.day, end.min),
    });
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

  /* ── 文件绑定（与 TodoCard 同通道：bind_files / open_file_path / copy_file_with_title） ── */
  const boundFiles = taskFiles(task);
  const [filesExpanded, setFilesExpanded] = useState(false);

  const pickFile = async () => {
    try {
      // 文件与文件夹不互斥：已绑文件夹也可继续添加文件
      const selected = await open({ multiple: true, directory: false });
      const paths = Array.isArray(selected)
        ? selected
        : typeof selected === "string"
          ? [selected]
          : [];
      if (paths.length === 0) return;
      // isDir 由 Rust 侧 fs::metadata 判定（前端无法 stat）
      const added = await invoke<{ path: string; isDir: boolean }[]>(
        "bind_files",
        { paths },
      );
      const { files, truncated } = mergeFiles(boundFiles, added);
      if (truncated)
        window.alert(`每个任务最多绑定 ${MAX_TASK_FILES} 个文件，超出部分已忽略`);
      if (files.length !== boundFiles.length) onUpdate(filesPatch(files));
    } catch (e) {
      handleCommandError(e, "pick file");
    }
  };

  const pickFolder = async () => {
    try {
      // 文件夹仍单选（最多一个文件夹）
      if (boundFiles.some((f) => f.isDir)) {
        window.alert("该任务已绑定文件夹，请先移除再重新绑定");
        return;
      }
      const selected = await open({ directory: true });
      if (typeof selected === "string") {
        const { files, truncated } = mergeFiles(boundFiles, [
          { path: selected, isDir: true },
        ]);
        if (truncated)
          window.alert(
            `每个任务最多绑定 ${MAX_TASK_FILES} 个文件，超出部分已忽略`,
          );
        if (files.length !== boundFiles.length) onUpdate(filesPatch(files));
      }
    } catch (e) {
      handleCommandError(e, "pick folder");
    }
  };

  const removeFile = (path: string) => {
    onUpdate(filesPatch(boundFiles.filter((f) => f.path !== path)));
  };

  // 点文件名直接打开：走 Rust open_file_path（前端 opener scope 限 $HOME/$APPDATA）
  const openOneFile = (path: string) => {
    invoke("open_file_path", { path }).catch((e) =>
      handleCommandError(e, "open file", { silent: true }),
    );
  };

  /* ── 🤖 交给机器人（与 TodoCard 同通道：execute-task 事件 + 唤起挂件窗口） ── */
  const runWithBot = () => {
    emit("execute-task", { id: task.id, title: task.title }).catch(() => {});
    WebviewWindow.getByLabel("widget")
      .then((w) => {
        if (w) {
          w.show()
            .then(() => w.setFocus())
            .catch(() => {});
        }
      })
      .catch(() => {});
  };

  // 执行痕迹弹层（TracePanel 自带 portal 到 body）
  const [traceOpen, setTraceOpen] = useState(false);

  // 复制反馈（惯例：原位瞬时换标 + aria-live 播报，~1.6s 还原）：
  // 按文件路径记 ok/err，成功「✓ 已复制」、失败「复制失败」
  const [copyState, setCopyState] = useState<Record<string, "ok" | "err">>({});
  const copyTimers = useRef<Record<string, ReturnType<typeof setTimeout>>>({});
  useEffect(
    () => () => {
      for (const t of Object.values(copyTimers.current)) clearTimeout(t);
    },
    [],
  );

  const basename = (p: string) => p.split(/[\\/]/).pop() || p;

  const copyOneFile = (path: string) => {
    invoke("copy_file_with_title", { path, title: task.title })
      .then(() => {
        setCopyState((m) => ({ ...m, [path]: "ok" }));
        clearTimeout(copyTimers.current[path]);
        copyTimers.current[path] = setTimeout(
          () =>
            setCopyState((m) => {
              const { [path]: _drop, ...rest } = m;
              return rest;
            }),
          1600,
        );
      })
      .catch((e) => {
        setCopyState((m) => ({ ...m, [path]: "err" }));
        clearTimeout(copyTimers.current[path]);
        copyTimers.current[path] = setTimeout(
          () =>
            setCopyState((m) => {
              const { [path]: _drop, ...rest } = m;
              return rest;
            }),
          1600,
        );
        handleCommandError(e, "copy_file_with_title", { silent: true });
      });
  };

  // 标题自适应高度：内容全部可见（换行展示，不截断）；Enter 提交语义不变
  const titleRef = useRef<HTMLTextAreaElement | null>(null);
  useEffect(() => {
    const el = titleRef.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${el.scrollHeight}px`;
  }, [titleDraft, task.id]);

  return (
    <aside
      aria-label="任务详情"
      className="glass detail-panel relative z-10 w-[292px] shrink-0 overflow-y-auto px-4 pt-6 pb-4"
    >
      <div className="flex items-start gap-2">
        <ActorAvatar bot={!!task.botAssigned || !!task.schedule} />
        <textarea
          ref={titleRef}
          rows={1}
          className="min-w-0 flex-1 resize-none overflow-hidden border-none bg-transparent text-base font-semibold leading-snug text-[var(--t1)] outline-none"
          value={titleDraft}
          autoFocus={autoFocusTitle}
          onFocus={(e) => autoFocusTitle && e.target.select()}
          onChange={(e) => setTitleDraft(e.target.value)}
          onBlur={commitTitle}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              (e.target as HTMLTextAreaElement).blur();
            }
            if (e.key === "Escape") {
              skipTitleCommit.current = true;
              setTitleDraft(task.title);
              (e.target as HTMLTextAreaElement).blur();
            }
          }}
          aria-label="任务标题"
        />
        <button
          type="button"
          aria-label="关闭详情"
          onClick={onClose}
          className="nm-icon-btn mt-0.5 h-6 w-6 shrink-0 text-xs"
        >
          ✕
        </button>
      </div>
      <p className="mt-1 pl-8 text-[10px] text-[var(--t5)]">
        {task.column === "done" && task.completedAt
          ? formatCompletedAt(task.completedAt)
          : task.botAssigned
            ? "机器人执行中…"
            : "未完成"}
      </p>

      <div className="mt-3 mb-2 text-[11px] font-medium tracking-wide text-[var(--t5)]">
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
            setPlan(
              p
                ? {
                    day: p.day,
                    min: Math.min(Math.max(p.min, DAY_START_MIN), DAY_END_MIN - 30),
                  }
                : null,
            );
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
          <span
            className="num border-y border-[var(--edge)] px-2 py-1 text-center text-xs font-medium text-[var(--t1)]"
            style={{ minWidth: 62 }}
          >
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

      <div className="mt-3 mb-1.5 text-[11px] font-medium tracking-wide text-[var(--t5)]">
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
                s.done
                  ? "border-[var(--success)] bg-[var(--success)]"
                  : "border-[var(--edge-strong)]"
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
        文件
      </div>
      {boundFiles.length > 0 ? (
        <div className="flex flex-col items-start gap-1" aria-live="polite">
          {(filesExpanded ? boundFiles : boundFiles.slice(0, 5)).map((f) => (
            <div
              key={f.path}
              className="nm-inset flex max-w-full items-center gap-1 px-2 py-0.5 text-xs"
            >
              <button
                type="button"
                className="max-w-40 truncate text-[var(--t4)] hover:text-[var(--t2)]"
                title={f.path}
                onClick={() => openOneFile(f.path)}
              >
                {f.isDir ? "📁 " : "📄 "}
                {basename(f.path)}
              </button>
              {copyState[f.path] ? (
                <span
                  className={`shrink-0 text-[10px] font-medium ${
                    copyState[f.path] === "ok"
                      ? "text-[var(--success)]"
                      : "text-[var(--danger)]"
                  }`}
                >
                  {copyState[f.path] === "ok" ? "✓ 已复制" : "复制失败"}
                </span>
              ) : (
                <button
                  type="button"
                  className="shrink-0 text-[10px] text-[var(--t5)] hover:text-[var(--t2)]"
                  title="复制到剪贴板"
                  onClick={() => copyOneFile(f.path)}
                >
                  复制
                </button>
              )}
              <button
                type="button"
                className="shrink-0 text-[var(--t5)] hover:text-[var(--danger)]"
                title="移除绑定"
                aria-label={`移除 ${basename(f.path)}`}
                onClick={() => removeFile(f.path)}
              >
                ×
              </button>
            </div>
          ))}
          {boundFiles.length > 5 && (
            <button
              type="button"
              className="self-start text-[11px] text-[var(--t5)] hover:text-[var(--t3)]"
              onClick={() => setFilesExpanded((v) => !v)}
            >
              {filesExpanded ? "收起" : `还有 ${boundFiles.length - 5} 个`}
            </button>
          )}
          <div className="flex items-center gap-2">
            {boundFiles.length < MAX_TASK_FILES && (
              <button
                type="button"
                className="nm-btn px-2 py-0.5 text-[11px] text-[var(--t4)]"
                title="继续绑定文件"
                onClick={pickFile}
              >
                ＋
              </button>
            )}
            {!boundFiles.some((f) => f.isDir) &&
              boundFiles.length < MAX_TASK_FILES && (
                <button
                  type="button"
                  className="nm-btn px-2 py-0.5 text-[11px] text-[var(--t4)]"
                  title="绑定文件夹"
                  onClick={pickFolder}
                >
                  <FolderOpen size={11} aria-hidden />
                </button>
              )}
            <button
              type="button"
              className="text-sm text-[var(--t5)] hover:text-[var(--danger)]"
              title="解绑全部文件"
              onClick={() => onUpdate(filesPatch([]))}
            >
              ×
            </button>
          </div>
        </div>
      ) : (
        <div className="flex items-center gap-2">
          <button
            type="button"
            className="nm-btn flex items-center gap-1 px-2 py-0.5 text-xs text-[var(--t4)]"
            onClick={pickFile}
          >
            <Paperclip size={11} aria-hidden /> 绑定文件
          </button>
          <button
            type="button"
            className="nm-btn flex items-center gap-1 px-2 py-0.5 text-xs text-[var(--t4)]"
            onClick={pickFolder}
          >
            <FolderOpen size={11} aria-hidden /> 绑定文件夹
          </button>
        </div>
      )}

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
          className="nm-btn flex w-9 items-center justify-center text-[var(--t3)]"
          title="交给机器人执行这张任务卡"
          aria-label="交给机器人"
          onClick={runWithBot}
        >
          <Bot size={14} aria-hidden />
        </button>
        <button
          type="button"
          className="nm-btn flex w-9 items-center justify-center text-[var(--t3)]"
          title="查看执行痕迹（工具调用时间线 / 文件 diff / 回滚）"
          aria-label="执行详情"
          onClick={() => setTraceOpen(true)}
        >
          <History size={14} aria-hidden />
        </button>
        <button
          type="button"
          aria-label="删除任务"
          title="删除任务"
          className="nm-icon-btn w-9 border border-[var(--edge)] text-sm text-[var(--t5)] hover:text-[var(--danger)]"
          onClick={onDelete}
        >
          <svg
            viewBox="0 0 24 24"
            width="14"
            height="14"
            fill="none"
            stroke="currentColor"
            strokeWidth="1.7"
            strokeLinecap="round"
            strokeLinejoin="round"
            aria-hidden
          >
            <path d="M4 6.5h16M9.5 6.5V4.8a1.3 1.3 0 0 1 1.3-1.3h2.4a1.3 1.3 0 0 1 1.3 1.3v1.7M6.5 6.5l.9 13a1.5 1.5 0 0 0 1.5 1.4h6.2a1.5 1.5 0 0 0 1.5-1.4l.9-13" />
          </svg>
        </button>
      </div>

      {traceOpen && (
        <TracePanel
          taskId={task.id}
          taskTitle={task.title}
          onClose={() => setTraceOpen(false)}
        />
      )}
    </aside>
  );
}
