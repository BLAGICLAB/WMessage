// 主窗口 TodoCard 与挂件 TaskCardContent 共用的展示格式化（保持两处显示一致）

export function basename(p: string): string {
  const parts = p.split(/[\\/]/);
  return parts[parts.length - 1] || p;
}

// due 格式兼容两种："YYYY-MM-DD"（旧数据）与 "YYYY-MM-DDTHH:mm"
export function formatDue(due: string): string {
  if (due.includes("T")) {
    const [d, t] = due.slice(0, 16).split("T");
    return `截止 ${d.slice(5)} ${t}`;
  }
  return `截止 ${due.slice(5)}`;
}
