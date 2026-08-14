import { useEffect, useState } from "react";
import { useDraggable } from "@dnd-kit/core";
import { CSS } from "@dnd-kit/utilities";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { openPath } from "@tauri-apps/plugin-opener";
import type { Task } from "../types";
import { basename, formatDue } from "../format";
import { DoneCircle } from "./DoneCircle";
import { FoldToggle } from "./FoldToggle";

const stop = (e: React.PointerEvent) => e.stopPropagation();

export function TodoCard({
  task,
  autoEdit = false,
  onUpdate,
  onDelete,
  archived = false,
  trashed = false,
}: {
  task: Task;
  autoEdit?: boolean;
  onUpdate: (id: string, patch: Partial<Task>) => void;
  onDelete: (id: string) => void;
  /** 归档视图：显示「恢复」按钮 */
  archived?: boolean;
  /** 回收站视图：显示「恢复 / 彻底删除」按钮 */
  trashed?: boolean;
}) {
  const { attributes, listeners, setNodeRef, transform, isDragging } = useDraggable({
    id: task.id,
  });
  const style = transform ? { transform: CSS.Translate.toString(transform) } : undefined;

  const [editing, setEditing] = useState(autoEdit);
  const [draft, setDraft] = useState(task.title);
  const [dueEditing, setDueEditing] = useState(false);
  const [noteEditing, setNoteEditing] = useState(false);
  const [noteDraft, setNoteDraft] = useState("");
  const [tagEditing, setTagEditing] = useState(false);
  const [tagDraft, setTagDraft] = useState("");
  const [addingSubtask, setAddingSubtask] = useState(false);
  const [subtaskDraft, setSubtaskDraft] = useState("");

  useEffect(() => {
    if (autoEdit) setEditing(true);
  }, [autoEdit]);

  const commitTitle = () => {
    const title = draft.trim() || task.title;
    if (title !== task.title) onUpdate(task.id, { title });
    setEditing(false);
  };

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
      const selected = await open({ multiple: false, directory: false });
      if (typeof selected === "string")
        onUpdate(task.id, { filePath: selected, fileIsDir: false });
    } catch (e) {
      console.error("pick file failed", e);
    }
  };

  const pickFolder = async () => {
    try {
      const selected = await open({ directory: true });
      if (typeof selected === "string")
        onUpdate(task.id, { filePath: selected, fileIsDir: true });
    } catch (e) {
      console.error("pick folder failed", e);
    }
  };

  // 标题右侧圆圈：待办/今日 → 完成（记完成时间）；完成 → 退回待办
  const toggleDone = () => {
    if (task.column === "done") {
      onUpdate(task.id, { column: "todo", completedAt: undefined, archived: undefined });
    } else {
      onUpdate(task.id, { column: "done", completedAt: Date.now(), archived: false });
    }
  };

  // 标题以下内容折叠/展开
  const toggleCollapsed = () => {
    onUpdate(task.id, { collapsed: !task.collapsed });
  };

  const openFile = () => {
    if (task.filePath)
      openPath(task.filePath).catch((e) => console.error("open failed", e));
  };

  const copyFile = () => {
    if (task.filePath)
      invoke("copy_file_with_title", { path: task.filePath, title: task.title }).catch((e) =>
        console.error("copy failed", e)
      );
  };

  const subtasks = task.subtasks ?? [];
  const doneCount = subtasks.filter((s) => s.done).length;

  return (
    <div
      ref={setNodeRef}
      style={style}
      {...attributes}
      {...listeners}
      className={`nm-card-hover p-4 w-full select-none ${isDragging ? "opacity-70" : ""}`}
    >
      <div className="flex items-start gap-2">
        <FoldToggle collapsed={!!task.collapsed} onToggle={toggleCollapsed} />
        {editing ? (
          <input
            autoFocus
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            onBlur={commitTitle}
            onKeyDown={(e) => {
              if (e.key === "Enter") commitTitle();
              if (e.key === "Escape") {
                setDraft(task.title);
                setEditing(false);
              }
            }}
            onPointerDown={stop}
            className="flex-1 min-w-0 rounded-lg bg-white/70 px-2 py-1 outline-none nm-task-title"
          />
        ) : (
          <h3
            className="flex-1 cursor-text nm-task-title"
            title="点击编辑"
            onPointerDown={stop}
            onClick={() => {
              setDraft(task.title);
              setEditing(true);
            }}
          >
            {task.title}
          </h3>
        )}
        {!archived && !trashed && (
          <DoneCircle done={task.column === "done"} onToggle={toggleDone} />
        )}
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
            if (e.key === "Enter") commitNote();
            if (e.key === "Escape") {
              setNoteDraft(task.note ?? "");
              setNoteEditing(false);
            }
          }}
          onPointerDown={stop}
          placeholder="备注…"
          className="mt-1.5 w-full rounded-lg bg-white/70 px-2 py-1 text-xs text-gray-600 outline-none"
        />
      ) : task.note ? (
        <p
          className="mt-1.5 text-xs text-gray-500 cursor-text"
          title="点击编辑备注"
          onPointerDown={stop}
          onClick={() => {
            setNoteDraft(task.note!);
            setNoteEditing(true);
          }}
        >
          {task.note}
        </p>
      ) : (
        <button
          className="mt-1.5 text-xs text-gray-300 hover:text-gray-500"
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
            className="nm-inset px-2 py-0.5 text-xs text-gray-500 flex items-center gap-1"
          >
            {tag}
            <button
              className="text-gray-400 hover:text-red-500 leading-none"
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
          </span>
        ))}
        {tagEditing ? (
          <input
            autoFocus
            value={tagDraft}
            onChange={(e) => setTagDraft(e.target.value)}
            onBlur={() => addTag(true)}
            onKeyDown={(e) => {
              if (e.key === "Enter") addTag(false);
              if (e.key === "Escape") {
                setTagDraft("");
                setTagEditing(false);
              }
            }}
            onPointerDown={stop}
            placeholder="标签名"
            className="w-20 rounded-lg bg-white/70 px-2 py-0.5 text-xs text-gray-700 outline-none"
          />
        ) : (
          <button
            className="text-xs text-gray-300 hover:text-gray-500"
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

      {/* 子任务清单 */}
      {subtasks.length > 0 && (
        <div className="mt-2 flex flex-col gap-1">
          {subtasks.map((s) => (
            <div key={s.id} className="flex items-center gap-2 group">
              <input
                type="checkbox"
                checked={s.done}
                onChange={() =>
                  onUpdate(task.id, {
                    subtasks: subtasks.map((x) =>
                      x.id === s.id ? { ...x, done: !x.done } : x
                    ),
                  })
                }
                onPointerDown={stop}
                className="shrink-0 w-3.5 h-3.5 accent-gray-500"
              />
              <span
                className={`flex-1 text-xs ${
                  s.done ? "text-gray-400 line-through" : "text-gray-600"
                }`}
              >
                {s.text}
              </span>
              <button
                className="opacity-0 group-hover:opacity-100 text-gray-400 hover:text-red-500 text-xs"
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
            if (e.key === "Enter") commitSubtask();
            if (e.key === "Escape") {
              setSubtaskDraft("");
              setAddingSubtask(false);
            }
          }}
          onPointerDown={stop}
          placeholder="子任务…"
          className="mt-2 w-full rounded-lg bg-white/70 px-2 py-1 text-xs text-gray-700 outline-none"
        />
      ) : (
        <button
          className="mt-2 text-xs text-gray-400 hover:text-gray-600"
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

      {task.filePath ? (
        <div className="mt-3 flex flex-col gap-2">
          <p className="text-xs text-gray-500 truncate" title={task.filePath}>
            {task.fileIsDir ? "📁" : "📎"} {basename(task.filePath)}
          </p>
          <div className="flex items-center gap-2">
            <button
              className="nm-inset px-2 py-0.5 text-[11px] leading-none text-gray-600"
              title={task.fileIsDir ? "打开文件夹" : "打开文件"}
              onPointerDown={stop}
              onClick={openFile}
            >
              📂
            </button>
            <button
              className="nm-inset px-2 py-0.5 text-[11px] leading-none text-gray-600"
              title="复制文件+标题"
              onPointerDown={stop}
              onClick={copyFile}
            >
              📋
            </button>
            <button
              className="text-gray-400 hover:text-red-500 text-sm"
              title="解绑文件"
              onPointerDown={stop}
              onClick={() => onUpdate(task.id, { filePath: undefined, fileIsDir: undefined })}
            >
              ×
            </button>
          </div>
        </div>
      ) : (
        <div className="mt-3 flex items-center gap-3">
          <button
            className="text-xs text-gray-300 hover:text-gray-500"
            onPointerDown={stop}
            onClick={pickFile}
          >
            <span className="text-[11px] leading-none">📎</span> 绑定文件
          </button>
          <button
            className="text-xs text-gray-300 hover:text-gray-500"
            onPointerDown={stop}
            onClick={pickFolder}
          >
            <span className="text-[11px] leading-none">📁</span> 绑定文件夹
          </button>
        </div>
      )}

      {archived && (
        <button
          className="nm-inset mt-3 px-3 py-1 text-xs text-gray-600"
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
            className="nm-inset px-3 py-1 text-xs text-gray-600"
            onPointerDown={stop}
            onClick={() => onUpdate(task.id, { deletedAt: undefined })}
          >
            ↩ 恢复
          </button>
          <button
            className="nm-inset px-3 py-1 text-xs text-red-400"
            onPointerDown={stop}
            onClick={() => onDelete(task.id)}
          >
            🗑 彻底删除
          </button>
        </div>
      )}

      {/* 截止时间 —— 永远在最下面，删除按钮在其右侧 */}
      <div className="flex items-center gap-1 mt-2">
        {dueEditing ? (
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
            onChange={(e) =>
              onUpdate(task.id, { due: e.target.value.slice(0, 16) || undefined })
            }
            onBlur={() => setDueEditing(false)}
            onPointerDown={stop}
            className="nm-inset px-2 py-1 text-xs text-gray-600 flex-1 min-w-0"
          />
        ) : task.due ? (
          <>
            <button
              className="nm-inset px-2 py-1 text-xs text-gray-500"
              onPointerDown={stop}
              onClick={() => setDueEditing(true)}
            >
              {formatDue(task.due)}
            </button>
            <button
              className="w-5 h-6 text-xs text-gray-400 hover:text-red-500"
              title="移除截止时间"
              onPointerDown={stop}
              onClick={() => onUpdate(task.id, { due: undefined })}
            >
              ×
            </button>
          </>
        ) : (
          <button
            className="nm-inset px-2 py-1 text-xs text-gray-500"
            onPointerDown={stop}
            onClick={() => setDueEditing(true)}
          >
            + 截止时间
          </button>
        )}

        {/* 删除任务：emoji 小图标，截止日期右侧（回收站视图已有「彻底删除」，不重复显示） */}
        {!trashed && (
          <button
            className="ml-auto shrink-0 w-5 h-5 flex items-center justify-center text-xs leading-none text-gray-400 hover:text-red-500"
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
    </div>
  );
}
