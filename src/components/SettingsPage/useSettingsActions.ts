/** 设置页的「actions」逻辑块（从 SettingsPage 主组件搬出）。
 *
 *  机器人 / Python / 本地服务 / 界面开关这一摊：状态、派生值与
 *  读配置的调用。挂载期多面板刷新那个 effect 也在里面——它调的函数
 *  捕获的 state 都在这个 hook 里，deps 保持空数组不变。
 *
 *  搬移是逐字的：语句原样保留、顺序不变；对外接口只有 params 与返回值。
 */
import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { emit } from "@tauri-apps/api/event";
import { handleCommandError, formatCommandError } from "../../lib/errorHandler";
import { getBotConfig } from "../../lib/botConfig";
import { DEFAULT_ARCHIVE_DAYS } from "../../lib/archiveRule";
import type {
  ModelsByProvider,
  ActiveModelId,
  ParamView,
  EnvCheck,
  TaskMode,
} from "./types";
import { ApiProvider, UiFontSize, AutoExtractMode, isAutoExtractMode, U32_MAX } from "./constants";
import type { ConfirmOptions } from "../../lib/useConfirm";

export type SettingsActionsParams = {
  confirm: (messageOrOptions: string | ConfirmOptions) => Promise<boolean>;
};

export function useSettingsActions(params: SettingsActionsParams) {
  const { confirm } = params;

  const [botEnabled, setBotEnabled] = useState(false);
  const [botBusy, setBotBusy] = useState(false);
  const [botError, setBotError] = useState("");
  const [autostartEnabled, setAutostartEnabled] = useState(false);
  const [autostartBusy, setAutostartBusy] = useState(false);
  const [autostartError, setAutostartError] = useState("");
  const [config, setConfig] = useState({
    // API 协议：openai=OpenAI 兼容（默认）/ anthropic=Anthropic 兼容
    apiProvider: "openai" as ApiProvider,
    // 每协议下的大模型列表：
    // 双协议各自独立一份 ModelEntry 列表，切换协议时整体切换显示；
    // 初始空 = 老板要求「不设置默认厂商」，用户点「添加大模型」自己加
    modelsByProvider: { openai: [], anthropic: [] } as ModelsByProvider,
    // 每协议当前选中的模型 id：null = 该协议还没选 active
    activeModelId: { openai: null, anthropic: null } as ActiveModelId,
    // 被禁用的厂商名列表：禁用后该厂商模型行变淡、开关禁用，聊天选模型入口过滤
    disabledVendors: [] as string[],
    // 连接测试通过的厂商名列表（可用性依据：左栏绿点 + 聊天下拉过滤；
    // key/URL/格式变更由后端置失效）
    verifiedVendors: [] as string[],
    // 界面字体大小：small/standard/large 三档
    // 默认 small（「目前字号为小」）；后端 None 也回退到 small
    uiFontSize: "small" as UiFontSize,
    hasApiKey: false,
    // 已存 API Key 的厂商名列表（后端按配置里的厂商名逐个探测 keyring）：
    // 厂商页按它显示「已存入 ✓」，key 按厂商名分条目存储
    vendorKeys: [] as string[],
    bypassLlmOnPreStepHit: true, // F-1 [P0] pre-step 路由外层是否跳过主 LLM；老配置默认 true
    // 本地文件工具白名单目录（textarea 一行一个；空 = 后端内置默认 桌面/下载/文档+绑定文件夹）
    allowedDirs: "",
    // 幻觉守卫「执行类谎报」触发词表（textarea 一行一个；GuardKeywordsPanel 编辑）
    guardClaimKeywords: "",
    // 内置默认词表：只读参照 + 「恢复默认」取值来源（后端 view 带下来）
    guardClaimKeywordsDefault: [] as string[],
    // Tavily 搜索 key 是否已存系统凭据存储（key 本体不回填，
    // 与主 API key 同模式：view 只给 has 标志，输入框独立 state 不回填）
    hasTavilyKey: false,
    // 「Tavily 搜索」开关：开 = web_search 走 Tavily；关 = Bing+百度双引擎
    tavilyEnabled: false,
    // Brave 搜索 key 是否已存系统凭据存储（同 hasTavilyKey）
    hasBraveKey: false,
    // 「Brave 搜索」开关：开 = web_search 走 Brave；与 Tavily 互斥，双开报错
    braveEnabled: false,
    // run_python 默认超时秒数（空 = 60s 默认；模型 timeoutSecs 参数优先；硬钳上限见后端参数表）
    pythonTimeoutSecs: "",
    // Function 调用熔断上限（空 = 100 默认； 全域，含工作流节点执行）
    maxFunctionCalls: "",
    // 任务卡归档天数（数据管理卡）：完成满 N 天自动归档；后端 migration 兜底归档同源
    // 显示为字符串输入；"7" = 默认（后端 None 亦回 7）
    archiveAfterDays: String(DEFAULT_ARCHIVE_DAYS),
    // 授权模式：strict=白名单外硬拒 / ask=白名单外弹授权（默认）/ yolo=全放行
    permMode: "ask" as "strict" | "ask" | "auto" | "yolo",
    //  per-tool 权限规则表（deny/ask/allow 首中即停；空表 = 无规则）
    toolRules: [] as Array<{ tool: string; action: "allow" | "ask" | "deny" }>,
    // max_tokens 兜底层（仅 Anthropic 模式发送）：无设置页 UI 入口（每模型编辑里
    // 都有 max_tokens，页底重复已删）；loadConfig 读到已存值原样透传保存，不丢
    maxTokens: "",
    // 推理强度后台默认（RE-1）：off/low/medium/high，默认 medium；
    // 抽象档位——发送时后端按具体模型族映射到 reasoning_effort / thinking.budget_tokens
    reasoningEffort: "medium" as "off" | "low" | "medium" | "high",
    // 定时记忆整理：开关 + 频率（off/12h/daily/weekly）+ 上次整理时间
    // 后端 None/缺字段 → 默认 { enabled: true, interval: "daily", lastRunAt: null }
    memoryConsolidation: {
      enabled: true,
      interval: "daily",
      lastRunAt: null as number | null,
    },
    //  记忆可控开关：注入总闸 + 模型主动记忆门禁（后端缺字段 = 全开）；
    //  自动记忆抽取档位（后端缺字段/非法值 = off）
    memoryControl: {
      injectionEnabled: true,
      autoWriteEnabled: true,
      autoExtract: "off" as AutoExtractMode,
    },
    //  记忆参数（手改 bot-config.json 生效，本页无 UI）：
    // 原样回传保存，防止设置页整体替换写把手改值冲掉
    memoryTuning: null as Record<string, number> | null,
    // ──  Agent 运行参数（空串 = 后端内置默认；显示与保存都是字符串输入） ──
    taskMode: "balanced" as TaskMode,
    maxRounds: "",
    historyBudgetChars: "",
    historyTruncateTargetPct: "",
    reasoningEchoEnabled: "",
    inTurnContextEdit: "",
    searchMaxResults: "",
    maxToolOutputBytes: "",
    askBudget: "",
  });
  const [traceRetainDays, setTraceRetainDays] = useState("");
  const [traceClearBusy, setTraceClearBusy] = useState(false);
  const [traceClearMsg, setTraceClearMsg] = useState("");
  const clearTraces = async () => {
    if (traceClearBusy) return;
    setTraceClearBusy(true);
    setTraceClearMsg("");
    try {
      const days = parseInt(traceRetainDays, 10);
      const removed = await invoke<number>("trace_clear_before", {
        days: Number.isFinite(days) && days > 0 ? days : null,
      });
      setTraceClearMsg(
        removed > 0 ? `已清理 ${removed} 条执行痕迹` : "没有需要清理的痕迹",
      );
    } catch (e) {
      handleCommandError(e, "清理执行痕迹", { silent: true });
      setTraceClearMsg(formatCommandError(e));
    } finally {
      setTraceClearBusy(false);
    }
  };
  const [deleteNonSelfBusy, setDeleteNonSelfBusy] = useState(false);
  const [deleteNonSelfDone, setDeleteNonSelfDone] = useState<string | null>(
    null,
  );
  const deleteNonSelfTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(
    () => () => {
      if (deleteNonSelfTimer.current) clearTimeout(deleteNonSelfTimer.current);
    },
    [],
  );
  const runDeleteNonSelf = async () => {
    if (deleteNonSelfBusy) return;
    setDeleteNonSelfBusy(true);
    try {
      const n = await invoke<number>("tasks_delete_non_self");
      setDeleteNonSelfDone(n > 0 ? `已删 ${n} 张` : "无此类卡");
      if (deleteNonSelfTimer.current) clearTimeout(deleteNonSelfTimer.current);
      deleteNonSelfTimer.current = setTimeout(
        () => setDeleteNonSelfDone(null),
        2500,
      );
    } catch (e) {
      handleCommandError(e, "删除非本人任务卡");
      setDeleteNonSelfBusy(false);
      return;
    }
    setDeleteNonSelfBusy(false);
  };
  const [tavilyKeyInput, setTavilyKeyInput] = useState("");
  const [braveKeyInput, setBraveKeyInput] = useState("");
  const [configBusy, setConfigBusy] = useState(false);
  const [configSaved, setConfigSaved] = useState(false);
  const [pyEnabled, setPyEnabled] = useState(false);
  const [pyBusy, setPyBusy] = useState(false);
  const [servicesEnabled, setServicesEnabled] = useState(false);
  const [servicesBusy, setServicesBusy] = useState(false);
  const [exposeNetAllowed, setExposeNetAllowed] = useState(false);
  const [exposeNetBusy, setExposeNetBusy] = useState(false);
  const [uiEnabled, setUiEnabled] = useState(false);
  const [uiBusy, setUiBusy] = useState(false);
  const [env, setEnv] = useState<EnvCheck | null>(null);
  const [envBusy, setEnvBusy] = useState(false);
  const [envErr, setEnvErr] = useState("");
  const refreshBot = async () => {
    try {
      const on = await invoke<boolean>("bot_get_enabled");
      setBotEnabled(on);
    } catch (e) {
      // 自动加载失败：只记 console，不打扰
      handleCommandError(e, "bot_get_enabled", { silent: true });
    }
  };
  const [paramViews, setParamViews] = useState<ParamView[] | null>(null);
  const traceRetentionDays = paramViews?.find(
    (p) => p.key === "trace.retention_days",
  )?.value;
  const pyTimeoutView = paramViews?.find(
    (p) => p.key === "tools.python_timeout_secs",
  );
  const paramMaxText = (key: string) => {
    const max = paramViews?.find((p) => p.key === key)?.max;
    return max != null ? String(max) : "";
  };
  const paramRangeText = (key: string) => {
    const row = paramViews?.find((p) => p.key === key);
    return row?.min != null && row?.max != null ? `${row.min}–${row.max}` : null;
  };
  const fmtDurationSecs = (secs: number) =>
    secs % 60 === 0 ? `${secs / 60} 分钟` : `${secs} 秒`;
  const extractIntervalText = (() => {
    const raw = paramViews?.find(
      (p) => p.key === "memory.extract_min_interval_secs",
    )?.value;
    const secs = Number(raw);
    return raw != null && Number.isFinite(secs) && secs > 0
      ? fmtDurationSecs(secs)
      : null;
  })();
  const pyTimeoutText = (() => {
    const raw = pyTimeoutView?.value;
    const secs = Number(raw);
    return raw != null && Number.isFinite(secs) && secs > 0
      ? fmtDurationSecs(secs)
      : null;
  })();
  const pySandboxDesc = `沙箱：独立临时目录 + ${
    pyTimeoutText ? `${pyTimeoutText} 超时` : "限时执行"
  } + 审计留痕`;
  const traceRetentionTitle = (() => {
    const range = paramRangeText("trace.retention_days");
    const tail = traceRetentionDays
      ? `默认 ${traceRetentionDays}`
      : "留空 = 用内置默认";
    return range ? `保留天数（${range}，${tail}）` : `保留天数（${tail}）`;
  })();
  const loadParamViews = async () => {
    try {
      const views = await invoke<ParamView[]>("bot_effective_params");
      setParamViews(Array.isArray(views) ? views : null);
    } catch (e) {
      // 拉不到就让卡片退回本地兜底：参数卡不能因为一个展示用的命令挂了就整块
      // 消失（设置页其余部分照常用）
      handleCommandError(e, "bot_effective_params", { silent: true });
      setParamViews(null);
    }
  };
  const paramNumber = (key: string, raw: string): number | null => {
    const t = raw.trim();
    if (!t) return null;
    const n = parseInt(t, 10);
    if (!Number.isFinite(n) || n < 0) return null;
    const view = paramViews?.find((p) => p.key === key);
    const lo = view?.min ?? 1;
    const hi = view?.max ?? U32_MAX;
    if (n === 0) return lo === 0 ? 0 : null;
    if (n < lo) return lo;
    if (n > hi) return hi;
    return n;
  };
  const loadConfig = async () => {
    try {
      const c = await getBotConfig();
      setConfig({
        // 老配置缺字段/非法值 → openai（与后端 ApiProvider::from_cfg 回退一致）
        apiProvider: c.apiProvider === "anthropic" ? "anthropic" : "openai",
        // 老后端版本没返 modelsByProvider → 空列表（无默认厂商）
        modelsByProvider: {
          openai: c.modelsByProvider?.openai ?? [],
          anthropic: c.modelsByProvider?.anthropic ?? [],
        },
        activeModelId: {
          openai: c.activeModelId?.openai ?? null,
          anthropic: c.activeModelId?.anthropic ?? null,
        },
        disabledVendors: c.disabledVendors ?? [],
        // 连接测试通过的厂商名单（老后端没返 → 空）
        verifiedVendors: c.verifiedVendors ?? [],
        // 界面字体大小：small/standard/large 三档
        // 默认 small（「目前字号为小」）；后端 None / 非法值（含历史遗留的
        // xlarge）都回退到 small
        uiFontSize:
          c.uiFontSize === "standard" || c.uiFontSize === "large"
            ? c.uiFontSize
            : "small",
        hasApiKey: !!c.hasApiKey,
        // 已存 key 的厂商名列表（老后端没返 → 空）
        vendorKeys: c.vendorKeys ?? [],
        // 老后端版本（没返 bypass 字段）默认 true，避免意外走 LEGACY 路径
        bypassLlmOnPreStepHit: c.bypassLlmOnPreStepHit ?? true,
        allowedDirs: (c.allowedDirs ?? []).join("\n"),
        guardClaimKeywords: (c.guardClaimKeywords ?? []).join("\n"),
        guardClaimKeywordsDefault: c.guardClaimDefaultKeywords ?? [],
        // view 只给 has 标志；key 本体不回填（与 hasApiKey/keyInput 同模式）。
        // 开关自动态：老配置没显式开关字段（null/undefined）时按 has 标志显示
        hasTavilyKey: c.hasTavilyKey ?? false,
        tavilyEnabled: c.tavilyEnabled ?? c.hasTavilyKey ?? false,
        hasBraveKey: c.hasBraveKey ?? false,
        braveEnabled: c.braveEnabled ?? c.hasBraveKey ?? false,
        pythonTimeoutSecs:
          c.pythonTimeoutSecs != null ? String(c.pythonTimeoutSecs) : "",
        maxFunctionCalls:
          c.maxFunctionCalls != null ? String(c.maxFunctionCalls) : "",
        // 归档天数：后端 None / 老版本没返 → 显示默认 7
        archiveAfterDays:
          c.archiveAfterDays != null
            ? String(c.archiveAfterDays)
            : String(DEFAULT_ARCHIVE_DAYS),
        // 老配置缺字段/非法值 → ask（与后端 PermMode::from_cfg 回退一致）
        permMode:
          c.permMode === "strict" ||
          c.permMode === "yolo" ||
          c.permMode === "auto"
            ? c.permMode
            : "ask",
        // 空 = 内置默认（仅 Anthropic 模式用）
        maxTokens: c.maxTokens != null ? String(c.maxTokens) : "",
        // 推理强度：缺字段/非法值 → medium（与后端 EffortLevel::from_cfg 回退一致）
        reasoningEffort:
          c.reasoningEffort === "off" ||
          c.reasoningEffort === "low" ||
          c.reasoningEffort === "high"
            ? c.reasoningEffort
            : "medium",
        // 记忆整理：缺字段/老后端 → 默认（启用 + daily）；非法 interval 回退 daily
        memoryConsolidation: {
          enabled: c.memoryConsolidation?.enabled ?? true,
          interval: ["off", "12h", "daily", "weekly"].includes(
            c.memoryConsolidation?.interval ?? "",
          )
            ? (c.memoryConsolidation?.interval as string)
            : "daily",
          lastRunAt: c.memoryConsolidation?.lastRunAt ?? null,
        },
        //  记忆可控开关：缺字段/老后端 → 全开； 抽取档位缺字段/非法 → off
        memoryControl: {
          injectionEnabled: c.memoryControl?.injectionEnabled ?? true,
          autoWriteEnabled: c.memoryControl?.autoWriteEnabled ?? true,
          autoExtract: isAutoExtractMode(c.memoryControl?.autoExtract)
            ? c.memoryControl.autoExtract
            : "off",
        },
        //  记忆参数原样回传
        memoryTuning: c.memoryTuning ?? null,
        // 规则表透传（老后端没返 → 空；非法 action 由后端清洗兜底）
        toolRules: Array.isArray(c.toolRules)
          ? c.toolRules.filter(
              (r): r is { tool: string; action: "allow" | "ask" | "deny" } =>
                typeof r?.tool === "string" &&
                (r.action === "allow" ||
                  r.action === "ask" ||
                  r.action === "deny"),
            )
          : [],
        //  Agent 运行参数：后端 None/老版本没返 → 空串（= 内置默认）
        maxRounds: c.maxRounds != null ? String(c.maxRounds) : "",
        historyBudgetChars:
          c.historyBudgetChars != null ? String(c.historyBudgetChars) : "",
        historyTruncateTargetPct:
          c.historyTruncateTargetPct != null
            ? String(c.historyTruncateTargetPct)
            : "",
        reasoningEchoEnabled:
          c.reasoningEchoEnabled != null
            ? String(c.reasoningEchoEnabled)
            : "",
        inTurnContextEdit:
          c.inTurnContextEdit != null ? String(c.inTurnContextEdit) : "",
        taskMode:
          c.taskMode === "light" || c.taskMode === "deep"
            ? c.taskMode
            : "balanced",
        searchMaxResults:
          c.searchMaxResults != null ? String(c.searchMaxResults) : "",
        maxToolOutputBytes:
          c.maxToolOutputBytes != null ? String(c.maxToolOutputBytes) : "",
        askBudget: c.askBudget != null ? String(c.askBudget) : "",
      });
    } catch (e) {
      handleCommandError(e, "bot_get_config", { silent: true });
    }
  };
  const refreshPy = async () => {
    try {
      const on = await invoke<boolean>("py_get_enabled");
      setPyEnabled(on);
    } catch (e) {
      handleCommandError(e, "py_get_enabled", { silent: true });
    }
  };
  const refreshServices = async () => {
    try {
      const on = await invoke<boolean>("service_get_enabled");
      setServicesEnabled(on);
    } catch (e) {
      handleCommandError(e, "service_get_enabled", { silent: true });
    }
    try {
      const on = await invoke<boolean>("service_get_expose_net_allowed");
      setExposeNetAllowed(on);
    } catch (e) {
      handleCommandError(e, "service_get_expose_net_allowed", { silent: true });
    }
  };
  const checkEnv = async () => {
    if (envBusy) return;
    setEnvBusy(true);
    setEnvErr("");
    try {
      const result = await invoke<EnvCheck>("env_check");
      setEnv(result);
    } catch (e) {
      handleCommandError(e, "env_check", { silent: true });
      setEnvErr(formatCommandError(e));
    } finally {
      setEnvBusy(false);
    }
  };
  useEffect(() => {
    // 挂载期多面板初始刷新（外部数据同步既有模式）：被调函数内部链路含 setState，
    // lint 对 effect 内调用一律按同步路径处理，保既有直写
    // oxlint-disable-next-line react/set-state-in-effect
    refreshBot();
    // oxlint-disable-next-line react/set-state-in-effect
    loadConfig();
    // Agent 运行参数卡的元数据（生效值/默认值/范围）——与 loadConfig 同步拉，
    // 两者都来自后端同一份参数表
    // oxlint-disable-next-line react/set-state-in-effect
    loadParamViews();
    // oxlint-disable-next-line react/set-state-in-effect
    refreshPy();
    // oxlint-disable-next-line react/set-state-in-effect
    refreshServices();
    // oxlint-disable-next-line react/set-state-in-effect
    refreshUi();
    // 拉取开机自启动状态
    invoke<boolean>("plugin:autostart|is_enabled")
      .then(setAutostartEnabled)
      .catch((e) => {
        handleCommandError(e, "plugin:autostart|is_enabled", { silent: true });
        setAutostartError(formatCommandError(e));
      });
  }, []);
  const togglePy = async () => {
    if (pyBusy) return;
    const enabling = !pyEnabled;
    if (
      enabling &&
      !(await confirm(
        `开启后机器人可执行 Python 代码（${pySandboxDesc}）。确认开启？`,
      ))
    )
      return;
    setPyBusy(true);
    setBotError("");
    try {
      const next = await invoke<boolean>("py_set_enabled", {
        enabled: enabling,
      });
      setPyEnabled(next);
      if (next) checkEnv();
    } catch (e) {
      handleCommandError(e, "py_set_enabled", { silent: true });
      setBotError(formatCommandError(e));
      await refreshPy();
    } finally {
      setPyBusy(false);
    }
  };
  const toggleExposeNet = async () => {
    if (exposeNetBusy) return;
    const enabling = !exposeNetAllowed;
    if (
      enabling &&
      !(await confirm(
        "开启后机器人起的本地服务可以监听 0.0.0.0 / :: 等非 loopback 地址，" +
          "也就是同局域网（乃至公网可达）的机器能连上它。" +
          "服务代码归机器人写、可能来自不可信来源，确认开启？",
      ))
    )
      return;
    setExposeNetBusy(true);
    setBotError("");
    try {
      const next = await invoke<boolean>("service_set_expose_net_allowed", {
        allowed: enabling,
      });
      setExposeNetAllowed(next);
    } catch (e) {
      handleCommandError(e, "service_set_expose_net_allowed", { silent: true });
      setBotError(formatCommandError(e));
      await refreshServices();
    } finally {
      setExposeNetBusy(false);
    }
  };
  const toggleServices = async () => {
    if (servicesBusy) return;
    const enabling = !servicesEnabled;
    if (
      enabling &&
      !(await confirm(
        "开启后机器人可在本机启动长驻进程（node MCP server、dev server 等）。" +
          "被启动的代码会继承宿主的环境变量，能读到其中的密钥；进程归机器人托管，" +
          "但用户自己写的代码也可能读到。确认开启？",
      ))
    )
      return;
    setServicesBusy(true);
    setBotError("");
    try {
      const next = await invoke<boolean>("service_set_enabled", {
        enabled: enabling,
      });
      setServicesEnabled(next);
    } catch (e) {
      handleCommandError(e, "service_set_enabled", { silent: true });
      setBotError(formatCommandError(e));
      await refreshServices();
    } finally {
      setServicesBusy(false);
    }
  };
  const refreshUi = async () => {
    try {
      const on = await invoke<boolean>("ui_get_enabled");
      setUiEnabled(on);
    } catch (e) {
      handleCommandError(e, "ui_get_enabled", { silent: true });
    }
  };
  const toggleUi = async () => {
    if (uiBusy) return;
    const enabling = !uiEnabled;
    if (
      enabling &&
      !(await confirm(
        "开启后机器人可以操作你电脑上的界面（读窗口控件、点击、输入文字）。涉及发送/提交/支付/删除的点击仍会弹窗请用户确认。确认开启？",
      ))
    )
      return;
    setUiBusy(true);
    setBotError("");
    try {
      const next = await invoke<boolean>("ui_set_enabled", {
        enabled: enabling,
      });
      setUiEnabled(next);
    } catch (e) {
      handleCommandError(e, "ui_set_enabled", { silent: true });
      setBotError(formatCommandError(e));
      await refreshUi();
    } finally {
      setUiBusy(false);
    }
  };
  const toggleBot = async () => {
    if (botBusy) return;
    setBotBusy(true);
    setBotError("");
    try {
      const next = await invoke<boolean>("bot_set_enabled", {
        enabled: !botEnabled,
      });
      setBotEnabled(next);
      // 广播 bot-changed：各页（聊天区高度随开关调整）同步刷新
      emit("bot-changed", next).catch(() => {});
    } catch (e) {
      handleCommandError(e, "bot_set_enabled", { silent: true });
      setBotError(formatCommandError(e));
      await refreshBot();
    } finally {
      setBotBusy(false);
    }
  };

  return {
    autostartBusy,
    autostartEnabled,
    autostartError,
    botBusy,
    botEnabled,
    botError,
    braveKeyInput,
    checkEnv,
    clearTraces,
    config,
    configBusy,
    configSaved,
    deleteNonSelfBusy,
    deleteNonSelfDone,
    exposeNetAllowed,
    exposeNetBusy,
    extractIntervalText,
    loadConfig,
    loadParamViews,
    paramMaxText,
    paramNumber,
    paramViews,
    pyBusy,
    pyEnabled,
    env,
    envBusy,
    envErr,
    pyTimeoutView,
    runDeleteNonSelf,
    servicesBusy,
    servicesEnabled,
    setAutostartBusy,
    setAutostartEnabled,
    setAutostartError,
    setBotError,
    setBraveKeyInput,
    setConfig,
    setConfigBusy,
    setConfigSaved,
    setTavilyKeyInput,
    setTraceRetainDays,
    tavilyKeyInput,
    toggleBot,
    toggleExposeNet,
    togglePy,
    toggleServices,
    toggleUi,
    traceClearBusy,
    traceClearMsg,
    traceRetainDays,
    traceRetentionDays,
    traceRetentionTitle,
    uiBusy,
    uiEnabled,
  };
}
