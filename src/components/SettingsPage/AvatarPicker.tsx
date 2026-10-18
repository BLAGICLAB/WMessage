// SettingsPage 子模块：预设头像选择器。
// 触发钮 = 当前头像（hover 出相机角标），nm-popover 浮层内 4×4 网格 16 选 1，
// 即点即换；「恢复默认头像」收在浮层底部（仅已设头像时显示）。
// 键盘 ←→↑↓/Home/End/Enter/Esc 走 ApiProviderSelect 同款 activedescendant
// 模式：焦点保持在触发钮，高亮位置由 aria-activedescendant 播报。

import { useId, useRef, useState } from "react";
import { Camera, Check } from "lucide-react";
import type { ReactNode } from "react";
import { useDismissable } from "../../lib/useDismissable";
import { AVATAR_IDS, avatarSrc, type AvatarId } from "../../avatars";

/** 网格列数：←→ 行内移动、↑↓ 跨行移动，到边缘截停（网格语义，不环形绕接） */
const COLS = 4;

export function AvatarPicker({
  kindLabel,
  avatarId,
  fallback,
  disabled,
  onPick,
  onClear,
}: {
  /** 「用户」/「机器人」——拼 aria 文案用 */
  kindLabel: string;
  avatarId: string | null;
  /** 未设头像时的占位渲染（bot=品牌图 / user=首字母），触发钮复用 */
  fallback: ReactNode;
  disabled?: boolean;
  onPick: (id: AvatarId) => void;
  onClear: () => void;
}) {
  const [open, setOpen] = useState(false);
  /** 键盘导航高亮项；open 时对齐当前值（未设头像则落第一格） */
  const [activeIdx, setActiveIdx] = useState(0);
  const rootRef = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const listboxId = useId();
  useDismissable(
    open,
    rootRef,
    () => setOpen(false),
    () => {
      setOpen(false);
      // 焦点归还触发钮（浮层卸载后焦点会掉到 body）；点外部收起不归还
      triggerRef.current?.focus();
    },
  );

  const currentSrc = avatarSrc(avatarId);
  const openList = () => {
    const cur = avatarId ? AVATAR_IDS.indexOf(avatarId as AvatarId) : -1;
    setActiveIdx(cur >= 0 ? cur : 0);
    setOpen(true);
  };
  const closeAndRefocus = () => {
    setOpen(false);
    // 选项按钮即将卸载：焦点收回触发钮，不掉 body
    triggerRef.current?.focus();
  };
  const commit = (id: AvatarId) => {
    onPick(id);
    closeAndRefocus();
  };

  const onListKeyDown = (e: React.KeyboardEvent) => {
    if (!open) {
      // 关闭态触发钮：方向键直接展开（高亮当前值）
      if (e.key === "ArrowDown" || e.key === "ArrowUp") {
        e.preventDefault();
        openList();
      }
      return;
    }
    if (e.key === "Tab") {
      // 离开组件时收起浮层（不拦默认移动焦点）
      setOpen(false);
      return;
    }
    const n = AVATAR_IDS.length;
    if (e.key === "ArrowRight") {
      e.preventDefault();
      setActiveIdx((i) => Math.min(n - 1, i + 1));
    } else if (e.key === "ArrowLeft") {
      e.preventDefault();
      setActiveIdx((i) => Math.max(0, i - 1));
    } else if (e.key === "ArrowDown") {
      e.preventDefault();
      setActiveIdx((i) => Math.min(n - 1, i + COLS));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setActiveIdx((i) => Math.max(0, i - COLS));
    } else if (e.key === "Home") {
      e.preventDefault();
      setActiveIdx(0);
    } else if (e.key === "End") {
      e.preventDefault();
      setActiveIdx(n - 1);
    } else if (e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      const id = AVATAR_IDS[activeIdx];
      if (id !== avatarId) commit(id);
      else closeAndRefocus();
    }
  };

  return (
    <div ref={rootRef} className="relative shrink-0" onKeyDown={onListKeyDown}>
      <button
        ref={triggerRef}
        type="button"
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-controls={open ? listboxId : undefined}
        aria-activedescendant={
          open ? `${listboxId}-opt-${AVATAR_IDS[activeIdx]}` : undefined
        }
        aria-label={`更换${kindLabel}头像`}
        title="更换头像"
        disabled={disabled}
        onClick={() => (open ? setOpen(false) : openList())}
        className="group relative block h-11 w-11 shrink-0 overflow-hidden rounded-full nm-inset disabled:opacity-55"
      >
        {currentSrc ? (
          <img
            src={currentSrc}
            alt={kindLabel}
            draggable={false}
            className="h-full w-full object-cover"
          />
        ) : (
          fallback
        )}
        {!disabled && (
          <span
            aria-hidden
            className="absolute inset-0 hidden items-center justify-center bg-black/35 text-white group-hover:flex"
          >
            <Camera size={14} />
          </span>
        )}
      </button>
      {open && (
        <div
          id={listboxId}
          role="listbox"
          aria-label={`${kindLabel}头像`}
          className="nm-popover absolute left-0 top-full z-50 mt-2 w-60 p-3"
        >
          <div className="grid grid-cols-4 gap-2">
            {AVATAR_IDS.map((id, i) => {
              const sel = id === avatarId;
              const act = i === activeIdx && !sel;
              return (
                <button
                  key={id}
                  id={`${listboxId}-opt-${id}`}
                  type="button"
                  role="option"
                  // activedescendant 模式：选项不进 Tab 序（焦点保持在触发钮）
                  tabIndex={-1}
                  aria-selected={sel}
                  aria-label={`头像 ${i + 1}`}
                  className="group/cell relative rounded-full outline-none"
                  onClick={() => commit(id)}
                >
                  <img
                    src={avatarSrc(id) ?? undefined}
                    alt=""
                    draggable={false}
                    className={`h-12 w-12 rounded-full object-cover transition-[box-shadow] duration-100 ${
                      sel
                        ? "ring-2 ring-[var(--brand)] ring-offset-2 ring-offset-[var(--surface-raised)]"
                        : act
                          ? "ring-2 ring-[var(--edge-strong)]"
                          : "group-hover/cell:ring-2 group-hover/cell:ring-[var(--edge-strong)]"
                    }`}
                  />
                  {sel && (
                    <span className="absolute right-0 bottom-0 flex h-4 w-4 items-center justify-center rounded-full bg-[var(--brand)] text-white">
                      <Check size={10} strokeWidth={3} aria-hidden />
                    </span>
                  )}
                </button>
              );
            })}
          </div>
          {avatarId && (
            <button
              type="button"
              className="mt-2.5 w-full rounded-lg px-2 py-1.5 text-xs text-[var(--t4)] transition-colors duration-100 hover:bg-[var(--hover-bg)] hover:text-[var(--t2)]"
              onClick={() => {
                onClear();
                closeAndRefocus();
              }}
            >
              恢复默认头像
            </button>
          )}
        </div>
      )}
    </div>
  );
}
