// 上下文水位条（P4，/context 借鉴，设计 §9.2-9）：当前会话累计 tokens +
// 可选的 contextK 百分比。数据源 = bot-usage-delta 事件（Anthropic 协议回合级
// usage；OpenAI 兼容网关的流式 usage 需要 stream_options 参数暂未启用——
// 没有数据时本条隐藏，不显示假数据）。
// ≥80% 时提示 /compact（对话场景）/ 结束后重开（执行场景）。

export function UsageMeter({
  input,
  output,
  contextK,
}: {
  input: number;
  output: number;
  contextK?: number;
}) {
  const total = input + output;
  if (total <= 0) return null;
  const fmt = (n: number) =>
    n >= 1000 ? `${(n / 1000).toFixed(1)}K` : String(n);
  const ratio = contextK && contextK > 0 ? total / (contextK * 1000) : null;
  const high = ratio != null && ratio >= 0.8;
  return (
    <div
      className={`flex items-center gap-1.5 px-3 pb-0.5 text-[10px] tabular-nums ${
        high ? "text-[var(--danger,#ef4444)]" : "text-[var(--t5)]"
      }`}
      title={`本会话累计：输入 ${input} + 输出 ${output} tokens${
        ratio != null
          ? `；约上下文窗口 ${(ratio * 100).toFixed(0)}%（模型条目 contextK=${contextK}）`
          : "（当前模型条目未设 contextK，不显示百分比）"
      }`}
    >
      <span className="font-mono">Σ {fmt(total)}</span>
      {ratio != null && (
        <>
          <span className="h-1 w-16 overflow-hidden rounded-full bg-[var(--inset-bg)]">
            <span
              className={`block h-full ${high ? "bg-[var(--danger,#ef4444)]" : "bg-[var(--brand)]"}`}
              style={{ width: `${Math.min(100, ratio * 100)}%` }}
            />
          </span>
          <span>{(ratio * 100).toFixed(0)}%</span>
          {high && <span>· 上下文将满，建议 /compact</span>}
        </>
      )}
    </div>
  );
}
