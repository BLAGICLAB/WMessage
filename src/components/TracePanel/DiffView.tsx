// TracePanel 子组件：unified diff 行着色渲染。
// 不引第三方 diff 库——后端生成的是 unified diff 文本（similar），前端只做
// 行级着色（+/−/@@/文件头）与「省略段」标记。解析逻辑 ~30 行，漂移面极小
// （ 决策留档）。着色与分段的规则在 lib/diff.ts（聊天侧栏的改动视图复用同源）。

import { diffLineClass, gapLabel, parseDiffSegments } from "../../lib/diff";

export function DiffView({
  diff,
  truncated,
}: {
  diff: string;
  truncated?: boolean;
}) {
  // 摊平成「行」序列：省略段也占一行，这样 pre 里仍是「一行一 div」的结构
  const rows: { key: string; text: string; cls: string }[] = [];
  parseDiffSegments(diff).forEach((seg, i) => {
    if (seg.type === "gap") {
      rows.push({
        key: `gap-${i}`,
        text: gapLabel(seg.omitted),
        cls: "text-[var(--t5)]",
      });
      return;
    }
    seg.lines.forEach((l, j) => {
      rows.push({ key: `${i}-${j}`, text: l || " ", cls: diffLineClass(l) });
    });
  });
  return (
    <div>
      <pre className="overflow-x-auto rounded-lg nm-inset p-2 font-mono text-[10px] leading-4">
        {rows.map((r) => (
          <div key={r.key} className={r.cls}>
            {r.text}
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
