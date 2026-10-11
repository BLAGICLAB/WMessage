// 必应每日一图卡：后端当天缓存（bing_daily），当天内只出网一次。
// 三态：加载骨架 / 网络失败友好占位（带重试）/ 成功铺图 + 底部标题与版权，
// 点卡片外跳看大图（openUrl，失败经 errorHandler 上报不静默）。

import { useCallback, useEffect, useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { ImageOff, RotateCw } from "lucide-react";
import { fetchBingDaily, type BingDaily } from "../../lib/bing";
import { handleCommandError } from "../../lib/errorHandler";

export function BingCard() {
  const [data, setData] = useState<BingDaily | null>(null);
  const [error, setError] = useState("");

  const load = useCallback(() => {
    setError("");
    fetchBingDaily()
      .then((d) => {
        setData(d);
      })
      .catch((e) => {
        setData(null);
        setError(formatBingError(e));
      });
  }, []);

  useEffect(() => {
    load();
  }, [load]);

  const open = () => {
    if (!data) return;
    openUrl(data.imageUrl).catch((e) => handleCommandError(e, "打开每日一图大图"));
  };

  return (
    <section
      className="nm-card relative aspect-video overflow-hidden"
      aria-label="必应每日一图"
    >
      {data ? (
        <button
          type="button"
          onClick={open}
          className="absolute inset-0 h-full w-full cursor-zoom-in text-left"
          title="点击查看大图"
        >
          <img
            src={data.dataUrl}
            alt={data.title}
            className="absolute inset-0 h-full w-full object-cover"
            draggable={false}
          />
          <span className="absolute left-3 top-3 flex items-center gap-1.5 rounded-md bg-black/45 px-2 py-0.5 text-[10px] font-medium text-white">
            <span aria-hidden className="inline-block h-1.5 w-1.5 rounded-full bg-white/90" />
            每日一图
          </span>
          <span
            aria-hidden
            className="absolute inset-x-0 bottom-0 h-2/5 bg-gradient-to-t from-black/75 to-transparent"
          />
          <span className="absolute inset-x-3 bottom-2.5 block">
            <span className="block truncate text-[13px] font-semibold text-white">
              {data.title}
            </span>
            <span className="block truncate text-[11px] text-white/75">
              {data.copyright}
            </span>
          </span>
        </button>
      ) : error ? (
        <div className="flex h-full flex-col items-center justify-center gap-2 px-4 py-8 text-center">
          <ImageOff size={20} className="text-[var(--t5)]" aria-hidden />
          <p className="text-[13px] font-medium text-[var(--t3)]">今日壁纸没拉到</p>
          <p className="line-clamp-2 text-xs text-[var(--t5)]">{error}</p>
          <button
            onClick={load}
            className="mt-1 flex items-center gap-1.5 rounded-md border border-[var(--edge-strong)] px-2.5 py-1 text-xs text-[var(--t2)] transition-colors hover:bg-[var(--hover-bg)]"
          >
            <RotateCw size={12} aria-hidden />
            重试
          </button>
        </div>
      ) : (
        <div
          aria-hidden
          className="absolute inset-0 animate-pulse bg-[var(--inset-bg)]"
        />
      )}
    </section>
  );
}

function formatBingError(e: unknown): string {
  const msg = typeof e === "string" ? e : e instanceof Error ? e.message : String(e);
  return msg.length > 80 ? `${msg.slice(0, 80)}…` : msg;
}
