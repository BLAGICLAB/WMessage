// 共用子任务行组件 —— 两种 variant 适配两个调用方：
//   - spacious: TodoCard（看板卡，预览/编辑一体；pre-wrap 文本 + hover ×）
//   - detail:   TaskDetailPanel（毛玻璃详情面板；按钮式勾选 + 图标动作）
//
// 动作显隐全部由可选回调决定：缺即不渲染对应按钮，
// 调用方按场景选择性接线即可。detail 永不入编辑态，
// 故 useInlineEdit 在 detail 下不会触发任何状态变更。

import { useEffect, useState } from "react";
import { ArrowUpRight, Plus } from "lucide-react";
import { useInlineEdit } from "../useInlineEdit";
import { stopPointer as stop } from "../../lib/stopPointer";

export type Subtask = {
  id: string;
  text: string;
  done: boolean;
  /** 已生成子任务卡：在场时显示 ↗ 跳转按钮（detail 模式无 taskId 则显示 ＋ 生成为任务卡） */
  taskId?: string;
};

export type SubtaskRowVariant = "spacious" | "detail";

export function SubtaskRow({
  sub,
  variant = "spacious",
  readOnly = false,
  onToggle,
  onCommit,
  onDelete,
  onOpenCard,
  onGenerateCard,
}: {
  sub: Subtask;
  /** spacious: TodoCard 全文（pre-wrap + break-words） + 内联编辑 + hover ×
   *  detail:   DetailPanel 按钮式勾选 + 图标 ↗/＋/× */
  variant?: SubtaskRowVariant;
  /** 只读契约：禁用 checkbox、屏蔽内联编辑、隐藏 × 删除按钮（spacious） */
  readOnly?: boolean;
  /** 切换 done；undefined = 无 checkbox 或禁用 */
  onToggle?: () => void;
  /** 内联编辑提交（spacious only）；undefined = 文本不可编辑 */
  onCommit?: (text: string) => void;
  /** × 删除；undefined = 无 × */
  onDelete?: () => void;
  /** ↗ 跳已生成子任务卡（sub.taskId 在场时显示）；undefined = 隐藏 ↗ */
  onOpenCard?: (cardId: string) => void;
  /** ＋ 生成为任务卡（detail only；sub.taskId 不在场时显示）；undefined = 隐藏 ＋ */
  onGenerateCard?: () => void;
}) {
  const detail = variant === "detail";

  // spacious-only：内联编辑 hook（detail 永不入编辑态）
  const [editing, setEditing] = useState(false);
  const edit = useInlineEdit({
    value: sub.text,
    editing,
    onCommit: (d) => {
      // 只读翻转触发的退编辑态会让 input 卸载并补发一次 blur（提交草稿）；
      // 只读契约：在飞编辑必须随退出丢弃，不得把部分草稿提交出去
      if (readOnly) {
        setEditing(false);
        return;
      }
      const text = d.trim();
      if (text && text !== sub.text) onCommit?.(text);
      setEditing(false);
    },
    onCancel: () => setEditing(false),
  });

  // readOnly flip 到 true 时退出编辑态（只读契约：不允许在飞编辑继续提交）；
  // props→state 受控同步，改成渲染期派生会改变编辑提交时序
  useEffect(() => {
    // oxlint-disable-next-line react/set-state-in-effect
    if (readOnly) setEditing(false);
  }, [readOnly]);


  if (detail) {
    // TaskDetailPanel：整行可点击 toggle（按钮式勾选），行尾 ↗/＋/× 悬停显现
    return (
      <div className="group flex items-center gap-1 py-1">
        <button
          type="button"
          className={`flex min-w-0 flex-1 items-center gap-2 text-left text-xs ${
            sub.done ? "text-[var(--t5)] line-through" : "text-[var(--t3)]"
          }`}
          onClick={onToggle}
        >
          <span
            className={`h-3.5 w-3.5 shrink-0 rounded border ${
              sub.done
                ? "border-[var(--success)] bg-[var(--success)]"
                : "border-[var(--edge-strong)]"
            }`}
            aria-hidden
          />
          {sub.text}
        </button>
        {/* 行尾动作：已生成卡 → ↗ 跳转，未生成 → ＋ 生成为任务卡；× 删行
            （已生成行 × = 解除关联保留子卡，语义在 App 侧接线）；悬停显现 */}
        {sub.taskId ? (
          onOpenCard && (
            <button
              type="button"
              title="打开生成的任务卡"
              aria-label={`打开子任务「${sub.text}」生成的任务卡`}
              className="shrink-0 text-[var(--t5)] opacity-0 transition-opacity duration-[var(--dur-chip)] ease-[var(--ease-out)] hover:text-[var(--t2)] group-hover:opacity-100"
              onClick={() => onOpenCard(sub.taskId!)}
            >
              <ArrowUpRight size={12} aria-hidden />
            </button>
          )
        ) : (
          onGenerateCard && (
            <button
              type="button"
              title="生成为任务卡"
              aria-label={`把子任务「${sub.text}」生成为任务卡`}
              className="shrink-0 text-[var(--t5)] opacity-0 transition-opacity duration-[var(--dur-chip)] ease-[var(--ease-out)] hover:text-[var(--t2)] group-hover:opacity-100"
              onClick={onGenerateCard}
            >
              <Plus size={12} aria-hidden />
            </button>
          )
        )}
        {onDelete && (
          <button
            type="button"
            title="删除子任务"
            aria-label={`删除子任务「${sub.text}」`}
            className="shrink-0 text-[var(--t5)] opacity-0 transition-opacity duration-[var(--dur-chip)] ease-[var(--ease-out)] hover:text-[var(--danger)] group-hover:opacity-100"
            onClick={onDelete}
          >
            ×
          </button>
        )}
      </div>
    );
  }

  // spacious: TodoCard
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
      {sub.taskId && onOpenCard && (
        <button
          className="shrink-0 text-[11px] leading-none text-[var(--t5)] hover:text-[var(--t2)]"
          title={`打开子任务「${sub.text}」生成的任务卡`}
          aria-label={`打开子任务「${sub.text}」生成的任务卡`}
          onPointerDown={stop}
          onClick={() => onOpenCard(sub.taskId!)}
        >
          ↗
        </button>
      )}
      {onDelete && !readOnly && !editing && (
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
