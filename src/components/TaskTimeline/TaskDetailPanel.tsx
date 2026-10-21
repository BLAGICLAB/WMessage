import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Bot, FolderOpen, History, Paperclip } from "lucide-react";
import type { ColumnId, Task } from "../../types";
import { runWithBot } from "../../lib/botRun";
import { openFilePath } from "../../lib/openTarget";
import { useTaskFiles } from "../../lib/useTaskFiles";
import {
  MAX_TASK_FILES,
  filesPatch,
  taskFiles,
} from "../../lib/taskFiles";
import { DOW1, formatCompletedAt } from "../../format";
import { handleCommandError } from "../../lib/errorHandler";
import { ActorAvatar } from "../ActorAvatar";
import { Toggle } from "../Toggle/Toggle";
import { TracePanel } from "../TracePanel";
import { TaskFileChips } from "../task/TaskFileChips";
import { SubtaskRow } from "../task/SubtaskRow";
import {
  DAY_END_MIN,
  DAY_START_MIN,
  addWindowMinutes,
  buildPlanDT,
  fmtMin,
  parsePlanDT,
  windowMinutesBetween,
} from "./week";

const QUICK_DUR = [30, 60, 90, 120];

/** 计划时长（窗口分钟）：脏数据兜底 60 */
function planDuration(task: Task): number {
  if (!task.planStart || !task.planEnd) return 60;
  return windowMinutesBetween(task.planStart, task.planEnd) ?? 60;
}

/** 任务卡详情面板（毛玻璃内容体，由 TaskEditorOverlay 悬浮宿主承载）。
 *  字段与旧看板卡（TodoCard）能力对齐：标题/备注/标签/子任务/文件绑定（多文件+
 *  文件夹，点开/复制/移除）/🤖 交给机器人/执行详情/截止时间/完成状态 + 计划时间
 *  编辑与完成入口。所有编辑走最小 task_patch。
 *
 *  关闭键左侧的拨杆决定关闭语义：开 = 保存关闭（提交草稿后关），关 = 不保存关闭
 *  （草稿不提交，已落库的改动由宿主按打开浮窗时的快照回滚）。拨杆不持久化，
 *  每次打开（新建/编辑/换一张卡）都是「开」。 */
export function TaskDetailPanel({
  task,
  autoFocusTitle = false,
  onUpdate,
  onSetColumn,
  onDelete,
  onClose,
  onDiscard,
  onCreateSubtaskCard,
  onOpenTask,
  onDeleteSubtask,
}: {
  task: Task;
  /** 新建任务打开时聚焦标题并全选（自动进入编辑态） */
  autoFocusTitle?: boolean;
  onUpdate: (patch: Partial<Task>) => void;
  onSetColumn: (col: ColumnId) => void;
  onDelete: () => void;
  onClose: () => void;
  /** 不保存关闭：宿主按打开浮窗时的快照回滚本次改动（必接——否则「不保存」名不副实） */
  onDiscard: () => void;
  /** 子任务行一键生成子任务卡（按钮悬停显现；接线缺省 = 不显示按钮） */
  onCreateSubtaskCard?: (subId: string, title: string) => void;
  /** 子任务行 ↗ 跳转到已生成的任务卡 */
  onOpenTask?: (id: string) => void;
  /** 子任务行 × 删除该行；已生成子任务卡时由接线方解除关联并保留子卡 */
  onDeleteSubtask?: (subId: string) => void;
}) {
  // 标题本地草稿：任务切换/外部改名时回填，blur/Enter 提交，Esc 还原
  const [titleDraft, setTitleDraft] = useState(task.title);
  // 草稿的最新值另存 ref：提交动作发生在事件回调里，读到的是上一帧的 state 闭包。
  // Esc 还原草稿后紧接着的关闭提交必须读到「还原后」的值——读旧闭包会把刚还原掉的
  // 草稿又提交出去（既有的「Esc 还原不提交」会被打破）
  const titleDraftRef = useRef(task.title);
  // Esc 还原路径：setState 异步而 blur 同步读旧草稿——用标志跳过一次提交
  const skipTitleCommit = useRef(false);
  // 已提交值：落库是异步的，task prop 要等回包才更新。拿它当「有没有待提交」的
  // 判据，才不会把同一份草稿提交两次（blur 提交一次 + 关闭时再提交一次）
  const titleSaved = useRef(task.title);
  useEffect(() => {
    setTitleDraft(task.title);
    titleDraftRef.current = task.title;
    titleSaved.current = task.title;
  }, [task.id, task.title]);
  // 备注/标签同样走草稿（避免每键一次 task_patch）
  const [noteDraft, setNoteDraft] = useState(task.note ?? "");
  const noteDraftRef = useRef(task.note ?? "");
  const noteSaved = useRef(task.note ?? "");
  useEffect(() => {
    setNoteDraft(task.note ?? "");
    noteDraftRef.current = task.note ?? "";
    noteSaved.current = task.note ?? "";
  }, [task.id, task.note]);
  const [tagsDraft, setTagsDraft] = useState((task.tags ?? []).join("，"));
  const tagsDraftRef = useRef((task.tags ?? []).join("，"));
  const tagsSaved = useRef((task.tags ?? []).join("，"));
  useEffect(() => {
    setTagsDraft((task.tags ?? []).join("，"));
    tagsDraftRef.current = (task.tags ?? []).join("，");
    tagsSaved.current = (task.tags ?? []).join("，");
  }, [task.id, task.tags]);

  // 关闭拨杆：开 = 保存关闭，关 = 不保存关闭。每次打开都是「开」——换一张卡等于
  // 换一次打开（浮窗原地换内容），不继承上一张卡的拨杆状态
  const [saveOnClose, setSaveOnClose] = useState(true);
  useEffect(() => {
    setSaveOnClose(true);
  }, [task.id]);

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
      planStart: buildPlanDT(start.day, start.min),
      planEnd: buildPlanDT(end.day, end.min),
    });
  };

  const commitTitle = () => {
    if (skipTitleCommit.current) {
      skipTitleCommit.current = false;
      titleDraftRef.current = titleSaved.current;
      setTitleDraft(titleSaved.current);
      return;
    }
    const t = titleDraftRef.current.trim();
    if (!t || t === titleSaved.current) {
      titleDraftRef.current = titleSaved.current;
      setTitleDraft(titleSaved.current);
      return;
    }
    titleSaved.current = t;
    onUpdate({ title: t });
  };

  const commitNote = () => {
    const v = noteDraftRef.current.trim();
    if (v === noteSaved.current) return;
    noteSaved.current = v;
    onUpdate({ note: v || undefined });
  };

  const commitTags = () => {
    if (tagsDraftRef.current === tagsSaved.current) return;
    const tags = tagsDraftRef.current
      .split(/[，,]/)
      .map((s) => s.trim())
      .filter(Boolean);
    tagsSaved.current = tagsDraftRef.current;
    onUpdate({ tags });
  };

  /* 关闭浮窗的两条路（拨杆决定）：
   *  开 = 保存关闭——先把三份草稿提交掉再关。Esc 触发时输入框未必失焦，
   *       只靠 blur 提交会漏掉最后一段输入；
   *  关 = 不保存关闭——草稿不提交，已落库的改动由宿主按打开浮窗时的快照回滚。 */
  const handleClose = () => {
    if (saveOnClose) {
      commitTitle();
      commitNote();
      commitTags();
      onClose();
    } else {
      onDiscard();
      onClose();
    }
  };

  // Esc 与关闭键同一条路（拨杆语义一致）。handleClose 每渲染重建，用 ref 兜住
  // 最新一份，免得每次渲染都重挂 window 监听
  const closeRef = useRef(handleClose);
  useEffect(() => {
    closeRef.current = handleClose;
  });
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && !e.isComposing) closeRef.current();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

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
  // 绑定走共享 hook：合并基线取 await 之后的最新 files——渲染期快照会把
  // 「面板开着的同时另一路径绑进来的那批」整列覆盖掉（两处实现曾就此漂移）
  const { pickFile, pickFolder, removeFile } = useTaskFiles(task, (files) =>
    onUpdate(filesPatch(files)),
  );

  // 点文件名直接打开：走 lib/openTarget 的 openFilePath（失败 silent 理由见彼处）

  /* ── 🤖 交给机器人（与 TodoCard 同通道，实现见 lib/botRun） ── */

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
      className="glass h-full w-full overflow-y-auto px-4 pt-6 pb-4"
    >
      <div className="flex items-start gap-2">
        <ActorAvatar bot={!!task.botAssigned} />
        <textarea
          ref={titleRef}
          rows={1}
          className="min-w-0 flex-1 resize-none overflow-hidden border-none bg-transparent text-base font-semibold leading-snug text-[var(--t1)] outline-none"
          value={titleDraft}
          autoFocus={autoFocusTitle}
          onFocus={(e) => autoFocusTitle && e.target.select()}
          onChange={(e) => {
            titleDraftRef.current = e.target.value;
            setTitleDraft(e.target.value);
          }}
          onBlur={commitTitle}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              (e.target as HTMLTextAreaElement).blur();
            }
            if (e.key === "Escape") {
              skipTitleCommit.current = true;
              titleDraftRef.current = titleSaved.current;
              setTitleDraft(titleSaved.current);
              (e.target as HTMLTextAreaElement).blur();
            }
          }}
          aria-label="任务标题"
        />
        {/* 关闭键 + 左侧的保存拨杆：拨杆开 = 保存关闭，关 = 不保存关闭。
            文案直接写在关闭键上，模式一眼可见，不靠 tooltip 解释 */}
        <div className="flex shrink-0 items-center gap-1.5 pt-0.5">
          <Toggle
            checked={saveOnClose}
            onChange={setSaveOnClose}
            size="sm"
            ariaLabel="关闭时保存改动"
          />
          <button
            type="button"
            onClick={handleClose}
            className="nm-btn shrink-0 whitespace-nowrap px-1.5 py-0.5 text-[11px] text-[var(--t3)]"
          >
            {saveOnClose ? "保存关闭" : "不保存关闭"}
          </button>
        </div>
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
        onChange={(e) => {
          noteDraftRef.current = e.target.value;
          setNoteDraft(e.target.value);
        }}
        onBlur={commitNote}
      />

      <div className="mt-3 mb-1.5 text-[11px] font-medium tracking-wide text-[var(--t5)]">
        标签
      </div>
      <input
        className="w-full rounded-lg border border-[var(--edge)] bg-[var(--input-bg)] px-2 py-1.5 text-xs text-[var(--t2)] outline-none focus:border-[var(--brand)]"
        value={tagsDraft}
        placeholder="逗号分隔，如：设计，改造"
        onChange={(e) => {
          tagsDraftRef.current = e.target.value;
          setTagsDraft(e.target.value);
        }}
        onBlur={commitTags}
        aria-label="标签"
      />

      <div className="mt-3 mb-1.5 text-[11px] font-medium tracking-wide text-[var(--t5)]">
        子任务
      </div>
      <div className="flex flex-col">
        {(task.subtasks ?? []).map((s) => (
          <SubtaskRow
            key={s.id}
            sub={s}
            variant="detail"
            onToggle={() => toggleSubtask(s.id)}
            onDelete={onDeleteSubtask ? () => onDeleteSubtask(s.id) : undefined}
            onOpenCard={onOpenTask ? (cardId) => onOpenTask(cardId) : undefined}
            onGenerateCard={
              onCreateSubtaskCard
                ? () => onCreateSubtaskCard(s.id, s.text)
                : undefined
            }
          />
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
        <div className="flex flex-col items-start gap-1">
          {/* chip 列表（与任务卡共用 TaskFileChips，spacious = 详情面板风格：emoji + 复制反馈） */}
          <TaskFileChips
            task={task}
            variant="spacious"
            onOpenPath={openFilePath}
            onCopyPath={copyOneFile}
            onRemovePath={removeFile}
            removeTitle="移除绑定"
            copyState={copyState}
          />
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
            {/* 文件夹槽替换制：按钮常驻——已绑文件夹时再点即替换（占原位，
                满员也可换槽不占新名额）；仅「满员且无文件夹槽」时隐藏 */}
            {(boundFiles.some((f) => f.isDir) ||
              boundFiles.length < MAX_TASK_FILES) && (
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
          onClick={() => runWithBot(task)}
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
