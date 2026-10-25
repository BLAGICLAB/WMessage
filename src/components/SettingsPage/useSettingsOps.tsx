/** 设置页的「ops」逻辑块（从 SettingsPage 主组件搬出）。
 *
 *  令牌 / 记忆整理 / 导入导出这一摊：状态与对应的 handler。
 *
 *  搬移是逐字的：语句原样保留、顺序不变；对外接口只有 params 与返回值。
 */
import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { handleCommandError, formatCommandError } from "../../lib/errorHandler";
import type { ApiStatus, ModelsByProvider, ActiveModelId } from "./types";
import type { AutoExtractMode } from "./constants";
import type { SettingsConfig } from "./types";
import type { ConfirmOptions } from "../../lib/useConfirm";
import type { Dispatch, SetStateAction } from "react";

export type SettingsOpsParams = {
  activeVendor: string | null;
  autostartBusy: boolean;
  autostartEnabled: boolean;
  config: SettingsConfig;
  configBusy: boolean;
  confirm: (messageOrOptions: string | ConfirmOptions) => Promise<boolean>;
  loadConfig: () => Promise<void>;
  onExportTasks: () => Promise<void>;
  onExportWorkspace: (() => Promise<void>) | undefined;
  onImportTasks: () => Promise<void>;
  onImportWorkspace: (() => Promise<void>) | undefined;
  saveConfig: (override?: { apiProvider: "openai" | "anthropic"; modelsByProvider: ModelsByProvider; activeModelId: ActiveModelId; disabledVendors: string[]; verifiedVendors: string[]; uiFontSize: "small" | "standard" | "large" | "xlarge"; hasApiKey: boolean; vendorKeys: string[]; bypassLlmOnPreStepHit: boolean; allowedDirs: string; guardClaimKeywords: string; guardClaimKeywordsDefault: string[]; hasTavilyKey: boolean; tavilyEnabled: boolean; hasBraveKey: boolean; braveEnabled: boolean; pythonTimeoutSecs: string; maxFunctionCalls: string; archiveAfterDays: string; permMode: "ask" | "strict" | "auto" | "yolo"; toolRules: { tool: string; action: "ask" | "allow" | "deny"; }[]; maxTokens: string; reasoningEffort: "medium" | "off" | "low" | "high"; memoryConsolidation: { enabled: boolean; interval: string; lastRunAt: number | null; }; memoryControl: { injectionEnabled: boolean; autoWriteEnabled: boolean; autoExtract: "auto" | "off" | "confirm"; }; memoryTuning: Record<string, number> | null; maxRounds: string; historyBudgetChars: string; historyTruncateTargetPct: string; subagentMaxTurns: string; subagentMaxWallSecs: string; searchMaxResults: string; maxToolOutputBytes: string; askBudget: string; } | ((c: { apiProvider: "openai" | "anthropic"; modelsByProvider: ModelsByProvider; activeModelId: ActiveModelId; disabledVendors: string[]; verifiedVendors: string[]; uiFontSize: "small" | "standard" | "large" | "xlarge"; hasApiKey: boolean; vendorKeys: string[]; bypassLlmOnPreStepHit: boolean; allowedDirs: string; guardClaimKeywords: string; guardClaimKeywordsDefault: string[]; hasTavilyKey: boolean; tavilyEnabled: boolean; hasBraveKey: boolean; braveEnabled: boolean; pythonTimeoutSecs: string; maxFunctionCalls: string; archiveAfterDays: string; permMode: "ask" | "strict" | "auto" | "yolo"; toolRules: { tool: string; action: "ask" | "allow" | "deny"; }[]; maxTokens: string; reasoningEffort: "medium" | "off" | "low" | "high"; memoryConsolidation: { enabled: boolean; interval: string; lastRunAt: number | null; }; memoryControl: { injectionEnabled: boolean; autoWriteEnabled: boolean; autoExtract: "auto" | "off" | "confirm"; }; memoryTuning: Record<string, number> | null; maxRounds: string; historyBudgetChars: string; historyTruncateTargetPct: string; subagentMaxTurns: string; subagentMaxWallSecs: string; searchMaxResults: string; maxToolOutputBytes: string; askBudget: string; }) => { apiProvider: "openai" | "anthropic"; modelsByProvider: ModelsByProvider; activeModelId: ActiveModelId; disabledVendors: string[]; verifiedVendors: string[]; uiFontSize: "small" | "standard" | "large" | "xlarge"; hasApiKey: boolean; vendorKeys: string[]; bypassLlmOnPreStepHit: boolean; allowedDirs: string; guardClaimKeywords: string; guardClaimKeywordsDefault: string[]; hasTavilyKey: boolean; tavilyEnabled: boolean; hasBraveKey: boolean; braveEnabled: boolean; pythonTimeoutSecs: string; maxFunctionCalls: string; archiveAfterDays: string; permMode: "ask" | "strict" | "auto" | "yolo"; toolRules: { tool: string; action: "ask" | "allow" | "deny"; }[]; maxTokens: string; reasoningEffort: "medium" | "off" | "low" | "high"; memoryConsolidation: { enabled: boolean; interval: string; lastRunAt: number | null; }; memoryControl: { injectionEnabled: boolean; autoWriteEnabled: boolean; autoExtract: "auto" | "off" | "confirm"; }; memoryTuning: Record<string, number> | null; maxRounds: string; historyBudgetChars: string; historyTruncateTargetPct: string; subagentMaxTurns: string; subagentMaxWallSecs: string; searchMaxResults: string; maxToolOutputBytes: string; askBudget: string; }) | undefined, opts?: { skipReload?: boolean | undefined; } | undefined) => Promise<boolean>;
  setAutostartBusy: Dispatch<SetStateAction<boolean>>;
  setAutostartEnabled: Dispatch<SetStateAction<boolean>>;
  setAutostartError: Dispatch<SetStateAction<string>>;
  setBotError: Dispatch<SetStateAction<string>>;
  setConfig: Dispatch<SetStateAction<SettingsConfig>>;
  setConfigBusy: Dispatch<SetStateAction<boolean>>;
};

export function useSettingsOps(params: SettingsOpsParams) {
  const {
    activeVendor,
    autostartBusy,
    autostartEnabled,
    config,
    configBusy,
    confirm,
    loadConfig,
    onExportTasks,
    onExportWorkspace,
    onImportTasks,
    onImportWorkspace,
    saveConfig,
    setAutostartBusy,
    setAutostartEnabled,
    setAutostartError,
    setBotError,
    setConfig,
    setConfigBusy,
  } = params;

  const [status, setStatus] = useState<ApiStatus>({
    enabled: false,
    port: 4763,
    token: "",
  });
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [copied, setCopied] = useState(false);
  const [exporting, setExporting] = useState(false);
  const [importing, setImporting] = useState(false);
  const [exportingWs, setExportingWs] = useState(false);
  const [importingWs, setImportingWs] = useState(false);
  const [consolidateBusy, setConsolidateBusy] = useState(false);
  const [consolidateMsg, setConsolidateMsg] = useState("");
  const toggleTavily = async () => {
    const next = { ...config, tavilyEnabled: !config.tavilyEnabled };
    setConfig(next);
    await saveConfig(next);
  };
  const toggleBrave = async () => {
    const next = { ...config, braveEnabled: !config.braveEnabled };
    setConfig(next);
    await saveConfig(next);
  };
  const setPermMode = async (mode: "strict" | "ask" | "auto" | "yolo") => {
    if (configBusy || config.permMode === mode) return;
    const next = { ...config, permMode: mode };
    setConfig(next);
    await saveConfig(next);
  };
  const setConsolidation = async (
    patch: Partial<{ enabled: boolean; interval: string }>,
  ) => {
    if (configBusy) return;
    const next = {
      ...config,
      memoryConsolidation: { ...config.memoryConsolidation, ...patch },
    };
    setConfig(next);
    await saveConfig(next);
  };
  const setMemoryControl = async (
    patch: Partial<{
      injectionEnabled: boolean;
      autoWriteEnabled: boolean;
      autoExtract: AutoExtractMode;
    }>,
  ) => {
    if (configBusy) return;
    const next = {
      ...config,
      memoryControl: { ...config.memoryControl, ...patch },
    };
    setConfig(next);
    await saveConfig(next);
  };
  const renderMemoryToggle = (
    label: string,
    description: string,
    field: "injectionEnabled" | "autoWriteEnabled",
  ) => {
    const on = config.memoryControl[field];
    return (
      <div className="flex items-center justify-between gap-4">
        <div className="min-w-0">
          <p className="text-xs text-[var(--t3)]">{label}</p>
          <p className="mt-0.5 text-xs text-[var(--t5)]">{description}</p>
        </div>
        <button
          role="switch"
          aria-checked={on}
          aria-label={label}
          className={`btn-sm-action ${
            on ? "nm-inset" : "nm-outset"
          }`}
          onClick={() => setMemoryControl({ [field]: !on })}
          disabled={configBusy}
        >
          {on ? "已开启" : "已关闭"}
        </button>
      </div>
    );
  };
  const consolidateNow = async () => {
    if (consolidateBusy) return;
    setConsolidateBusy(true);
    setConsolidateMsg("");
    try {
      const r = await invoke<{
        merged: number;
        distilled: number;
        contradictions: number;
      }>("memory_consolidate_now");
      setConsolidateMsg(
        `整理完成：合并 ${r.merged} 条、提炼 ${r.distilled} 条规律、解决 ${r.contradictions} 处矛盾`,
      );
      await loadConfig();
    } catch (e) {
      handleCommandError(e, "memory_consolidate_now", { silent: true });
      setConsolidateMsg(`整理失败：${formatCommandError(e)}`);
    } finally {
      setConsolidateBusy(false);
      setTimeout(() => setConsolidateMsg(""), 4000);
    }
  };
  const clearVendorKey = async () => {
    if (configBusy || !activeVendor) return;
    if (
      !(await confirm(
        `清除厂商「${activeVendor}」已保存的 API Key？清除后该厂商的机器人调用将失败。`,
      ))
    )
      return;
    setConfigBusy(true);
    setBotError("");
    try {
      await invoke("bot_clear_vendor_key", { vendor: activeVendor });
      await loadConfig();
    } catch (e) {
      handleCommandError(e, "bot_clear_vendor_key", { silent: true });
      setBotError(formatCommandError(e));
    } finally {
      setConfigBusy(false);
    }
  };
  const toggleAutostart = async () => {
    if (autostartBusy) return;
    setAutostartBusy(true);
    setAutostartError("");
    const next = !autostartEnabled;
    try {
      await invoke(
        next ? "plugin:autostart|enable" : "plugin:autostart|disable",
      );
      setAutostartEnabled(next);
    } catch (e) {
      handleCommandError(
        e,
        next ? "plugin:autostart|enable" : "plugin:autostart|disable",
        { silent: true },
      );
      setAutostartError(formatCommandError(e));
      // 重新拉一次真实状态，UI 不漂
      try {
        const real = await invoke<boolean>("plugin:autostart|is_enabled");
        setAutostartEnabled(real);
      } catch {
        // 拉失败也无所谓，保持 next 翻转前的状态
      }
    } finally {
      setAutostartBusy(false);
    }
  };
  const refresh = async () => {
    try {
      const s = await invoke<ApiStatus>("api_status");
      setStatus(s);
    } catch (e) {
      handleCommandError(e, "api_status", { silent: true });
    }
  };
  useEffect(() => {
    // 挂载即刷新（外部数据同步既有模式）：refresh 链路含 setState，
    // lint 对 effect 内调用按同步路径处理，保既有直写
    // oxlint-disable-next-line react/set-state-in-effect
    refresh();
  }, []);
  const toggle = async () => {
    if (busy) return;
    setBusy(true);
    setError("");
    try {
      if (status.enabled) {
        await invoke("api_stop");
      } else {
        await invoke("api_start");
      }
      await refresh();
    } catch (e) {
      handleCommandError(e, status.enabled ? "api_stop" : "api_start", {
        silent: true,
      });
      setError(formatCommandError(e));
      await refresh();
    } finally {
      setBusy(false);
    }
  };
  const runExport = async () => {
    if (exporting) return;
    setExporting(true);
    try {
      await onExportTasks();
    } finally {
      setExporting(false);
    }
  };
  const runImport = async () => {
    if (importing) return;
    setImporting(true);
    try {
      await onImportTasks();
    } finally {
      setImporting(false);
    }
  };
  const runExportWs = async () => {
    if (exportingWs) return;
    if (!onExportWorkspace) return;
    setExportingWs(true);
    try {
      await onExportWorkspace();
    } finally {
      setExportingWs(false);
    }
  };
  const runImportWs = async () => {
    if (importingWs) return;
    if (!onImportWorkspace) return;
    setImportingWs(true);
    try {
      await onImportWorkspace();
    } finally {
      setImportingWs(false);
    }
  };
  const copyToken = async () => {
    // disabled 状态下后端字段缺席(见 ApiStatus.token 注释),直接 return 避免
    // 把 undefined 喂给 clipboard.writeText
    if (!status.token) return;
    try {
      await navigator.clipboard.writeText(status.token);
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch (e) {
      handleCommandError(e, "clipboard", { silent: true });
    }
  };
  const rotateToken = async () => {
    if (busy) return;
    if (
      !(await confirm(
        "重新生成 token？旧 token 会立即失效，已授权的客户端需要换新 token。",
      ))
    )
      return;
    setBusy(true);
    setError("");
    try {
      await invoke("api_rotate_token");
      await refresh();
    } catch (e) {
      handleCommandError(e, "api_rotate_token", { silent: true });
      setError(formatCommandError(e));
      await refresh();
    } finally {
      setBusy(false);
    }
  };

  return {
    busy,
    clearVendorKey,
    consolidateBusy,
    consolidateMsg,
    consolidateNow,
    copied,
    copyToken,
    error,
    exporting,
    exportingWs,
    importing,
    importingWs,
    renderMemoryToggle,
    rotateToken,
    runExport,
    runExportWs,
    runImport,
    runImportWs,
    setConsolidation,
    setMemoryControl,
    setPermMode,
    status,
    toggle,
    toggleAutostart,
    toggleBrave,
    toggleTavily,
  };
}
