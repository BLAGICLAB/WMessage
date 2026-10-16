// SettingsPage 子模块：诊断卡（基础设置分区）。
// 展示 error.log 最近的警告与错误（倒序最新在前），「复制全部日志」把全量
// 账本（error/bot/api/migration 含轮转历史）打包进剪贴板给开发者定位。
// 复制成功显示「已复制 ✓」1.5s（ProfileRow 同款计时器纪律）。

import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Check, Copy, RefreshCw } from "lucide-react";
import { formatCommandError, handleCommandError } from "../../lib/errorHandler";

const EMPTY_MARK = "（暂无日志）";

export function DiagnosticsCard() {
  const [lines, setLines] = useState<string[] | null>(null);
  const [error, setError] = useState("");
  const [copied, setCopied] = useState(false);
  /** 「已复制 ✓」复位计时器句柄（重复复制先清旧的，卸载时清理） */
  const copiedTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(
    () => () => {
      if (copiedTimer.current) clearTimeout(copiedTimer.current);
    },
    [],
  );

  const reload = useCallback(() => {
    invoke<string>("error_log_read", { limit: 50 })
      .then((s) => {
        // 运行时可能不是 string（旧后端/未知命令返回 null）——按无内容处理
        setLines(typeof s === "string" && s !== EMPTY_MARK ? s.split("\n") : []);
        setError("");
      })
      .catch((e) => {
        handleCommandError(e, "error_log_read", { silent: true });
        setError(formatCommandError(e));
        setLines([]);
      });
  }, []);
  useEffect(() => {
    reload();
  }, [reload]);

  const copyAll = async () => {
    setError("");
    try {
      const text = await invoke<string>("logs_bundle_text");
      await navigator.clipboard.writeText(text);
      setCopied(true);
      if (copiedTimer.current) clearTimeout(copiedTimer.current);
      copiedTimer.current = setTimeout(() => setCopied(false), 1500);
    } catch (e) {
      handleCommandError(e, "logs_bundle_text", { silent: true });
      setError(formatCommandError(e));
    }
  };

  return (
    <div className="nm-card p-5">
      <div className="flex items-start justify-between gap-3">
        <div className="min-w-0">
          <h2 className="text-lg font-semibold text-[var(--t1)]">诊断</h2>
          <p className="mt-1 text-xs text-[var(--t5)]">
            最近的警告与错误，供问题反推；程序表现异常时点「复制全部日志」，整包发给开发者即可定位。
          </p>
        </div>
        <div className="flex shrink-0 gap-1.5">
          <button
            className="nm-btn inline-flex items-center gap-1 px-2.5 py-1 text-[11px] text-[var(--t3)]"
            onClick={reload}
            title="重新读取错误记录"
          >
            <RefreshCw size={11} aria-hidden /> 刷新
          </button>
          <button
            className="nm-btn inline-flex items-center gap-1 px-2.5 py-1 text-[11px] text-[var(--t3)]"
            onClick={() => void copyAll()}
          >
            {copied ? (
              <>
                <Check size={11} aria-hidden /> 已复制
              </>
            ) : (
              <>
                <Copy size={11} aria-hidden /> 复制全部日志
              </>
            )}
          </button>
        </div>
      </div>
      <div className="nm-inset mt-3 max-h-44 overflow-y-auto rounded-xl p-2.5">
        {error ? (
          <p className="text-[10px] text-[var(--danger)]">{error}</p>
        ) : lines === null ? (
          <p className="text-[10px] text-[var(--t5)]">读取中…</p>
        ) : lines.length === 0 ? (
          <p className="text-[10px] text-[var(--t5)]">暂无错误记录</p>
        ) : (
          <pre className="whitespace-pre-wrap break-all font-mono text-[10px] leading-relaxed text-[var(--t3)]">
            {lines.join("\n")}
          </pre>
        )}
      </div>
    </div>
  );
}
