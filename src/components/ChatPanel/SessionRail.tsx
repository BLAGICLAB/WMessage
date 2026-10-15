// ChatPanel 子模块：常驻会话侧栏。
// 取代原先藏在顶部药丸里的下拉——历史会话始终可见，当前会话高亮成块，
// 「新建对话」提到栏顶独立按钮。对话类界面的通行做法（常驻侧栏而非
// 点击展开的菜单）：下拉一旦折叠，入口就退回成一个不起眼的控件。

import { MessageSquare, Plus, Trash2 } from "lucide-react";
import { relativeTime } from "../../format";
import type { Session } from "./types";

type SessionRailProps = {
  sessions: Session[];
  sessionId: string | null;
  onSelect: (sid: string) => void;
  onNew: () => void;
  onDelete: (sid: string) => void;
};

export function SessionRail({
  sessions,
  sessionId,
  onSelect,
  onNew,
  onDelete,
}: SessionRailProps) {
  return (
    <aside
      id="chat-session-rail"
      aria-label="会话列表"
      // 窗口无最小宽度约束（tauri.conf.json），侧栏自身不缩：先压聊天主体
      className="flex w-[200px] shrink-0 flex-col border-r border-[var(--edge)] pr-3"
    >
      {/* 「新建对话」占满整行：折叠入口统一在聊天头部，侧栏内不再放第二个 */}
      <button
        type="button"
        className="nm-btn-brand mb-3 flex w-full shrink-0 items-center justify-center gap-1.5 px-3 py-2 text-sm"
        title="新建对话"
        onClick={onNew}
      >
        <Plus size={14} aria-hidden />
        新建对话
      </button>

      <div className="min-h-0 flex-1 space-y-0.5 overflow-y-auto">
        {sessions.length === 0 ? (
          <p className="px-2 py-1 text-xs text-[var(--t5)]">还没有对话</p>
        ) : (
          sessions.map((s) => {
            const active = s.id === sessionId;
            return (
              <div key={s.id} className="group relative flex items-center">
                {/* 品牌色竖条：选中态的第二个信号，比仅靠底色对比更易扫读 */}
                <span
                  aria-hidden
                  className={`absolute left-0 top-1/2 h-4 w-0.5 -translate-y-1/2 rounded-full ${
                    active ? "bg-[var(--brand)]" : "bg-transparent"
                  }`}
                />
                <button
                  type="button"
                  aria-current={active ? "page" : undefined}
                  title={s.title}
                  className={`flex min-w-0 flex-1 items-center gap-1.5 rounded-lg py-2 pl-2.5 pr-7 text-left ${
                    active
                      ? "nm-inset text-[var(--t1)]"
                      : "text-[var(--t3)] hover:bg-[var(--hover-bg)]"
                  }`}
                  onClick={() => onSelect(s.id)}
                >
                  {/* 实心/空心图标对：当前会话与历史会话的第二重区分，
                      不只靠底色（色觉障碍与低对比屏下底色差异不足） */}
                  <MessageSquare
                    size={13}
                    aria-hidden
                    className={`shrink-0 ${active ? "fill-current" : ""}`}
                  />
                  <span className="min-w-0 flex-1 truncate text-xs">
                    {s.title}
                  </span>
                  {s.updatedAt != null && (
                    <span className="shrink-0 text-[10px] tabular-nums text-[var(--t5)] group-hover:hidden">
                      {relativeTime(s.updatedAt)}
                    </span>
                  )}
                </button>
                {/* 删除键悬停显形：留白恒定，列表不会因悬停而抖动。
                    未显形时禁用指针事件——否则不可见的按钮仍占着行尾点击区 */}
                <button
                  type="button"
                  aria-label={`删除对话 ${s.title}`}
                  title="删除此对话"
                  className="pointer-events-none absolute right-1 flex h-5 w-5 items-center justify-center rounded text-[var(--t5)] opacity-0 transition-opacity group-hover:pointer-events-auto group-hover:opacity-100 focus-visible:pointer-events-auto focus-visible:opacity-100 hover:bg-[var(--hover-bg)] hover:text-[var(--danger)]"
                  onClick={() => onDelete(s.id)}
                >
                  <Trash2 size={11} aria-hidden />
                </button>
              </div>
            );
          })
        )}
      </div>

      {sessions.length > 0 && (
        <p className="mt-2 shrink-0 border-t border-[var(--edge)] pt-2 text-[10px] text-[var(--t5)]">
          共 {sessions.length} 个会话
        </p>
      )}
    </aside>
  );
}
