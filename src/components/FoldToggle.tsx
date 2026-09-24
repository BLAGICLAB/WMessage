// 标题行右侧的折叠/展开开关 —— 主窗口 TodoCard 与挂件 TaskCardContent 共用（保持一致）
// 位于标题与打勾圆圈之间；三角符号加大；未折叠时卡片悬停才显现，已折叠时强制可见（保证能点开）
// alwaysVisible：始终显示（挂件窄面板用，避免 hover 才显现难找）
export function FoldToggle({
  collapsed,
  onToggle,
  alwaysVisible = false,
}: {
  collapsed: boolean;
  onToggle: () => void;
  alwaysVisible?: boolean;
}) {
  const stop = (e: React.PointerEvent) => e.stopPropagation();
  return (
    <button
      type="button"
      aria-expanded={!collapsed}
      aria-label={collapsed ? "展开" : "收起"}
      className={`shrink-0 w-5 h-5 flex items-center justify-center rounded-full text-sm leading-none text-[var(--t5)] hover:text-[var(--t2)] hover:bg-[var(--hover-bg)] transition-opacity ${
        alwaysVisible || collapsed
          ? "opacity-100"
          : "opacity-0 group-hover:opacity-100"
      }`}
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
