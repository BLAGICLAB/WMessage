// ChatPanel 子模块：会话切换器 + 顶部行（U3a 拆分自 ChatPanel.tsx，JSX 逐字搬移）。
// 左侧 🤖 会话切换按钮 + 会话下拉列表（切换/删除/新建），右侧 🎯 选任务模式钮。

import type { RefObject } from "react";
import type { Session } from "./types";

type SessionListProps = {
  sessions: Session[];
  sessionId: string | null;
  /** 当前会话标题（找不到时回落「新对话」，由父级算好传入） */
  currentTitle: string;
  sessionMenuOpen: boolean;
  onToggleMenu: () => void;
  /** 顶部行根节点（dropdownTop 测量基准） */
  topBarRef: RefObject<HTMLDivElement | null>;
  /** 🤖 触发按钮（dropdownTop 测量 + 点击外关闭双 ref 之一） */
  menuRef: RefObject<HTMLButtonElement | null>;
  /** 会话下拉容器（点击外关闭双 ref 之二） */
  dropdownRef: RefObject<HTMLDivElement | null>;
  /** 下拉 top（紧贴 🤖 按钮底部，父级 useDropdownTop 测量） */
  dropdownTop: number;
  selecting: boolean;
  onToggleSelecting: () => void;
  onSelect: (sid: string) => void;
  onNew: () => void;
  onDelete: (sid: string) => void;
};

export function SessionList({
  sessions,
  sessionId,
  currentTitle,
  sessionMenuOpen,
  onToggleMenu,
  topBarRef,
  menuRef,
  dropdownRef,
  dropdownTop,
  selecting,
  onToggleSelecting,
  onSelect,
  onNew,
  onDelete,
}: SessionListProps) {
  return (
    <>
      <div ref={topBarRef} className="flex items-center mb-2 shrink-0 gap-1">
        {/* 左侧：🤖 会话切换器（U3b 会话栈入口：标题 + 会话数徽章） */}
        <button
          ref={menuRef}
          type="button"
          aria-haspopup="true"
          aria-expanded={sessionMenuOpen}
          className="nm-outset flex-1 min-w-0 flex items-center gap-1.5 rounded-lg px-2 py-1 text-xs text-[var(--t2)]"
          title="切换会话"
          onClick={onToggleMenu}
        >
          <span className="truncate flex-1 text-left">🤖 {currentTitle}</span>
          <span className="shrink-0 rounded-full border border-[var(--edge)] px-1.5 font-mono text-[10px] leading-4 text-[var(--t5)]">
            {sessions.length}
          </span>
          <span className="shrink-0 text-[10px] text-[var(--t5)]">
            {sessionMenuOpen ? "▴" : "▾"}
          </span>
        </button>
        {/* 右侧：🎯 移到原 🧹 位置（最右；外框 px-2 py-1 跟 🤖 等高，emoji 内部 16px 免受字体档位影响） */}
        <button
          type="button"
          aria-label="选任务模式"
          aria-pressed={selecting}
          className={`shrink-0 px-2 py-1 flex items-center justify-center ${
            selecting
              ? "nm-inset text-[var(--t1)] font-medium"
              : "nm-outset text-[var(--t4)]"
          }`}
          onClick={onToggleSelecting}
          title="选任务模式：点击下方任务卡选中，再输入操作指令"
        >
          <span className="text-[16px] leading-none">🎯</span>
        </button>
      </div>
      {/* 会话栈（U3b）：作为 rootRef 直接子元素，absolute 横跨整个 panel 宽度；
          行 = 活动圆点 + 标题，悬停浮出删除 */}
      {sessionMenuOpen && (
        <div
          ref={dropdownRef}
          role="menu"
          aria-label="会话列表"
          className="absolute left-0 right-0 nm-card p-1 rounded-xl z-50 max-h-40 overflow-y-auto"
          style={{ top: dropdownTop > 0 ? `${dropdownTop}px` : undefined }}
        >
          {sessions.map((s) => (
            <div key={s.id} className="flex items-center gap-1">
              <button
                type="button"
                role="menuitem"
                aria-current={s.id === sessionId || undefined}
                className={`flex-1 min-w-0 flex items-center gap-1.5 text-left px-2 py-1.5 rounded-lg text-xs truncate ${
                  s.id === sessionId
                    ? "nm-inset text-[var(--t1)] font-medium"
                    : "text-[var(--t3)] hover:bg-[var(--hover-bg)]"
                }`}
                onClick={() => onSelect(s.id)}
                title={s.title}
              >
                <span
                  aria-hidden
                  className={`shrink-0 h-1.5 w-1.5 rounded-full ${
                    s.id === sessionId ? "bg-[var(--brand)]" : "bg-[var(--t6)]"
                  }`}
                />
                <span className="truncate">{s.title}</span>
              </button>
              <button
                type="button"
                role="menuitem"
                aria-label={`删除对话 ${s.title}`}
                className="shrink-0 w-5 h-5 flex items-center justify-center rounded text-[10px] text-[var(--t5)] hover:text-[var(--danger)] hover:bg-[var(--hover-bg)]"
                title="删除此对话"
                onClick={() => onDelete(s.id)}
              >
                🗑
              </button>
            </div>
          ))}
          <button
            type="button"
            role="menuitem"
            className="w-full text-left px-2 py-1 rounded-lg text-xs text-[var(--brand)] hover:bg-[var(--hover-bg)]"
            onClick={onNew}
          >
            ＋ 新建对话
          </button>
        </div>
      )}
    </>
  );
}
