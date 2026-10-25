/** 设置页的「vendors」逻辑块（从 SettingsPage 主组件搬出）。
 *
 *  模型库与厂商页：供应商元数据同步、厂商增删改、模型条目维护、
 *  配置保存。这一摊的 state 与 effect 放在一起，effect 的依赖才
 *  是本作用域内的稳定值。
 *
 *  搬移是逐字的：语句原样保留、顺序不变；对外接口只有 params 与返回值。
 */
import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { emit } from "@tauri-apps/api/event";
import { Check } from "lucide-react";
import { handleCommandError, formatCommandError } from "../../lib/errorHandler";
import { MAX_ARCHIVE_DAYS } from "../../lib/archiveRule";
import { fetchProviders, fetchModelsByProvider, syncModelsDev, MetaProvider, MetaModel } from "../../lib/modelMeta";
import { ModelEntry, SectionKey, genModelId, metaModelToEntry } from "./types";
import { PROVIDER_PRESETS, KNOWN_PROTOCOL_URLS, CURATED_PROVIDER_KEYS } from "./constants";
import { normalizeVendorName } from "./providerLogoMap";
import type { SettingsConfig } from "./types";
import type { ConfirmOptions } from "../../lib/useConfirm";
import type { Dispatch, SetStateAction } from "react";

export type SettingsVendorsParams = {
  braveKeyInput: string;
  config: SettingsConfig;
  configBusy: boolean;
  configSaved: boolean;
  confirm: (messageOrOptions: string | ConfirmOptions) => Promise<boolean>;
  loadConfig: () => Promise<void>;
  loadParamViews: () => Promise<void>;
  paramNumber: (key: string, raw: string) => number | null;
  setActiveSection: Dispatch<SetStateAction<SectionKey>>;
  setBotError: Dispatch<SetStateAction<string>>;
  setBraveKeyInput: Dispatch<SetStateAction<string>>;
  setConfig: Dispatch<SetStateAction<SettingsConfig>>;
  setConfigBusy: Dispatch<SetStateAction<boolean>>;
  setConfigSaved: Dispatch<SetStateAction<boolean>>;
  setTavilyKeyInput: Dispatch<SetStateAction<string>>;
  tavilyKeyInput: string;
};

export function useSettingsVendors(params: SettingsVendorsParams) {
  const {
    braveKeyInput,
    config,
    configBusy,
    configSaved,
    confirm,
    loadConfig,
    loadParamViews,
    paramNumber,
    setActiveSection,
    setBotError,
    setBraveKeyInput,
    setConfig,
    setConfigBusy,
    setConfigSaved,
    setTavilyKeyInput,
    tavilyKeyInput,
  } = params;

  const [mountedSections, setMountedSections] = useState<
    ReadonlySet<SectionKey>
  >(new Set(["general"]));
  const openSection = (key: SectionKey) => {
    setActiveSection(key);
    setMountedSections((prev) =>
      prev.has(key) ? prev : new Set(prev).add(key),
    );
  };
  const [showProviderPicker, setShowProviderPicker] = useState(false);
  const [metaProviders, setMetaProviders] = useState<MetaProvider[] | null>(
    null,
  );
  const [metaSearch, setMetaSearch] = useState("");
  const [metaSyncBusy, setMetaSyncBusy] = useState(false);
  const [metaSyncMsg, setMetaSyncMsg] = useState("");
  const [metaApplyBusy, setMetaApplyBusy] = useState(false);
  const applyVendorModels = async (
    name: string,
    protocol: "openai" | "anthropic",
    models: ModelEntry[],
  ) => {
    const list = config.modelsByProvider[protocol] ?? [];
    const norm = normalizeVendorName(name);
    // 厂商制语义：同协议下其他厂商条目保留，仅替换同名（归一化后）厂商旧条目（若有）。
    // 无 vendor 的老条目用 __legacy__ 哨兵而非 vendorNameOf 的「兼容」兜底名——
    // 保证永不会被归一化匹配进任何真实厂商（哨兵名不可能出现在条目里）
    const kept = list.filter(
      (m) => normalizeVendorName(m.vendor ?? `__legacy__${protocol}`) !== norm,
    );
    const existingName =
      list.find(
        (m) =>
          normalizeVendorName(m.vendor ?? `__legacy__${protocol}`) === norm,
      )?.vendor ?? name;
    const flagged = models.map((m, i) => ({
      ...m,
      vendor: existingName,
      enabled: i === 0,
    }));
    const curActiveId = config.activeModelId[protocol] ?? null;
    const activeIsMine =
      config.apiProvider === protocol &&
      curActiveId !== null &&
      list.some(
        (m) =>
          m.id === curActiveId &&
          normalizeVendorName(m.vendor ?? `__legacy__${protocol}`) === norm,
      );
    const nothingActive =
      !(config.activeModelId.openai ?? null) &&
      !(config.activeModelId.anthropic ?? null);
    const takeActive = activeIsMine || nothingActive;
    const next = {
      ...config,
      apiProvider: takeActive ? protocol : config.apiProvider,
      modelsByProvider: {
        ...config.modelsByProvider,
        [protocol]: [...kept, ...flagged],
      },
      activeModelId: takeActive
        ? { ...config.activeModelId, [protocol]: flagged[0]?.id ?? null }
        : config.activeModelId,
    };
    setConfig(next);
    // skipReload：本地 state 已是合并后最新（loadConfig 会拿旧 mock/磁盘值覆盖刚点字段）
    await saveConfig(next, { skipReload: true });
    setShowProviderPicker(false);
    setActiveVendor(existingName);
  };
  const applyProviderPreset = async (p: (typeof PROVIDER_PRESETS)[number]) => {
    const models = p.models.map((m) => ({
      id: genModelId(),
      label: m.id,
      model: m.id,
      baseUrl: p.baseUrl,
      vendor: p.name,
      ...(m.contextK != null ? { contextK: m.contextK } : {}),
    }));
    await applyVendorModels(p.name, p.protocol, models);
  };
  const applyMetaProvider = async (p: MetaProvider) => {
    if (metaApplyBusy) return;
    setMetaApplyBusy(true);
    setMetaSyncMsg("");
    try {
      const ms = await fetchModelsByProvider(p.provider_key);
      if (ms === null) {
        // meta 模块中途异常：网格整体降级回内置预设
        setMetaProviders(null);
        return;
      }
      if (ms.length === 0) {
        setMetaSyncMsg(`「${p.provider_name}」在模型库中暂无模型数据`);
        return;
      }
      const baseUrl = p.default_base_url ?? "";
      const protocol: "openai" | "anthropic" =
        p.provider_key === "anthropic" || baseUrl.includes("anthropic")
          ? "anthropic"
          : "openai";
      await applyVendorModels(
        p.provider_name,
        protocol,
        ms.map((m) => metaModelToEntry(m, p.provider_name, baseUrl)),
      );
    } finally {
      setMetaApplyBusy(false);
    }
  };
  const syncMetaLibrary = async () => {
    if (metaSyncBusy) return;
    setMetaSyncBusy(true);
    setMetaSyncMsg("");
    try {
      const r = await syncModelsDev();
      if (r?.ok) {
        setMetaSyncMsg(`已更新：${r.providers} 个厂商 / ${r.models} 个模型`);
        const ps = await fetchProviders();
        if (ps) setMetaProviders(curateProviders(ps));
      } else {
        setMetaSyncMsg(
          r
            ? `更新失败：${r.error ?? "未知错误"}`
            : "更新失败：模型库服务不可用",
        );
      }
    } finally {
      setMetaSyncBusy(false);
    }
  };
  const [keyInput, setKeyInput] = useState("");
  const renderSaveButton = (cls: string) => (
    <div className={cls}>
      <button
        aria-label="保存配置"
        className={`btn-sm-action ${
          configBusy ? "nm-inset" : "nm-outset"
        }`}
        onClick={() => saveConfig()}
        disabled={configBusy}
      >
        {configSaved ? (
          <>
            <Check size={12} aria-hidden /> 已保存
          </>
        ) : configBusy ? (
          "保存中…"
        ) : (
          "保存配置"
        )}
      </button>
    </div>
  );
  const saveConfig = async (
    override?: typeof config | ((c: typeof config) => typeof config),
    opts?: { skipReload?: boolean },
  ): Promise<boolean> => {
    if (configBusy) return false;
    const c =
      typeof override === "function" ? override(config) : (override ?? config);
    setConfigBusy(true);
    setBotError("");
    try {
      await invoke("bot_set_config", {
        config: {
          // 新结构——每协议下的大模型列表 + active 模型 id
          // 后端落盘前会从 active 模型派生 base_url/model 回填老字段
          // （bot_model_loop 不感知新结构，沿用 base_url/model/api_provider 三个老字段）
          modelsByProvider: c.modelsByProvider,
          activeModelId: c.activeModelId,
          disabledVendors: c.disabledVendors,
          // 已验证厂商名单透传（后端按 key/URL 变化 prune；前端整体替换写必须带上，
          // 否则每次保存配置都把验证状态清成空）
          verifiedVendors: c.verifiedVendors,
          // 字体大小直接透传，后端原样存（None = small 默认）
          uiFontSize: c.uiFontSize,
          bypassLlmOnPreStepHit: c.bypassLlmOnPreStepHit,
          // textarea 一行一个路径；空行/空白剔除；全空 = 后端内置默认白名单
          allowedDirs: c.allowedDirs
            .split("\n")
            .map((s) => s.trim())
            .filter((s) => s.length > 0),
          // 触发词表全量落盘（删词即失效，所以不能省字段——空数组是
          // 「显式关掉这道触发器」的合法取值，必须能写上去）
          guardClaimKeywords: c.guardClaimKeywords
            .split("\n")
            .map((s) => s.trim())
            .filter((s) => s.length > 0),
          // 空串视为未配置（后端 Option 语义）——key 存系统凭据存储，
          // config 对象里的 key 字段固定传 null（后端强制置 None 双保险，不落明文）；
          // 新 key 走顶层 tavilyKey/braveKey 参数（见下方 invoke 调用）
          tavilyKey: null,
          // 开关显式落盘：开 = Tavily，关 = Bing+百度双引擎
          tavilyEnabled: c.tavilyEnabled,
          braveKey: null,
          // 开关显式落盘：开 = Brave，关 = 不走 Brave（与 Tavily 互斥，双开后端报错）
          braveEnabled: c.braveEnabled,
          // 空 = 用内置默认；非法输入按未配置处理（后端 resolve_timeout 硬钳到上限）
          pythonTimeoutSecs: paramNumber(
            "tools.python_timeout_secs",
            c.pythonTimeoutSecs,
          ),
          // 任务卡归档天数：空/非法 = null（后端回默认 7）；钳 1..=MAX 与后端一致
          archiveAfterDays: (() => {
            const n = parseInt(c.archiveAfterDays.trim(), 10);
            return Number.isFinite(n) && n > 0
              ? Math.min(MAX_ARCHIVE_DAYS, n)
              : null;
          })(),
          // 授权模式（strict/ask/yolo）
          permMode: c.permMode,
          // API 协议（openai/anthropic）
          apiProvider: c.apiProvider,
          // max_tokens：空 = 内置默认；非法输入按未配置处理（后端自钳）
          maxTokens: paramNumber("model.max_tokens", c.maxTokens),
          // 推理强度后台默认（RE-1）：四档抽象档位原样落盘
          reasoningEffort: c.reasoningEffort,
          // ──  Agent 运行参数：空/非法 = null（后端内置默认）；数值合法即落盘。
          //    范围由后端参数表给出（与运行时钳制同一个数），这里先钳一遍：
          //    一来落盘值 = 生效值，二来超 u32 的数字会让整个 bot_set_config
          //    反序列化失败——一处输错字连带整份设置都存不下去。
          maxRounds: paramNumber("loop.max_rounds", c.maxRounds),
          maxFunctionCalls: paramNumber(
            "loop.max_function_calls",
            c.maxFunctionCalls,
          ),
          historyBudgetChars: paramNumber(
            "loop.history_budget_chars",
            c.historyBudgetChars,
          ),
          historyTruncateTargetPct: paramNumber(
            "loop.history_truncate_target_pct",
            c.historyTruncateTargetPct,
          ),
          subagentMaxTurns: paramNumber(
            "subagent.max_turns",
            c.subagentMaxTurns,
          ),
          subagentMaxWallSecs: paramNumber(
            "subagent.max_wall_secs",
            c.subagentMaxWallSecs,
          ),
          searchMaxResults: paramNumber(
            "search.max_results",
            c.searchMaxResults,
          ),
          maxToolOutputBytes: paramNumber(
            "tools.max_output_bytes",
            c.maxToolOutputBytes,
          ),
          askBudget: paramNumber("tools.ask_budget", c.askBudget),
          // 规则表透传（空 tool 名/非法 action 由后端 sanitize 清洗）
          toolRules: c.toolRules.length > 0 ? c.toolRules : null,
          // 定时记忆整理：原样透传（后端 serde default 兜底缺字段）
          memoryConsolidation: c.memoryConsolidation,
          //  记忆可控开关：原样透传
          memoryControl: c.memoryControl,
          //  记忆参数：原样透传（手改 bot-config.json 的值不被整体替换写冲掉）
          memoryTuning: c.memoryTuning,
        },
        // 厂商页 key 输入框非空且当前在厂商页 → 按厂商名写入凭据存储；
        // 留空保持该厂商已存 key 不变。apiKey 是旧全局槽位参数，前端已弃用（固定 null）
        apiKey: null,
        vendorKey:
          keyInput.trim() && activeVendor
            ? { vendor: activeVendor, key: keyInput.trim() }
            : null,
        // Tavily/Brave key 同主 key 模式——非空才覆盖写入系统凭据存储，
        // null = 不动已存 key（开关切换走 toggleTavily/toggleBrave → saveConfig，
        // 此时输入框为空 → keyring 不受任何影响）
        tavilyKey: tavilyKeyInput.trim() ? tavilyKeyInput.trim() : null,
        braveKey: braveKeyInput.trim() ? braveKeyInput.trim() : null,
      });
      setKeyInput("");
      setTavilyKeyInput("");
      setBraveKeyInput("");
      // skipReload=true 用于点档即时落盘场景：末尾 loadConfig 会拿 mock/磁盘上的旧值覆盖刚点的字段
      if (!opts?.skipReload) {
        await loadConfig();
        // 生效值跟着变（写侧钳过的值、刚改的默认值来源）——不重拉的话卡片上
        // 还会显示保存前的「当前生效」
        await loadParamViews();
      }
      // 广播 bot-config-changed：聊天头部 🧠 模型标签同步刷新
      emit("bot-config-changed", null).catch(() => {});
      setConfigSaved(true);
      setTimeout(() => setConfigSaved(false), 1500);
      return true;
    } catch (e) {
      handleCommandError(e, "bot_set_config", { silent: true });
      setBotError(formatCommandError(e));
      return false;
    } finally {
      setConfigBusy(false);
    }
  };
  const [vendorTestBusy, setVendorTestBusy] = useState(false);
  const [vendorTestOk, setVendorTestOk] = useState<boolean | null>(null);
  const [vendorTestMsg, setVendorTestMsg] = useState("");
  const saveVendorPage = async () => {
    if (!(await saveConfig())) return;
    if (!activeVendor || !activeVendorEntries.some((m) => m.enabled !== false))
      return;
    setVendorTestBusy(true);
    setVendorTestOk(null);
    setVendorTestMsg("");
    try {
      const r = await invoke<{ ok: boolean; status?: number; error?: string }>(
        "bot_test_connection",
        {
          baseUrl: vendorBaseUrl,
          apiFormat: activeVendorProtocol,
          vendor: activeVendor,
        },
      );
      const ok = r?.ok === true;
      setVendorTestOk(ok);
      setVendorTestMsg(
        ok
          ? "连接测试通过 ✓"
          : `连接测试失败：${r?.error ?? (r?.status ? `HTTP ${r.status}` : "未知错误")}`,
      );
    } catch (e) {
      setVendorTestOk(false);
      setVendorTestMsg(`连接测试失败：${formatCommandError(e)}`);
    } finally {
      setVendorTestBusy(false);
    }
    await loadConfig();
    emit("bot-config-changed", null).catch(() => {});
  };
  const updateModel = (id: string, patch: Partial<ModelEntry>) => {
    setConfig((c) => {
      const prov = (["openai", "anthropic"] as const).find((p) =>
        (c.modelsByProvider[p] ?? []).some((m) => m.id === id),
      );
      if (!prov) return c;
      const list = (c.modelsByProvider[prov] ?? []).map((m) =>
        m.id === id ? { ...m, ...patch } : m,
      );
      return {
        ...c,
        modelsByProvider: {
          ...c.modelsByProvider,
          [prov]: list,
        },
      };
    });
  };
  const deleteModel = async (id: string) => {
    const prov = (["openai", "anthropic"] as const).find((p) =>
      (config.modelsByProvider[p] ?? []).some((m) => m.id === id),
    );
    if (!prov) return;
    if (!(await confirm("删除该模型？删除后立即生效。"))) return;
    const list = (config.modelsByProvider[prov] ?? []).filter(
      (m) => m.id !== id,
    );
    let nextActive = config.activeModelId[prov] ?? null;
    if (nextActive === id) {
      nextActive = list[0]?.id ?? null;
    }
    const next = {
      ...config,
      modelsByProvider: { ...config.modelsByProvider, [prov]: list },
      activeModelId: { ...config.activeModelId, [prov]: nextActive },
    };
    setConfig(next);
    // skipReload：本地 state 已是删除后最新（loadConfig 会拿盘上旧值把模型复活）
    await saveConfig(next, { skipReload: true });
  };
  const [activeVendor, setActiveVendor] = useState<string | null>(null);
  const [showKey, setShowKey] = useState(false);
  const vendorNameOf = (m: ModelEntry, prov: "openai" | "anthropic") =>
    m.vendor ?? (prov === "anthropic" ? "Anthropic 兼容" : "OpenAI 兼容");
  const vendorGroups = (() => {
    const map = new Map<string, { name: string; isActiveVendor: boolean }>();
    (["openai", "anthropic"] as const).forEach((prov) => {
      const list = config.modelsByProvider[prov] ?? [];
      const activeId = config.activeModelId[prov] ?? null;
      list.forEach((m) => {
        const name = vendorNameOf(m, prov);
        const g = map.get(name) ?? { name, isActiveVendor: false };
        if (m.id === activeId && config.apiProvider === prov)
          g.isActiveVendor = true;
        map.set(name, g);
      });
    });
    return [...map.values()].sort((a, b) => a.name.localeCompare(b.name));
  })();
  const needVendorReVerify =
    ((config.modelsByProvider.openai ?? []).some((m) => m.vendor) ||
      (config.modelsByProvider.anthropic ?? []).some((m) => m.vendor)) &&
    config.verifiedVendors.length === 0;
  const vendorOf = (name: string) =>
    vendorGroups.find((v) => v.name === name) ?? null;
  const protocolOfVendor = (name: string): "openai" | "anthropic" | null =>
    protocolOfVendorIn(config, name);
  const protocolOfVendorIn = (
    cfg: typeof config,
    name: string,
  ): "openai" | "anthropic" | null => {
    if (
      (cfg.modelsByProvider.openai ?? []).some(
        (m) => vendorNameOf(m, "openai") === name,
      )
    )
      return "openai";
    if (
      (cfg.modelsByProvider.anthropic ?? []).some(
        (m) => vendorNameOf(m, "anthropic") === name,
      )
    )
      return "anthropic";
    return null;
  };
  const activeVendorProtocol: "openai" | "anthropic" =
    activeVendor !== null && protocolOfVendor(activeVendor) === "anthropic"
      ? "anthropic"
      : "openai";
  const activeVendorEntries =
    activeVendor === null
      ? []
      : (config.modelsByProvider[activeVendorProtocol] ?? []).filter(
          (m) => vendorNameOf(m, activeVendorProtocol) === activeVendor,
        );
  const vendorBaseUrl = activeVendorEntries[0]?.baseUrl ?? "";
  const setVendorBaseUrl = (url: string) => {
    setConfig((c) => {
      const rewrite = (list: ModelEntry[], prov: "openai" | "anthropic") =>
        list.map((m) =>
          vendorNameOf(m, prov) === activeVendor ? { ...m, baseUrl: url } : m,
        );
      return {
        ...c,
        modelsByProvider: {
          openai: rewrite(c.modelsByProvider.openai ?? [], "openai"),
          anthropic: rewrite(c.modelsByProvider.anthropic ?? [], "anthropic"),
        },
      };
    });
  };
  const [vendorUrlMem, setVendorUrlMem] = useState<
    Record<string, Partial<Record<"openai" | "anthropic", string>>>
  >({});
  const seedVendorUrl = (
    vendor: string,
    prov: "openai" | "anthropic",
  ): string | null => {
    const norm = normalizeVendorName(vendor);
    const known = KNOWN_PROTOCOL_URLS[norm]?.[prov];
    if (known) return known;
    const preset = PROVIDER_PRESETS.find(
      (p) => normalizeVendorName(p.name) === norm,
    );
    if (preset?.protocol === prov) return preset.baseUrl;
    const meta = metaProviders?.find(
      (p) =>
        p.provider_key === norm ||
        normalizeVendorName(p.provider_name) === norm,
    );
    if (meta?.default_base_url) {
      const metaIsAnthropic =
        meta.provider_key === "anthropic" ||
        meta.default_base_url.includes("anthropic");
      if (metaIsAnthropic === (prov === "anthropic"))
        return meta.default_base_url;
    }
    return null;
  };
  const setVendorProtocol = (prov: "openai" | "anthropic") => {
    if (!activeVendor) return;
    const from = protocolOfVendor(activeVendor);
    if (from === null || from === prov) return;
    const vendor = activeVendor;
    const curUrl = vendorBaseUrl;
    if (curUrl) {
      setVendorUrlMem((m) => ({
        ...m,
        [vendor]: { ...m[vendor], [from]: curUrl },
      }));
    }
    const targetUrl =
      vendorUrlMem[vendor]?.[prov] || seedVendorUrl(vendor, prov);
    setConfig((c) => {
      const fromList = c.modelsByProvider[from] ?? [];
      const toList = c.modelsByProvider[prov] ?? [];
      const moving = fromList
        .filter((m) => vendorNameOf(m, from) === vendor)
        // 有目标协议的 URL 数据才改写；没有则保持用户现值
        .map((m) => (targetUrl ? { ...m, baseUrl: targetUrl } : m));
      const stay = fromList.filter((m) => vendorNameOf(m, from) !== vendor);
      const curActive = c.activeModelId[from];
      return {
        ...c,
        modelsByProvider: {
          openai: prov === "openai" ? [...toList, ...moving] : stay,
          anthropic: prov === "anthropic" ? [...toList, ...moving] : stay,
        },
        activeModelId: {
          openai: prov === "openai" ? curActive : null,
          anthropic: prov === "anthropic" ? curActive : null,
        },
      };
    });
  };
  const addModelToVendor = () => {
    setConfig((c) => {
      if (!activeVendor) return c;
      const from = protocolOfVendorIn(c, activeVendor) ?? "openai";
      const list = c.modelsByProvider[from] ?? [];
      const hadModels = list.some(
        (m) => vendorNameOf(m, from) === activeVendor,
      );
      const baseUrl =
        (list.find((m) => vendorNameOf(m, from) === activeVendor) ?? {})
          .baseUrl ?? "";
      const entry: ModelEntry = {
        id: genModelId(),
        label: "",
        baseUrl,
        model: "",
        vendor: activeVendor,
      };
      return {
        ...c,
        modelsByProvider: { ...c.modelsByProvider, [from]: [...list, entry] },
        activeModelId:
          hadModels || c.activeModelId[from]
            ? c.activeModelId
            : { ...c.activeModelId, [from]: entry.id },
      };
    });
  };
  const deleteVendor = async (name: string) => {
    const drop = (list: ModelEntry[], prov: "openai" | "anthropic") =>
      list.filter((m) => vendorNameOf(m, prov) !== name);
    const openai = drop(config.modelsByProvider.openai ?? [], "openai");
    const anthropic = drop(
      config.modelsByProvider.anthropic ?? [],
      "anthropic",
    );
    const fix = (prov: "openai" | "anthropic", list: ModelEntry[]) => {
      const cur = config.activeModelId[prov] ?? null;
      if (cur === null) return null;
      return list.some((m) => m.id === cur) ? cur : (list[0]?.id ?? null);
    };
    const next = {
      ...config,
      modelsByProvider: { openai, anthropic },
      disabledVendors: config.disabledVendors.filter((v) => v !== name),
      activeModelId: {
        openai: fix("openai", openai),
        anthropic: fix("anthropic", anthropic),
      },
    };
    setConfig(next);
    // skipReload：本地 state 已是删除后最新（loadConfig 会拿盘上旧值把厂商复活）
    await saveConfig(next, { skipReload: true });
    setActiveVendor((cur) => (cur === name ? null : cur));
  };
  const makeVendorActive = () => {
    if (!activeVendor) return;
    if (!config.verifiedVendors.includes(activeVendor)) return;
    setConfig((c) => {
      if (!activeVendor) return c;
      const from = protocolOfVendorIn(c, activeVendor);
      if (from === null) return c;
      const mine = (c.modelsByProvider[from] ?? []).filter(
        (m) => vendorNameOf(m, from) === activeVendor,
      );
      if (!mine.length) return c;
      const cur = c.activeModelId[from] ?? null;
      const target = mine.some((m) => m.id === cur) ? cur : mine[0].id;
      return {
        ...c,
        apiProvider: from,
        activeModelId: { ...c.activeModelId, [from]: target },
      };
    });
  };
  const vendorEnabled =
    activeVendor !== null && !config.disabledVendors.includes(activeVendor);
  const toggleVendorEnabled = async (next: boolean) => {
    if (!activeVendor || configBusy) return;
    const cur = config.disabledVendors;
    const disabledVendors = next
      ? cur.filter((v) => v !== activeVendor)
      : [...cur.filter((v) => v !== activeVendor), activeVendor];
    const nextCfg = { ...config, disabledVendors };
    setConfig(nextCfg);
    await saveConfig(nextCfg, { skipReload: true });
  };
  const activeVendorKeyUrl =
    PROVIDER_PRESETS.find((p) => p.name === activeVendor)?.keyUrl ?? null;
  const activeVendorHasKey =
    activeVendor !== null && config.vendorKeys.includes(activeVendor);
  useEffect(() => {
    // oxlint-disable-next-line react/set-state-in-effect
    setKeyInput("");
    // oxlint-disable-next-line react/set-state-in-effect
    setShowKey(false);
    // oxlint-disable-next-line react/set-state-in-effect
    setVendorTestOk(null);
    // oxlint-disable-next-line react/set-state-in-effect
    setVendorTestMsg("");
  }, [activeVendor]);
  const curateProviders = (ps: MetaProvider[]) =>
    ps.filter((p) => CURATED_PROVIDER_KEYS.has(p.provider_key));
  const modelMounted = mountedSections.has("model");
  useEffect(() => {
    if (!modelMounted) return;
    let cancelled = false;
    fetchProviders().then((ps) => {
      if (!cancelled) setMetaProviders(ps ? curateProviders(ps) : ps);
    });
    return () => {
      cancelled = true;
    };
  }, [modelMounted]);
  const vendorMetaOf = (name: string): MetaProvider | null => {
    if (!metaProviders) return null;
    const norm = normalizeVendorName(name);
    return (
      metaProviders.find(
        (p) => p.provider_name === name || p.provider_key === name,
      ) ??
      metaProviders.find(
        (p) => normalizeVendorName(p.provider_name) === norm,
      ) ??
      null
    );
  };
  const activeVendorMeta =
    activeVendor !== null ? vendorMetaOf(activeVendor) : null;
  const [metaPickerOpen, setMetaPickerOpen] = useState(false);
  const [metaPickerModels, setMetaPickerModels] = useState<MetaModel[] | null>(
    null,
  );
  const toggleMetaPicker = async () => {
    if (metaPickerOpen) {
      setMetaPickerOpen(false);
      return;
    }
    setMetaPickerOpen(true);
    if (activeVendorMeta) {
      setMetaPickerModels(
        await fetchModelsByProvider(activeVendorMeta.provider_key),
      );
    }
  };
  const addMetaModel = async (m: MetaModel) => {
    if (!activeVendor || !activeVendorMeta || configBusy) return;
    const proto = activeVendorProtocol;
    const entry: ModelEntry = {
      ...metaModelToEntry(
        m,
        activeVendor,
        vendorBaseUrl || activeVendorMeta.default_base_url || "",
      ),
      enabled: false,
    };
    const list = config.modelsByProvider[proto] ?? [];
    const next = {
      ...config,
      modelsByProvider: {
        ...config.modelsByProvider,
        [proto]: [...list, entry],
      },
    };
    setConfig(next);
    setMetaPickerOpen(false);
    await saveConfig(next, { skipReload: true });
  };

  return {
    activeVendor,
    activeVendorEntries,
    activeVendorHasKey,
    activeVendorKeyUrl,
    activeVendorMeta,
    activeVendorProtocol,
    addMetaModel,
    addModelToVendor,
    applyMetaProvider,
    applyProviderPreset,
    deleteModel,
    deleteVendor,
    keyInput,
    makeVendorActive,
    metaApplyBusy,
    metaPickerModels,
    metaPickerOpen,
    metaProviders,
    metaSearch,
    metaSyncBusy,
    metaSyncMsg,
    mountedSections,
    needVendorReVerify,
    openSection,
    renderSaveButton,
    saveConfig,
    saveVendorPage,
    setActiveVendor,
    setKeyInput,
    setMetaPickerOpen,
    setMetaSearch,
    setShowKey,
    setShowProviderPicker,
    setVendorBaseUrl,
    setVendorProtocol,
    showKey,
    showProviderPicker,
    syncMetaLibrary,
    toggleMetaPicker,
    toggleVendorEnabled,
    updateModel,
    vendorBaseUrl,
    vendorEnabled,
    vendorGroups,
    vendorMetaOf,
    vendorOf,
    vendorTestBusy,
    vendorTestMsg,
    vendorTestOk,
  };
}
