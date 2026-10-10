import type { ReactNode } from "react";

// 空态统一组件：图标 + 标题 + 说明 + 可选行动按钮。
// 收敛原先五处各写一遍的「居中灰字」空态，动线（引导下一步）成为空态标配。

type EmptyStateProps = {
  /** lucide 图标（弱色，置于软底圆内） */
  icon: ReactNode;
  /** 空态标题（保持与既有测试断言一致的短句） */
  title: string;
  /** 说明：引导下一步动作 */
  description?: string;
  /** 行动按钮（无则只展示引导说明） */
  action?: { label: string; onClick: () => void };
};

export function EmptyState({
  icon,
  title,
  description,
  action,
}: EmptyStateProps) {
  return (
    <div className="flex flex-col items-center gap-1.5 py-10 text-center">
      <div
        aria-hidden
        className="flex h-10 w-10 items-center justify-center rounded-full bg-[var(--inset-bg)] text-[var(--t5)]"
      >
        {icon}
      </div>
      <p className="text-sm font-medium text-[var(--t2)]">{title}</p>
      {description && (
        <p className="max-w-[320px] text-xs text-[var(--t5)]">{description}</p>
      )}
      {action && (
        <button
          type="button"
          className="nm-btn mt-1.5 px-3 py-1.5 text-xs text-[var(--t2)]"
          onClick={action.onClick}
        >
          {action.label}
        </button>
      )}
    </div>
  );
}
