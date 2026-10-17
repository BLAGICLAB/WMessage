// 上下文水位条（/context 借鉴，设计 §9.2-9）：当前会话累计 tokens +
// 可选的 contextK 百分比。数据源 = bot-usage-delta 事件（Anthropic 协议回合级
// usage；OpenAI 兼容网关的流式 usage 需要 stream_options 参数暂未启用——
// 没有数据时本条隐藏，不显示假数据）。
// 水位语义：每轮 input 是完整重发的 prompt，跨轮 Σ 会随轮数
// 平方膨胀——百分比必须用「最近一轮 prompt 体量」（即当前上下文）对窗口，
// Σ 只作累计展示。≥80% 时提示 /compact（对话场景）/ 结束后重开（执行场景）。
// 缓存两档单列不并入输入：Anthropic 的 cache_read_input_tokens 不含在
// input_tokens 里，两者是协议上并列的两档；折进输入会与 trace 弹窗的
// 「tokens 输入/输出 · 缓存 读/写」分档展示口径不一致。同理百分比里的
// 「最近一轮 prompt 体量」= input_tokens + cache_read_input_tokens。

export function UsageMeter({
  input,
  output,
  cacheRead,
  cacheCreation,
  lastInput,
  lastCacheRead,
  contextK,
}: {
  input: number;
  output: number;
  /** 累计缓存读 / 缓存写（Anthropic 计价档，与输入输出分列） */
  cacheRead: number;
  cacheCreation: number;
  /** 最近一轮请求的 input tokens（未含缓存读，协议上另计） */
  lastInput: number;
  lastCacheRead: number;
  contextK?: number;
}) {
  const cacheTotal = cacheRead + cacheCreation;
  const total = input + output + cacheTotal;
  if (total <= 0) return null;
  const fmt = (n: number) =>
    n >= 1000 ? `${(n / 1000).toFixed(1)}K` : String(n);
  // 缓存命中的回合里 input_tokens 只剩增量，prompt 体量的大头在 cache_read 上
  const lastPrompt = lastInput + lastCacheRead;
  const ratio =
    contextK && contextK > 0 && lastPrompt > 0
      ? lastPrompt / (contextK * 1000)
      : null;
  const high = ratio != null && ratio >= 0.8;
  // 显示口径与水位条一致：条已钳到 100% 封顶，标签/悬浮提示同样钳位（不出现 137%）
  const pct = ratio != null ? Math.min(100, ratio * 100).toFixed(0) : null;
  return (
    <div
      className={`flex items-center gap-1.5 px-3 pb-0.5 text-[10px] tabular-nums ${
        high ? "text-[var(--danger,#ef4444)]" : "text-[var(--t5)]"
      }`}
      title={`本会话累计：输入 ${input} + 输出 ${output} tokens${
        cacheTotal > 0
          ? ` · 缓存读 ${cacheRead} / 缓存写 ${cacheCreation}（协议上与输入输出分档计价，不并入前两项）`
          : ""
      }${
        pct != null
          ? `；最近一轮 prompt ${lastPrompt} ≈ 上下文窗口 ${pct}%（模型条目 contextK=${contextK}）`
          : "（当前模型条目未设 contextK，不显示百分比）"
      }`}
    >
      <span className="font-mono">Σ {fmt(total)}</span>
      {cacheTotal > 0 && (
        <span className="font-mono" title="缓存读 / 缓存写（Anthropic 计价档）">
          缓存 {fmt(cacheRead)}/{fmt(cacheCreation)}
        </span>
      )}
      {ratio != null && (
        <>
          <span className="h-1 w-16 overflow-hidden rounded-full bg-[var(--inset-bg)]">
            <span
              className={`block h-full ${high ? "bg-[var(--danger,#ef4444)]" : "bg-[var(--brand)]"}`}
              style={{ width: `${pct}%` }}
            />
          </span>
          <span>{pct}%</span>
          {high && <span>· 上下文将满，建议 /compact</span>}
        </>
      )}
    </div>
  );
}
