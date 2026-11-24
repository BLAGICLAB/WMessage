/** 设置页「检查更新」卡：官方 updater 插件的前端面。
 *
 *  口径：仅按钮手动触发，不做启动自动检查、不做静默更新——定时作业/
 *  无人值守场景不被更新打断；失败显式报错，不许「检查失败当最新」的假成功；
 *  版本/进度一律显示真值；非 Windows 平台不渲染（更新源目前只有 Windows 安装包）。
 *
 *  Windows 安装语义（插件源码实证）：downloadAndInstall 末段拉起安装器后进程
 *  exit(0)，本组件的 await 之后代码不可达——重启由安装器 /R 参数完成，前端
 *  不调 relaunch；下载全程进度真值，Finished 即安装器接管。
 */
import { useCallback, useEffect, useState } from "react";
import { check } from "@tauri-apps/plugin-updater";
import { getVersion } from "@tauri-apps/api/app";
import { openUrl } from "@tauri-apps/plugin-opener";
import { Download, RefreshCw } from "lucide-react";
import { handleCommandError } from "../../lib/errorHandler";

/** 发布页——与 updater 端点同仓库。检查或下载失败时给用户的兜底出路：
 *  浏览器与 updater 的网络路径未必相同（浏览器常另有代理），应用内拉不到
 *  清单时发布页手动下载往往仍可行。 */
const RELEASES_PAGE_URL = "https://github.com/BLAGICLAB/WMessage/releases";

/** 检查请求的超时（毫秒）。updater 插件默认不设超时：更新服务器不可达时
 *  请求会一直挂到操作系统的连接超时，按钮全程停在「检查中」。清单是个很小的
 *  文件，正常网络一两秒到手，15 秒对慢网足够宽裕，又远短于系统超时。
 *  只约束检查请求——安装包体积大且下载另有进度反馈，不受它限制。 */
const CHECK_TIMEOUT_MS = 15_000;

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
  | { t: "error"; stage: "check" | "download"; message: string };

/** 失败后的出路提示：按阶段区分（检查没拿到清单 / 安装包没装上），不去
 *  解析错误文本——插件经 IPC 传回的是字符串，按内容猜类型是脆口径。 */
function failureHint(stage: "check" | "download"): string {
  return stage === "check"
    ? "可稍后重试；网络受限时可到发布页手动下载安装包。"
    : "当前版本不受影响，可重试；也可到发布页手动下载安装包。";
}

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
      const u = await check({ timeout: CHECK_TIMEOUT_MS });
      if (!u) {
        const v = await getVersion();
        setState({ t: "latest", current: v });
      } else {
        setState({ t: "available", update: u, got: 0, total: undefined });
      }
    } catch (e) {
      setState({
        t: "error",
        stage: "check",
        message: e instanceof Error ? e.message : String(e),
      });
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
      /* 进程在此前已被插件 exit(0) 接管（安装器自动重启），此处不可达不写代码 */
    } catch (e) {
      setState({
        t: "error",
        stage: "download",
        message: e instanceof Error ? e.message : String(e),
      });
    }
  }, []);

  const openReleasesPage = useCallback(() => {
    openUrl(RELEASES_PAGE_URL).catch((e) => handleCommandError(e, "打开发布页"));
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
        <div className="mt-3 text-sm">已更新，安装器接管中，应用将自动重启…</div>
      )}

      {state.t === "error" && (
        <div className="mt-3">
          <div className="text-sm text-[var(--danger)]">
            更新失败：{state.message}
          </div>
          <p className="mt-1 text-xs text-[var(--t3)]">
            {failureHint(state.stage)}
          </p>
          <div className="mt-2 flex items-center gap-2">
            <button className="nm-btn" onClick={checkForUpdate}>
              重试
            </button>
            <button className="nm-btn" onClick={openReleasesPage}>
              打开发布页
            </button>
          </div>
        </div>
      )}
    </div>
  );
}
