// TodoCard 子模块：子任务行组件。
//
// 子任务行：checkbox + 全文显示（不截断）+ 点击文本内联编辑。
// 每行独立 editing 状态，所以拆成组件（useInlineEdit 一份状态管一行）。
// 空提交保留原文（与标题编辑一致，避免误触清空子任务）。

import { useEffect, useState } from "react";
import { useInlineEdit } from "../useInlineEdit";

/** 阻止拖拽手柄触发卡片点击编辑（SubtaskRow 内联使用） */
const stop = (e: React.PointerEvent) => e.stopPropagation();

export function SubtaskRow({
  sub,
  readOnly,
  onToggle,
  onCommit,
  onDelete,
}: {
  sub: { id: string; text: string; done: boolean };
  readOnly: boolean;
  onToggle: () => void;
  onCommit: (text: string) => void;
  onDelete: () => void;
}) {
  const [editing, setEditing] = useState(false);
  const edit = useInlineEdit({
    value: sub.text,
    editing,
    onCommit: (d) => {
      const text = d.trim();
      if (text && text !== sub.text) onCommit(text);
      setEditing(false);
    },
    onCancel: () => setEditing(false),
  });

  // readOnly flip 到 true 时退出编辑态（只读契约：不允许在飞编辑继续提交）
  useEffect(() => {
    if (readOnly) setEditing(false);
  }, [readOnly]);

  return (
    <div className="flex items-start gap-2 group py-1.5">
      <input
        type="checkbox"
        checked={sub.done}
        disabled={readOnly}
        onChange={onToggle}
        onPointerDown={stop}
        className="shrink-0 mt-0.5 w-3.5 h-3.5 accent-[var(--brand)]"
      />
      {editing ? (
        <input
          autoFocus
          value={edit.draft}
          readOnly={readOnly}
          onChange={(e) => edit.setDraft(e.target.value)}
          onBlur={edit.onBlur}
          onKeyDown={edit.onKeyDown}
          onPointerDown={stop}
          className="flex-1 min-w-0 rounded-lg bg-[var(--input-bg)] px-2 py-0.5 text-xs text-[var(--t2)] outline-none"
        />
      ) : (
        <span
          className={`flex-1 min-w-0 whitespace-pre-wrap break-words text-xs ${
            sub.done ? "text-[var(--t5)] line-through" : "text-[var(--t3)]"
          } ${readOnly ? "" : "cursor-text"}`}
          title={readOnly ? undefined : "点击编辑"}
          onPointerDown={stop}
          onClick={readOnly ? undefined : () => setEditing(true)}
        >
          {sub.text}
        </span>
      )}
      {!readOnly && !editing && (
        <button
          className="shrink-0 opacity-0 group-hover:opacity-100 text-[var(--t5)] hover:text-[var(--danger)] text-xs"
          title="删除子任务"
          onPointerDown={stop}
          onClick={onDelete}
        >
          ×
        </button>
      )}
    </div>
  );
}
