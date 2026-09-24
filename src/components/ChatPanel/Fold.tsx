// ChatPanel 子模块：折叠块（标题 + 点击展开内容）。
// 用于：思考过程、Skill 失败、工具调用详情等折叠显示。

import { useId, useState, type ReactNode } from "react";

export function Fold({
  title,
  children,
}: {
  title: ReactNode;
  children?: ReactNode;
}) {
  const [open, setOpen] = useState(false);
  const panelId = useId();
  return (
    <div>
      <button
        type="button"
        aria-expanded={open}
        aria-controls={panelId}
        className="inline-flex items-center gap-1 max-w-full text-[10px] text-[var(--t5)] hover:text-[var(--t3)]"
        onClick={() => setOpen((v) => !v)}
      >
        <span className="w-2 shrink-0 inline-block">{open ? "▾" : "▸"}</span>
        <span className="truncate">{title}</span>
      </button>
      {open && children ? (
        <div
          id={panelId}
          className="mt-0.5 pl-3 text-[10px] leading-relaxed text-[var(--t4)] whitespace-pre-wrap break-words max-h-40 overflow-y-auto"
        >
          {children}
        </div>
      ) : null}
    </div>
  );
}
