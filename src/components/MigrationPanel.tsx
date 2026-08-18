import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { MigrationRule, MigrationReport } from "../types";

type MigrationStatus = {
  rules_count: number;
  poll_interval_secs: number;
};

/** 桌面清理面板：规则表展示 + 模版下载/导入 + 手动触发迁移 + 日志 */
export function MigrationPanel() {
  const [rules, setRules] = useState<MigrationRule[]>([]);
  const [running, setRunning] = useState(false);
  const [report, setReport] = useState<MigrationReport | null>(null);
  const [status, setStatus] = useState<MigrationStatus | null>(null);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  /** 迁移日志弹窗（日志可能很大，弹新框看，与机器人审计日志同模式） */
  const [logOpen, setLogOpen] = useState(false);
  const [logText, setLogText] = useState("");
  const [logBusy, setLogBusy] = useState(false);

  useEffect(() => {
    refresh();
  }, []);

  const refresh = async () => {
    try {
      const loaded = await invoke<{ version: number; rules: MigrationRule[] }>(
        "migration_rules_load"
      );
      setRules(loaded.rules ?? []);
      const st = await invoke<MigrationStatus>("migration_status");
      setStatus(st);
    } catch (e) {
      console.error("migration refresh failed", e);
    }
  };

  const downloadTemplate = async () => {
    try {
      const path = await invoke<string>("migration_rules_template_save");
      if (path) setNotice(`模版已保存：${path}`);
      setTimeout(() => setNotice(""), 5000);
    } catch (e) {
      setError(String(e));
    }
  };

  const importRules = async () => {
    try {
      const count = await invoke<number>("migration_rules_import");
      setNotice(`已导入 ${count} 条规则`);
      setTimeout(() => setNotice(""), 3000);
      await refresh();
    } catch (e) {
      setError(String(e));
    }
  };

  const openLog = async () => {
    if (logBusy) return;
    setLogBusy(true);
    try {
      setLogText(await invoke<string>("migration_log_read", { limit: 500 }));
    } catch (e) {
      setLogText(String(e));
    } finally {
      setLogBusy(false);
    }
    setLogOpen(true);
  };

  const runNow = async () => {
    // 上线安全审计：一键执行会按规则移动/删除已归档任务的绑定文件，先确认
    if (!window.confirm("立即执行桌面清理？将按规则表对已归档任务的绑定文件执行移动/删除。")) return;
    if (running) return;
    setRunning(true);
    setError("");
    try {
      const rep = await invoke<MigrationReport>("migration_run");
      setReport(rep);
      await refresh();
    } catch (e) {
      setError(String(e));
    } finally {
      setRunning(false);
    }
  };

  const deleteEnabled = rules.some((r) => r.action === "delete" && r.enabled);

  return (
    <div className="nm-card p-5">
      <h2 className="text-lg font-semibold text-[var(--t1)]">桌面清理</h2>
      <p className="mt-1 text-xs text-[var(--t5)]">
        任务完成满 7 天进入归档后，按规则自动迁移其绑定的桌面文件（看板任务附件不受影响）
      </p>
      <p className="mt-1 text-[11px] text-[var(--t5)]">
        规则用表格管理：下载表格模版（CSV，Excel/WPS 可直接打开编辑），改完导入即生效。
        规则顺序即优先级：靠前的行先匹配
      </p>

      {/* 规则表 */}
      <div className="mt-4 space-y-2">
        {rules.length === 0 && (
          <p className="text-xs text-[var(--t5)]">
            暂无规则：下载表格模版，用 Excel/WPS 编辑后导入
          </p>
        )}
        {rules.map((r, i) => (
          <div
            key={r.id}
            className="nm-inset flex items-center gap-2 rounded-xl px-3 py-1.5"
          >
            <span className="w-5 shrink-0 text-xs text-[var(--t5)]">{i + 1}</span>
            <span
              className={`shrink-0 text-xs ${r.enabled ? "text-[var(--success)]" : "text-[var(--t5)]"}`}
              title={r.enabled ? "已启用" : "已停用"}
            >
              {r.enabled ? "✓" : "—"}
            </span>
            <span className="min-w-0 flex-1 truncate text-xs text-[var(--t3)]" title={r.keywords.join("，")}>
              {r.keywords.join("，") || "（无关键字）"}
            </span>
            <span
              className={`shrink-0 text-xs ${
                r.action === "delete" && r.enabled ? "text-[var(--danger)]" : "text-[var(--t4)]"
              }`}
            >
              {r.action === "move" ? "移动归档" : "删除文件"}
            </span>
            <span className="min-w-0 flex-1 truncate text-xs text-[var(--t4)]" title={r.archiveDir}>
              {r.action === "move" ? r.archiveDir : ""}
            </span>
          </div>
        ))}
      </div>

      {/* 规则表操作：下载模版 → 本地编辑 JSON → 导入 */}
      <div className="mt-3 flex flex-wrap items-center gap-2">
        <button
          className="nm-btn px-3 py-1.5 text-xs text-[var(--t3)]"
          onClick={downloadTemplate}
        >
          ⬇ 下载表格模版
        </button>
        <button
          className="nm-btn px-3 py-1.5 text-xs text-[var(--t3)]"
          onClick={importRules}
        >
          ⬆ 导入规则表
        </button>
        <button
          className="nm-btn px-3 py-1.5 text-xs text-[var(--t3)]"
          onClick={openLog}
          disabled={logBusy}
        >
          {logBusy ? "读取中…" : "📋 查看迁移日志"}
        </button>
      </div>

      {deleteEnabled && (
        <p className="mt-2 text-xs text-[var(--danger)]">
          ⚠ 有启用的删除规则：匹配的文件将被直接删除，请确认规则无误
        </p>
      )}

      {/* 手动触发 */}
      <div className="mt-4 flex items-center gap-2 border-t border-[var(--t6)] pt-3">
        <button
          className={`px-3 py-1.5 text-sm text-[var(--t2)] ${
            running ? "nm-inset" : "nm-btn"
          }`}
          onClick={runNow}
          disabled={running}
        >
          {running ? "迁移中…" : "▶ 立即执行迁移"}
        </button>
        <p className="text-xs text-[var(--t5)]">
          后台每 {Math.round((status?.poll_interval_secs ?? 600) / 60)} 分钟自动检测一次
          {status ? ` · 现有规则 ${status.rules_count} 条` : ""}
        </p>
      </div>

      {notice && <p className="mt-2 text-xs text-[var(--brand)]">{notice}</p>}
      {error && <p className="mt-2 text-xs text-[var(--danger)]">{error}</p>}

      {/* 上次执行结果 */}
      {report && (
        <div className="mt-3 nm-inset rounded-xl p-3">
          <p className="text-xs text-[var(--t3)]">
            上次执行（{new Date(report.ts).toLocaleString()}）：归档{" "}
            {report.archived} · 移动 {report.moved} · 删除 {report.deleted} ·
            跳过 {report.skipped}
          </p>
          {report.log.length > 0 && (
            <pre className="mt-2 max-h-40 overflow-auto whitespace-pre-wrap text-[11px] leading-relaxed text-[var(--t4)]">
              {report.log.join("\n")}
            </pre>
          )}
        </div>
      )}

      {/* 迁移日志弹窗（日志可能很大，新框滚动查看） */}
      {logOpen && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/40 p-6">
          <div className="nm-card w-full max-w-2xl h-[70vh] flex flex-col p-4">
            <div className="flex items-center justify-between mb-2 shrink-0">
              <p className="text-sm font-medium text-[var(--t2)]">
                迁移日志（最新在前，最多 500 行）
              </p>
              <div className="flex gap-2">
                <button
                  className="nm-btn px-3 py-1 text-xs text-[var(--t3)]"
                  onClick={openLog}
                >
                  刷新
                </button>
                <button
                  className="nm-btn px-3 py-1 text-xs text-[var(--t3)]"
                  onClick={() => setLogOpen(false)}
                >
                  关闭
                </button>
              </div>
            </div>
            <pre className="flex-1 min-h-0 overflow-auto text-[11px] leading-relaxed text-[var(--t4)] whitespace-pre-wrap break-all">
              {logText}
            </pre>
          </div>
        </div>
      )}
    </div>
  );
}
