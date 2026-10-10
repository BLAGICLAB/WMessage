// TracePanel 子组件：unified diff 行着色渲染。
// 不引第三方 diff 库——后端生成的是 unified diff 文本（similar），前端只做
// 行级着色（+/−/@@/文件头）。解析逻辑 ~30 行，漂移面极小（ 决策留档）。

/** 单行着色类名：文件头弱化 / hunk 头高亮 / +/- 红绿 / 上下文原样 */
function lineClass(line: string): string {
  if (line.startsWith("+++") || line.startsWith("---"))
    return "text-[var(--t5)]";
  if (line.startsWith("@@")) return "text-[var(--info,#3b82f6)]";
  if (line.startsWith("+"))
    return "bg-[var(--ok,#22c55e)]/10 text-[var(--ok,#22c55e)]";
  if (line.startsWith("-"))
    return "bg-[var(--danger,#ef4444)]/10 text-[var(--danger,#ef4444)]";
  return "";
}

export function DiffView({
  diff,
  truncated,
}: {
  diff: string;
  truncated?: boolean;
}) {
  const lines = diff.split("\n");
  return (
    <div>
      <pre className="overflow-x-auto rounded-lg nm-inset p-2 font-mono text-[10px] leading-4">
        {lines.map((l, i) => (
          // oxlint-disable-next-line react/no-array-index-key
          <div key={i} className={lineClass(l)}>
            {l || " "}
          </div>
        ))}
      </pre>
      {truncated && (
        <p className="mt-0.5 text-[10px] text-[var(--t5)]">
          diff 超长已截断（完整内容看文件）
        </p>
      )}
    </div>
  );
}
