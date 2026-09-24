// SettingsPage 子模块：API 协议自绘下拉。
// 原生 <select> 在 macOS 上弹系统级菜单——样式脱离新拟态主题，深浅色都不跟随。
// 自绘下拉：触发钮 + 浮层全部走主题变量（nm-inset/nm-outset/var(--t*)），
// 深浅色自动生效。交互：点触发钮开合；点外部 / Esc 收起；点选项即选即收。

import { useEffect, useId, useRef, useState } from "react";
import { API_PROVIDER_OPTIONS, type ApiProvider } from "./constants";

export function ApiProviderSelect({
  value,
  onChange,
}: {
  value: ApiProvider;
  onChange: (v: ApiProvider) => void;
}) {
  const [open, setOpen] = useState(false);
  /** 键盘导航高亮项；open 时对齐当前值 */
  const [activeIdx, setActiveIdx] = useState(0);
  const rootRef = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const listboxId = useId();
  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      if (rootRef.current && !rootRef.current.contains(e.target as Node)) setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        setOpen(false);
        // 焦点归还触发钮（选项卸载后焦点会掉到 body）
        triggerRef.current?.focus();
      }
    };
    document.addEventListener("mousedown", onDown);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDown);
      document.removeEventListener("keydown", onKey);
    };
  }, [open]);
  const current =
    API_PROVIDER_OPTIONS.find((o) => o.value === value) ?? API_PROVIDER_OPTIONS[0];
  const openList = () => {
    setActiveIdx(
      Math.max(0, API_PROVIDER_OPTIONS.findIndex((o) => o.value === value))
    );
    setOpen(true);
  };
  const commit = (idx: number) => {
    onChange(API_PROVIDER_OPTIONS[idx].value);
    setOpen(false);
    // 选项按钮即将卸载：焦点收回触发钮，不掉 body
    triggerRef.current?.focus();
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
    const n = API_PROVIDER_OPTIONS.length;
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setActiveIdx((i) => (i + 1) % n);
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setActiveIdx((i) => (i - 1 + n) % n);
    } else if (e.key === "Home") {
      e.preventDefault();
      setActiveIdx(0);
    } else if (e.key === "End") {
      e.preventDefault();
      setActiveIdx(n - 1);
    } else if (e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      commit(activeIdx);
    }
  };
  return (
    <div ref={rootRef} className="relative" onKeyDown={onListKeyDown}>
      <button
        ref={triggerRef}
        type="button"
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-controls={listboxId}
        aria-activedescendant={
          open ? `${listboxId}-opt-${API_PROVIDER_OPTIONS[activeIdx].value}` : undefined
        }
        className="nm-inset w-full rounded-xl px-3 py-2 text-xs text-[var(--t3)] outline-none flex items-center justify-between gap-2"
        onClick={() => (open ? setOpen(false) : openList())}
      >
        <span className="truncate">{current.label}</span>
        <span
          className={`shrink-0 text-[var(--t5)] transition-transform ${open ? "rotate-180" : ""}`}
        >
          ▾
        </span>
      </button>
      {open && (
        <div
          id={listboxId}
          role="listbox"
          className="nm-outset absolute z-50 mt-1 w-full p-1 space-y-0.5"
        >
          {API_PROVIDER_OPTIONS.map((o, i) => (
            <button
              key={o.value}
              id={`${listboxId}-opt-${o.value}`}
              type="button"
              role="option"
              // activedescendant 模式：选项不进 Tab 序（焦点保持在触发钮）
              tabIndex={-1}
              aria-selected={o.value === value}
              className={`w-full text-left px-3 py-1.5 text-xs rounded-lg ${
                o.value === value
                  ? "nm-inset text-[var(--t1)]"
                  : "text-[var(--t3)] hover:bg-[var(--hover-bg)]"
              } ${i === activeIdx && o.value !== value ? "bg-[var(--hover-bg)]" : ""}`}
              onClick={() => commit(i)}
            >
              {o.label}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
