import { useEffect, useState } from "react";
import type { Task } from "../types";
import { basename, formatDue } from "../format";
import { DoneCircle } from "./DoneCircle";
import { FoldToggle } from "./FoldToggle";

/**
 * 任务卡展示内容 —— 供挂件（WidgetApp）使用。
 *
 * ⚠️ 字段与顺序必须与 TodoCard 一致（老板要求挂件与主窗口显示一致）：
 * 标题行（折叠按钮 + 标题 + 打勾圆圈） → 备注 → 标签 → 子任务 → 文件 → 截止时间（截止永远最底）。
 * 标题以下内容可折叠。改动 TodoCard 展示时记得同步这里。
 */
export function TaskCardContent({
  task,
  editingTitle = false,
  onTitleClick,
  onCommitTitle,
  onCancelTitle,
  onToggleDone,
  onToggleCollapsed,
  onToggleSubtask,
  onOpenFile,
  onCopyFile,
}: {
  task: Task;
  /** 标题编辑态（新建任务后自动进入） */
  editingTitle?: boolean;
  /** 提供时标题可点击进入编辑态（与主窗口一致） */
  onTitleClick?: () => void;
  /** 标题编辑提交（blur/Enter） */
  onCommitTitle?: (title: string) => void;
  /** 标题编辑取消（Escape） */
  onCancelTitle?: () => void;
  /** 提供时显示标题右侧完成圆圈（点击完成/取消完成，与主窗口一致） */
  onToggleDone?: () => void;
  /** 提供时显示标题左侧折叠按钮（点击折叠/展开标题以下内容，与主窗口一致） */
  onToggleCollapsed?: () => void;
  /** 提供时子任务 checkbox 可勾选（与主窗口一致） */
  onToggleSubtask?: (subtaskId: string) => void;
  /** 提供时显示 📂 打开文件按钮 */
  onOpenFile?: () => void;
  /** 提供时显示 📋 复制文件按钮 */
  onCopyFile?: () => void;
}) {
  const stop = (e: React.PointerEvent) => e.stopPropagation();
  const subtasks = task.subtasks ?? [];
  const doneCount = subtasks.filter((s) => s.done).length;

  // 进入编辑态时初始化草稿
  const [draft, setDraft] = useState(task.title);
  useEffect(() => {
    if (editingTitle) setDraft(task.title);
  }, [editingTitle]);
  // 标题以下有实际内容时才显示折叠按钮（挂件是纯展示，空卡没有可折叠的内容）
  const hasBelow = !!(
    task.note ||
    (task.tags ?? []).length > 0 ||
    subtasks.length > 0 ||
    task.filePath ||
    task.due
  );

  return (
    <>
      {/* 标题行：折叠按钮 + 标题（可编辑） + 打勾圆圈 */}
      <div className="flex items-start gap-2">
        {hasBelow && onToggleCollapsed && (
          <FoldToggle collapsed={!!task.collapsed} onToggle={onToggleCollapsed} />
        )}
        {editingTitle ? (
          <input
            autoFocus
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            onBlur={() => onCommitTitle?.(draft)}
            onKeyDown={(e) => {
              if (e.key === "Enter") onCommitTitle?.(draft);
              if (e.key === "Escape") onCancelTitle?.();
            }}
            onPointerDown={stop}
            onClick={(e) => e.stopPropagation()}
            className="flex-1 min-w-0 rounded-lg bg-white/70 px-2 py-1 outline-none nm-task-title"
          />
        ) : (
          <h3
            className={`flex-1 nm-task-title ${onTitleClick ? "cursor-text" : ""}`}
            title={onTitleClick ? "点击去主窗口编辑" : undefined}
            onPointerDown={onTitleClick ? stop : undefined}
            onClick={
              onTitleClick
                ? (e) => {
                    e.stopPropagation();
                    onTitleClick();
                  }
                : undefined
            }
          >
            {task.title}
          </h3>
        )}
        {onToggleDone && (
          <DoneCircle done={task.column === "done"} onToggle={onToggleDone} />
        )}
      </div>

      {/* 标题以下内容（可折叠） */}
      {!task.collapsed && (
        <>
          {/* 标题下的小字备注 */}
          {task.note && <p className="mt-1.5 text-xs text-gray-500">{task.note}</p>}

          {/* 标签 */}
          {(task.tags ?? []).length > 0 && (
            <div className="mt-2 flex flex-wrap items-center gap-1.5">
              {(task.tags ?? []).map((tag, i) => (
                <span
                  key={`${tag}-${i}`}
                  className="nm-inset px-2 py-0.5 text-xs text-gray-500"
                >
                  {tag}
                </span>
              ))}
            </div>
          )}

          {/* 子任务清单（checkbox 可勾选，与主窗口一致） */}
          {subtasks.length > 0 && (
            <div className="mt-2 flex flex-col gap-1">
              {subtasks.map((s) => (
                <div key={s.id} className="flex items-center gap-2">
                  <input
                    type="checkbox"
                    checked={s.done}
                    disabled={!onToggleSubtask}
                    onChange={() => onToggleSubtask?.(s.id)}
                    onPointerDown={stop}
                    onClick={(e) => e.stopPropagation()}
                    className={`shrink-0 w-3.5 h-3.5 accent-gray-500 ${
                      onToggleSubtask ? "cursor-pointer" : ""
                    }`}
                  />
                  <span
                    className={`flex-1 text-xs ${
                      s.done ? "text-gray-400 line-through" : "text-gray-600"
                    }`}
                  >
                    {s.text}
                  </span>
                </div>
              ))}
              <p className="text-[10px] text-gray-400 tabular-nums">
                {doneCount}/{subtasks.length}
              </p>
            </div>
          )}

          {/* 绑定文件（展示 + 打开/复制按钮，与主窗口一致；点击按钮不触发卡片聚焦） */}
          {task.filePath && (
            <div className="mt-3 flex flex-col gap-2">
              <p className="text-xs text-gray-500 truncate" title={task.filePath}>
                {task.fileIsDir ? "📁" : "📎"} {basename(task.filePath)}
              </p>
              {(onOpenFile || onCopyFile) && (
                <div className="flex items-center gap-2">
                  {onOpenFile && (
                    <button
                      className="nm-btn px-2 py-0.5 text-[11px] leading-none text-gray-600"
                      title={task.fileIsDir ? "打开文件夹" : "打开文件"}
                      onPointerDown={stop}
                      onClick={(e) => {
                        e.stopPropagation();
                        onOpenFile();
                      }}
                    >
                      📂
                    </button>
                  )}
                  {onCopyFile && (
                    <button
                      className="nm-btn px-2 py-0.5 text-[11px] leading-none text-gray-600"
                      title="复制文件+标题"
                      onPointerDown={stop}
                      onClick={(e) => {
                        e.stopPropagation();
                        onCopyFile();
                      }}
                    >
                      📋
                    </button>
                  )}
                </div>
              )}
            </div>
          )}

          {/* 截止时间 —— 永远在最下面 */}
          {task.due && (
            <div className="mt-2">
              <span className="nm-inset px-2 py-1 text-xs text-gray-500">
                {formatDue(task.due)}
              </span>
            </div>
          )}
        </>
      )}
    </>
  );
}
