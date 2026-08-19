import { useEffect, useRef, useState } from "react";
import type { DraggableSyntheticListeners } from "@dnd-kit/core";
import type { Task } from "../types";
import { basename, formatDue, formatSchedule, scheduleToDatetime } from "../format";
import { DoneCircle } from "./DoneCircle";
import { FoldToggle } from "./FoldToggle";
import { ActorAvatar } from "./ActorAvatar";

/**
 * 任务卡展示内容 —— 供挂件（WidgetApp）使用。
 *
 * ⚠️ 字段与顺序必须与 TodoCard 一致（老板要求挂件与主窗口显示一致）：
 * 标题行（标题 + 折叠开关 + 打勾圆圈；折叠时标题单行截断） → 备注 → 标签 → 子任务 → 文件 → 截止时间（截止永远最底）。
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
  onBotExecute,
  onSetSchedule,
  /** 拖拽排序手柄的 dnd listeners（挂件排序；不传则手柄仅展示） */
  handleListeners,
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
  /** 提供时显示 🤖 交给机器人按钮 */
  onBotExecute?: () => void;
  /** 提供时显示 ⏰ 定时执行按钮 */
  onSetSchedule?: (schedule: string | undefined) => void;
  /** 拖拽排序手柄的 dnd listeners（挂件排序；不传则手柄仅展示） */
  handleListeners?: DraggableSyntheticListeners | undefined;
}) {
  const stop = (e: React.PointerEvent) => e.stopPropagation();
  const subtasks = task.subtasks ?? [];
  const doneCount = subtasks.filter((s) => s.done).length;
  // 定时执行面板
  const [schedOpen, setSchedOpen] = useState(false);
  const [schedOnce, setSchedOnce] = useState("");

  // 进入编辑态时初始化草稿
  const [draft, setDraft] = useState(task.title);
  useEffect(() => {
    if (editingTitle) setDraft(task.title);
  }, [editingTitle]);
  // E4（2026-08-19）：Escape 取消标记——Escape 只取消内存草稿，但随后的 blur
  // 会再触发 onCommit 把草稿写库，等于 Escape 没生效；取消后 blur 必须跳过提交
  const cancelledRef = useRef(false);
  /** blur 提交：Escape 已取消则跳过并重置标记（不污染下一轮编辑） */
  const commitOnBlur = () => {
    if (cancelledRef.current) {
      cancelledRef.current = false;
      return;
    }
    onCommitTitle?.(draft);
  };
  // 挂件任务卡永远显示折叠键（老板 2026-08-17 12:33 指令：复用现有 FoldToggle，
  // 不重新设计折叠窗口）：折叠态只露标题（单行截断），展开态显示标题完整 + 🤖 + ⏰ + 其他内容

  return (
    <>
      {/* 标题行：标题（可编辑） + 打勾圆圈（☰ 拖拽手柄标题左侧占位） */}
      <div className="flex items-start gap-2">
        {/* ☰ 拖拽手柄：标题左侧占位，卡片悬停才显现；与标题保持间距 */}
        {handleListeners && (
          <span
            {...handleListeners}
            title="拖拽移动"
            className="shrink-0 mt-0.5 w-4 h-4 flex items-center justify-center text-[12px] leading-none text-[var(--t5)] rounded hover:bg-[var(--hover-bg)] opacity-0 group-hover:opacity-100 transition-opacity cursor-grab active:cursor-grabbing"
          >
            ☰
          </span>
        )}
        {editingTitle ? (
          <input
            autoFocus
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            onBlur={commitOnBlur}
            onKeyDown={(e) => {
              if (e.key === "Enter" && !e.nativeEvent.isComposing) {
                cancelledRef.current = false; // 显式提交，重置取消标记
                onCommitTitle?.(draft);
              }
              if (e.key === "Escape") {
                cancelledRef.current = true;
                onCancelTitle?.();
              }
            }}
            onPointerDown={stop}
            onClick={(e) => e.stopPropagation()}
            className="flex-1 min-w-0 rounded-lg bg-[var(--input-bg)] px-2 py-1 outline-none nm-task-title"
          />
        ) : (
          <h3
            className={`flex-1 min-w-0 nm-task-title ${
              task.collapsed ? "truncate" : ""
            } ${onTitleClick ? "cursor-text" : ""}`}
            title={
              task.collapsed
                ? task.title
                : onTitleClick
                  ? "双击去主窗口编辑"
                  : undefined
            }
            onPointerDown={onTitleClick ? stop : undefined}
            onDoubleClick={
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
        {/* 折叠/展开开关：挂件永远显示（包括新建空任务），复用现有 FoldToggle 不重新设计
            （老板 2026-08-17 12:33 指令） */}
        {onToggleCollapsed && (
          <FoldToggle collapsed={!!task.collapsed} onToggle={onToggleCollapsed} />
        )}
        {onToggleDone && (
          <DoneCircle done={task.column === "done"} onToggle={onToggleDone} />
        )}
        {/* 归属头像：交给机器人 → 机器人头像；否则用户头像。悬停显示姓名（与主窗口一致） */}
        <ActorAvatar bot={!!task.botAssigned} />
      </div>

      {/* 标题以下内容（可折叠） */}
      {!task.collapsed && (
        <>
          {/* 标题下的小字备注 */}
          {task.note && <p className="mt-1.5 text-xs text-[var(--t4)]">{task.note}</p>}

          {/* 标签 */}
          {(task.tags ?? []).length > 0 && (
            <div className="mt-2 flex flex-wrap items-center gap-1.5">
              {(task.tags ?? []).map((tag, i) => (
                <span
                  key={`${tag}-${i}`}
                  className="nm-inset px-2 py-0.5 text-xs text-[var(--t4)]"
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
                    className={`shrink-0 w-3.5 h-3.5 accent-[var(--brand)] ${
                      onToggleSubtask ? "cursor-pointer" : ""
                    }`}
                  />
                  <span
                    className={`flex-1 text-xs ${
                      s.done ? "text-[var(--t5)] line-through" : "text-[var(--t3)]"
                    }`}
                  >
                    {s.text}
                  </span>
                </div>
              ))}
              <p className="text-[10px] text-[var(--t5)] tabular-nums">
                {doneCount}/{subtasks.length}
              </p>
            </div>
          )}

          {/* 绑定文件（展示 + 打开/复制按钮，与主窗口一致；点击按钮不触发卡片聚焦） */}
          {task.filePath && (
            <div className="mt-3 flex flex-col gap-2">
              <p className="text-xs text-[var(--t4)] truncate" title={task.filePath}>
                {task.fileIsDir ? "📁" : "📎"} {basename(task.filePath)}
              </p>
              {(onOpenFile || onCopyFile) && (
                <div className="flex items-center gap-2">
                  {onOpenFile && (
                    <button
                      className="nm-btn px-2 py-0.5 text-[11px] leading-none text-[var(--t3)]"
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
                      className="nm-btn px-2 py-0.5 text-[11px] leading-none text-[var(--t3)]"
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

          {/* 🤖 交给机器人 + ⏰ 定时：在折叠段内，复用现有 FoldToggle
              （老板 2026-08-17 12:33 指令：不重新设计折叠窗口，折叠态只露标题，
               展开态显示标题完整 + 🤖 + ⏰ + 其他内容） */}
          {(onBotExecute || onSetSchedule) && (
            <div className="mt-3 flex items-center gap-1.5">
              {onBotExecute && (
                <button
                  className="nm-btn px-2 py-0.5 text-[11px] leading-none text-[var(--t3)] flex items-center gap-1"
                  onPointerDown={stop}
                  onClick={(e) => {
                    e.stopPropagation();
                    onBotExecute();
                  }}
                  title="交给机器人执行这张任务卡"
                >
                  🤖 交给机器人
                </button>
              )}
              {onSetSchedule && (
                <button
                  className={`nm-btn px-2 py-0.5 text-[11px] leading-none ${
                    task.schedule ? "text-[var(--brand)]" : "text-[var(--t3)]"
                  }`}
                  onPointerDown={stop}
                  onClick={(e) => {
                    e.stopPropagation();
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
              )}
            </div>
          )}
          {schedOpen && onSetSchedule && (
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
                    onClick={(e) => {
                      e.stopPropagation();
                      if (!schedOnce) return;
                      const dt = schedOnce.slice(0, 16);
                      // 防御：无效日期（手动输入不完整等）不写库，防 NaN 进 schedule
                      if (isNaN(new Date(dt).getTime())) return;
                      const hm = dt.slice(11, 16);
                      if (kind === "once") {
                        onSetSchedule(`at:${dt}`);
                      } else if (kind === "daily") {
                        onSetSchedule(`daily:${hm}`);
                      } else if (kind === "weekly") {
                        // 取所选日期的星期几（1=周一 ... 7=周日）
                        const wd = ((new Date(dt).getDay() + 6) % 7) + 1;
                        onSetSchedule(`weekly:${wd}:${hm}`);
                      } else {
                        onSetSchedule(`monthly:${dt.slice(8, 10)}:${hm}`);
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
                    onClick={(e) => {
                      e.stopPropagation();
                      onSetSchedule(undefined);
                      setSchedOpen(false);
                    }}
                  >
                    取消
                  </button>
                )}
              </div>
            </div>
          )}

          {/* 截止时间 —— 永远在最下面 */}
          {task.due && (
            <div className="mt-2">
              <span className="nm-inset px-2 py-1 text-xs text-[var(--t4)]">
                {formatDue(task.due)}
              </span>
            </div>
          )}
        </>
      )}
    </>
  );
}
