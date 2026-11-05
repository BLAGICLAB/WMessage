/** 设置页「检查更新」卡：官方 updater 插件的前端面。
 *
 *  口径：仅按钮手动触发，不做启动自动检查、不做静默更新——定时作业/
 *  无人值守场景不被更新打断；失败显式报错，不许「检查失败当最新」的假成功；
 *  版本/进度一律显示真值；非 Windows 平台不渲染（更新源目前只有 Windows 安装包）。
 */
import { useCallback, useEffect, useState } from "react";
import { check } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import { getVersion } from "@tauri-apps/api/app";
import { Download, RefreshCw } from "lucide-react";

/** 非 Windows 平台不渲染；UA 判定单测可注入 */
export function isWindowsPlatform(ua: string): boolean {
  return ua.includes("Windows NT");
}

type Update = NonNullable<Awaited<ReturnType<typeof check>>>;

type UpdateState =
  | { t: "idle" | "checking" | "installed"; current?: string }
  | { t: "latest"; current: string }
  | { t: "available"; update: Update; got: number; total?: number }
  | { t: "downloading"; version: string; got: number; total?: number }
  | { t: "error"; message: string };

export function UpdateCard({ ua = navigator.userAgent }: { ua?: string }) {
  const [state, setState] = useState<UpdateState>({ t: "idle" });

  useEffect(() => {
    let alive = true;
    getVersion()
      .then((v) => {
        if (alive)
          setState((s) => (s.t === "idle" ? { t: "idle", current: v } : s));
      })
      .catch(() => {
        /* 拿不到版本号不阻塞：点检查时会再取并随错误上屏 */
      });
    return () => {
      alive = false;
    };
  }, []);

  const checkForUpdate = useCallback(async () => {
    setState({ t: "checking" });
    try {
      const u = await check();
      if (!u) {
        const v = await getVersion();
        setState({ t: "latest", current: v });
      } else {
        setState({ t: "available", update: u, got: 0, total: undefined });
      }
    } catch (e) {
      setState({ t: "error", message: e instanceof Error ? e.message : String(e) });
    }
  }, []);

  const startDownload = useCallback(async (u: Update) => {
    setState({ t: "downloading", version: u.version, got: 0, total: undefined });
    try {
      await u.downloadAndInstall((event) => {
        if (event.event === "Started") {
          setState({
            t: "downloading",
            version: u.version,
            got: 0,
            total: event.data.contentLength,
          });
        } else if (event.event === "Progress") {
          setState((s) =>
            s.t === "downloading"
              ? { ...s, got: s.got + event.data.chunkLength }
              : s,
          );
        } else if (event.event === "Finished") {
          setState({ t: "installed" });
        }
      });
      await relaunch();
    } catch (e) {
      setState({ t: "error", message: e instanceof Error ? e.message : String(e) });
    }
  }, []);

  if (!isWindowsPlatform(ua)) return null;

  return (
    <div className="nm-card p-5">
      <div className="flex items-center gap-2">
        <RefreshCw className="h-4 w-4 text-[var(--t3)]" />
        <h3 className="font-medium">检查更新</h3>
      </div>
      <p className="mt-1 text-sm text-[var(--t3)]">
        从 GitHub Releases 检查新版本；下载后自动验签安装，装完重启生效
      </p>

      {state.t === "idle" && (
        <div className="mt-3 flex items-center gap-3">
          <button className="nm-btn" onClick={checkForUpdate}>
            检查更新
          </button>
          {state.current && (
            <span className="text-sm text-[var(--t3)]">
              当前版本 v{state.current}
            </span>
          )}
        </div>
      )}

      {state.t === "checking" && (
        <div className="mt-3 text-sm text-[var(--t3)]">检查中…</div>
      )}

      {state.t === "latest" && (
        <div className="mt-3 text-sm">✓ 已是最新（v{state.current}）</div>
      )}

      {state.t === "available" && (
        <div className="mt-3">
          <div className="text-sm">发现新版本 v{state.update.version}</div>
          {state.update.body?.trim() ? (
            <p className="mt-1 whitespace-pre-wrap text-sm text-[var(--t3)]">
              {state.update.body}
            </p>
          ) : null}
          <button
            className="nm-btn nm-btn-brand mt-2"
            onClick={() => startDownload(state.update)}
          >
            <Download className="h-4 w-4" />
            下载并安装
          </button>
        </div>
      )}

      {state.t === "downloading" && (
        <div className="mt-3 text-sm">
          下载中{" "}
          {state.total
            ? `${Math.min(100, Math.round((state.got / state.total) * 100))}%`
            : `${(state.got / 1024 / 1024).toFixed(1)} MB`}
        </div>
      )}

      {state.t === "installed" && (
        <div className="mt-3 text-sm">已更新，应用即将重启…</div>
      )}

      {state.t === "error" && (
        <div className="mt-3">
          <div className="text-sm text-[var(--danger)]">
            更新失败：{state.message}
          </div>
          <button className="nm-btn mt-2" onClick={checkForUpdate}>
            重试
          </button>
        </div>
      )}
    </div>
  );
}
