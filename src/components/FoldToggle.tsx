// 标题以下内容的折叠按钮 —— 主窗口 TodoCard 与挂件 TaskCardContent 共用（保持一致）
export function FoldToggle({
  collapsed,
  onToggle,
}: {
  collapsed: boolean;
  onToggle: () => void;
}) {
  const stop = (e: React.PointerEvent) => e.stopPropagation();
  return (
    <button
      className="shrink-0 w-5 h-5 flex items-center justify-center rounded-full text-xs leading-none text-gray-400 hover:text-gray-600 hover:bg-black/5"
      title={collapsed ? "展开" : "收起"}
      onPointerDown={stop}
      onClick={(e) => {
        e.stopPropagation();
        onToggle();
      }}
    >
      {collapsed ? "▸" : "▾"}
    </button>
  );
}
