// SettingsPage 子模块：API 协议自绘下拉。
// 原生 <select> 在 macOS 上弹系统级菜单——样式脱离新拟态主题，深浅色都不跟随。
// 自绘下拉：触发钮 + 浮层全部走主题变量（nm-inset/nm-outset/var(--t*)），
// 深浅色自动生效。交互：点触发钮开合；点外部 / Esc 收起；点选项即选即收。

import { useEffect, useRef, useState } from "react";
import { API_PROVIDER_OPTIONS, type ApiProvider } from "./constants";

export function ApiProviderSelect({
  value,
  onChange,
}: {
  value: ApiProvider;
  onChange: (v: ApiProvider) => void;
}) {
  const [open, setOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      if (rootRef.current && !rootRef.current.contains(e.target as Node)) setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setOpen(false);
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
  return (
    <div ref={rootRef} className="relative">
      <button
        type="button"
        className="nm-inset w-full rounded-xl px-3 py-2 text-xs text-[var(--t3)] outline-none flex items-center justify-between gap-2"
        onClick={() => setOpen((v) => !v)}
      >
        <span className="truncate">{current.label}</span>
        <span
          className={`shrink-0 text-[var(--t5)] transition-transform ${open ? "rotate-180" : ""}`}
        >
          ▾
        </span>
      </button>
      {open && (
        <div className="nm-outset absolute z-50 mt-1 w-full p-1 space-y-0.5">
          {API_PROVIDER_OPTIONS.map((o) => (
            <button
              key={o.value}
              type="button"
              className={`w-full text-left px-3 py-1.5 text-xs rounded-lg ${
                o.value === value
                  ? "nm-inset text-[var(--t1)]"
                  : "text-[var(--t3)] hover:bg-[var(--hover-bg)]"
              }`}
              onClick={() => {
                onChange(o.value);
                setOpen(false);
              }}
            >
              {o.label}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
