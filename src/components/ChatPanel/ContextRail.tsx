// ChatPanel 子模块：右侧上下文栏（Agent workbench 式「对话锚点 + 工作面」）。
// 三类工作面切换：任务池 / 文件 / 子 Agent。栏宽占聊天区 1/3——固定 px 在
// 大窗口下显得局促，而这一栏要看的是任务标题与文件路径，宽一点才读得完。
//
// 「终端」「审查」不在此列：项目内无后端命令也无数据源，留空壳入口会变成
// 永远点不动的死 UI（那是从零做 PTY / diff 模型的事，另有工程）。

import type { ReactNode } from "react";
import {
  Bot,
  Check,
  FileText,
  FolderOpen,
  ListTodo,
  Loader2,
} from "lucide-react";
import { basename } from "../../format";
import type { Task } from "../../types";

/** 三类工作面。key 用窄口径字符串，不另立枚举 */
export type ContextTab = "tasks" | "files" | "agents";

type Tab = ContextTab;

export type SubagentRun = {
  /** 子 agent id（sa_ 前缀）——点开查详情/停止都用它 */
  id: string;
  title: string;
  running: boolean;
  error?: string | null;
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
  tab,
  onTabChange,
}: ContextRailProps) {
  const runningCount = subagents.filter((s) => s.running).length;

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
                <button
                  key={s.id}
                  type="button"
                  title={s.title}
                  className="flex w-full items-center gap-1.5 rounded-md px-1.5 py-1.5 text-left text-xs text-[var(--t3)] hover:bg-[var(--hover-bg)] hover:text-[var(--t2)]"
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
                      className={`shrink-0 ${
                        s.error
                          ? "text-[var(--danger)]"
                          : "text-[var(--success)]"
                      }`}
                    />
                  )}
                  <span className="min-w-0 flex-1 truncate">{s.title}</span>
                  {s.error ? (
                    <span className="shrink-0 text-[10px] text-[var(--danger)]">
                      失败
                    </span>
                  ) : !s.running ? (
                    <span className="shrink-0 text-[10px] text-[var(--success)]">
                      完成
                    </span>
                  ) : null}
                </button>
              ))}
            </div>
          )}
        </section>
      )}
    </aside>
  );
}
