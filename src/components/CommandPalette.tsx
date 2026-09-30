import { Fragment, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  Archive,
  MessageSquare,
  Search,
  SquareKanban,
  Trash2,
} from "lucide-react";
import type { Task } from "../types";

// ⌘K 命令面板（U2）：任务标题 + 会话标题聚合搜索，键盘上下选择 + 回车跳转。
// 任务 → 主窗口切视图并进入编辑；会话 → 发 chat-focus-session 事件让挂件
// 展开并切换（ChatPanel/WidgetApp 各自监听）。数据：tasks 来自 props 快照，
// 会话在挂载时经 bot_sessions_load 现读（失败静默为空，面板仍可搜任务）。
// 无 open prop：由使用方条件挂载（{open && <CommandPalette/>}），
// 每次打开都是全新实例，输入/选中态天然复位。

/** 挂件聊天会话（与 ChatPanel 的 Session 同形；独立声明避免跨目录耦合） */
type PaletteSession = { id: string; title: string };

type Row =
  | { kind: "task"; task: Task }
  | { kind: "session"; session: PaletteSession };

type CommandPaletteProps = {
  onClose: () => void;
  tasks: Task[];
  onJumpTask: (task: Task) => void;
  onJumpSession: (sessionId: string) => void;
};

/** 任务所在位置徽标（列名 / 归档 / 回收站） */
function taskMeta(t: Task): string {
  if (t.deletedAt) return "回收站";
  if (t.archived) return "归档";
  if (t.column === "doing") return "今日";
  if (t.column === "done") return "完成";
  return "待办";
}

function TaskIcon({ task }: { task: Task }) {
  if (task.deletedAt) return <Trash2 size={15} aria-hidden className="shrink-0 text-[var(--t4)]" />;
  if (task.archived) return <Archive size={15} aria-hidden className="shrink-0 text-[var(--t4)]" />;
  return <SquareKanban size={15} aria-hidden className="shrink-0 text-[var(--t4)]" />;
}

export function CommandPalette({
  onClose,
  tasks,
  onJumpTask,
  onJumpSession,
}: CommandPaletteProps) {
  const [query, setQuery] = useState("");
  const [sessions, setSessions] = useState<PaletteSession[]>([]);
  const [active, setActive] = useState(0);

  // 挂载时现读会话列表（失败降级为空：面板仍可搜任务，warn 留痕便于排查）
  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const list = await invoke<PaletteSession[]>("bot_sessions_load");
        if (!cancelled) setSessions(list ?? []);
      } catch (e) {
        console.warn("[CommandPalette] bot_sessions_load failed", e);
        if (!cancelled) setSessions([]);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  const { rows, taskCount } = useMemo(() => {
    const q = query.trim().toLowerCase();
    const match = (s: string) => !q || s.toLowerCase().includes(q);
    const taskRows: Row[] = tasks
      .filter((t) => match(t.title))
      .sort((a, b) => (b.updatedAt ?? 0) - (a.updatedAt ?? 0))
      .slice(0, 8)
      .map((task) => ({ kind: "task" as const, task }));
    const sessionRows: Row[] = sessions
      .filter((s) => match(s.title))
      .slice(0, 6)
      .map((session) => ({ kind: "session" as const, session }));
    return { rows: [...taskRows, ...sessionRows], taskCount: taskRows.length };
  }, [query, tasks, sessions]);

  // 键盘选中项滚动进可视区（jsdom 无 scrollIntoView，做能力守卫）
  useEffect(() => {
    const el = document.querySelector('[data-cmd-active="true"]');
    if (el && typeof el.scrollIntoView === "function") {
      el.scrollIntoView({ block: "nearest" });
    }
  }, [active, rows]);

  const choose = (row: Row) => {
    if (row.kind === "task") onJumpTask(row.task);
    else onJumpSession(row.session.id);
    onClose();
  };

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setActive((i) => Math.min(i + 1, Math.max(rows.length - 1, 0)));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setActive((i) => Math.max(i - 1, 0));
    } else if (e.key === "Enter") {
      e.preventDefault();
      const row = rows[active];
      if (row) choose(row);
    } else if (e.key === "Escape") {
      e.preventDefault();
      onClose();
    }
  };

  return (
    <div
      className="fixed inset-0 z-50 flex items-start justify-center bg-black/50 pt-[12vh]"
      onMouseDown={onClose}
    >
      <div
        role="dialog"
        aria-modal="true"
        aria-label="搜索任务与会话"
        onMouseDown={(e) => e.stopPropagation()}
        /* Tailwind 的 shadow-[var(--x)] 会被解析成阴影颜色而非阴影值，投影走内联 style */
        style={{ boxShadow: "var(--shadow-lg)" }}
        className="w-[560px] max-w-[92vw] overflow-hidden rounded-[var(--r-lg)] border border-[var(--edge)] bg-[var(--surface-raised)]"
      >
        <div className="flex items-center gap-2 border-b border-[var(--edge)] px-3.5 py-3">
          <Search size={16} aria-hidden className="shrink-0 text-[var(--t5)]" />
          <input
            autoFocus
            value={query}
            onChange={(e) => {
              setQuery(e.target.value);
              setActive(0); // 输入变化后选中项回到首位
            }}
            onKeyDown={onKeyDown}
            placeholder="搜索任务与会话…"
            aria-label="搜索关键词"
            className="min-w-0 flex-1 bg-transparent text-sm text-[var(--t2)] outline-none"
          />
          <kbd className="shrink-0 rounded border border-[var(--edge)] px-1.5 py-0.5 text-[10px] font-medium text-[var(--t5)]">
            Esc
          </kbd>
        </div>
        <div className="max-h-[52vh] overflow-y-auto p-1.5">
          {rows.length === 0 && (
            <p className="px-3 py-6 text-center text-sm text-[var(--t5)]">
              没有匹配的任务或会话
            </p>
          )}
          {rows.map((row, i) => {
            const key =
              row.kind === "task" ? `t-${row.task.id}` : `s-${row.session.id}`;
            return (
              <Fragment key={key}>
                {i === 0 && taskCount > 0 && (
                  <p className="px-2.5 pb-1 pt-2 text-[11px] text-[var(--t5)]">任务</p>
                )}
                {i === taskCount && i < rows.length && (
                  <p className="px-2.5 pb-1 pt-2 text-[11px] text-[var(--t5)]">会话</p>
                )}
                <button
                  // 键盘导航集中在搜索框（↑↓/回车），结果行不进 Tab 序
                  tabIndex={-1}
                  data-cmd-active={i === active || undefined}
                  onClick={() => choose(row)}
                  onMouseEnter={() => setActive(i)}
                  className={`flex w-full items-center gap-2.5 rounded-[var(--r-sm)] px-2.5 py-2 text-left text-sm transition-colors duration-100 ${
                    i === active
                      ? "bg-[var(--hover-bg)] text-[var(--t1)]"
                      : "text-[var(--t3)]"
                  }`}
                >
                  {row.kind === "task" ? (
                    <TaskIcon task={row.task} />
                  ) : (
                    <MessageSquare size={15} aria-hidden className="shrink-0 text-[var(--t4)]" />
                  )}
                  <span className="min-w-0 flex-1 truncate">
                    {row.kind === "task"
                      ? row.task.title
                      : row.session.title || "未命名会话"}
                  </span>
                  <span className="shrink-0 text-[11px] text-[var(--t5)]">
                    {row.kind === "task" ? taskMeta(row.task) : "挂件聊天"}
                  </span>
                </button>
              </Fragment>
            );
          })}
        </div>
      </div>
    </div>
  );
}
