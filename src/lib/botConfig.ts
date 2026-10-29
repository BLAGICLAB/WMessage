// bot_get_config 的统一入口：原先各页面各自 invoke 并内联 payload
// 类型（9 份拷贝），线格式按 Rust BotConfigView（serde rename_all = camelCase，
// 见 src-tauri/src/bot/config/types.rs）收敛于此。所有字段可选——后端
// Option + skip_serializing_if 在 None/空时直接缺键，调用方按需兜底。
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type {
  McpServerConfig,
  ModelEntry,
} from "../components/SettingsPage/types";
import { unlistenSafe } from "./useTauriListen";

/** bot_get_config 返回视图（Rust BotConfigView camelCase 线格式） */
export interface BotConfigView {
  baseUrl?: string;
  model?: string;
  /** view 不含 key 本体，只有 has 标志 */
  hasApiKey?: boolean;
  bypassLlmOnPreStepHit?: boolean;
  allowedDirs?: string[];
  hasTavilyKey?: boolean;
  tavilyEnabled?: boolean | null;
  hasBraveKey?: boolean;
  braveEnabled?: boolean | null;
  pythonTimeoutSecs?: number | null;
  maxFunctionCalls?: number | null;
  /** 执行类谎报守卫触发词表（后端已把「没配过」回落成内置默认表）；
   *  guardClaimDefaultKeywords 是内置默认，只读参照 + 设置页「恢复默认」用 */
  guardClaimKeywords?: string[];
  guardClaimDefaultKeywords?: string[];
  /** 任务卡归档天数（None = 后端默认 7） */
  archiveAfterDays?: number | null;
  /** 授权模式：strict/ask/auto/yolo（None = ask） */
  permMode?: string | null;
  apiProvider?: string | null;
  maxTokens?: number | null;
  /** 推理强度后台默认：off/low/medium/high（缺字段/非法 → 前端按 medium） */
  reasoningEffort?: string | null;
  modelsByProvider?: {
    openai?: ModelEntry[];
    anthropic?: ModelEntry[];
  } | null;
  activeModelId?: {
    openai?: string | null;
    anthropic?: string | null;
  } | null;
  disabledVendors?: string[] | null;
  vendorKeys?: string[] | null;
  verifiedVendors?: string[] | null;
  /** 界面字体大小：small/standard/large（None = small；历史遗留的 xlarge
   *  按非法值同口径回落 small） */
  uiFontSize?: string | null;
  memoryConsolidation?: {
    enabled?: boolean;
    interval?: string;
    lastRunAt?: number | null;
  } | null;
  memoryControl?: {
    injectionEnabled?: boolean;
    autoWriteEnabled?: boolean;
    autoExtract?: string | null;
  } | null;
  memoryTuning?: Record<string, number> | null;
  mcpServers?: McpServerConfig[];
  taskMode?: string | null;
  maxRounds?: number | null;
  historyBudgetChars?: number | null;
  historyTruncateTargetPct?: number | null;
  reasoningEchoEnabled?: number | null;
  inTurnContextEdit?: number | null;
  searchMaxResults?: number | null;
  maxToolOutputBytes?: number | null;
  askBudget?: number | null;
  toolRules?: Array<{ tool: string; action: string }> | null;
}

export function getBotConfig(): Promise<BotConfigView> {
  return invoke<BotConfigView>("bot_get_config");
}

/** 字号同步共享体：页面各自独立 document，都要
 *  「启动拉一次 bot config 套 documentElement[data-font-size] + 监听
 *  bot-config-changed 重拉」（设置页保存会广播，点选未保存也会被广播）。
 *  onConfig 供 App 挂额外同步（归档天数刷新），仅在拉取成功时回调。
 *  拉取失败也套默认 small，避免界面还没应用就被卡。返回卸载清理函数；
 *  卸载早于 listen resolve 时自注销，防监听泄漏（同 useTauriListen 模式）。 */
export function applyFontSizeSync(
  onConfig?: (c: BotConfigView) => void,
): () => void {
  let cancelled = false;
  let unlistenCfg: (() => void) | undefined;
  const applyFromConfig = async () => {
    try {
      const c = await getBotConfig();
      document.documentElement.dataset.fontSize =
        c.uiFontSize === "standard" || c.uiFontSize === "large"
          ? c.uiFontSize
          : "small";
      onConfig?.(c);
    } catch {
      document.documentElement.dataset.fontSize = "small";
    }
  };
  void (async () => {
    await applyFromConfig();
    try {
      const u = await listen("bot-config-changed", applyFromConfig);
      if (cancelled) unlistenSafe(u);
      else unlistenCfg = u;
    } catch {
      // listen 失败只丢后续广播同步，启动值已套
    }
  })();
  return () => {
    cancelled = true;
    if (unlistenCfg) unlistenSafe(unlistenCfg);
  };
}
