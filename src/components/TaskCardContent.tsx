import type { Task } from "../types";
import { formatCompletedAt, formatDue } from "../format";
import {
  Bot,
  Puzzle,
  Timer,
  TriangleAlert,
} from "lucide-react";
import { DoneCircle } from "./DoneCircle";
import { FoldToggle } from "./FoldToggle";
import { ActorAvatar } from "./ActorAvatar";
import { useInlineEdit } from "./useInlineEdit";
import { planWindowLabel } from "./TaskTimeline/week";
import { TaskFileChips } from "./task/TaskFileChips";
import { SubtaskRow } from "./task/SubtaskRow";

/**
 * 任务卡展示内容 —— 供挂件（WidgetApp）使用。
 *
 * ⚠️ 字段与顺序必须与主窗口一致：
 * 色条（左缘，TaskCard 外壳）→ 标题行（标题 + 折叠开关 + 打勾圆圈；折叠时标题单行截断）
 * → 备注 → 标签 → 子任务 → 文件 → 🤖 → 截止时间 → 完成时间（截止永远最底，完成时间在截止时间下一行）。
 * 标题以下内容可折叠。改动主窗口任务卡展示时记得同步这里。
 * 定时设置已迁出卡片 → 「定时任务」模块（SchedulePage）统一管理。
 */
export function TaskCardContent({
  task,
  dueOverdue = false,
  editingTitle = false,
  onTitleClick,
  onCommitTitle,
  onCancelTitle,
  onToggleDone,
  onToggleCollapsed,
  onToggleSubtask,
  onOpenFilePath,
  onCopyFilePath,
  onRemoveFile,
  onBotExecute,
  onOpenSubtaskCard,
}: {
  task: Task;
  /** 截止时间已过且未完成：截止徽标标红（与首页到期横条同一判定，由父层传入） */
  dueOverdue?: boolean;
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
  /** 点绑定文件名/文件夹名直接打开（替代原 📂 按钮 + 多选列表） */
  onOpenFilePath?: (path: string) => void;
  /** chip 内「复制」字样：复制该文件+标题（替代原 📋 按钮） */
  onCopyFilePath?: (path: string) => void;
  /** 提供时每个绑定文件 chip 显示单独移除按钮（×） */
  onRemoveFile?: (path: string) => void;
  /** 提供时显示 🤖 交给机器人按钮 */
  onBotExecute?: () => void;
  /** 子任务行已生成任务卡（sub.taskId 在场）时显示 ↗；点击跳主窗口编辑页 */
  onOpenSubtaskCard?: (cardId: string) => void;
}) {
  const stop = (e: React.PointerEvent) => e.stopPropagation();
  const subtasks = task.subtasks ?? [];
  const doneCount = subtasks.filter((s) => s.done).length;

  // 标题内联编辑：草稿 + Enter/Escape/Blur 行为统一走 useInlineEdit（与主窗口共用）
  const titleEdit = useInlineEdit({
    value: task.title,
    editing: editingTitle,
    onCommit: (d) => onCommitTitle?.(d),
    onCancel: () => onCancelTitle?.(),
  });
  // 挂件任务卡永远显示折叠键（老板指令：复用现有 FoldToggle，
  // 不重新设计折叠窗口）：折叠态只露标题（单行截断），展开态显示标题完整 + 🤖 + 其他内容

  return (
    <>
      {/* 标题行：标题（可编辑） + 打勾圆圈 */}
      <div className="flex items-start gap-2">
        {editingTitle ? (
          <input
            autoFocus
            value={titleEdit.draft}
            onChange={(e) => titleEdit.setDraft(e.target.value)}
            onBlur={titleEdit.onBlur}
            onKeyDown={titleEdit.onKeyDown}
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
         */}
        {onToggleCollapsed && (
          <FoldToggle
            collapsed={!!task.collapsed}
            onToggle={onToggleCollapsed}
          />
        )}
        {onToggleDone && (
          <DoneCircle done={task.column === "done"} onToggle={onToggleDone} />
        )}
        {/* 归属头像规则：设了定时 → 一直机器人头像；
            🤖 执行中 → 机器人头像；执行完（botAssigned 清除且无定时）→ 用户头像。
            悬停显示姓名（与主窗口一致） */}
        <ActorAvatar bot={!!task.botAssigned || !!task.schedule} />
      </div>

      {/* 标题以下内容（可折叠） */}
      {!task.collapsed && (
        <>
          {/* 标题下的小字备注 */}
          {task.note && (
            <p className="mt-1.5 text-xs text-[var(--t4)]">{task.note}</p>
          )}

          {/* 每卡验收标准（W-QA 卡即契约）：AI 拆解生成的一行可验证完成标准 */}
          {task.acceptance && (
            <p
              className="mt-1.5 text-xs text-[var(--t4)]"
              title="验收标准：机器人执行时会对照此项自检"
            >
              <span aria-hidden>📌</span> 验收：{task.acceptance}
            </p>
          )}

          {/* 标签 */}
          {(task.tags ?? []).length > 0 && (
            <div className="mt-2 flex flex-wrap items-center gap-1.5">
              {(task.tags ?? []).map((tag, i) => (
                <span
                  // 键带序号防重复标签撞 key（标签可手输，非天然唯一）
                  // oxlint-disable-next-line react/no-array-index-key
                  key={`${tag}-${i}`}
                  className="nm-inset px-2 py-0.5 text-xs text-[var(--t4)]"
                >
                  {tag}
                </span>
              ))}
            </div>
          )}

          {/* 子 agent 编排：预算徽标 + 收尾结果折叠展示。
              两个字段均可选——普通任务卡无此信息不渲染（undefined 安全） */}
          {(task.budget || task.result) && (
            <div className="mt-2 flex flex-col gap-1.5">
              {task.budget && (
                <div className="flex flex-wrap items-center gap-1.5 text-xs text-[var(--t4)]">
                  <span
                    className="nm-inset px-2 py-0.5 inline-flex items-center gap-1 whitespace-nowrap"
                    title={`预算：轮数 ${task.budget.maxTurns} / 工具调用 ${task.budget.maxToolCalls} / 墙钟 ${task.budget.maxWallSeconds}s`}
                  >
                    <Timer size={10} aria-hidden />
                    {task.budget.maxTurns}轮·{task.budget.maxWallSeconds}s
                  </span>
                </div>
              )}
              {task.result && (
                <details className="nm-inset rounded-lg px-2 py-1.5 text-xs">
                  <summary className="cursor-pointer select-none text-[var(--t4)]">
                    <Puzzle
                      size={11}
                      aria-hidden
                      className="inline-block align-[-2px]"
                    />{" "}
                    收尾结果
                    {typeof task.result.status === "string" && (
                      <span className="ml-1">
                        （{task.result.status}
                        {typeof task.result.confidence === "number" &&
                          ` · 置信 ${task.result.confidence}`}
                        ）
                      </span>
                    )}
                  </summary>
                  {task.result.summary && (
                    <p className="mt-1 whitespace-pre-wrap break-words text-[var(--t4)]">
                      {task.result.summary}
                    </p>
                  )}
                  {(task.result.blockers ?? []).length > 0 && (
                    <p className="mt-1 text-[var(--t4)]">
                      <TriangleAlert
                        size={11}
                        aria-hidden
                        className="inline-block align-[-2px]"
                      />{" "}
                      未完成项 {(task.result.blockers ?? []).length} 条
                    </p>
                  )}
                  {(task.result.artifacts ?? []).length > 0 && (
                    <ul className="mt-1 list-inside list-disc text-[var(--t4)]">
                      {(task.result.artifacts ?? []).map((a, i) => (
                        <li
                          key={a.path ?? `artifact-${i}`}
                          className="truncate"
                          title={a.path}
                        >
                          {a.path}
                        </li>
                      ))}
                    </ul>
                  )}
                </details>
              )}
            </div>
          )}

          {/* 子任务清单（checkbox 可勾选，与主窗口一致；分隔线分行 + 长文本单行截断） */}
          {subtasks.length > 0 && (
            <div className="mt-2 flex flex-col divide-y divide-[var(--edge)]">
              {subtasks.map((s) => (
                <SubtaskRow
                  key={s.id}
                  sub={s}
                  variant="compact"
                  onToggle={onToggleSubtask ? () => onToggleSubtask(s.id) : undefined}
                  onOpenCard={onOpenSubtaskCard}
                />
              ))}
              <p className="text-[10px] text-[var(--t5)] tabular-nums">
                {doneCount}/{subtasks.length}
              </p>
            </div>
          )}

          {/* 绑定文件 chip 列表（与主窗口共用 TaskFileChips）：点文件名/文件夹名
              直接打开；每 chip「复制」字样在解绑 × 前；超过 5 个折叠「还有 N 个」 */}
          <div className="mt-3">
            <TaskFileChips
              task={task}
              variant="compact"
              onOpenPath={onOpenFilePath}
              onCopyPath={onCopyFilePath}
              onRemovePath={onRemoveFile}
              stopClickPropagation
            />
          </div>

          {/* 🤖 交给机器人：在折叠段内，复用现有 FoldToggle
              （老板指令：不重新设计折叠窗口，折叠态只露标题，
               展开态显示标题完整 + 🤖 + 其他内容） */}
          {onBotExecute && (
            <div className="mt-2 flex items-center gap-1.5">
              <button
                className="nm-btn px-2 py-0.5 text-[11px] leading-none text-[var(--t3)] flex items-center gap-1 whitespace-nowrap"
                onPointerDown={stop}
                onClick={(e) => {
                  e.stopPropagation();
                  onBotExecute();
                }}
                title="交给机器人执行这张任务卡"
              >
                <Bot size={11} aria-hidden /> 交给机器人
              </button>
            </div>
          )}

          {/* 截止时间 + 状态行（两行布局，与主窗口一致）；
              截止已过且未完成 → 徽标标红（与首页到期横条口径一致）；
              排期窗口 chip：与待办/今日过滤同源的 planStart，只读展示
              （挂件不排期——排期编辑在任务页详情面板）；
              状态行三态与任务页详情面板一致：
              「未完成」/「机器人执行中…」/「完成 YYYY-MM-DD HH:mm」；
              无截止未完成的普通卡不渲染状态行（保持挂件卡紧凑），
              机器人认领（botAssigned）时强制显示执行中状态 */}
          {(task.due ||
            task.planStart ||
            task.column === "done" ||
            task.botAssigned) && (
            <div className="mt-2">
              {(() => {
                const plan = planWindowLabel(task.planStart, task.planEnd);
                return plan ? (
                  <span className="nm-inset px-2 py-1 text-xs shrink-0 mr-1.5 text-[var(--t4)]">
                    📅 {plan}
                  </span>
                ) : null;
              })()}
              {task.due && (
                <span
                  className={`nm-inset px-2 py-1 text-xs shrink-0 ${
                    dueOverdue ? "text-[var(--danger)]" : "text-[var(--t4)]"
                  }`}
                >
                  {formatDue(task.due)}
                </span>
              )}
              <p className="mt-1 ml-2.5 text-[10px] text-[var(--t5)]">
                {task.column === "done" && task.completedAt
                  ? formatCompletedAt(task.completedAt)
                  : task.botAssigned
                    ? "机器人执行中…"
                    : "未完成"}
              </p>
            </div>
          )}
        </>
      )}
    </>
  );
}
