// 词元统计卡（P3-b，Agent 透明化设计 §3.3）：设置页「词元统计」section 实装。
// 数据源 = exec_traces 表按日聚合（usage_stats_daily 命令，P1-d 就绪）——
// prompt/completion tokens、执行次数、工具调用数；无外部上报，纯本地。
// 依赖挂载期拉一次 + 「刷新」按钮手动重拉（统计是复盘视角，不做轮询）。

import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { RefreshCw } from "lucide-react";

export type UsageDay = {
  day: string;
  promptTokens: number;
  completionTokens: number;
  runs: number;
  toolCalls: number;
};

function fmtTokens(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}K`;
  return String(n);
}

export function UsageStatsCard() {
  const [days, setDays] = useState<UsageDay[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const reload = useCallback(() => {
    setLoading(true);
    setError(null);
    invoke<UsageDay[]>("usage_stats_daily", { days: 30 })
      // 防御归一：mock/异常路径可能给 null（null.map 的 uncaught 会炸掉整个设置页测试宿主）
      .then((r) => setDays(Array.isArray(r) ? r : []))
      .catch((e) => setError(String(e)))
      .finally(() => setLoading(false));
  }, []);

  // 挂载期拉一次（setState 在 await 后，lint 显式豁免——项目既有模式）
  useEffect(() => {
    reload();
  }, [reload]);

  const maxTotal = Math.max(1, ...days.map((d) => d.promptTokens + d.completionTokens));
  const totalPrompt = days.reduce((a, d) => a + d.promptTokens, 0);
  const totalCompletion = days.reduce((a, d) => a + d.completionTokens, 0);
  const totalRuns = days.reduce((a, d) => a + d.runs, 0);
  const totalTools = days.reduce((a, d) => a + d.toolCalls, 0);

  return (
    <div className="nm-card p-5">
      <div className="flex items-center justify-between gap-4">
        <div>
          <h2 className="text-lg font-semibold text-[var(--t1)]">词元统计</h2>
          <p className="mt-1 text-xs text-[var(--t5)]">
            最近 30 天执行痕迹的词元用量（本地统计，含聊天/任务卡/定时/工作流全部执行）
          </p>
        </div>
        <button
          className="shrink-0 min-w-[76px] px-4 py-1.5 text-sm text-[var(--t3)] nm-outset inline-flex items-center justify-center gap-1"
          onClick={reload}
          disabled={loading}
        >
          <RefreshCw size={12} aria-hidden className={loading ? "animate-spin" : ""} />
          {loading ? "读取中…" : "刷新"}
        </button>
      </div>

      {/* 汇总行 */}
      <div className="mt-4 grid grid-cols-4 gap-2 text-center">
        {[
          ["输入 tokens", fmtTokens(totalPrompt)],
          ["输出 tokens", fmtTokens(totalCompletion)],
          ["执行次数", String(totalRuns)],
          ["工具调用", String(totalTools)],
        ].map(([label, value]) => (
          <div key={label} className="nm-inset rounded-lg px-2 py-2">
            <p className="font-mono text-sm tabular-nums text-[var(--t1)]">{value}</p>
            <p className="mt-0.5 text-[10px] text-[var(--t5)]">{label}</p>
          </div>
        ))}
      </div>

      {error && <p className="mt-3 text-xs text-[var(--danger)]">{error}</p>}

      {/* 按日条形（纯 div 宽度比例，不引图表库；最近在上） */}
      {!loading && !error && days.length === 0 && (
        <p className="mt-6 text-center text-xs text-[var(--t5)]">
          还没有执行记录——跑一次对话或任务卡后这里会出现按日统计。
        </p>
      )}
      {days.length > 0 && (
        <div className="mt-4 space-y-1.5">
          {days.map((d) => {
            const total = d.promptTokens + d.completionTokens;
            return (
              <div key={d.day} className="flex items-center gap-2">
                <span className="w-20 shrink-0 font-mono text-[10px] tabular-nums text-[var(--t5)]">
                  {d.day.slice(5)}
                </span>
                <div className="flex h-3 min-w-0 flex-1 overflow-hidden rounded-sm bg-[var(--inset-bg)]">
                  <div
                    className="h-full bg-[var(--brand)] opacity-60"
                    style={{ width: `${(d.promptTokens / maxTotal) * 100}%` }}
                    title={`输入 ${d.promptTokens}`}
                  />
                  <div
                    className="h-full bg-[var(--brand)]"
                    style={{ width: `${(d.completionTokens / maxTotal) * 100}%` }}
                    title={`输出 ${d.completionTokens}`}
                  />
                </div>
                <span
                  className="w-40 shrink-0 text-right font-mono text-[10px] tabular-nums text-[var(--t4)]"
                  title={`${d.day}：输入 ${d.promptTokens} / 输出 ${d.completionTokens} / 执行 ${d.runs} 次 / 工具 ${d.toolCalls} 次`}
                >
                  {fmtTokens(total)} · {d.runs}次
                </span>
              </div>
            );
          })}
          <p className="pt-1 text-[10px] text-[var(--t5)]">
            浅色 = 输入 tokens，深色 = 输出 tokens；右侧 = 当日总量与执行次数
          </p>
        </div>
      )}
    </div>
  );
}
