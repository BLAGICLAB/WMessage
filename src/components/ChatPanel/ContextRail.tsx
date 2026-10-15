// ChatPanel 子模块：右侧上下文栏。
// 三块面板：任务池（可选中交给 agent / 直接勾完成）、本会话文件变更、
// 在途的子 agent 执行会话。三块各有真实数据源——「终端」「审查」这类
// 需要从零做后端的能力没有数据源，故不在此处留空壳入口。

import { useState } from "react";
import {
  Bot,
  Check,
  FileText,
  FolderOpen,
  Loader2,
  PanelRightClose,
  Sparkles,
} from "lucide-react";
import { basename } from "../../format";
import type { Task } from "../../types";
import type { FileChangeLite, Session } from "./types";

type ContextRailProps = {
  /** 未完成任务（App 侧已过滤归档/删除/他人任务） */
  tasks: Task[];
  /** 勾完成/改列（走 App 的 updateTask，后端广播 tasks-updated 刷新全局） */
  onUpdateTask: (taskId: string, patch: Partial<Task>) => void;
  /** 交给 agent 做：发 execute-task + 切到聊天页（与任务卡 🤖 同通道） */
  onRunTask: (t: { id: string; title: string }) => void;
  /** 本会话累积的文件变更（bot-file-changed 累积） */
  fileChanges: FileChangeLite[];
  /** 在途执行会话（inflight），用于子 agent 面板 */
  inflightSessions: Session[];
  onOpenSession: (sid: string) => void;
  onCollapse: () => void;
};

/** 面板折叠态（组件内记忆，不落盘——三块的信息价值随会话变化，
 *  记住「哪块常看」不如记住栏本身的开关） */
function usePanel(open: boolean) {
  const [expanded, setExpanded] = useState(open);
  return [expanded, () => setExpanded((v) => !v)] as const;
}

function PanelHeader({
  icon,
  label,
  count,
  expanded,
  onToggle,
}: {
  icon: React.ReactNode;
  label: string;
  count: number;
  expanded: boolean;
  onToggle: () => void;
}) {
  return (
    <button
      type="button"
      className="flex w-full items-center gap-1.5 rounded-lg px-1.5 py-1.5 text-left hover:bg-[var(--hover-bg)]"
      aria-expanded={expanded}
      onClick={onToggle}
    >
      <span className="shrink-0 text-[var(--t4)]">{icon}</span>
      <span className="min-w-0 flex-1 text-xs font-medium text-[var(--t2)]">
        {label}
      </span>
      {count > 0 && (
        <span className="shrink-0 rounded-full border border-[var(--edge)] px-1.5 font-mono text-[10px] leading-4 text-[var(--t5)]">
          {count}
        </span>
      )}
      <span aria-hidden className="shrink-0 text-[10px] text-[var(--t5)]">
        {expanded ? "▴" : "▾"}
      </span>
    </button>
  );
}

export function ContextRail({
  tasks,
  onUpdateTask,
  onRunTask,
  fileChanges,
  inflightSessions,
  onOpenSession,
  onCollapse,
}: ContextRailProps) {
  const [taskOpen, toggleTaskOpen] = usePanel(true);
  const [fileOpen, toggleFileOpen] = usePanel(false);
  const [agentOpen, toggleAgentOpen] = usePanel(true);

  return (
    <aside
      id="chat-context-rail"
      aria-label="上下文侧栏"
      className="flex w-[220px] shrink-0 flex-col overflow-y-auto pl-3"
    >
      <div className="mb-1 flex shrink-0 items-center justify-between">
        <span className="text-[10px] text-[var(--t5)]">上下文</span>
        <button
          type="button"
          className="nm-icon-btn h-6 w-6"
          aria-label="收起上下文栏"
          title="收起上下文栏"
          onClick={onCollapse}
        >
          <PanelRightClose size={14} aria-hidden />
        </button>
      </div>

      {/* 任务池：勾完成 = column 切 done；🤖 = 交给 agent（与任务卡同通道） */}
      <section className="mb-1">
        <PanelHeader
          icon={<Sparkles size={12} />}
          label="任务池"
          count={tasks.length}
          expanded={taskOpen}
          onToggle={toggleTaskOpen}
        />
        {taskOpen &&
          (tasks.length === 0 ? (
            <p className="px-1.5 py-1 text-[10px] text-[var(--t5)]">
              没有待办任务
            </p>
          ) : (
            <div className="mt-0.5 space-y-0.5">
              {tasks.map((t) => (
                <div key={t.id} className="group flex items-center gap-1">
                  <button
                    type="button"
                    title={t.title}
                    className="min-w-0 flex-1 truncate rounded-md py-1 pl-1.5 pr-1 text-left text-[11px] text-[var(--t3)] hover:bg-[var(--hover-bg)] hover:text-[var(--t2)]"
                    onClick={() => onRunTask({ id: t.id, title: t.title })}
                    aria-label={`交给机器人执行：${t.title}`}
                  >
                    {t.title}
                  </button>
                  <button
                    type="button"
                    className="shrink-0 rounded p-0.5 text-[var(--t5)] hover:text-[var(--success)] hover:bg-[var(--hover-bg)]"
                    title="标记完成"
                    aria-label={`完成 ${t.title}`}
                    onClick={() => onUpdateTask(t.id, { column: "done" })}
                  >
                    <Check size={11} aria-hidden />
                  </button>
                </div>
              ))}
            </div>
          ))}
      </section>

      {/* 文件：本会话 bot-file-changed 累积（主会话走正则兜底时此块为空，
          那是既有口径——没有结构化数据就不编造） */}
      <section className="mb-1">
        <PanelHeader
          icon={<FileText size={12} />}
          label="文件"
          count={fileChanges.length}
          expanded={fileOpen}
          onToggle={toggleFileOpen}
        />
        {fileOpen &&
          (fileChanges.length === 0 ? (
            <p className="px-1.5 py-1 text-[10px] text-[var(--t5)]">
              本会话还没有文件改动
            </p>
          ) : (
            <div className="mt-0.5 space-y-0.5">
              {fileChanges.map((f, i) => (
                <div
                  // oxlint-disable-next-line react/no-array-index-key
                  key={`${f.path}-${i}`}
                  className="flex items-center gap-1 rounded-md px-1.5 py-1 text-[11px]"
                  title={`${f.path}（+${f.added} −${f.deleted}）`}
                >
                  <FolderOpen
                    size={11}
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
          ))}
      </section>

      {/* 子 agent：在途执行会话。点条目跳过去围观 */}
      <section>
        <PanelHeader
          icon={<Bot size={12} />}
          label="子 Agent"
          count={inflightSessions.length}
          expanded={agentOpen}
          onToggle={toggleAgentOpen}
        />
        {agentOpen &&
          (inflightSessions.length === 0 ? (
            <p className="px-1.5 py-1 text-[10px] text-[var(--t5)]">
              当前没有执行中的任务
            </p>
          ) : (
            <div className="mt-0.5 space-y-0.5">
              {inflightSessions.map((s) => (
                <button
                  key={s.id}
                  type="button"
                  className="flex w-full items-center gap-1.5 rounded-md px-1.5 py-1 text-left text-[11px] text-[var(--t3)] hover:bg-[var(--hover-bg)] hover:text-[var(--t2)]"
                  title={s.title}
                  onClick={() => onOpenSession(s.id)}
                >
                  <Loader2
                    size={11}
                    aria-hidden
                    className="shrink-0 animate-spin text-[var(--brand)]"
                  />
                  <span className="min-w-0 flex-1 truncate">{s.title}</span>
                </button>
              ))}
            </div>
          ))}
      </section>
    </aside>
  );
}
