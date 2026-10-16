// ChatPanel 子模块：右侧上下文栏（Agent workbench 式「对话锚点 + 工作面」）。
// 三类工作面切换：任务池 / 文件 / 子 Agent。栏宽占聊天区 1/3——固定 px 在
// 大窗口下显得局促，而这一栏要看的是任务标题与文件路径，宽一点才读得完。
//
// 「终端」「审查」不在此列：项目内无后端命令也无数据源，留空壳入口会变成
// 永远点不动的死 UI（那是从零做 PTY / diff 模型的事，另有工程）。

import type { ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  Bot,
  Check,
  FileText,
  FolderOpen,
  ListTodo,
  Loader2,
  Paperclip,
  Square,
} from "lucide-react";
import { basename } from "../../format";
import { handleCommandError } from "../../lib/errorHandler";
import {
  MAX_TASK_FILES,
  filesPatch,
  mergeFiles,
  taskFiles,
  type TaskFile,
} from "../../lib/taskFiles";
import type { Task } from "../../types";

/** 三类工作面。key 用窄口径字符串，不另立枚举 */
export type ContextTab = "tasks" | "files" | "agents";

type Tab = ContextTab;

export type SubagentRun = {
  /** 子 agent id（sa_ 前缀）——点开查详情/取消都用它 */
  id: string;
  title: string;
  /** 行上六态原值 */
  status: "queued" | "running" | "succeeded" | "failed" | "cancelled" | "budget_exceeded";
  running: boolean;
  error?: string | null;
};

/** 六态 → 侧栏徽标。取消/预算耗尽各有自己的说法：都写成「失败」等于把
 *  用户的选择说成系统出错 */
const SUBAGENT_STATUS: Record<
  SubagentRun["status"],
  { label: string; color: string }
> = {
  queued: { label: "排队中", color: "text-[var(--t5)]" },
  running: { label: "执行中", color: "text-[var(--brand)]" },
  succeeded: { label: "完成", color: "text-[var(--success)]" },
  failed: { label: "失败", color: "text-[var(--danger)]" },
  cancelled: { label: "已取消", color: "text-[var(--t5)]" },
  budget_exceeded: { label: "预算耗尽", color: "text-[var(--danger)]" },
};

type ContextRailProps = {
  tasks: Task[];
  onUpdateTask: (taskId: string, patch: Partial<Task>) => void;
  /** 点任务标题 = 把它引用进当前消息（同附件：摆进对话让机器人看着办），
   *  不是「立刻让机器人去做」——那是任务卡 🤖 的活 */
  onRefTask: (t: { id: string; title: string }) => void;
  /** 已在待发引用里的任务 id（标出点过的项） */
  refIds: string[];
  fileChanges: { path: string; added: number; deleted: number }[];
  /** 子 agent 运行列表（含在途与刚结束），按新到旧 */
  subagents: SubagentRun[];
  /** 点子 agent 条目 → 右侧栏切到它的详情视图 */
  onOpenSubagent: (id: string) => void;
  /** 取消在途子 agent */
  onCancelSubagent: (id: string) => void;
  /** 当前工作面（受控）：内嵌子 agent 区域要点侧栏切类别，状态得在父级 */
  tab: Tab;
  onTabChange: (t: Tab) => void;
};

const TABS: { key: Tab; label: string; icon: ReactNode }[] = [
  { key: "tasks", label: "任务", icon: <ListTodo size={14} /> },
  { key: "files", label: "文件", icon: <FileText size={14} /> },
  { key: "agents", label: "子 Agent", icon: <Bot size={14} /> },
];

function EmptyHint({ children }: { children: ReactNode }) {
  return <p className="px-1.5 py-2 text-[11px] text-[var(--t5)]">{children}</p>;
}

export function ContextRail({
  tasks,
  onUpdateTask,
  onRefTask,
  refIds,
  fileChanges,
  subagents,
  onOpenSubagent,
  onCancelSubagent,
  tab,
  onTabChange,
}: ContextRailProps) {
  const runningCount = subagents.filter((s) => s.running).length;

  /** 聊天窗口直接把本地文件绑到任务卡：选文件 → bind_files 探测 isDir →
   *  统一内核合并（去重/文件夹槽替换/10 上限）→ 走 onUpdateTask 既有落库通道。
   *  绑定一律加法，不动卡上已有绑定 */
  const bindFilesToTask = async (t: Task) => {
    try {
      const picked = await invoke<string[]>("pick_files_dialog");
      if (!picked?.length) return;
      const added = await invoke<TaskFile[]>("bind_files", { paths: picked });
      const cur = taskFiles(t);
      const { files, truncated, applied } = mergeFiles(cur, added);
      if (truncated)
        window.alert(
          `每个任务最多绑定 ${MAX_TASK_FILES} 个文件，超出部分已忽略`,
        );
      if (applied > 0) onUpdateTask(t.id, filesPatch(files));
    } catch (e) {
      handleCommandError(e, "bind files to task", { silent: true });
    }
  };

  return (
    <aside
      id="chat-context-rail"
      aria-label="上下文侧栏"
      // flex-1 与聊天主体的 flex-[2] 按 1:2 分剩余空间 → 侧栏 = 聊天区的 1/3；
      // min/max 只在极端窗口兜底（窄窗不让侧栏挤成缝，宽窗不让正文消失）
      className="flex min-w-[240px] max-w-[420px] flex-1 flex-col overflow-y-auto pl-3"
    >
      {/* 类别切换：切类别不打断对话（工作面在侧栏，主对话留在原位） */}
      <div className="mb-2 flex shrink-0 gap-1">
          {TABS.map((t) => {
            const on = tab === t.key;
            return (
              <button
                key={t.key}
                type="button"
                aria-pressed={on}
                className={`flex min-w-0 flex-1 items-center justify-center gap-1 rounded-lg px-2 py-1.5 text-[11px] ${
                  on
                    ? "nm-inset text-[var(--t1)]"
                    : "text-[var(--t3)] hover:bg-[var(--hover-bg)]"
                }`}
                onClick={() => onTabChange(t.key)}
              >
                {t.icon}
                <span className="truncate">{t.label}</span>
                {/* 在途计数常驻：切走类别也能看到后台还在跑几个 */}
                {t.key === "agents" && runningCount > 0 && (
                  <span
                    aria-hidden
                    className="h-1.5 w-1.5 shrink-0 animate-pulse rounded-full bg-[var(--brand)]"
                  />
                )}
              </button>
            );
          })}
      </div>

      {tab === "tasks" && (
        <section aria-label="任务池">
          <div className="mb-1 flex items-center gap-1.5 px-1.5">
            <span className="text-[11px] font-medium text-[var(--t2)]">
              待办任务
            </span>
            <span className="font-mono text-[10px] text-[var(--t5)]">
              {tasks.length}
            </span>
          </div>
          {tasks.length === 0 ? (
            <EmptyHint>没有待办任务</EmptyHint>
          ) : (
            <div className="space-y-0.5">
              {tasks.map((t) => {
                const referenced = refIds.includes(t.id);
                return (
                <div key={t.id} className="group flex items-center gap-1">
                  <button
                    type="button"
                    aria-label={`引用任务：${t.title}`}
                    title={`引用「${t.title}」到对话`}
                    className={`min-w-0 flex-1 truncate rounded-md py-1.5 pl-1.5 pr-1 text-left text-xs hover:bg-[var(--hover-bg)] ${
                      referenced
                        ? "text-[var(--t1)]"
                        : "text-[var(--t3)] hover:text-[var(--t2)]"
                    }`}
                    aria-pressed={referenced}
                    onClick={() => onRefTask({ id: t.id, title: t.title })}
                  >
                    {t.title}
                  </button>
                  <button
                    type="button"
                    className="shrink-0 rounded p-1 text-[var(--t5)] hover:bg-[var(--hover-bg)] hover:text-[var(--success)]"
                    title="标记完成"
                    aria-label={`完成 ${t.title}`}
                    onClick={() => onUpdateTask(t.id, { column: "done" })}
                  >
                    <Check size={12} aria-hidden />
                  </button>
                  <button
                    type="button"
                    className="shrink-0 rounded p-1 text-[var(--t5)] hover:bg-[var(--hover-bg)] hover:text-[var(--brand)]"
                    title="绑定文件到此卡"
                    aria-label={`绑定文件到 ${t.title}`}
                    onClick={() => void bindFilesToTask(t)}
                  >
                    <Paperclip size={12} aria-hidden />
                  </button>
                </div>
                );
              })}
            </div>
          )}
        </section>
      )}

      {tab === "files" && (
        <section aria-label="文件变更">
          <div className="mb-1 flex items-center gap-1.5 px-1.5">
            <span className="text-[11px] font-medium text-[var(--t2)]">
              本会话文件改动
            </span>
            <span className="font-mono text-[10px] text-[var(--t5)]">
              {fileChanges.length}
            </span>
          </div>
          {fileChanges.length === 0 ? (
            <EmptyHint>本会话还没有文件改动</EmptyHint>
          ) : (
            <div className="space-y-0.5">
              {fileChanges.map((f, i) => (
                <div
                  // oxlint-disable-next-line react/no-array-index-key
                  key={`${f.path}-${i}`}
                  className="flex items-center gap-1.5 rounded-md px-1.5 py-1.5 text-xs"
                  title={`${f.path}（+${f.added} −${f.deleted}）`}
                >
                  <FolderOpen
                    size={12}
                    aria-hidden
                    className="shrink-0 text-[var(--t5)]"
                  />
                  <span className="min-w-0 flex-1 truncate text-[var(--t3)]">
                    {basename(f.path)}
                  </span>
                  <span className="shrink-0 font-mono text-[10px] text-[var(--success)]">
                    +{f.added}
                  </span>
                  <span className="shrink-0 font-mono text-[10px] text-[var(--danger)]">
                    −{f.deleted}
                  </span>
                </div>
              ))}
            </div>
          )}
        </section>
      )}

      {tab === "agents" && (
        <section aria-label="子 Agent">
          <div className="mb-1 flex items-center gap-1.5 px-1.5">
            <span className="text-[11px] font-medium text-[var(--t2)]">
              子 Agent
            </span>
            {runningCount > 0 && (
              <span className="text-[10px] text-[var(--brand)]">
                {runningCount} 个执行中
              </span>
            )}
          </div>
          {subagents.length === 0 ? (
            <EmptyHint>当前没有子 agent 执行</EmptyHint>
          ) : (
            <div className="space-y-0.5">
              {subagents.map((s) => (
                <div
                  key={s.id}
                  className="flex w-full items-center gap-1.5 rounded-md px-1.5 py-1.5 text-xs text-[var(--t3)] hover:bg-[var(--hover-bg)]"
                >
                  <button
                    type="button"
                    title={s.title}
                    className="flex min-w-0 flex-1 items-center gap-1.5 text-left hover:text-[var(--t2)]"
                    onClick={() => onOpenSubagent(s.id)}
                  >
                    {s.running ? (
                      <Loader2
                        size={12}
                        aria-hidden
                        className="shrink-0 animate-spin text-[var(--brand)]"
                      />
                    ) : (
                      <Bot
                        size={12}
                        aria-hidden
                        className={`shrink-0 ${SUBAGENT_STATUS[s.status].color}`}
                      />
                    )}
                    <span className="min-w-0 flex-1 truncate">{s.title}</span>
                    <span
                      className={`shrink-0 text-[10px] ${SUBAGENT_STATUS[s.status].color}`}
                    >
                      {SUBAGENT_STATUS[s.status].label}
                    </span>
                  </button>
                  {s.running && (
                    <button
                      type="button"
                      title="取消这个子 agent"
                      aria-label={`取消子 agent：${s.title}`}
                      className="shrink-0 text-[var(--t5)] hover:text-[var(--danger)]"
                      onClick={(e) => {
                        // 取消不是「看详情」，别顺手切侧栏
                        e.stopPropagation();
                        onCancelSubagent(s.id);
                      }}
                    >
                      <Square size={10} aria-hidden className="fill-current" />
                    </button>
                  )}
                </div>
              ))}
            </div>
          )}
        </section>
      )}
    </aside>
  );
}
