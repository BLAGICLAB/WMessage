// SettingsPage 子模块：per-onion 配置档列表 + 导入/导出。
//
// 配置档 = 数据目录 onions/<name>.yaml。派发子 agent 时用 `onion` 参数点名，
// 服务端按「实际可用工具 = profile 白名单 ∩ 配置档 allowed」收窄权限。
// 内置三份（probe / writereditor / validator）在首次启动时播种，列表里标「内置」。

import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open, save } from "@tauri-apps/plugin-dialog";
import { Download, Layers, Upload } from "lucide-react";
import { handleCommandError, formatCommandError } from "../../lib/errorHandler";
import { EmptyState } from "../EmptyState";
import type { OnionSummary } from "./types";

/** 单条配置档：名字 + 角色 + 档位/工具数/轮数上限；坏文件单独标红并说明坏在哪 */
function OnionRow({
  cfg,
  busy,
  onExport,
}: {
  cfg: OnionSummary;
  busy: boolean;
  onExport: (name: string) => void;
}) {
  const broken = cfg.broken;
  return (
    <div className="flex items-center gap-2">
      <span className="min-w-0 flex-1 truncate text-xs text-[var(--t3)]">
        <span
          className={`font-medium ${broken ? "text-[var(--danger)]" : "text-[var(--t2)]"}`}
          title={broken ?? undefined}
        >
          {cfg.name}
        </span>
        {cfg.builtin && (
          <span className="ml-1 text-xs text-[var(--t5)]">（内置）</span>
        )}
        {broken ? (
          <span className="text-[var(--t5)]"> — 此文件无效：{broken}</span>
        ) : (
          <span className="text-[var(--t5)]">
            {" "}
            — {cfg.role || "（未写 role）"} · {cfg.profile} · 工具 {cfg.allowedCount} 个
            {cfg.maxTurn ? ` · 轮数上限 ${cfg.maxTurn}` : ""}
          </span>
        )}
      </span>
      {/* 坏文件也能导出：坏在 YAML 或字段上，原文往往正是要手改的东西 */}
      <button
        className="nm-btn shrink-0 px-2 py-1 text-xs text-[var(--t3)] inline-flex items-center gap-1"
        onClick={() => onExport(cfg.name)}
        disabled={busy}
        aria-label={`导出配置档 ${cfg.name}`}
        title="导出为 .yaml（原样保留注释）"
      >
        <Download size={12} aria-hidden />
      </button>
    </div>
  );
}

/** per-onion 配置档管理：导入 .yaml / 导出原样 */
export function OnionsPanel() {
  const [configs, setConfigs] = useState<OnionSummary[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const noticeTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  const refresh = useCallback(async () => {
    try {
      // `?? []` 不是吞错：invoke 抛错走下面的 catch 并显示；返回 null/非数组
      // 才是这里要挡的（后端命令改名或返回异常时，一个空列表远好过整个设置页
      // 白屏——SettingsPage.test.tsx 正是因为 mock 没覆盖该命令才崩过一次）
      const rows = await invoke<OnionSummary[] | null>("onions_list");
      setConfigs(Array.isArray(rows) ? rows : []);
    } catch (e) {
      handleCommandError(e, "onions_list", { silent: true });
      setError(formatCommandError(e));
    }
  }, []);

  useEffect(() => {
    refresh();
  }, [refresh]);

  const flash = (msg: string) => {
    setNotice(msg);
    if (noticeTimer.current) clearTimeout(noticeTimer.current);
    noticeTimer.current = setTimeout(() => setNotice(""), 3000);
  };

  const importConfig = async () => {
    if (busy) return;
    setBusy(true);
    setError("");
    try {
      // filters 让系统对话框只列 yaml；Rust 侧仍会再校验一次后缀
      const selected = await open({
        multiple: false,
        directory: false,
        filters: [{ name: "YAML", extensions: ["yaml", "yml"] }],
      });
      if (typeof selected !== "string") return;
      const name = await invoke<string>("onions_import", { path: selected });
      flash(`配置档「${name}」已导入`);
      await refresh();
    } catch (e) {
      handleCommandError(e, "onions_import", { silent: true });
      setError(formatCommandError(e));
    } finally {
      setBusy(false);
    }
  };

  const exportConfig = async (name: string) => {
    if (busy) return;
    setBusy(true);
    setError("");
    try {
      const dest = await save({
        defaultPath: `${name}.yaml`,
        filters: [{ name: "YAML", extensions: ["yaml"] }],
      });
      if (typeof dest !== "string") return;
      await invoke<string>("onions_export", { name, path: dest });
      flash(`已导出到 ${dest}`);
    } catch (e) {
      handleCommandError(e, "onions_export", { silent: true });
      setError(formatCommandError(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="nm-card p-5">
      <h2 className="text-lg font-semibold text-[var(--t1)]">子 Agent 配置档</h2>
      <p className="mt-1 text-xs text-[var(--t5)]">
        配置档 = 一份 yaml，派发子 agent 时用
        <code className="mx-1">onion</code>
        参数点名，按「实际可用工具 = 工具档位 ∩ 配置档清单」收窄权限。
        文件名须与文件内的 onion_id 一致；同名不会被静默覆盖
      </p>
      <div className="mt-3 flex items-center gap-2">
        <button
          className="nm-btn px-3 py-1.5 text-xs text-[var(--t3)] inline-flex items-center gap-1 whitespace-nowrap"
          onClick={importConfig}
          disabled={busy}
        >
          {busy ? (
            "处理中…"
          ) : (
            <>
              <Upload size={12} aria-hidden /> 导入配置档
            </>
          )}
        </button>
        {notice && <span className="text-xs text-[var(--success)]">{notice}</span>}
        {error && <span className="text-xs text-[var(--danger)]">{error}</span>}
      </div>
      {configs.length === 0 ? (
        <EmptyState
          icon={<Layers size={18} aria-hidden />}
          title="暂无配置档"
          description="内置三份会在首次启动时播种到数据目录；也可以点上方导入自己的 yaml"
        />
      ) : (
        <div className="mt-3 flex flex-col gap-1.5">
          {configs.map((c) => (
            <OnionRow
              key={c.name}
              cfg={c}
              busy={busy}
              onExport={exportConfig}
            />
          ))}
        </div>
      )}
    </div>
  );
}
