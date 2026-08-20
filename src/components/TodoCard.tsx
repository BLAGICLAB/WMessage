import { useEffect, useState } from "react";
import type { CSSProperties } from "react";
import { createPortal } from "react-dom";
import { useDraggable } from "@dnd-kit/core";
import type { DraggableAttributes, DraggableSyntheticListeners } from "@dnd-kit/core";
import { useSortable } from "@dnd-kit/sortable";
import { CSS } from "@dnd-kit/utilities";
import { invoke } from "@tauri-apps/api/core";
import { emit } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import { openPath } from "@tauri-apps/plugin-opener";
import { WebviewWindow } from "@tauri-apps/api/webviewWindow";
import { handleCommandError, formatCommandError } from "../lib/errorHandler";
import type { Task } from "../types";
import { taskFiles, filesPatch, mergeFiles, MAX_TASK_FILES } from "../lib/taskFiles";
import { basename, formatCompletedAt, formatDue, formatSchedule, isDueToday, isValidDateTimeLocal, scheduleToDatetime } from "../format";
import { DoneCircle } from "./DoneCircle";
import { FoldToggle } from "./FoldToggle";
import { ActorAvatar } from "./ActorAvatar";
import { useInlineEdit } from "./useInlineEdit";

const stop = (e: React.PointerEvent) => e.stopPropagation();

export interface TodoCardViewProps {
  task: Task;
  autoEdit?: boolean;
  onUpdate: (id: string, patch: Partial<Task>) => void;
  onDelete: (id: string) => void;
  /** 归档视图：显示「恢复」按钮 */
  archived?: boolean;
  /** 回收站视图：显示「恢复 / 彻底删除」按钮 */
  trashed?: boolean;
}

/** 拖拽能力由外部 hook（useDraggable / useSortable）注入，View 本体不关心排序上下文 */
export interface CardDrag {
  setNodeRef: (node: HTMLElement | null) => void;
  style?: CSSProperties;
  attributes: DraggableAttributes;
  listeners: DraggableSyntheticListeners | undefined;
  isDragging: boolean;
}

export function TodoCardView({
  task,
  autoEdit = false,
  onUpdate,
  onDelete,
  archived = false,
  trashed = false,
  drag,
}: TodoCardViewProps & { drag?: CardDrag }) {

  const [editing, setEditing] = useState(autoEdit && !archived && !trashed);
  const [dueEditing, setDueEditing] = useState(false);
  const [noteEditing, setNoteEditing] = useState(false);
  const [noteDraft, setNoteDraft] = useState("");
  const [tagEditing, setTagEditing] = useState(false);
  const [tagDraft, setTagDraft] = useState("");
  const [addingSubtask, setAddingSubtask] = useState(false);
  const [subtaskDraft, setSubtaskDraft] = useState("");
  // 回收站彻底删除：绑了本地文件/文件夹时的三选项弹窗（老板 2026-08-17）
  const [purgeOpen, setPurgeOpen] = useState(false);
  const [purgeBusy, setPurgeBusy] = useState(false);

  // autoEdit 切到 true 时进入编辑态（草稿同步由 useInlineEdit 在 editing false→true 时负责：
  // 挂件新建/改名后主窗口的 TodoCardView 因 key 不变不会重挂，hook 会把草稿重置为
  // 当前 task.title，不会显示成旧的"新任务"）。
  useEffect(() => {
    if (autoEdit && !archived && !trashed) {
      setEditing(true);
    } else if (!autoEdit) {
      setEditing(false);
    }
  }, [autoEdit, archived, trashed]);

  // 标题内联编辑：草稿 + Enter/Escape/Blur 行为统一走 useInlineEdit（P2-23，与 TaskCardContent 共用）
  const titleEdit = useInlineEdit({
    value: task.title,
    editing,
    onCommit: (d) => {
      const title = d.trim() || task.title;
      if (title !== task.title) onUpdate(task.id, { title });
      setEditing(false);
    },
    onCancel: () => setEditing(false),
  });

  const commitNote = () => {
    const note = noteDraft.trim();
    if (note !== (task.note ?? "")) onUpdate(task.id, { note: note || undefined });
    setNoteEditing(false);
  };

  // close=true 时收口输入框（回车保持打开方便连加多个）
  const addTag = (close: boolean) => {
    const t = tagDraft.trim();
    if (t) {
      const tags = task.tags ?? [];
      if (!tags.includes(t)) onUpdate(task.id, { tags: [...tags, t] });
    }
    setTagDraft("");
    if (close) setTagEditing(false);
  };

  const commitSubtask = () => {
    const text = subtaskDraft.trim();
    if (text) {
      onUpdate(task.id, {
        subtasks: [
          ...(task.subtasks ?? []),
          { id: crypto.randomUUID(), text, done: false },
        ],
      });
    }
    setSubtaskDraft("");
    setAddingSubtask(false);
  };

  const pickFile = async () => {
    try {
      const cur = taskFiles(task);
      // 文件与文件夹不互斥（老板 2026-08-19）：已绑文件夹也可继续添加文件
      const selected = await open({ multiple: true, directory: false });
      const paths = Array.isArray(selected)
        ? selected
        : typeof selected === "string"
        ? [selected]
        : [];
      if (paths.length === 0) return;
      // isDir 由 Rust 侧 fs::metadata 判定（前端无法 stat）
      const added = await invoke<{ path: string; isDir: boolean }[]>("bind_files", { paths });
      const { files, truncated } = mergeFiles(cur, added);
      if (truncated) window.alert(`每个任务最多绑定 ${MAX_TASK_FILES} 个文件，超出部分已忽略`);
      if (files.length !== cur.length) onUpdate(task.id, filesPatch(files));
    } catch (e) {
      handleCommandError(e, "pick file");
    }
  };

  const pickFolder = async () => {
    try {
      const cur = taskFiles(task);
      // 文件夹仍单选（最多一个文件夹）；文件与文件夹不互斥（老板 2026-08-19）
      if (cur.some((f) => f.isDir)) {
        window.alert("该任务已绑定文件夹，请先移除再重新绑定");
        return;
      }
      const selected = await open({ directory: true });
      // 文件夹追加进绑定列表（不再替换掉已绑文件），去重保序
      if (typeof selected === "string") {
        const { files, truncated } = mergeFiles(cur, [{ path: selected, isDir: true }]);
        if (truncated) window.alert(`每个任务最多绑定 ${MAX_TASK_FILES} 个文件，超出部分已忽略`);
        if (files.length !== cur.length) onUpdate(task.id, filesPatch(files));
      }
    } catch (e) {
      handleCommandError(e, "pick folder");
    }
  };

  const removeFile = (path: string) => {
    onUpdate(task.id, filesPatch(taskFiles(task).filter((f) => f.path !== path)));
  };

  // 标题右侧圆圈：待办/今日 → 完成（自动记完成时间）；完成 → 截止日期是今天回「今日」、否则回「待办」，完成时间删除
  const toggleDone = () => {
    if (task.column === "done") {
      const back = isDueToday(task.due) ? "doing" : "todo";
      onUpdate(task.id, { column: back, completedAt: undefined, archived: undefined });
    } else {
      // 人完成：清机器人标记 → 显示用户头像
      onUpdate(task.id, { column: "done", completedAt: Date.now(), archived: false, botAssigned: undefined });
    }
  };

  // 标题以下内容折叠/展开
  const toggleCollapsed = () => {
    onUpdate(task.id, { collapsed: !task.collapsed });
  };

  const boundFiles = taskFiles(task);
  // 绑定文件折叠：超过 5 个收起到「还有 N 个」（2026-08-19 多文件绑定）
  const [filesExpanded, setFilesExpanded] = useState(false);
  // 多文件打开选择列表（📂 点击：单文件直开，多文件弹列表让用户选）
  const [openChooser, setOpenChooser] = useState(false);

  const openFile = () => {
    if (boundFiles.length === 0) return;
    if (boundFiles.length > 1) {
      setOpenChooser((v) => !v);
      return;
    }
    openPath(boundFiles[0].path).catch((e) =>
      handleCommandError(e, "open file", { silent: true })
    );
  };

  const openOneFile = (path: string) => {
    setOpenChooser(false);
    openPath(path).catch((e) =>
      handleCommandError(e, "open file", { silent: true })
    );
  };

  const copyFile = () => {
    if (boundFiles.length === 0) return;
    if (boundFiles.length === 1) {
      // 单文件行为不变
      invoke("copy_file_with_title", { path: boundFiles[0].path, title: task.title }).catch((e) =>
        // 复制失败：用户点了按钮，但失败通常不是关键操作（如源文件被删），不打扰
        handleCommandError(e, "copy_file_with_title", { silent: true })
      );
      return;
    }
    // 多文件：复制全部文件，文本命名为 {title}-{basename}（Rust 侧拼接）
    invoke("copy_files_with_title", {
      paths: boundFiles.map((f) => f.path),
      title: task.title,
    }).catch((e) =>
      handleCommandError(e, "copy_files_with_title", { silent: true })
    );
  };

  // 定时执行面板
  const [schedOpen, setSchedOpen] = useState(false);
  const [schedOnce, setSchedOnce] = useState("");

  // 交给机器人执行：发事件给挂件聊天区（ChatPanel 监听），并唤起挂件窗口
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

  const subtasks = task.subtasks ?? [];
  const doneCount = subtasks.filter((s) => s.done).length;

  return (
    <div
      ref={drag?.setNodeRef}
      style={drag?.style}
      {...drag?.attributes}
      className={`group nm-card-hover p-4 w-full select-none ${drag?.isDragging ? "opacity-70" : ""}`}
    >
      <div className="flex items-start gap-2">
        {/* ☰ 拖拽手柄：标题左侧占位，卡片悬停才显现；与标题保持间距 */}
        {drag?.listeners && !archived && !trashed && (
          <span
            {...drag.listeners}
            title="拖拽移动"
            className="shrink-0 mt-0.5 w-4 h-4 flex items-center justify-center text-[12px] leading-none text-[var(--t5)] rounded hover:bg-[var(--hover-bg)] opacity-0 group-hover:opacity-100 transition-opacity cursor-grab active:cursor-grabbing"
          >
            ☰
          </span>
        )}
        {editing ? (
          <input
            autoFocus
            value={titleEdit.draft}
            onChange={(e) => titleEdit.setDraft(e.target.value)}
            onBlur={titleEdit.onBlur}
            onKeyDown={titleEdit.onKeyDown}
            onPointerDown={stop}
            className="flex-1 min-w-0 rounded-lg bg-[var(--input-bg)] px-2 py-1 outline-none nm-task-title"
          />
        ) : (
          <h3
            className={`flex-1 min-w-0 nm-task-title ${
              archived || trashed ? "" : "cursor-text"
            } ${task.collapsed ? "truncate" : ""}`}
            title={
              archived || trashed
                ? task.collapsed
                  ? task.title
                  : undefined
                : task.collapsed
                ? task.title
                : "点击编辑"
            }
            onPointerDown={stop}
            onClick={
              archived
                ? undefined
                : () => setEditing(true)
            }
          >
            {task.title}
          </h3>
        )}
        {/* 折叠/展开开关：标题右侧、标题与对勾之间 */}
        <FoldToggle collapsed={!!task.collapsed} onToggle={toggleCollapsed} />
        {!archived && !trashed && (
          <DoneCircle done={task.column === "done"} onToggle={toggleDone} />
        )}
        {/* 归属头像：交给机器人 → 机器人头像；否则用户头像。悬停显示姓名。 */}
        <ActorAvatar bot={!!task.botAssigned} />
      </div>

      {/* 标题以下内容（可折叠） */}
      {!task.collapsed && (
        <>
      {/* 标题下的小字备注 */}
      {noteEditing ? (
        <input
          autoFocus
          value={noteDraft}
          onChange={(e) => setNoteDraft(e.target.value)}
          onBlur={commitNote}
          onKeyDown={(e) => {
            if (e.key === "Enter" && !e.nativeEvent.isComposing) commitNote();
            if (e.key === "Escape") {
              setNoteDraft(task.note ?? "");
              setNoteEditing(false);
            }
          }}
          onPointerDown={stop}
          placeholder="备注…"
          className="mt-1.5 w-full rounded-lg bg-[var(--input-bg)] px-2 py-1 text-xs text-[var(--t3)] outline-none"
        />
      ) : task.note ? (
        <p
          className={`mt-1.5 text-xs text-[var(--t4)] ${archived || trashed ? "" : "cursor-text"}`}
          title={archived || trashed ? undefined : "点击编辑备注"}
          onPointerDown={stop}
          onClick={
            archived || trashed
              ? undefined
              : () => {
                  setNoteDraft(task.note!);
                  setNoteEditing(true);
                }
          }
        >
          {task.note}
        </p>
      ) : archived || trashed ? null : (
        <button
          className="mt-1.5 text-xs text-[var(--t6)] hover:text-[var(--t3)]"
          onPointerDown={stop}
          onClick={() => {
            setNoteDraft("");
            setNoteEditing(true);
          }}
        >
          + 备注
        </button>
      )}

      {/* 标签 */}
      <div className="mt-2 flex flex-wrap items-center gap-1.5">
        {(task.tags ?? []).map((tag, i) => (
          <span
            key={`${tag}-${i}`}
            className="nm-inset px-2 py-0.5 text-xs text-[var(--t4)] flex items-center gap-1"
          >
            {tag}
            {!archived && !trashed && (
              <button
                className="text-[var(--t5)] hover:text-[var(--danger)] leading-none"
                title="移除标签"
                onPointerDown={stop}
                onClick={() =>
                  onUpdate(task.id, {
                    tags: (task.tags ?? []).filter((_, j) => j !== i),
                  })
                }
              >
                ×
              </button>
            )}
          </span>
        ))}
        {tagEditing ? (
          <input
            autoFocus
            value={tagDraft}
            onChange={(e) => setTagDraft(e.target.value)}
            onBlur={() => addTag(true)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && !e.nativeEvent.isComposing) addTag(false);
              if (e.key === "Escape") {
                setTagDraft("");
                setTagEditing(false);
              }
            }}
            onPointerDown={stop}
            placeholder="标签名"
            className="w-20 rounded-lg bg-[var(--input-bg)] px-2 py-0.5 text-xs text-[var(--t2)] outline-none"
          />
        ) : archived || trashed ? null : (
          <button
            className="text-xs text-[var(--t6)] hover:text-[var(--t3)]"
            onPointerDown={stop}
            onClick={() => {
              setTagDraft("");
              setTagEditing(true);
            }}
          >
            + 标签
          </button>
        )}
      </div>

      {/* 子任务清单（分隔线分行；长文本单行截断 + hover 显示全文，2026-08-19） */}
      {subtasks.length > 0 && (
        <div className="mt-2 flex flex-col divide-y divide-[var(--edge)]">
          {subtasks.map((s) => (
            <div key={s.id} className="flex items-center gap-2 group py-1">
              <input
                type="checkbox"
                checked={s.done}
                disabled={archived || trashed}
                onChange={() =>
                  onUpdate(task.id, {
                    subtasks: subtasks.map((x) =>
                      x.id === s.id ? { ...x, done: !x.done } : x
                    ),
                  })
                }
                onPointerDown={stop}
                className="shrink-0 w-3.5 h-3.5 accent-[var(--brand)]"
              />
              <span
                className={`flex-1 min-w-0 truncate text-xs ${
                  s.done ? "text-[var(--t5)] line-through" : "text-[var(--t3)]"
                }`}
                title={s.text}
              >
                {s.text}
              </span>
              {!archived && !trashed && (
                <button
                  className="opacity-0 group-hover:opacity-100 text-[var(--t5)] hover:text-[var(--danger)] text-xs"
                  title="删除子任务"
                  onPointerDown={stop}
                  onClick={() =>
                    onUpdate(task.id, {
                      subtasks: subtasks.filter((x) => x.id !== s.id),
                    })
                  }
                >
                  ×
                </button>
              )}
            </div>
          ))}
        </div>
      )}

      {addingSubtask ? (
        <input
          autoFocus
          value={subtaskDraft}
          onChange={(e) => setSubtaskDraft(e.target.value)}
          onBlur={commitSubtask}
          onKeyDown={(e) => {
            if (e.key === "Enter" && !e.nativeEvent.isComposing) commitSubtask();
            if (e.key === "Escape") {
              setSubtaskDraft("");
              setAddingSubtask(false);
            }
          }}
          onPointerDown={stop}
          placeholder="子任务…"
          className="mt-2 w-full rounded-lg bg-[var(--input-bg)] px-2 py-1 text-xs text-[var(--t2)] outline-none"
        />
      ) : archived || trashed ? null : (
        <button
          className="mt-2 text-xs text-[var(--t5)] hover:text-[var(--t2)]"
          onPointerDown={stop}
          onClick={() => {
            setSubtaskDraft("");
            setAddingSubtask(true);
          }}
        >
          + 添加子任务
          {subtasks.length > 0 ? ` · ${doneCount}/${subtasks.length}` : ""}
        </button>
      )}

      {boundFiles.length > 0 ? (
        <div className="mt-3 flex flex-col gap-2">
          {/* 绑定文件 chip 列表：📁/📎 + basename + 单独移除（×）；超过 5 个折叠为「还有 N 个」 */}
          <div className="flex flex-col gap-1">
            {(filesExpanded ? boundFiles : boundFiles.slice(0, 5)).map((f) => (
              <div key={f.path} className="flex items-center gap-1.5">
                <p className="flex-1 min-w-0 text-xs text-[var(--t4)] truncate" title={f.path}>
                  {f.isDir ? "📁" : "📎"} {basename(f.path)}
                </p>
                {!archived && !trashed && (
                  <button
                    className="shrink-0 text-[var(--t5)] hover:text-[var(--danger)] text-sm"
                    title="移除该文件"
                    onPointerDown={stop}
                    onClick={() => removeFile(f.path)}
                  >
                    ×
                  </button>
                )}
              </div>
            ))}
            {boundFiles.length > 5 && (
              <button
                className="self-start text-[11px] text-[var(--t5)] hover:text-[var(--t3)]"
                onPointerDown={stop}
                onClick={() => setFilesExpanded((v) => !v)}
              >
                {filesExpanded ? "收起" : `还有 ${boundFiles.length - 5} 个`}
              </button>
            )}
          </div>
          <div className="flex items-center gap-2">
            <button
              className="nm-btn px-2 py-0.5 text-[11px] leading-none text-[var(--t3)]"
              title={boundFiles.length > 1 ? "打开文件（多选列表）" : boundFiles[0].isDir ? "打开文件夹" : "打开文件"}
              onPointerDown={stop}
              onClick={openFile}
            >
              📂
            </button>
            <button
              className="nm-btn px-2 py-0.5 text-[11px] leading-none text-[var(--t3)]"
              title="复制文件+标题"
              onPointerDown={stop}
              onClick={copyFile}
            >
              📋
            </button>
            {!archived && !trashed && boundFiles.length < MAX_TASK_FILES && (
              <button
                className="nm-btn px-2 py-0.5 text-[11px] leading-none text-[var(--t4)]"
                title="继续绑定文件"
                onPointerDown={stop}
                onClick={pickFile}
              >
                ＋
              </button>
            )}
            {/* 文件与文件夹不互斥：已绑文件但未绑文件夹时仍可绑定文件夹 */}
            {!archived && !trashed && !boundFiles.some((f) => f.isDir) && boundFiles.length < MAX_TASK_FILES && (
              <button
                className="nm-btn px-2 py-0.5 text-[11px] leading-none text-[var(--t4)]"
                title="绑定文件夹"
                onPointerDown={stop}
                onClick={pickFolder}
              >
                📁
              </button>
            )}
            {!archived && !trashed && (
              <button
                className="text-[var(--t5)] hover:text-[var(--danger)] text-sm"
                title="解绑全部文件"
                onPointerDown={stop}
                onClick={() => onUpdate(task.id, filesPatch([]))}
              >
                ×
              </button>
            )}
          </div>
          {/* 多文件打开选择列表（原生 dialog 无多选列表，退到 UI 列表） */}
          {openChooser && boundFiles.length > 1 && (
            <div className="nm-inset rounded-lg p-1.5 flex flex-col gap-0.5">
              {boundFiles.map((f) => (
                <button
                  key={f.path}
                  className="text-left text-xs text-[var(--t3)] px-2 py-1 rounded hover:bg-[var(--hover-bg)] truncate"
                  title={f.path}
                  onPointerDown={stop}
                  onClick={() => openOneFile(f.path)}
                >
                  {f.isDir ? "📁" : "📎"} {basename(f.path)}
                </button>
              ))}
            </div>
          )}
        </div>
      ) : archived || trashed ? null : (
        <div className="mt-3 flex items-center gap-3">
          <button
            className="nm-btn px-2 py-0.5 text-xs text-[var(--t4)] flex items-center gap-1"
            onPointerDown={stop}
            onClick={pickFile}
          >
            <span className="text-[11px] leading-none">📎</span> 绑定文件
          </button>
          <button
            className="nm-btn px-2 py-0.5 text-xs text-[var(--t4)] flex items-center gap-1"
            onPointerDown={stop}
            onClick={pickFolder}
          >
            <span className="text-[11px] leading-none">📁</span> 绑定文件夹
          </button>
        </div>
      )}

      {archived && (
        <button
          className="nm-btn mt-3 px-3 py-1 text-xs text-[var(--t3)]"
          onPointerDown={stop}
          onClick={() =>
            onUpdate(task.id, { archived: false, completedAt: Date.now() })
          }
        >
          ↩ 恢复
        </button>
      )}

      {trashed && (
        <div className="mt-3 flex items-center gap-2">
          <button
            className="nm-btn px-3 py-1 text-xs text-[var(--t3)]"
            onPointerDown={stop}
            onClick={() => onUpdate(task.id, { deletedAt: undefined })}
          >
            ↩ 恢复
          </button>
          <button
            className="nm-btn px-3 py-1 text-xs text-red-400"
            onPointerDown={stop}
            onClick={() => {
              // 绑本地文件/文件夹时弹三选项（老板 2026-08-17）：
              //   全部删除 / 保留文件删除 / 取消
              // 未绑文件时保持原两选项 confirm（无需三选）
              if (boundFiles.length > 0) {
                setPurgeOpen(true);
                return;
              }
              if (!window.confirm(`确定彻底删除任务「${task.title}」？\n此操作不可撤销。`)) return;
              onDelete(task.id);
            }}
          >
            🗑 彻底删除
          </button>
        </div>
      )}

      {/* 🤖 交给机器人执行 + ⏰ 定时执行（与挂件一致）；归档卡只读不显示 */}
      {!archived && !trashed && (
      <>
      <div className="mt-2 flex items-center gap-1.5">
        <button
          className="nm-btn px-2 py-0.5 text-[11px] leading-none text-[var(--t3)] flex items-center gap-1"
          onPointerDown={stop}
          onClick={runWithBot}
          title="交给机器人执行这张任务卡"
        >
          🤖 交给机器人
        </button>
        <button
          className={`nm-btn px-2 py-0.5 text-[11px] leading-none ${
            task.schedule ? "text-[var(--brand)]" : "text-[var(--t3)]"
          }`}
          onPointerDown={stop}
          onClick={() => {
            setSchedOnce(scheduleToDatetime(task.schedule));
            setSchedOpen((v) => !v);
          }}
          title={
            task.schedule
              ? `定时：${formatSchedule(task.schedule)}（点击修改/取消）`
              : "定时执行：到点自动交给机器人跑"
          }
        >
          ⏰ {task.schedule ? formatSchedule(task.schedule) : "定时"}
        </button>
      </div>
      {schedOpen && (
        <div className="mt-1.5 nm-inset rounded-lg p-2 space-y-1.5">
          <p className="text-[10px] text-[var(--t5)]">
            到点自动执行这张任务卡（结果写进备注）
          </p>
          <input
            type="datetime-local"
            value={schedOnce}
            onChange={(e) => setSchedOnce(e.target.value)}
            onPointerDown={stop}
            className="nm-inset w-full rounded-lg px-2 py-1 text-xs text-[var(--t3)] outline-none"
          />
          <div className="flex flex-wrap gap-1">
            {(
              [
                ["一次", "once"],
                ["每天", "daily"],
                ["每周", "weekly"],
                ["每月", "monthly"],
              ] as [string, string][]
            ).map(([label, kind]) => (
              <button
                key={kind}
                className="nm-btn px-2 py-0.5 text-[10px] text-[var(--t3)]"
                onPointerDown={stop}
                onClick={() => {
                  if (!schedOnce) return;
                  const dt = schedOnce.slice(0, 16);
                  // 防御：无效日期（手动输入不完整等）不写库，防 NaN 进 schedule
                  // （历史事故：NaN 星期 → weekly:NaN:... → 回填死循环卡死 App）
                  if (isNaN(new Date(dt).getTime())) return;
                  const hm = dt.slice(11, 16);
                  if (kind === "once") {
                    onUpdate(task.id, { schedule: `at:${dt}` });
                  } else if (kind === "daily") {
                    onUpdate(task.id, { schedule: `daily:${hm}` });
                  } else if (kind === "weekly") {
                    // 取所选日期的星期几（1=周一 ... 7=周日）
                    const wd = ((new Date(dt).getDay() + 6) % 7) + 1;
                    onUpdate(task.id, { schedule: `weekly:${wd}:${hm}` });
                  } else {
                    onUpdate(task.id, { schedule: `monthly:${dt.slice(8, 10)}:${hm}` });
                  }
                  setSchedOpen(false);
                }}
              >
                {label}
              </button>
            ))}
            {task.schedule && (
              <button
                className="nm-btn px-2 py-0.5 text-[10px] text-[var(--danger)]"
                onPointerDown={stop}
                onClick={() => {
                  onUpdate(task.id, { schedule: undefined });
                  setSchedOpen(false);
                }}
              >
                取消
              </button>
            )}
          </div>
        </div>
      )}
      </>
      )}

      {/* 截止时间 —— 永远在最下面，删除按钮在其右侧 */}
      <div className="flex items-center gap-1 mt-2">
        {dueEditing && !archived && !trashed ? (
          <input
            type="datetime-local"
            autoFocus
            value={
              task.due
                ? task.due.includes("T")
                  ? task.due.slice(0, 16)
                  : `${task.due}T09:00`
                : ""
            }
            onChange={(e) => {
              const v = e.target.value;
              if (v === "") {
                onUpdate(task.id, { due: undefined });
              } else if (isValidDateTimeLocal(v)) {
                onUpdate(task.id, { due: v.slice(0, 16) });
              }
              // 不完整/非法值：不写库，blur 时回退已提交值
            }}
            onBlur={() => setDueEditing(false)}
            onPointerDown={stop}
            className="nm-inset px-2 py-1 text-xs text-[var(--t3)] flex-1 min-w-0"
          />
        ) : (
          <>
            {task.due ? (
              <span className="flex items-center shrink-0">
                {archived || trashed ? (
                  <span className="nm-inset px-2 py-1 text-xs text-[var(--t4)]">
                    {formatDue(task.due)}
                  </span>
                ) : (
                  <>
                    <button
                      className="nm-inset px-2 py-1 text-xs text-[var(--t4)]"
                      onPointerDown={stop}
                      onClick={() => setDueEditing(true)}
                    >
                      {formatDue(task.due)}
                    </button>
                    <button
                      className="w-4 h-6 text-xs text-[var(--t5)] hover:text-[var(--danger)]"
                      title="移除截止时间"
                      onPointerDown={stop}
                      onClick={() => onUpdate(task.id, { due: undefined })}
                    >
                      ×
                    </button>
                  </>
                )}
              </span>
            ) : archived || trashed ? null : (
              <button
                className="nm-inset px-2 py-1 text-xs text-[var(--t4)]"
                onPointerDown={stop}
                onClick={() => setDueEditing(true)}
              >
                + 截止时间
              </button>
            )}
            {/* 完成时间：完成/归档的任务显示在 × 与 🗑️ 之间居中；取消完成即删除 */}
            {task.column === "done" && task.completedAt && (
              <span className="flex-1 min-w-0 text-center whitespace-nowrap text-[10px] text-[var(--t5)]">
                {formatCompletedAt(task.completedAt)}
              </span>
            )}
          </>
        )}

        {/* 删除任务：emoji 小图标，截止日期右侧（回收站视图已有「彻底删除」，不重复显示；归档卡只读不显示） */}
        {!trashed && !archived && (
          <button
            className="ml-auto shrink-0 w-5 h-5 flex items-center justify-center text-xs leading-none text-[var(--t5)] hover:text-[var(--danger)]"
            title="删除任务"
            onPointerDown={stop}
            onClick={() => onDelete(task.id)}
          >
            🗑️
          </button>
        )}
      </div>
        </>
      )}

      {/* 回收站彻底删除·绑文件三选项弹窗（老板 2026-08-17）：全部删除 / 保留文件删除 / 取消
          ⚠️ 必须用 createPortal 渲染到 document.body —— TodoCard 容器 hover 触发 transform: translateY(-3px) scale(1.01)
          (main.css .nm-card-hover:hover) + dnd-kit useSortable 的 transform style，二者都会创建 CSS 包含块，
          使 position:fixed 子元素不再相对视口定位而被裁缩到卡片边界内（老板 21:10 报 bug）。 */}
      {purgeOpen && boundFiles.length > 0 && createPortal(
        <div
          className="fixed inset-0 z-[100] flex items-center justify-center bg-black/40 p-6"
          onPointerDown={() => { if (!purgeBusy) setPurgeOpen(false); }}
        >
          <div
            className="nm-card w-full max-w-md p-5 flex flex-col gap-3"
            onPointerDown={(e) => e.stopPropagation()}
          >
            <div className="text-sm font-medium text-[var(--t2)]">
              🗑 彻底删除任务卡
            </div>
            <div className="text-xs text-[var(--t3)] leading-relaxed">
              任务卡「<span className="text-[var(--t2)] font-medium">{task.title}</span>」绑定了
              {boundFiles.length > 1 ? (
                <span className="text-[var(--t2)]"> {boundFiles.length} 个文件/文件夹</span>
              ) : (
                <>
                  <span className="text-[var(--t2)]">{boundFiles[0].isDir ? "文件夹" : "文件"}</span>「
                  <span className="text-[var(--t2)]">{basename(boundFiles[0].path)}</span>」
                </>
              )}。
            </div>
            <div
              className="text-[11px] text-[var(--t5)] break-all px-2 py-1.5 rounded bg-[var(--bg)] border border-[var(--bd)] max-h-28 overflow-y-auto whitespace-pre-line"
              title={boundFiles.map((f) => f.path).join("\n")}
            >
              完整路径：{boundFiles.length > 1 ? "\n" : ""}{boundFiles.map((f) => f.path).join("\n")}
            </div>
            <div className="text-[11px] text-[var(--t4)]">
              此操作不可撤销，请选择：
            </div>
            <div className="flex flex-col gap-2 mt-1">
              <button
                className="nm-btn px-3 py-2 text-xs text-red-400 flex flex-col items-start gap-0.5 disabled:opacity-50"
                disabled={purgeBusy}
                onClick={async () => {
                  setPurgeBusy(true);
                  try {
                    // 多文件绑定：逐个移入废纸篓/回收站
                    for (const f of boundFiles) {
                      await invoke("delete_bound_file", {
                        path: f.path,
                        isDir: f.isDir,
                      });
                    }
                    onDelete(task.id);
                    setPurgeOpen(false);
                  } catch (e) {
                    // 删除失败：Toast 给完整 message + 下一步提示，任务卡仍保留在回收站可重试
                    handleCommandError(e, "delete_bound_file");
                    alert(`任务卡保留在回收站，可重试或手动从废纸篓/回收站清理后再试。\n\n${formatCommandError(e)}`);
                    setPurgeBusy(false);
                  }
                }}
              >
                <span className="font-medium">🗑 全部删除</span>
                <span className="text-[10px] text-[var(--t4)] font-normal">任务卡删除，并把绑定的本地{boundFiles.length > 1 ? "文件/文件夹" : boundFiles[0].isDir ? "文件夹" : "文件"}移到废纸篓/回收站</span>
              </button>
              <button
                className="nm-btn px-3 py-2 text-xs text-[var(--t2)] flex flex-col items-start gap-0.5 disabled:opacity-50"
                disabled={purgeBusy}
                onClick={() => {
                  onDelete(task.id);
                  setPurgeOpen(false);
                }}
              >
                <span className="font-medium">📄 保留文件删除</span>
                <span className="text-[10px] text-[var(--t4)] font-normal">只删除任务卡，本地{boundFiles.length > 1 ? "文件/文件夹" : boundFiles[0].isDir ? "文件夹" : "文件"}保留</span>
              </button>
              <button
                className="nm-btn px-3 py-2 text-xs text-[var(--t3)] disabled:opacity-50"
                disabled={purgeBusy}
                onClick={() => setPurgeOpen(false)}
              >
                取消
              </button>
            </div>
          </div>
        </div>,
        document.body
      )}
    </div>
  );
}

/** 归档/回收站版：普通可拖拽卡片（无排序上下文，保留原 useDraggable 行为） */
export function TodoCard(props: TodoCardViewProps) {
  const { attributes, listeners, setNodeRef, transform, isDragging } = useDraggable({
    id: props.task.id,
  });
  const style = transform ? { transform: CSS.Translate.toString(transform) } : undefined;
  return <TodoCardView {...props} drag={{ attributes, listeners, setNodeRef, style, isDragging }} />;
}

/** 看板版：列内/跨列排序卡片（必须渲染在 SortableContext 内） */
export function SortableTodoCard(props: TodoCardViewProps) {
  const { attributes, listeners, setNodeRef, transform, transition, isDragging } = useSortable({
    id: props.task.id,
  });
  const style = { transform: CSS.Transform.toString(transform), transition };
  return <TodoCardView {...props} drag={{ attributes, listeners, setNodeRef, style, isDragging }} />;
}
