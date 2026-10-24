// TodoCard 主组件（orchestrator）。
// 972 → 872 行：types / SubtaskRow 拆到同目录子文件，本文件保留 TodoCardView + TodoCard。
//
// 公开 import 路径保持稳定：外部 `import { TodoCard } from "./components/TodoCard"` /
// `import { TodoCardView } from "./components/TodoCard"`，
// Vite 解析到 `./TodoCard/index.tsx` → 透传本目录各文件。

import { useEffect, useState } from "react";
import {
  Bot,
  FileText,
  FolderOpen,
  GripVertical,
  History,
  Paperclip,
  Trash2,
  Undo2,
} from "lucide-react";
import { useDraggable } from "@dnd-kit/core";
import { CSS } from "@dnd-kit/utilities";
import { invoke } from "@tauri-apps/api/core";
import { handleCommandError, formatCommandError } from "../../lib/errorHandler";
import { runWithBot } from "../../lib/botRun";
import { openFilePath } from "../../lib/openTarget";
import { isTaskCompleted } from "../../lib/taskScope";
import { planWindowLabel } from "../TaskTimeline/week";
import { useConfirm } from "../../lib/useConfirm";
import { useTaskFiles } from "../../lib/useTaskFiles";
import {
  taskFiles,
  filesPatch,
  MAX_TASK_FILES,
} from "../../lib/taskFiles";
import {
  basename,
  formatCompletedAt,
  formatDue,
  isDueToday,
  isValidDateTimeLocal,
} from "../../format";
import { DoneCircle } from "../DoneCircle";
import { FoldToggle } from "../FoldToggle";
import { ActorAvatar } from "../ActorAvatar";
import { TracePanel } from "../TracePanel";
import { useInlineEdit } from "../useInlineEdit";
import { TaskFileChips } from "../task/TaskFileChips";
import { Modal } from "../../ui/Modal";

import type { CardDrag, TodoCardViewProps } from "./types";
import { SubtaskRow } from "../task/SubtaskRow";
import { stopPointer as stop } from "../../lib/stopPointer";

/** 视图本体：所有拖拽能力由外部 hook 注入（TodoCard / SortableTodoCard 调用） */
export function TodoCardView({
  task,
  onUpdate,
  onDelete,
  onSetColumn,
  onOpenSubtaskCard,
  archived = false,
  trashed = false,
  drag,
}: TodoCardViewProps & { drag?: CardDrag }) {
  const [editing, setEditing] = useState(false);
  const confirm = useConfirm();
  const [dueEditing, setDueEditing] = useState(false);
  // 执行痕迹弹层（TracePanel 自带 portal，任务卡内点击「执行详情」打开）
  const [traceOpen, setTraceOpen] = useState(false);
  const [noteEditing, setNoteEditing] = useState(false);
  const [noteDraft, setNoteDraft] = useState("");
  const [tagEditing, setTagEditing] = useState(false);
  const [tagDraft, setTagDraft] = useState("");
  const [addingSubtask, setAddingSubtask] = useState(false);
  const [subtaskDraft, setSubtaskDraft] = useState("");
  // 回收站彻底删除：绑了本地文件/文件夹时的三选项弹窗
  const [purgeOpen, setPurgeOpen] = useState(false);
  const [purgeBusy, setPurgeBusy] = useState(false);

  // 卡片转入归档/回收站（只读）时退出编辑态：
  // 留在编辑态会绕过归档/回收站的只读交互继续提交改动
  useEffect(() => {
    if (archived || trashed) {
      // oxlint-disable-next-line react/set-state-in-effect
      setEditing(false);
    }
  }, [archived, trashed]);

  // 标题内联编辑：草稿 + Enter/Escape/Blur 行为统一走 useInlineEdit
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
    if (note !== (task.note ?? ""))
      onUpdate(task.id, { note: note || undefined });
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

  // 文件绑定：统一走共享 hook（合并基线取 await 之后的最新 files，
  // 不吃点击时的快照——并发进来的那批文件不能被整列覆盖掉）
  const { pickFile, pickFolder, removeFile } = useTaskFiles(task, (files) =>
    onUpdate(task.id, filesPatch(files)),
  );

  // 标题右侧圆圈：待办/今日 → 完成（自动记完成时间）；完成 → 截止日期是今天回「今日」、否则回「待办」，完成时间删除
  // TP-1：优先走 onSetColumn（服务端定向命令，同锁内读现值打基线，免快照竞态）；
  // 未接线时回落 onUpdate 整行写（向后兼容）
  const toggleDone = () => {
    if (task.column === "done") {
      const back = isDueToday(task.due) ? "doing" : "todo";
      if (onSetColumn) {
        onSetColumn(task.id, back);
        return;
      }
      onUpdate(task.id, {
        column: back,
        completedAt: undefined,
        archived: undefined,
      });
    } else {
      if (onSetColumn) {
        onSetColumn(task.id, "done");
        return;
      }
      // 人完成：清机器人标记 → 显示用户头像
      onUpdate(task.id, {
        column: "done",
        completedAt: Date.now(),
        archived: false,
        botAssigned: undefined,
      });
    }
  };

  // 标题以下内容折叠/展开
  const toggleCollapsed = () => {
    onUpdate(task.id, { collapsed: !task.collapsed });
  };

  // 折叠/展开内部封进 TaskFileChips；此处仍需 boundFiles 用于回收站彻底删除的
  // 三选项弹窗（绑了本地文件时弹「全部删除/保留文件/取消」）
  const boundFiles = taskFiles(task);

  // 点绑定文件名/文件夹名直接打开（不再有 📂 按钮和多选列表）：走 lib/openTarget
  // 的 openFilePath（Rust 侧命令不受前端 opener scope 限；失败 silent 理由见彼处）
  const copyOneFile = (path: string) => {
    invoke("copy_file_with_title", { path, title: task.title }).catch((e) =>
      // 复制失败：用户点了按钮，但失败通常不是关键操作（如源文件被删），不打扰
      handleCommandError(e, "copy_file_with_title", { silent: true }),
    );
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
            className="shrink-0 mt-0.5 w-4 h-4 flex items-center justify-center text-[12px] leading-none text-[var(--t5)] rounded hover:bg-[var(--hover-bg)] opacity-0 group-hover:opacity-100 transition-opacity duration-[var(--dur-chip)] ease-[var(--ease-out)] cursor-grab active:cursor-grabbing"
          >
            <GripVertical size={12} aria-hidden />
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
              // 回收站/归档只读：trashed 卡也不得点进标题编辑（blur 会提交改写）
              archived || trashed ? undefined : () => setEditing(true)
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
        {/* 归属头像规则：🤖 交给机器人执行中 → 机器人头像；执行完（botAssigned 清除）→ 用户头像。
            悬停显示姓名。 */}
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
                if (e.key === "Enter" && !e.nativeEvent.isComposing)
                  commitNote();
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
              className={`mt-1.5 text-xs text-[var(--t4)] break-all ${archived || trashed ? "" : "cursor-text"}`}
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
                // 键带序号防重复标签撞 key（标签可手输，非天然唯一）
                // oxlint-disable-next-line react/no-array-index-key
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
                  if (e.key === "Enter" && !e.nativeEvent.isComposing)
                    addTag(false);
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

          {/* 子任务清单：文本完整显示不截断、点击文本内联编辑、
          行间分割线淡化为半透明 --edge，避免抢眼 */}
          {subtasks.length > 0 && (
            <div className="mt-2 flex flex-col divide-y divide-[color-mix(in_srgb,var(--edge),transparent_55%)]">
              {subtasks.map((s) => (
                <SubtaskRow
                  key={s.id}
                  sub={s}
                  readOnly={archived || trashed}
                  onOpenCard={onOpenSubtaskCard}
                  onToggle={() =>
                    onUpdate(task.id, {
                      subtasks: subtasks.map((x) =>
                        x.id === s.id ? { ...x, done: !x.done } : x,
                      ),
                    })
                  }
                  onCommit={(text) =>
                    onUpdate(task.id, {
                      subtasks: subtasks.map((x) =>
                        x.id === s.id ? { ...x, text } : x,
                      ),
                    })
                  }
                  onDelete={() =>
                    onUpdate(task.id, {
                      subtasks: subtasks.filter((x) => x.id !== s.id),
                    })
                  }
                />
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
                if (e.key === "Enter" && !e.nativeEvent.isComposing)
                  commitSubtask();
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
              {/* chip 列表（与详情面板共用 TaskFileChips）：折叠态、复制/打开/× 按钮统一走组件 */}
              <TaskFileChips
                task={task}
                variant="compact"
                onOpenPath={openFilePath}
                onCopyPath={copyOneFile}
                onRemovePath={
                  archived || trashed ? undefined : removeFile
                }
              />
              {/* 绑定操作行（只留绑定类按钮；打开=点文件名、复制=chip 内「复制」字样） */}
              {!archived && !trashed && (
                <div className="flex items-center gap-2">
                  {boundFiles.length < MAX_TASK_FILES && (
                    <button
                      className="nm-btn px-2 py-0.5 text-[11px] leading-none text-[var(--t4)]"
                      title="继续绑定文件"
                      onPointerDown={stop}
                      onClick={pickFile}
                    >
                      ＋
                    </button>
                  )}
                  {/* 文件夹槽替换制：按钮常驻——已绑文件夹时再点即替换（占原位，
                      满员也可换槽不占新名额）；仅「满员且无文件夹槽」时隐藏
                      （点了也只会溢出提示） */}
                  {(boundFiles.some((f) => f.isDir) ||
                    boundFiles.length < MAX_TASK_FILES) && (
                      <button
                        className="nm-btn px-2 py-0.5 text-[11px] leading-none text-[var(--t4)]"
                        title="绑定文件夹"
                        onPointerDown={stop}
                        onClick={pickFolder}
                      >
                        <FolderOpen size={11} aria-hidden />
                      </button>
                    )}
                  <button
                    className="text-[var(--t5)] hover:text-[var(--danger)] text-sm"
                    title="解绑全部文件"
                    onPointerDown={stop}
                    onClick={() => onUpdate(task.id, filesPatch([]))}
                  >
                    ×
                  </button>
                </div>
              )}
            </div>
          ) : archived || trashed ? null : (
            <div className="mt-3 flex items-center gap-3">
              <button
                className="nm-btn px-2 py-0.5 text-xs text-[var(--t4)] flex items-center gap-1 whitespace-nowrap"
                onPointerDown={stop}
                onClick={pickFile}
              >
                <span className="text-[11px] leading-none inline-flex">
                  <Paperclip size={11} aria-hidden />
                </span>{" "}
                绑定文件
              </button>
              <button
                className="nm-btn px-2 py-0.5 text-xs text-[var(--t4)] flex items-center gap-1 whitespace-nowrap"
                onPointerDown={stop}
                onClick={pickFolder}
              >
                <span className="text-[11px] leading-none inline-flex">
                  <FolderOpen size={11} aria-hidden />
                </span>{" "}
                绑定文件夹
              </button>
            </div>
          )}

          {archived && (
            <button
              className="nm-btn mt-3 px-3 py-1 text-xs text-[var(--t3)] flex items-center gap-1 whitespace-nowrap"
              onPointerDown={stop}
              onClick={() =>
                onUpdate(task.id, { archived: false, completedAt: Date.now() })
              }
            >
              <Undo2 size={11} aria-hidden />
              恢复
            </button>
          )}

          {trashed && (
            <div className="mt-3 flex items-center gap-2">
              <button
                className="nm-btn px-3 py-1 text-xs text-[var(--t3)] flex items-center gap-1 whitespace-nowrap"
                onPointerDown={stop}
                onClick={() => onUpdate(task.id, { deletedAt: undefined })}
              >
                <Undo2 size={11} aria-hidden />
                恢复
              </button>
              <button
                className="nm-btn px-3 py-1 text-xs text-red-400 flex items-center gap-1 whitespace-nowrap"
                onPointerDown={stop}
                onClick={async () => {
                  // 绑本地文件/文件夹时弹三选项：
                  //   全部删除 / 保留文件删除 / 取消
                  // 未绑文件时保持原两选项 confirm（无需三选）
                  if (boundFiles.length > 0) {
                    setPurgeOpen(true);
                    return;
                  }
                  if (
                    !(await confirm(
                      `确定彻底删除任务「${task.title}」？\n此操作不可撤销。`,
                    ))
                  )
                    return;
                  onDelete(task.id);
                }}
              >
                <Trash2 size={11} aria-hidden />
                彻底删除
              </button>
            </div>
          )}

          {/* 🤖 交给机器人执行；归档卡只读不显示。
          定时设置已迁出卡片 → 「定时任务」模块（SchedulePage）统一管理 */}
          {!archived && !trashed && (
            <div className="mt-2 flex items-center gap-1.5">
              <button
                className="nm-btn px-2 py-0.5 text-[11px] leading-none text-[var(--t3)] flex items-center gap-1 whitespace-nowrap"
                onPointerDown={stop}
                onClick={() => runWithBot(task)}
                title="交给机器人执行这张任务卡"
              >
                <Bot size={11} aria-hidden /> 交给机器人
              </button>
              {/* 执行痕迹入口——时间线看每次工具调用（入参/结果/耗时/成败）+ 文件 diff/回滚 */}
              <button
                className="nm-btn px-2 py-0.5 text-[11px] leading-none text-[var(--t3)] flex items-center gap-1 whitespace-nowrap"
                onPointerDown={stop}
                onClick={() => setTraceOpen(true)}
                title="查看执行痕迹（工具调用时间线 / 文件 diff / 回滚）"
              >
                <History size={11} aria-hidden /> 执行详情
              </button>
            </div>
          )}

          {/* 截止时间 + 状态行（两行布局统一，老板）：
          第一行：截止时间（带 × 移除）。
          第二行：状态标签（未完成 / 完成 YYYY-MM-DD HH:mm）+ 删除按钮，两态布局一致；
            ml-2.5 空一个字符宽对齐截止时间文字，删除按钮 ml-auto 推到行尾 + text-[14px] 稳定 emoji。 */}
          <div className="mt-2">
            {/* 排期窗口（只读，归档/回收站回看用）：与任务页详情面板同一数据、同一文案口径 */}
            {(() => {
              const plan = planWindowLabel(task.planStart, task.planEnd);
              return plan ? (
                <div className="mb-1.5">
                  <span className="nm-inset px-2 py-1 text-xs text-[var(--t4)]">
                    📅 {plan}
                  </span>
                </div>
              ) : null;
            })()}
            <div className="flex items-center gap-1">
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
                            onClick={() =>
                              onUpdate(task.id, { due: undefined })
                            }
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
                </>
              )}
            </div>
            {/* 状态行：未完成 / 完成 YYYY-MM-DD HH:mm + 删除按钮（两态布局一致） */}
            {!dueEditing && !trashed && (
              <div className="mt-1 flex items-center gap-1">
                <p className="ml-2.5 text-[10px] text-[var(--t5)]">
                  {/* 完成态按完成列判（isTaskCompleted 单源）；completedAt 只管时间文案——
                      遗留 done 无 completedAt 的卡绿勾亮着状态行却写「未完成」 */}
                  {isTaskCompleted(task)
                    ? task.completedAt
                      ? formatCompletedAt(task.completedAt)
                      : "已完成"
                    : "未完成"}
                </p>
                {!archived && (
                  <button
                    className="ml-auto shrink-0 w-5 h-5 flex items-center justify-center text-[var(--t5)] hover:text-[var(--danger)]"
                    title="删除任务"
                    onPointerDown={stop}
                    onClick={() => onDelete(task.id)}
                  >
                    <Trash2 size={12} aria-hidden />
                  </button>
                )}
              </div>
            )}
          </div>
        </>
      )}

      {/* 回收站彻底删除·绑文件三选项弹窗：全部删除 / 保留文件删除 / 取消
          portal 必须开：TodoCard 容器 hover 触发 transform: translateY(-3px) scale(1.01)
          (main.css .nm-card-hover:hover) + dnd-kit useSortable 的 transform style，二者都会创建 CSS 包含块，
          使 position:fixed 子元素不再相对视口定位而被裁缩到卡片边界内。 */}
      {purgeOpen &&
        boundFiles.length > 0 && (
          <Modal
            className="fixed inset-0 z-[100] flex items-center justify-center bg-black/40 p-6"
            onClose={() => {
              if (!purgeBusy) setPurgeOpen(false);
            }}
            portal
          >
            <div
              className="nm-card w-full max-w-md p-5 flex flex-col gap-3"
              onPointerDown={stop}
            >
              <div className="flex items-center gap-1.5 text-sm font-medium text-[var(--t2)]">
                <Trash2
                  size={13}
                  aria-hidden
                  className="text-[var(--danger)]"
                />
                彻底删除任务卡
              </div>
              <div className="text-xs text-[var(--t3)] leading-relaxed">
                任务卡「
                <span className="text-[var(--t2)] font-medium">
                  {task.title}
                </span>
                」绑定了
                {boundFiles.length > 1 ? (
                  <span className="text-[var(--t2)]">
                    {" "}
                    {boundFiles.length} 个文件/文件夹
                  </span>
                ) : (
                  <>
                    <span className="text-[var(--t2)]">
                      {boundFiles[0].isDir ? "文件夹" : "文件"}
                    </span>
                    「
                    <span className="text-[var(--t2)]">
                      {basename(boundFiles[0].path)}
                    </span>
                    」
                  </>
                )}
                。
              </div>
              <div
                className="text-[11px] text-[var(--t5)] break-all px-2 py-1.5 rounded bg-[var(--bg)] border border-[var(--bd)] max-h-28 overflow-y-auto whitespace-pre-line"
                title={boundFiles.map((f) => f.path).join("\n")}
              >
                完整路径：{boundFiles.length > 1 ? "\n" : ""}
                {boundFiles.map((f) => f.path).join("\n")}
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
                    // 多文件绑定：逐个移入废纸篓/回收站。全部尝试完再汇总——
                    // 遇错就中断只会留下「删了一半」的任意前缀，用户既不知道
                    // 哪些已经进了回收站，也没法只处理剩下那几个
                    //  #4：删 is_dir 参数（trash::delete 内部递归）；Rust IPC 表面只接 path
                    const failed: string[] = [];
                    for (const f of boundFiles) {
                      try {
                        // 顺序删除是有意为之（保持失败顺序可观测），不并行
                        // oxlint-disable-next-line eslint/no-await-in-loop
                        await invoke("delete_bound_file", {
                          path: f.path,
                          // 带卡 id：后端据此判归属 + 拒绝删「别的卡/工作区
                          // 也绑着」的文件（绑集是全局的，只看白名单会把
                          // 别人的引用一起变成死链）
                          taskId: task.id,
                        });
                      } catch (e) {
                        failed.push(`${basename(f.path)}：${formatCommandError(e)}`);
                      }
                    }
                    if (failed.length === 0) {
                      onDelete(task.id);
                      setPurgeOpen(false);
                      return;
                    }
                    const doneCount = boundFiles.length - failed.length;
                    // 有失败：一次说清「删了几个、哪些没删成、为什么」，任务卡保留
                    // 在回收站可重试。不再叠一个 handleCommandError 弹窗——两份
                    // 弹窗等于让用户对着同一件事点两次确认
                    alert(
                      `已移入回收站 ${doneCount}/${boundFiles.length} 个，任务卡保留在回收站可重试。\n\n未删除：\n${failed.join("\n")}`,
                    );
                    setPurgeBusy(false);
                  }}
                >
                  <span className="flex items-center gap-1 font-medium">
                    <Trash2 size={11} aria-hidden />
                    全部删除
                  </span>
                  <span className="text-[10px] text-[var(--t4)] font-normal">
                    任务卡删除，并把绑定的本地
                    {boundFiles.length > 1
                      ? "文件/文件夹"
                      : boundFiles[0].isDir
                        ? "文件夹"
                        : "文件"}
                    移到废纸篓/回收站
                  </span>
                </button>
                <button
                  className="nm-btn px-3 py-2 text-xs text-[var(--t2)] flex flex-col items-start gap-0.5 disabled:opacity-50"
                  disabled={purgeBusy}
                  onClick={() => {
                    onDelete(task.id);
                    setPurgeOpen(false);
                  }}
                >
                  <span className="font-medium inline-flex items-center gap-1">
                    <FileText size={11} aria-hidden /> 保留文件删除
                  </span>
                  <span className="text-[10px] text-[var(--t4)] font-normal">
                    只删除任务卡，本地
                    {boundFiles.length > 1
                      ? "文件/文件夹"
                      : boundFiles[0].isDir
                        ? "文件夹"
                        : "文件"}
                    保留
                  </span>
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
          </Modal>
        )}

      {/* 执行痕迹弹层：TracePanel 自带 portal 到 body——同 purge 弹窗的
          包含块裁缩问题，必须 portal 渲染 */}
      {traceOpen && (
        <TracePanel
          taskId={task.id}
          taskTitle={task.title}
          onClose={() => setTraceOpen(false)}
        />
      )}
    </div>
  );
}

/** 归档/回收站版：普通可拖拽卡片（无排序上下文，保留原 useDraggable 行为） */
export function TodoCard(props: TodoCardViewProps) {
  const { attributes, listeners, setNodeRef, transform, isDragging } =
    useDraggable({
      id: props.task.id,
    });
  const style = transform
    ? { transform: CSS.Translate.toString(transform) }
    : undefined;
  return (
    <TodoCardView
      {...props}
      drag={{ attributes, listeners, setNodeRef, style, isDragging }}
    />
  );
}

