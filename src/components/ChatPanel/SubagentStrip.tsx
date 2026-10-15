// ChatPanel 子模块：聊天记录内嵌的子 agent 区域。
//
// 子 agent 不再自己开一个对话框（那会让对话列表被一堆执行现场淹没），
// 它的运行状态就地长在主对话里：一块紧凑区域列出派出去的子 agent、
// 各自跑到哪一步。点条目把右侧栏切到「子 Agent」工作面看详情——
// 区域负责「发生了什么」，侧栏负责「细节是什么」。

import { Bot, Check, Loader2, TriangleAlert, X } from "lucide-react";

export type SubagentChip = {
  id: string;
  title: string;
  running: boolean;
  error?: string | null;
};

const STATUS_ICON = {
  running: (
    <Loader2
      size={11}
      aria-hidden
      className="shrink-0 animate-spin text-[var(--brand)]"
    />
  ),
  done: (
    <Check
      size={11}
      aria-hidden
      className="shrink-0 text-[var(--success)]"
    />
  ),
  failed: (
    <TriangleAlert
      size={11}
      aria-hidden
      className="shrink-0 text-[var(--danger)]"
    />
  ),
  cancelled: (
    <X size={11} aria-hidden className="shrink-0 text-[var(--t5)]" />
  ),
} as const;

/** 把子 agent 归到四个展示态。失败/取消各有独立图标——都归「非运行」会让用户
 *  分不清是跑完了还是挂了 */
function statusOf(s: SubagentChip): keyof typeof STATUS_ICON {
  if (s.running) return "running";
  if (s.error) return "failed";
  return "done";
}

export function SubagentStrip({
  subagents,
  onOpen,
  /** 已展开（点标题栏切换）——展开后列全部，折叠只显示在途数量 */
  expanded,
  onToggleExpanded,
}: {
  subagents: SubagentChip[];
  onOpen: (id: string) => void;
  expanded: boolean;
  onToggleExpanded: () => void;
}) {
  // 没有子 agent 就不占位置——空区域只会让对话显得稀疏
  if (subagents.length === 0) return null;
  const runningCount = subagents.filter((s) => s.running).length;
  const shown = expanded ? subagents : subagents.filter((s) => s.running);

  return (
    <div className="nm-card mb-2 rounded-xl px-2 py-1.5">
      <button
        type="button"
        className="flex w-full items-center gap-1.5 text-left"
        aria-expanded={expanded}
        onClick={onToggleExpanded}
      >
        <Bot size={12} aria-hidden className="shrink-0 text-[var(--brand)]" />
        <span className="text-[11px] font-medium text-[var(--t2)]">
          子 Agent
        </span>
        <span className="font-mono text-[10px] text-[var(--t5)]">
          {runningCount > 0 ? `${runningCount} 个执行中` : `${subagents.length} 个`}
        </span>
        <span aria-hidden className="ml-auto shrink-0 text-[10px] text-[var(--t5)]">
          {expanded ? "▴" : "▾"}
        </span>
      </button>
      {shown.length > 0 && (
        <div className="mt-1 space-y-0.5">
          {shown.map((s) => (
            <button
              key={s.id}
              type="button"
              title={`${s.title}${s.error ? `（${s.error}）` : ""}`}
              className="flex w-full items-center gap-1.5 rounded-md px-1.5 py-1 text-left text-[11px] text-[var(--t3)] hover:bg-[var(--hover-bg)] hover:text-[var(--t2)]"
              onClick={() => onOpen(s.id)}
            >
              {STATUS_ICON[statusOf(s)]}
              <span className="min-w-0 flex-1 truncate">{s.title}</span>
              {/* 侧栏入口提示：点它去哪儿，得让人看得见 */}
              <span className="shrink-0 text-[10px] text-[var(--t6)]">
                详情 ›
              </span>
            </button>
          ))}
        </div>
      )}
      {!expanded && runningCount > 0 && subagents.length > runningCount && (
        <p className="px-1.5 pt-0.5 text-[10px] text-[var(--t6)]">
          另有 {subagents.length - runningCount} 个已结束
        </p>
      )}
    </div>
  );
}
