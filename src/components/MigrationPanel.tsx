import { useEffect, useState } from "react";
import { useFlashMsg } from "../lib/useFlashMsg";
import { invoke } from "@tauri-apps/api/core";
import {
  Check,
  ClipboardList,
  Download,
  FolderPlus,
  Play,
  ScanSearch,
  Trash2,
  TriangleAlert,
  Upload,
} from "lucide-react";
import { handleCommandError, formatCommandError } from "../lib/errorHandler";
import { getArchiveAfterDays } from "../lib/archiveRule";
import { useConfirm } from "../lib/useConfirm";
import { Modal } from "../ui/Modal";
import type {
  MigrationRule,
  MigrationReport,
  ScanSource,
  SourcesFile,
} from "../types";

type MigrationStatus = {
  rules_count: number;
  poll_interval_secs: number;
};

/** 来源行显示名：桌面预设 / 自定义取文件夹名 */
function sourceLabel(s: ScanSource): string {
  if (s.kind === "desktop") return "桌面（预设）";
  const parts = s.path.split(/[\\/]/).filter(Boolean);
  return parts[parts.length - 1] || s.path;
}

/** 文件整理面板：整理来源管理 + 全局规则表 + 绑定整理/扫描整理双入口 + 共享整理记录 */
export function MigrationPanel() {
  const confirm = useConfirm();
  const [rules, setRules] = useState<MigrationRule[]>([]);
  const [sources, setSources] = useState<ScanSource[]>([]);
  const [sourcesBusy, setSourcesBusy] = useState(false);
  const [running, setRunning] = useState(false);
  const [scanning, setScanning] = useState(false);
  const [report, setReport] = useState<MigrationReport | null>(null);
  const [scanReport, setScanReport] = useState<MigrationReport | null>(null);
  const [status, setStatus] = useState<MigrationStatus | null>(null);
  const [error, setError] = useState("");
  const [notice, flashNotice] = useFlashMsg(3000);
  /** 整理记录弹窗（绑定整理与扫描整理写同一份 migration.log，最新在前） */
  const [logOpen, setLogOpen] = useState(false);
  const [logText, setLogText] = useState("");
  const [logBusy, setLogBusy] = useState(false);

  const refresh = async () => {
    try {
      const loaded = await invoke<{ version: number; rules: MigrationRule[] }>(
        "migration_rules_load",
      );
      setRules(loaded.rules ?? []);
      const st = await invoke<MigrationStatus>("migration_status");
      setStatus(st);
      const src = await invoke<SourcesFile>("migration_sources_load");
      setSources(src.sources ?? []);
    } catch (e) {
      handleCommandError(e, "migration refresh", { silent: true });
    }
  };

  // 挂载即刷新（原 effect 声明在 refresh 定义之前——immutability lint 误判自引用）
  useEffect(() => {
    refresh();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const persistSources = async (next: ScanSource[]) => {
    if (sourcesBusy) return;
    setSourcesBusy(true);
    setError("");
    try {
      await invoke("migration_sources_save", {
        sources: { sources: next } satisfies SourcesFile,
      });
      setSources(next);
    } catch (e) {
      handleCommandError(e, "migration_sources_save", { silent: true });
      setError(formatCommandError(e));
    } finally {
      setSourcesBusy(false);
    }
  };

  const toggleSource = (idx: number) => {
    const next = sources.map((s, i) =>
      i === idx ? { ...s, enabled: !s.enabled } : s,
    );
    void persistSources(next);
  };

  const removeSource = async (idx: number) => {
    const target = sources[idx];
    if (target.kind === "desktop") return; // 预设源只可停用，不可移除
    if (
      !(await confirm(`移除整理来源「${sourceLabel(target)}」？\n${target.path}`))
    )
      return;
    await persistSources(sources.filter((_, i) => i !== idx));
  };

  const updateArchiveRoot = (idx: number, value: string) => {
    const target = sources[idx];
    if (target.archiveRoot === value) return; // 未变化不落盘
    const next = sources.map((s, i) =>
      i === idx ? { ...s, archiveRoot: value } : s,
    );
    void persistSources(next);
  };

  const addFolder = async () => {
    if (sourcesBusy) return;
    setSourcesBusy(true);
    setError("");
    try {
      const src = await invoke<SourcesFile>("migration_sources_add_folder");
      setSources(src.sources ?? []);
      flashNotice("已添加整理来源");
    } catch (e) {
      handleCommandError(e, "migration_sources_add_folder", { silent: true });
      setError(formatCommandError(e));
    } finally {
      setSourcesBusy(false);
    }
  };

  const downloadTemplate = async () => {
    try {
      const path = await invoke<string>("migration_rules_template_save");
      if (path) flashNotice(`模版已保存：${path}`, 5000);
    } catch (e) {
      handleCommandError(e, "migration_rules_template_save", { silent: true });
      setError(formatCommandError(e));
    }
  };

  const importRules = async () => {
    try {
      const count = await invoke<number>("migration_rules_import");
      flashNotice(`已导入 ${count} 条规则`);
      await refresh();
    } catch (e) {
      handleCommandError(e, "migration_rules_import", { silent: true });
      setError(formatCommandError(e));
    }
  };

  const openLog = async () => {
    if (logBusy) return;
    setLogBusy(true);
    try {
      setLogText(await invoke<string>("migration_log_read", { limit: 500 }));
    } catch (e) {
      handleCommandError(e, "migration_log_read", { silent: true });
      setLogText(formatCommandError(e));
    } finally {
      setLogBusy(false);
    }
    setLogOpen(true);
  };

  const runNow = async () => {
    // 一键执行会按规则移动/删除已归档任务的绑定文件，先确认
    if (
      !(await confirm(
        "立即执行绑定文件整理？将按规则表对已归档任务的绑定文件执行移动/删除。",
      ))
    )
      return;
    if (running) return;
    setRunning(true);
    setError("");
    try {
      const rep = await invoke<MigrationReport>("migration_run");
      setReport(rep);
      await refresh();
    } catch (e) {
      handleCommandError(e, "migration_run", { silent: true });
      setError(formatCommandError(e));
    } finally {
      setRunning(false);
    }
  };

  const scanNow = async () => {
    // 扫描整理按规则移动/删除各来源中的未绑定文件，先确认
    if (
      !(await confirm(
        "立即扫描整理？将按规则表对各整理来源中的未绑定文件执行移动/删除（任务卡绑定的文件不受影响）。",
      ))
    )
      return;
    if (scanning) return;
    setScanning(true);
    setError("");
    try {
      const rep = await invoke<MigrationReport>("migration_scan");
      setScanReport(rep);
    } catch (e) {
      handleCommandError(e, "migration_scan", { silent: true });
      setError(formatCommandError(e));
    } finally {
      setScanning(false);
    }
  };

  const deleteEnabled = rules.some((r) => r.action === "delete" && r.enabled);

  return (
    <div className="nm-card p-5">
      <p className="text-xs text-[var(--t5)]">
        两种方式：① 任务卡完成满 {getArchiveAfterDays()}{" "}
        天归档时，自动按规则整理其绑定文件（任意地址，看板任务附件不受影响）；
        ② 手动扫描各整理来源，按规则归类其中未绑定的文件（任务卡绑定的一律跳过）
      </p>

      {/* 整理来源 */}
      <div className="mt-4 flex items-center justify-between">
        <p className="text-xs font-medium text-[var(--t3)]">整理来源</p>
        <button
          className="nm-btn px-2.5 py-1 text-xs text-[var(--t3)] inline-flex items-center gap-1 whitespace-nowrap"
          onClick={addFolder}
          disabled={sourcesBusy}
        >
          <FolderPlus size={12} aria-hidden /> 添加文件夹
        </button>
      </div>
      <div className="mt-2 space-y-1.5">
        {sources.map((s, i) => (
          <div
            key={`${s.kind}-${s.path}`}
            className="nm-inset flex items-center gap-2 rounded-xl px-3 py-1.5"
          >
            <button
              className="shrink-0"
              title={s.enabled ? "已启用，点击停用" : "已停用，点击启用"}
              onClick={() => toggleSource(i)}
              disabled={sourcesBusy}
            >
              <span
                className={`text-xs ${s.enabled ? "text-[var(--success)]" : "text-[var(--t5)]"}`}
              >
                {s.enabled ? <Check size={12} strokeWidth={3} /> : "—"}
              </span>
            </button>
            <span className="shrink-0 text-xs text-[var(--t3)]">
              {sourceLabel(s)}
            </span>
            <span
              className="min-w-0 flex-1 truncate text-[11px] text-[var(--t5)]"
              title={s.kind === "custom" ? s.path : "运行时解析真实桌面位置"}
            >
              {s.kind === "custom" ? s.path : "运行时解析真实桌面位置"}
            </span>
            {s.kind === "custom" ? (
              <>
                <input
                  className="w-28 shrink-0 rounded-lg bg-transparent px-1.5 py-0.5 text-[11px] text-[var(--t4)] nm-inset"
                  placeholder="归档根（可留空）"
                  defaultValue={s.archiveRoot}
                  title="归档根：相对该文件夹；留空 = 文件夹本身"
                  onBlur={(e) => updateArchiveRoot(i, e.target.value.trim())}
                />
                <button
                  className="shrink-0 text-xs text-[var(--t5)] hover:text-[var(--danger)]"
                  title="移除该来源"
                  onClick={() => void removeSource(i)}
                  disabled={sourcesBusy}
                >
                  <Trash2 size={12} aria-hidden />
                </button>
              </>
            ) : (
              <span
                className="shrink-0 text-[11px] text-[var(--t5)]"
                title="桌面来源的归档根固定为桌面本身（与既有规则表兼容）"
              >
                归档根 = 桌面
              </span>
            )}
          </div>
        ))}
        {sources.length === 0 && (
          <p className="text-xs text-[var(--t5)]">
            暂无来源：添加文件夹，或恢复默认桌面来源
          </p>
        )}
      </div>

      {/* 规则表（全局一张，两种整理共用） */}
      <p className="mt-4 text-xs font-medium text-[var(--t3)]">
        规则表（全局共用）
      </p>
      <p className="mt-1 text-[11px] text-[var(--t5)]">
        规则用表格管理：下载表格模版（CSV，Excel/WPS
        可直接打开编辑），改完导入即生效。 规则顺序即优先级：靠前的行先匹配
      </p>
      <p className="mt-1 text-[11px] text-[var(--t5)]">
        {
          "归档目录可写相对目录（相对该文件的来源文件夹，如 工资/{year}），也可写绝对路径固定归档位置（如 E:\\归档\\{year}）；含「..」一律拒绝"
        }
      </p>
      <div className="mt-2 space-y-1.5">
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
            <span className="w-5 shrink-0 text-xs text-[var(--t5)]">
              {i + 1}
            </span>
            <span
              className={`shrink-0 text-xs ${r.enabled ? "text-[var(--success)]" : "text-[var(--t5)]"}`}
              title={r.enabled ? "已启用" : "已停用"}
            >
              {r.enabled ? <Check size={10} strokeWidth={3} /> : "—"}
            </span>
            <span
              className="min-w-0 flex-1 truncate text-xs text-[var(--t3)]"
              title={r.keywords.join("，")}
            >
              {r.keywords.join("，") || "（无关键字）"}
            </span>
            <span
              className={`shrink-0 text-xs ${
                r.action === "delete" && r.enabled
                  ? "text-[var(--danger)]"
                  : "text-[var(--t4)]"
              }`}
            >
              {r.action === "move" ? "移动归档" : "删除文件"}
            </span>
            <span
              className="min-w-0 flex-1 truncate text-xs text-[var(--t4)]"
              title={r.archiveDir}
            >
              {r.action === "move" ? r.archiveDir : ""}
            </span>
          </div>
        ))}
      </div>

      {/* 规则表操作：下载模版 → 本地编辑 JSON → 导入 */}
      <div className="mt-3 flex flex-wrap items-center gap-2">
        <button
          className="nm-btn px-3 py-1.5 text-xs text-[var(--t3)] inline-flex items-center gap-1 whitespace-nowrap"
          onClick={downloadTemplate}
        >
          <Download size={12} aria-hidden /> 下载表格模版
        </button>
        <button
          className="nm-btn px-3 py-1.5 text-xs text-[var(--t3)] inline-flex items-center gap-1 whitespace-nowrap"
          onClick={importRules}
        >
          <Upload size={12} aria-hidden /> 导入规则表
        </button>
        <button
          className="nm-btn px-3 py-1.5 text-xs text-[var(--t3)] inline-flex items-center gap-1 whitespace-nowrap"
          onClick={openLog}
          disabled={logBusy}
        >
          {logBusy ? (
            "读取中…"
          ) : (
            <>
              <ClipboardList size={12} aria-hidden /> 查看整理记录
            </>
          )}
        </button>
      </div>

      {deleteEnabled && (
        <p className="mt-2 text-xs text-[var(--danger)]">
          <TriangleAlert
            size={11}
            aria-hidden
            className="inline-block align-[-2px]"
          />{" "}
          有启用的删除规则：匹配的文件将被直接删除，请确认规则无误
        </p>
      )}

      {/* 双入口：绑定整理（归档自动 + 手动）与扫描整理（仅手动，不定时） */}
      <div className="mt-4 flex flex-wrap items-center gap-2 border-t border-[var(--t6)] pt-3">
        <button
          className={`px-3 py-1.5 text-sm text-[var(--t2)] inline-flex items-center gap-1 ${
            running ? "nm-inset" : "nm-btn"
          }`}
          onClick={runNow}
          disabled={running || scanning}
        >
          <Play size={12} aria-hidden />
          {running ? "整理中…" : "立即执行绑定整理"}
        </button>
        <button
          className={`px-3 py-1.5 text-sm text-[var(--t2)] inline-flex items-center gap-1 ${
            scanning ? "nm-inset" : "nm-btn"
          }`}
          onClick={scanNow}
          disabled={running || scanning}
        >
          <ScanSearch size={12} aria-hidden />
          {scanning ? "扫描中…" : "扫描整理"}
        </button>
        <p className="text-xs text-[var(--t5)]">
          绑定整理后台每{" "}
          {Math.round((status?.poll_interval_secs ?? 600) / 60)}{" "}
          分钟随归档自动执行 · 扫描整理仅手动
          {status ? ` · 现有规则 ${status.rules_count} 条` : ""}
        </p>
      </div>

      {notice && <p className="mt-2 text-xs text-[var(--brand)]">{notice}</p>}
      {error && <p className="mt-2 text-xs text-[var(--danger)]">{error}</p>}

      {/* 上次执行结果（绑定整理 / 扫描整理各留一份） */}
      {report && (
        <div className="mt-3 nm-inset rounded-xl p-3">
          <p className="text-xs text-[var(--t3)]">
            上次绑定整理（{new Date(report.ts).toLocaleString()}）：归档{" "}
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
      {scanReport && (
        <div className="mt-3 nm-inset rounded-xl p-3">
          <p className="text-xs text-[var(--t3)]">
            上次扫描整理（{new Date(scanReport.ts).toLocaleString()}）：移动{" "}
            {scanReport.moved} · 删除 {scanReport.deleted} · 跳过{" "}
            {scanReport.skipped}
          </p>
          {scanReport.log.length > 0 && (
            <pre className="mt-2 max-h-40 overflow-auto whitespace-pre-wrap text-[11px] leading-relaxed text-[var(--t4)]">
              {scanReport.log.join("\n")}
            </pre>
          )}
        </div>
      )}

      {/* 整理记录弹窗（绑定整理与扫描整理共用一份日志，最新在前） */}
      {logOpen && (
        <Modal className="fixed inset-0 z-50 flex items-center justify-center bg-black/40 p-6">
          <div className="nm-card w-full max-w-2xl h-[70vh] flex flex-col p-4">
            <div className="flex items-center justify-between mb-2 shrink-0">
              <p className="text-sm font-medium text-[var(--t2)]">
                整理记录（最新在前，最多 500 行）
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
        </Modal>
      )}
    </div>
  );
}
