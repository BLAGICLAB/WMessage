// 任务卡标题右侧的完成圆圈 —— 主窗口 TodoCard 与挂件 TaskCardContent 共用（保持一致）
export function DoneCircle({
  done,
  onToggle,
  title,
}: {
  done: boolean;
  onToggle: () => void;
  title?: string;
}) {
  const stop = (e: React.PointerEvent) => e.stopPropagation();
  return (
    <button
      type="button"
      className={`shrink-0 w-5 h-5 rounded-full nm-inset flex items-center justify-center text-xs leading-none ${
        done ? "text-[var(--success)]" : "text-transparent hover:text-[var(--t5)]"
      }`}
      title={title ?? (done ? "取消完成" : "标记完成")}
      onPointerDown={stop}
      onClick={(e) => {
        e.stopPropagation();
        onToggle();
      }}
    >
      ✓
    </button>
  );
}
