// SettingsPage 主组件（orchestrator）。
// 1846 → 1340 行：types / constants / SkillsPanel / ApiProviderSelect / ProfileRow / ModelRow
// 拆到同目录子文件，本文件保留所有 useState / handlers / JSX render。
//
// 公开 import 路径保持稳定：外部仍 `import { SettingsPage } from "./components/SettingsPage"`，
// Vite 解析到 `./SettingsPage/index.tsx` → 透传 `./SettingsPage`。

import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { emit } from "@tauri-apps/api/event";
import {
  ArrowLeft,
  BarChart3,
  Bot,
  Brain,
  Cpu,
  DatabaseBackup,
  FolderOutput,
  GitBranch,
  Plug,
  Plus,
  Settings2,
  Sparkles,
  Trash2,
  type LucideIcon,
} from "lucide-react";

import { handleCommandError, formatCommandError } from "../../lib/errorHandler";
import { EmptyState } from "../EmptyState";
import type { ThemeSetting } from "../../theme";
import { MigrationPanel } from "../MigrationPanel";

import {
  type ApiStatus,
  type ModelsByProvider,
  type ActiveModelId,
  type ModelEntry,
  genModelId,
} from "./types";
import { UI_FONT_SIZE_OPTIONS, type ApiProvider, type UiFontSize } from "./constants";
import { SkillsPanel } from "./SkillsPanel";
import { McpPanel } from "./McpPanel";
import { EvolutionPanel } from "../EvolutionPanel";
import { ProfileRow } from "./ProfileRow";
import { ModelRow } from "./ModelRow";

/** 供应商预设（U9 添加供应商网格）：点击 = 切协议 + 预填 baseUrl/模型列表，
 *  完全落在现有双协议数据模型内（同一协议仅一份配置，预设会覆盖该协议当前
 *  Base URL/模型列表——网格页注明）。模型名用户可改。 */
type ProviderPreset = {
  name: string;
  icon: string;
  protocol: "openai" | "anthropic";
  baseUrl: string;
  models: string[];
};
const PROVIDER_PRESETS: ProviderPreset[] = [
  { name: "DeepSeek", icon: "🐳", protocol: "openai", baseUrl: "https://api.deepseek.com", models: ["deepseek-chat", "deepseek-reasoner"] },
  { name: "Kimi", icon: "🌙", protocol: "openai", baseUrl: "https://api.moonshot.cn/v1", models: ["kimi-k2-0711-preview", "moonshot-v1-8k"] },
  { name: "MiniMax", icon: "🔴", protocol: "anthropic", baseUrl: "https://api.minimaxi.com/anthropic", models: ["MiniMax-M3"] },
  { name: "OpenRouter", icon: "🛰️", protocol: "openai", baseUrl: "https://openrouter.ai/api/v1", models: ["openrouter/auto"] },
  { name: "阿里云百炼", icon: "☁️", protocol: "openai", baseUrl: "https://dashscope.aliyuncs.com/compatible-mode/v1", models: ["qwen-plus", "qwen-max"] },
  { name: "OpenAI", icon: "🟢", protocol: "openai", baseUrl: "https://api.openai.com/v1", models: ["gpt-4o", "gpt-4o-mini"] },
  { name: "Anthropic", icon: "🅰️", protocol: "anthropic", baseUrl: "https://api.anthropic.com", models: ["claude-sonnet-4-20250514"] },
  { name: "xAI", icon: "⚡", protocol: "openai", baseUrl: "https://api.x.ai/v1", models: ["grok-4"] },
];

type Props = {
  theme: ThemeSetting;
  onThemeChange: (t: ThemeSetting) => void;
  onExportTasks: () => Promise<void>;
  onImportTasks: () => Promise<void>;
  /** 可选：未传时按钮置 disabled（App.tsx 已传；测试可选） */
  onExportWorkspace?: () => Promise<void>;
  onImportWorkspace?: () => Promise<void>;
  /** 返回入口（U7 设置壳）：未传时按钮 disabled（App 恒传；测试可省） */
  onBack?: () => void;
};

/** 设置分类（U8 十项，老板拍板）：key 顺序 = 侧栏顺序。
 *  「机器人」section 在源码中三段出现（同名同亮）：卡1 开关/Python/日志、
 *  卡2 授权/白名单、卡3 技能路由/外部 API；「MCP 服务」两段：Tavily/Brave
 *  搜索引擎卡 + McpPanel。面板惰性挂载同 U7。 */
type SectionKey =
  | "general"
  | "data"
  | "bot"
  | "model"
  | "memory"
  | "skills"
  | "mcp"
  | "evolution"
  | "desk"
  | "tokens";
const SECTIONS: { key: SectionKey; label: string; icon: LucideIcon }[] = [
  { key: "general", label: "通用设置", icon: Settings2 },
  { key: "data", label: "数据管理", icon: DatabaseBackup },
  { key: "bot", label: "机器人", icon: Bot },
  { key: "model", label: "模型设置", icon: Cpu },
  { key: "memory", label: "记忆", icon: Brain },
  { key: "skills", label: "技能", icon: Sparkles },
  { key: "mcp", label: "MCP 服务", icon: Plug },
  { key: "evolution", label: "自进化", icon: GitBranch },
  { key: "desk", label: "桌面整理", icon: FolderOutput },
  { key: "tokens", label: "词元统计", icon: BarChart3 },
];

/** 设置壳（U7 重设计）：左侧分类导航 + 右侧分类内容（大标题 + 卡片流）。
 *  面板惰性挂载（ocr HIGH）：分类首次激活才挂载，挂后保留（hidden 切换）——
 *  首屏不跑未访问面板的加载 invoke，未保存输入跨分类保留。 */
export function SettingsPage({ theme, onThemeChange, onExportTasks, onImportTasks, onExportWorkspace, onImportWorkspace, onBack }: Props) {
  const [activeSection, setActiveSection] = useState<SectionKey>("general");
  const [mountedSections, setMountedSections] = useState<ReadonlySet<SectionKey>>(
    new Set(["general"])
  );
  const openSection = (key: SectionKey) => {
    setActiveSection(key);
    setMountedSections((prev) =>
      prev.has(key) ? prev : new Set(prev).add(key)
    );
  };
  const [status, setStatus] = useState<ApiStatus>({
    enabled: false,
    port: 4763,
    token: "",
  });
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [copied, setCopied] = useState(false);
  /** 添加供应商网格（U9）：model 分类右栏的子视图 */
  const [showProviderPicker, setShowProviderPicker] = useState(false);
  /** 从厂商网站获取模型列表（U11）：busy + 结果文案 */
  const [fetchModelsBusy, setFetchModelsBusy] = useState(false);
  const [fetchModelsMsg, setFetchModelsMsg] = useState("");
  /** 应用供应商预设（U9）：切协议 + 覆盖该协议模型列表（预填 baseUrl/模型名）+
   *  首个设为 active——完全落在现有双协议数据模型内，预设可整表替换。
   *  覆盖后立即落盘（同 toggleTavily 模式；显式选择预设=接受覆盖该协议配置）。 */
  const applyProviderPreset = async (p: (typeof PROVIDER_PRESETS)[number]) => {
    const models = p.models.map((m) => ({
      id: genModelId(),
      label: m,
      model: m,
      baseUrl: p.baseUrl,
      vendor: p.name,
    }));
    // 厂商制语义：同协议下其他厂商条目保留，仅替换同名厂商旧条目（若有）
    const list = config.modelsByProvider[p.protocol] ?? [];
    const kept = list.filter(
      (m) => (m.vendor ?? `__legacy__${p.protocol}`) !== p.name,
    );
    const next = {
      ...config,
      apiProvider: p.protocol,
      modelsByProvider: {
        ...config.modelsByProvider,
        [p.protocol]: [...kept, ...models],
      },
      activeModelId: {
        ...config.activeModelId,
        [p.protocol]: models[0]?.id ?? null,
      },
    };
    setConfig(next);
    // skipReload：本地 state 已是合并后最新（loadConfig 会拿旧 mock/磁盘值覆盖刚点字段）
    await saveConfig(next, { skipReload: true });
    setShowProviderPicker(false);
    setActiveVendor(p.name);
  };
  const [exporting, setExporting] = useState(false);
  const [importing, setImporting] = useState(false);
  const [exportingWs, setExportingWs] = useState(false);
  const [importingWs, setImportingWs] = useState(false);
  const [botEnabled, setBotEnabled] = useState(false);
  const [botBusy, setBotBusy] = useState(false);
  const [botError, setBotError] = useState("");
  // 开机自启动：登录系统时自动拉起 wmessage
  // 走 tauri-plugin-autostart：is_enabled / enable / disable 三个命令
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
    // 界面字体大小：small/standard/large/xlarge 四档
    // 默认 small（老板拍板「目前字号为小」）；后端 None 也回退到 small
    uiFontSize: "small" as UiFontSize,
    hasApiKey: false,
    bypassLlmOnPreStepHit: true, // F-1 [P0] pre-step 路由外层是否跳过主 LLM；老配置默认 true
    // 本地文件工具白名单目录（textarea 一行一个；空 = 后端内置默认 桌面/下载/文档+绑定文件夹）
    allowedDirs: "",
    // Tavily 搜索 key 是否已存系统凭据存储（key 本体不回填，
    // 与主 API key 同模式：view 只给 has 标志，输入框独立 state 不回填）
    hasTavilyKey: false,
    // 「Tavily 搜索」开关：开 = web_search 走 Tavily；关 = Bing+百度双引擎
    tavilyEnabled: false,
    // Brave 搜索 key 是否已存系统凭据存储（同 hasTavilyKey）
    hasBraveKey: false,
    // 「Brave 搜索」开关：开 = web_search 走 Brave；与 Tavily 互斥，双开报错
    braveEnabled: false,
    // run_python 默认超时秒数（空 = 60s 默认；模型 timeoutSecs 参数优先；硬钳 300s）
    pythonTimeoutSecs: "",
    // 授权模式：strict=白名单外硬拒 / ask=白名单外弹授权（默认）/ yolo=全放行
    permMode: "ask" as "strict" | "ask" | "yolo",
    // max_tokens（仅 Anthropic 模式用；空 = 8192 默认，范围 256-200000）
    // 仍是顶层配置——同一协议下多个模型共用一个 max_tokens
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
  });
  // 「立即整理」按钮状态与结果提示（转圈 → 短暂 toast 式文案，同 configSaved 模式）
  const [consolidateBusy, setConsolidateBusy] = useState(false);
  const [consolidateMsg, setConsolidateMsg] = useState("");
  const [keyInput, setKeyInput] = useState("");
  // Tavily/Brave key 输入框（与主 keyInput 同模式：不回填已存 key，
  // 非空保存时覆盖写入系统凭据存储；空 = 不动已存 key）
  const [tavilyKeyInput, setTavilyKeyInput] = useState("");
  const [braveKeyInput, setBraveKeyInput] = useState("");
  const [configBusy, setConfigBusy] = useState(false);
  const [configSaved, setConfigSaved] = useState(false);
  /** 「保存配置」按钮（U8 拆卡后各含 setConfig 字段的卡各配一枚；cls 控制外距）。
   *  aria-label 恒为「保存配置」：可见文本随状态变（已保存 ✓/保存中…），可访问名不变。 */
  const renderSaveButton = (cls: string) => (
    <div className={cls}>
      <button
        aria-label="保存配置"
        className={`shrink-0 min-w-[76px] px-4 py-1.5 text-sm text-[var(--t3)] ${
          configBusy ? "nm-inset" : "nm-outset"
        }`}
        onClick={() => saveConfig()}
        disabled={configBusy}
      >
        {configSaved ? "已保存 ✓" : configBusy ? "保存中…" : "保存配置"}
      </button>
    </div>
  );
  const [pyEnabled, setPyEnabled] = useState(false);
  const [pyBusy, setPyBusy] = useState(false);
  const [pyEnv, setPyEnv] = useState<{ available: boolean; python: string; version: string; libs: string[] } | null>(null);
  const [pyEnvBusy, setPyEnvBusy] = useState(false);
  /** 环境检查失败的错误文案（可见反馈，不再只 console.error） */
  const [pyEnvErr, setPyEnvErr] = useState("");
  const [logOpen, setLogOpen] = useState(false);
  const [logText, setLogText] = useState("");
  const [logBusy, setLogBusy] = useState(false);

  const refreshBot = async () => {
    try {
      const on = await invoke<boolean>("bot_get_enabled");
      setBotEnabled(on);
    } catch (e) {
      // 自动加载失败：只记 console，不打扰
      handleCommandError(e, "bot_get_enabled", { silent: true });
    }
  };

  const loadConfig = async () => {
    try {
      const c = await invoke<{
        hasApiKey: boolean;
        bypassLlmOnPreStepHit?: boolean;
        allowedDirs?: string[];
        // view 不含 key 本体，只有 has 标志
        hasTavilyKey?: boolean;
        tavilyEnabled?: boolean | null;
        hasBraveKey?: boolean;
        braveEnabled?: boolean | null;
        pythonTimeoutSecs?: number | null;
        permMode?: string | null;
        apiProvider?: string | null;
        maxTokens?: number | null;
        // 推理强度后台默认（RE-1）：off/low/medium/high；缺字段/非法值 → 前端按 medium 显示
        reasoningEffort?: string | null;
        // 每协议下的大模型列表 + active 模型 id
        // 老后端版本（无这俩字段）→ undefined → 前端按空列表处理（"不设置默认厂商"）
        modelsByProvider?: ModelsByProvider | null;
        activeModelId?: ActiveModelId | null;
        // 界面字体大小（small/standard/large/xlarge）
        // 老后端版本没返 → 前端按 small 回退（老板拍板默认）
        uiFontSize?: string | null;
        // 定时记忆整理配置（老后端没返 → 默认启用 + 每天）
        memoryConsolidation?: {
          enabled?: boolean;
          interval?: string;
          lastRunAt?: number | null;
        } | null;
      }>(
        "bot_get_config"
      );
      setConfig({
        // 老配置缺字段/非法值 → openai（与后端 ApiProvider::from_cfg 回退一致）
        apiProvider: c.apiProvider === "anthropic" ? "anthropic" : "openai",
        // 老后端版本没返 modelsByProvider → 空列表（无默认厂商）
        modelsByProvider: c.modelsByProvider ?? { openai: [], anthropic: [] },
        activeModelId: c.activeModelId ?? { openai: null, anthropic: null },
        // 字体大小：老后端 / None / 非法值都回退 small（"目前字号为小"）
        uiFontSize:
          c.uiFontSize === "standard" ||
          c.uiFontSize === "large" ||
          c.uiFontSize === "xlarge"
            ? c.uiFontSize
            : "small",
        hasApiKey: !!c.hasApiKey,
        // 老后端版本（没返 bypass 字段）默认 true，避免意外走 LEGACY 路径
        bypassLlmOnPreStepHit: c.bypassLlmOnPreStepHit ?? true,
        allowedDirs: (c.allowedDirs ?? []).join("\n"),
        // view 只给 has 标志；key 本体不回填（与 hasApiKey/keyInput 同模式）。
        // 开关自动态：老配置没显式开关字段（null/undefined）时按 has 标志显示
        hasTavilyKey: c.hasTavilyKey ?? false,
        tavilyEnabled: c.tavilyEnabled ?? (c.hasTavilyKey ?? false),
        hasBraveKey: c.hasBraveKey ?? false,
        braveEnabled: c.braveEnabled ?? (c.hasBraveKey ?? false),
        pythonTimeoutSecs: c.pythonTimeoutSecs != null ? String(c.pythonTimeoutSecs) : "",
        // 老配置缺字段/非法值 → ask（与后端 PermMode::from_cfg 回退一致）
        permMode:
          c.permMode === "strict" || c.permMode === "yolo" ? c.permMode : "ask",
        // 空 = 8192 默认（仅 Anthropic 模式用）
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

  const checkPyEnv = async () => {
    if (pyEnvBusy) return;
    setPyEnvBusy(true);
    setPyEnvErr("");
    try {
      const env = await invoke<{ available: boolean; python: string; version: string; libs: string[] }>("py_env_check");
      setPyEnv(env);
    } catch (e) {
      handleCommandError(e, "py_env_check", { silent: true });
      setPyEnvErr(formatCommandError(e));
    } finally {
      setPyEnvBusy(false);
    }
  };

  // 查看机器人审计日志（最新在前，默认 200 行）
  const openLog = async () => {
    if (logBusy) return;
    setLogBusy(true);
    try {
      setLogText(await invoke<string>("bot_log_read", { limit: 200 }));
    } catch (e) {
      handleCommandError(e, "bot_log_read", { silent: true });
      setLogText(formatCommandError(e));
    } finally {
      setLogBusy(false);
    }
    setLogOpen(true);
  };

  useEffect(() => {
    // 挂载期多面板初始刷新（外部数据同步既有模式）：被调函数内部链路含 setState，
    // lint 对 effect 内调用一律按同步路径处理，保既有直写
    // oxlint-disable-next-line react/set-state-in-effect
    refreshBot();
    // oxlint-disable-next-line react/set-state-in-effect
    loadConfig();
    // oxlint-disable-next-line react/set-state-in-effect
    refreshPy();
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
    if (enabling && !window.confirm("开启后机器人可执行 Python 代码（沙箱：独立临时目录 + 60 秒超时 + 审计留痕）。确认开启？")) return;
    setPyBusy(true);
    setBotError("");
    try {
      const next = await invoke<boolean>("py_set_enabled", { enabled: enabling });
      setPyEnabled(next);
      if (next) checkPyEnv();
    } catch (e) {
      handleCommandError(e, "py_set_enabled", { silent: true });
      setBotError(formatCommandError(e));
      await refreshPy();
    } finally {
      setPyBusy(false);
    }
  };

  const toggleBot = async () => {
    if (botBusy) return;
    setBotBusy(true);
    setBotError("");
    try {
      const next = await invoke<boolean>("bot_set_enabled", { enabled: !botEnabled });
      setBotEnabled(next);
      // 广播给挂件：展开状态下同步调整窗口高度（加/减聊天区）
      emit("bot-changed", next).catch(() => {});
    } catch (e) {
      handleCommandError(e, "bot_set_enabled", { silent: true });
      setBotError(formatCommandError(e));
      await refreshBot();
    } finally {
      setBotBusy(false);
    }
  };

  const saveConfig = async (
    override?: typeof config | ((c: typeof config) => typeof config),
    opts?: { skipReload?: boolean },
  ) => {
    if (configBusy) return;
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
          // 字体大小直接透传，后端原样存（None = small 默认）
          uiFontSize: c.uiFontSize,
          bypassLlmOnPreStepHit: c.bypassLlmOnPreStepHit,
          // textarea 一行一个路径；空行/空白剔除；全空 = 后端内置默认白名单
          allowedDirs: c.allowedDirs
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
          // 空 = 60s 默认；非法输入按未配置处理（后端 resolve_timeout 硬钳 300s）
          pythonTimeoutSecs: (() => {
            const n = parseInt(c.pythonTimeoutSecs.trim(), 10);
            return Number.isFinite(n) && n > 0 ? n : null;
          })(),
          // 授权模式（strict/ask/yolo）
          permMode: c.permMode,
          // API 协议（openai/anthropic）
          apiProvider: c.apiProvider,
          // max_tokens：空 = 8192 默认；非法输入按未配置处理（后端钳 256..=200000）
          maxTokens: (() => {
            const n = parseInt(c.maxTokens.trim(), 10);
            return Number.isFinite(n) && n > 0 ? n : null;
          })(),
          // 推理强度后台默认（RE-1）：四档抽象档位原样落盘
          reasoningEffort: c.reasoningEffort,
          // 定时记忆整理：原样透传（后端 serde default 兜底缺字段）
          memoryConsolidation: c.memoryConsolidation,
        },
        // 输入框非空才写凭据存储；留空保持原 key 不变
        apiKey: keyInput.trim() ? keyInput.trim() : null,
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
      }
      // 广播给挂件聊天区：头部 🧠 模型标签同步刷新
      emit("bot-config-changed", null).catch(() => {});
      setConfigSaved(true);
      setTimeout(() => setConfigSaved(false), 1500);
    } catch (e) {
      handleCommandError(e, "bot_set_config", { silent: true });
      setBotError(formatCommandError(e));
    } finally {
      setConfigBusy(false);
    }
  };

  // ───────── 双协议下大模型列表 handlers ─────────
  // 厂商中心（U10）按厂商分组渲染；老配置可能缺 anthropic/openai 字段，
  // 读取处一律 ?? [] 兜底

  /** 更新大模型条目（label / baseUrl / model 任一字段变化） */
  const updateModel = (id: string, patch: Partial<ModelEntry>) => {
    setConfig((c) => {
      const list = (c.modelsByProvider[c.apiProvider] ?? []).map((m) =>
        m.id === id ? { ...m, ...patch } : m,
      );
      return {
        ...c,
        modelsByProvider: {
          ...c.modelsByProvider,
          [c.apiProvider]: list,
        },
      };
    });
  };

  /** 删除大模型条目：若删的是 active → 取列表第一个作为新 active（无则 null） */
  const deleteModel = (id: string) => {
    setConfig((c) => {
      const list = (c.modelsByProvider[c.apiProvider] ?? []).filter((m) => m.id !== id);
      let nextActive = c.activeModelId[c.apiProvider] ?? null;
      if (nextActive === id) {
        nextActive = list.length > 0 ? list[0].id : null;
      }
      return {
        ...c,
        modelsByProvider: {
          ...c.modelsByProvider,
          [c.apiProvider]: list,
        },
        activeModelId: {
          ...c.activeModelId,
          [c.apiProvider]: nextActive,
        },
      };
    });
  };

  /** 选中大模型（设为当前协议 active）；仅切 activeModelId 字段，UI 由 currentActiveId 联动刷新 */
  const selectModel = (id: string) => {
    setConfig((c) => ({
      ...c,
      activeModelId: { ...c.activeModelId, [c.apiProvider]: id },
    }));
  };

  // ───────── 厂商中心（U10）：按厂商分组渲染，条目仍存双协议列表 ─────────
  // 全部 handler 走 setConfig 函数式（updater 内读最新 c.modelsByProvider）——
  // 连续操作（改 URL→切格式→加模型）不得用外层 render 快照互相覆盖（ocr HIGH）。
  /** 选中厂商页（左栏）；null = 未选（显示引导）。值 = 厂商名 */
  const [activeVendor, setActiveVendor] = useState<string | null>(null);

  /** 条目归属厂商名：有 vendor 用 vendor，老条目按所在列表协议名兜底。
   *  prov 必须传「条目所在列表」的协议——不能用活的 apiProvider（会漂）。
   *  已知边界：兜底名与预设厂商名撞名时二者合并展示（概率低，登记）。 */
  const vendorNameOf = (m: ModelEntry, prov: "openai" | "anthropic") =>
    m.vendor ?? (prov === "anthropic" ? "Anthropic 兼容" : "OpenAI 兼容");

  /** 厂商分组：跨双协议列表聚合；绿点 = 该厂商含全局 active 模型。
   *  普通派生而非 useMemo（数据量小；compiler 对含 config 依赖的 memo 化报 warn） */
  const vendorGroups = (() => {
    const map = new Map<
      string,
      { name: string; hasActive: boolean; isActiveVendor: boolean }
    >();
    (["openai", "anthropic"] as const).forEach((prov) => {
      const list = config.modelsByProvider[prov] ?? [];
      const activeId = config.activeModelId[prov] ?? null;
      list.forEach((m) => {
        const name = vendorNameOf(m, prov);
        const g = map.get(name) ?? { name, hasActive: false, isActiveVendor: false };
        if (m.id === activeId) {
          g.hasActive = true;
          if (config.apiProvider === prov) g.isActiveVendor = true;
        }
        map.set(name, g);
      });
    });
    return [...map.values()].sort((a, b) => a.name.localeCompare(b.name));
  })();

  const vendorOf = (name: string) =>
    vendorGroups.find((v) => v.name === name) ?? null;

  /** 厂商名 → 所在协议列表（渲染期版本，读 config state）；找不到 = null */
  const protocolOfVendor = (name: string): "openai" | "anthropic" | null =>
    protocolOfVendorIn(config, name);

  /** updater 内版本：读传入的 config 快照（函数式 handler 用） */
  const protocolOfVendorIn = (
    cfg: typeof config,
    name: string,
  ): "openai" | "anthropic" | null => {
    if ((cfg.modelsByProvider.openai ?? []).some((m) => vendorNameOf(m, "openai") === name))
      return "openai";
    if ((cfg.modelsByProvider.anthropic ?? []).some((m) => vendorNameOf(m, "anthropic") === name))
      return "anthropic";
    return null;
  };

  /** 当前厂商页条目与协议（activeVendor 为 null 时空） */
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

  /** 厂商页 Base URL 编辑：函数式同步全部同名条目 */
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

  /** 厂商 API 格式切换：整厂商条目搬到另一协议列表（active 指针随条目走） */
  const setVendorProtocol = (prov: "openai" | "anthropic") => {
    setConfig((c) => {
      if (!activeVendor) return c;
      const from = protocolOfVendorIn(c, activeVendor);
      if (from === null || from === prov) return c;
      const fromList = c.modelsByProvider[from] ?? [];
      const toList = c.modelsByProvider[prov] ?? [];
      const moving = fromList.filter((m) => vendorNameOf(m, from) === activeVendor);
      const stay = fromList.filter((m) => vendorNameOf(m, from) !== activeVendor);
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

  /** 厂商页「＋ 添加模型」：新条目带厂商名；该厂商首条 → 自动 active */
  const addModelToVendor = () => {
    setConfig((c) => {
      if (!activeVendor) return c;
      const from = protocolOfVendorIn(c, activeVendor) ?? "openai";
      const list = c.modelsByProvider[from] ?? [];
      const hadModels = list.some((m) => vendorNameOf(m, from) === activeVendor);
      const baseUrl =
        (list.find((m) => vendorNameOf(m, from) === activeVendor) ?? {}).baseUrl ?? "";
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
        activeModelId: hadModels
          ? c.activeModelId
          : { ...c.activeModelId, [from]: entry.id },
      };
    });
  };

  /** 删除厂商：移除其全部条目（active 指针函数式兜底） */
  const deleteVendor = (name: string) => {
    setConfig((c) => {
      const drop = (list: ModelEntry[], prov: "openai" | "anthropic") =>
        list.filter((m) => vendorNameOf(m, prov) !== name);
      const openai = drop(c.modelsByProvider.openai ?? [], "openai");
      const anthropic = drop(c.modelsByProvider.anthropic ?? [], "anthropic");
      const fix = (prov: "openai" | "anthropic", list: ModelEntry[]) => {
        const cur = c.activeModelId[prov] ?? null;
        if (cur === null) return null;
        return list.some((m) => m.id === cur) ? cur : list[0]?.id ?? null;
      };
      return {
        ...c,
        modelsByProvider: { openai, anthropic },
        activeModelId: {
          openai: fix("openai", openai),
          anthropic: fix("anthropic", anthropic),
        },
      };
    });
    setActiveVendor((cur) => (cur === name ? null : cur));
  };

  /** 厂商页「设为当前使用」：把该厂商 active 模型设为全局聊天所用 */
  const makeVendorActive = () => {
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

  /** 「Tavily 搜索」开关：与「开启机器人聊天」同款——点击即持久化（复用整份配置保存） */
  const toggleTavily = async () => {
    const next = { ...config, tavilyEnabled: !config.tavilyEnabled };
    setConfig(next);
    await saveConfig(next);
  };

  /** 「Brave 搜索」开关：同 toggleTavily——点击即持久化（复用整份配置保存） */
  const toggleBrave = async () => {
    const next = { ...config, braveEnabled: !config.braveEnabled };
    setConfig(next);
    await saveConfig(next);
  };

  /** 授权模式切换：点击即持久化（同 toggleTavily 模式） */
  const setPermMode = async (mode: "strict" | "ask" | "yolo") => {
    if (configBusy || config.permMode === mode) return;
    const next = { ...config, permMode: mode };
    setConfig(next);
    await saveConfig(next);
  };

  /** 记忆整理开关/频率：点击即持久化（同 toggleTavily / setPermMode 模式） */
  const setConsolidation = async (patch: Partial<{ enabled: boolean; interval: string }>) => {
    if (configBusy) return;
    const next = {
      ...config,
      memoryConsolidation: { ...config.memoryConsolidation, ...patch },
    };
    setConfig(next);
    await saveConfig(next);
  };

  /** 从厂商网站拉取模型列表（U11）：GET {baseUrl}/models，按 API 格式带鉴权头。
   *  合并语义：按 model id 去重，新 id append（label=id、baseUrl=厂商 Base URL、
   *  enabled、vendor=当前厂商）；已存在的不动。完成后立即落盘。 */
  const fetchModelsForVendor = async () => {
    if (!activeVendor || !activeVendorEntries.length) return;
    const vendor = activeVendor;
    const proto = activeVendorProtocol;
    const baseUrl = vendorBaseUrl;
    setFetchModelsBusy(true);
    setFetchModelsMsg("");
    try {
      const ids = await invoke<string[]>("fetch_provider_models", {
        baseUrl,
        apiFormat: proto,
        apiKey: keyInput.trim() ? keyInput.trim() : null,
      });
      // 合并：新 id append（label=model 名、baseUrl=厂商 Base URL、enabled=false、
      // vendor=当前厂商）；其他厂商条目不动。
      // 从 config 闭包快照算出 next 并显式传给 setConfig + saveConfig（critical 修：
      // 无参 saveConfig() 读闭包旧 config 会丢刚拉的模型）
      const list = config.modelsByProvider[proto] ?? [];
      const mine = list.filter((m) => (m.vendor ?? "") === vendor);
      const existing = new Set(mine.map((m) => m.model));
      const fresh = ids
        .filter((id) => !existing.has(id))
        .map((id) => ({
          id: genModelId(),
          label: id,
          model: id,
          baseUrl,
          vendor,
          enabled: false,
        }));
      const next = {
        ...config,
        modelsByProvider: {
          ...config.modelsByProvider,
          [proto]: [...list.filter((m) => (m.vendor ?? "") !== vendor), ...mine, ...fresh],
        },
      };
      setConfig(next);
      const newCount = fresh.length;
      setFetchModelsMsg(
        newCount > 0
          ? `已获取 ${ids.length} 个模型，新增 ${newCount} 个（默认禁用，打开开关启用）`
          : `厂商返回 ${ids.length} 个模型，均已存在`,
      );
      await saveConfig(next, { skipReload: true });
    } catch (e) {
      handleCommandError(e, "fetch_provider_models");
      setFetchModelsMsg("");
    } finally {
      setFetchModelsBusy(false);
    }
  };

  /** 立即整理：转圈 → 结果文案短暂展示（同 configSaved 的 setTimeout 清除模式），并刷新「上次整理时间」 */
  const consolidateNow = async () => {
    if (consolidateBusy) return;
    setConsolidateBusy(true);
    setConsolidateMsg("");
    try {
      const r = await invoke<{ merged: number; distilled: number; contradictions: number }>(
        "memory_consolidate_now",
      );
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

  const clearApiKey = async () => {
    if (configBusy) return;
    if (!window.confirm("清除已保存的 API Key？清除后机器人将无法调用大模型。")) return;
    setConfigBusy(true);
    setBotError("");
    try {
      await invoke("bot_clear_api_key");
      await loadConfig();
    } catch (e) {
      handleCommandError(e, "bot_clear_api_key", { silent: true });
      setBotError(formatCommandError(e));
    } finally {
      setConfigBusy(false);
    }
  };

  /** 开机自启动切换：点击即落盘——macOS 写 LaunchAgent plist,
   *  Windows 写注册表 Run 项，Linux 写 ~/.config/autostart/*.desktop。
   *  失败时回滚到后端真实状态（部分成功的情况）。 */
  const toggleAutostart = async () => {
    if (autostartBusy) return;
    setAutostartBusy(true);
    setAutostartError("");
    const next = !autostartEnabled;
    try {
      await invoke(next ? "plugin:autostart|enable" : "plugin:autostart|disable");
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
      handleCommandError(e, status.enabled ? "api_stop" : "api_start", { silent: true });
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
    if (!window.confirm("重新生成 token？旧 token 会立即失效，已授权的客户端需要换新 token。")) return;
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

  const activeMeta = SECTIONS.find((s) => s.key === activeSection) ?? SECTIONS[0];
  return (
    <div className="flex h-full min-h-0 bg-[var(--bg)]">
      {/* 左侧分类导航（U7 设置壳）：返回 + 分类项（选中 nm-inset 语义） */}
      <aside className="flex w-52 shrink-0 flex-col gap-1 overflow-y-auto border-r border-[var(--edge)] p-3">
        <button
          type="button"
          className="nm-btn mb-2 flex h-8 items-center gap-2 px-2.5 text-sm text-[var(--t3)]"
          onClick={() => onBack?.()}
          disabled={!onBack}
          title="返回主界面"
        >
          <ArrowLeft size={15} aria-hidden />
          返回
        </button>
        {SECTIONS.map((s) => (
        <button
          key={s.key}
          type="button"
          aria-current={activeSection === s.key || undefined}
          onClick={() => openSection(s.key)}
            className={`flex h-8 w-full items-center gap-2 rounded-[var(--r-sm)] px-2.5 text-sm transition-colors duration-100 ${
              activeSection === s.key
                ? "nm-inset rounded-[var(--r-sm)] text-[var(--t1)]"
                : "text-[var(--t3)] hover:bg-[var(--hover-bg)]"
            }`}
          >
            <s.icon size={15} aria-hidden />
            <span className="flex-1 text-left">{s.label}</span>
          </button>
        ))}
      </aside>
      {/* 右侧分类内容：大标题 + 卡片流（max-w 收行宽，截图节奏） */}
      <div className="min-w-0 flex-1 overflow-y-auto">
        <div className="mx-auto max-w-3xl px-8 py-6">
          <h1 className="text-xl font-semibold text-[var(--t1)]">
            {activeMeta.label}
          </h1>
          {mountedSections.has("general") && (
          <section hidden={activeSection !== "general"} className="space-y-4 pt-4">
      {/* 个人资料 */}
      <div className="nm-card p-5">
        <h2 className="text-lg font-semibold text-[var(--t1)]">个人资料</h2>
        <p className="mt-1 text-xs text-[var(--t5)]">
          任务卡上的归属头像：设了定时 → 一直机器人头像；🤖 交给机器人执行中 → 机器人头像；执行完且无定时 → 用户头像。悬停头像显示姓名。
        </p>
        <div className="mt-4 space-y-4">
          <ProfileRow kind="user" label="用户" defaultName="我" />
          <ProfileRow kind="bot" label="机器人" defaultName="机器人" />
        </div>
      </div>

      {/* 通用设置：外观 + 开机自启动 wmessage */}
      <div className="nm-card p-5">
        <h2 className="text-lg font-semibold text-[var(--t1)]">通用设置</h2>
        <div className="mt-4 space-y-3">
          {/* 外观（原「深浅色模式」改名：主题是外观的一部分，名字更准确） */}
          <div>
            <p className="text-sm font-medium text-[var(--t2)]">外观</p>
            <div className="mt-3 flex gap-1">
              {(
                [
                  ["light", "☀️ 浅色"],
                  ["dark", "🌙 深色"],
                  ["system", "🖥️ 跟随系统"],
                ] as [ThemeSetting, string][]
              ).map(([value, label]) => (
                <button
                  key={value}
                  className={`px-4 py-1.5 text-sm text-[var(--t3)] ${
                    theme === value ? "nm-inset" : "nm-outset"
                  }`}
                  onClick={() => onThemeChange(value)}
                >
                  {label}
                </button>
              ))}
            </div>
            {/* 字体大小：四档 small/standard/large/xlarge
                默认 small（“目前字号为小”）。走 data-attr 全局套用，
                视觉缩放在 main.css 里。点选即生效 + 即时落盘（与外观一致，
                不用再点「保存配置」）。 */}
            <p className="mt-3 text-sm font-medium text-[var(--t2)]">字体大小</p>
            <div className="mt-2 flex gap-1">
              {UI_FONT_SIZE_OPTIONS.map((o) => (
                <button
                  key={o.value}
                  className={`flex-1 px-2 py-1.5 text-xs ${
                    config.uiFontSize === o.value ? "nm-inset" : "nm-outset"
                  } text-[var(--t3)]`}
                  onClick={() => {
                    const v = o.value;
                    setConfig((c) => ({ ...c, uiFontSize: v }));
                    // 点选即生效：data-attr 预览 + saveConfig 落盘（functional override
                    // 拿最新 config，skipReload 避免末尾重拉用磁盘上可能的旧 uiFontSize
                    // 覆盖刚点的字段）
                    document.documentElement.dataset.fontSize = v;
                    saveConfig((c) => ({ ...c, uiFontSize: v }), { skipReload: true });
                  }}
                >
                  {o.label}
                </button>
              ))}
            </div>
          </div>
          {/* 开机自动启动 wmessage：走 tauri-plugin-autostart，
              走现成的 plugin:autostart|enable / disable / is_enabled 命令。
              点击即落盘：macOS 写 LaunchAgent plist / Windows 写注册表 Run / Linux 写 .desktop。 */}
          <div className="pt-3 border-t border-[var(--edge)] flex items-center justify-between gap-4">
            <div className="min-w-0">
              <p className="text-sm font-medium text-[var(--t2)]">开机自动启动 wmessage</p>
              <p className="mt-1 text-xs text-[var(--t5)]">
                登录系统时自动拉起 wmessage，开关改完即生效
              </p>
              {autostartError && (
                <p className="mt-1 text-xs text-[var(--danger)]">{autostartError}</p>
              )}
            </div>
            <button
              className={`shrink-0 min-w-[76px] px-4 py-1.5 text-sm text-[var(--t3)] ${
                autostartEnabled ? "nm-inset" : "nm-outset"
              }`}
              onClick={toggleAutostart}
              disabled={autostartBusy}
            >
              {autostartBusy ? "…" : autostartEnabled ? "已开启" : "已关闭"}
            </button>
          </div>
        </div>
      </div>

      </section>
          )}
          {mountedSections.has("data") && (
          <section hidden={activeSection !== "data"} className="space-y-4 pt-4">
      {/* 任务数据管理 */}
      <div className="nm-card p-5">
        <h2 className="text-lg font-semibold text-[var(--t1)]">任务数据管理</h2>
        <div className="mt-4 space-y-3">
          <div className="flex items-center justify-between gap-4">
            <div className="min-w-0">
              <p className="text-sm font-medium text-[var(--t2)]">导出任务数据</p>
              <p className="mt-1 text-xs text-[var(--t5)]">
                全部任务卡（含归档、回收站）导出为 JSON 文件
              </p>
            </div>
            <button
              className={`shrink-0 min-w-[76px] px-4 py-1.5 text-sm text-[var(--t3)] ${
                exporting ? "nm-inset" : "nm-outset"
              }`}
              onClick={runExport}
              disabled={exporting}
            >
              {exporting ? "导出中…" : "📤 导出"}
            </button>
          </div>
          <div className="flex items-center justify-between gap-4">
            <div className="min-w-0">
              <p className="text-sm font-medium text-[var(--t2)]">导入任务数据</p>
              <p className="mt-1 text-xs text-[var(--t5)]">
                从 JSON 文件导入任务卡；按 id 合并，同 id 保留最后修改的记录
              </p>
            </div>
            <button
              className={`shrink-0 min-w-[76px] px-4 py-1.5 text-sm text-[var(--t3)] ${
                importing ? "nm-inset" : "nm-outset"
              }`}
              onClick={runImport}
              disabled={importing}
            >
              {importing ? "导入中…" : "📥 导入"}
            </button>
          </div>
        </div>
      </div>

      {/* 工作区管理：与任务数据管理风格一致；workspace_items 数据独立于任务数据 */}
      <div className="nm-card p-5">
        <h2 className="text-lg font-semibold text-[var(--t1)]">工作区管理</h2>
        <p className="mt-1 text-xs text-[var(--t5)]">
          工作区是按用途分组的链接集合（文件路径 / 文件夹 / 网页），与任务数据分开独立存储
        </p>
        <div className="mt-4 space-y-3">
          <div className="flex items-center justify-between gap-4">
            <div className="min-w-0">
              <p className="text-sm font-medium text-[var(--t2)]">导出工作区链接</p>
              <p className="mt-1 text-xs text-[var(--t5)]">
                全部工作区条目（含分组、折叠状态、链接列表）导出为 JSON 文件
              </p>
            </div>
            <button
              className={`shrink-0 min-w-[76px] px-4 py-1.5 text-sm text-[var(--t3)] ${
                exportingWs ? "nm-inset" : "nm-outset"
              }`}
              onClick={runExportWs}
              disabled={exportingWs}
            >
              {exportingWs ? "导出中…" : "📤 导出"}
            </button>
          </div>
          <div className="flex items-center justify-between gap-4">
            <div className="min-w-0">
              <p className="text-sm font-medium text-[var(--t2)]">导入工作区链接</p>
              <p className="mt-1 text-xs text-[var(--t5)]">
                从 JSON 文件导入工作区条目；按 id 合并，同 id 保留最后修改的记录
              </p>
            </div>
            <button
              className={`shrink-0 min-w-[76px] px-4 py-1.5 text-sm text-[var(--t3)] ${
                importingWs ? "nm-inset" : "nm-outset"
              }`}
              onClick={runImportWs}
              disabled={importingWs}
            >
              {importingWs ? "导入中…" : "📥 导入"}
            </button>
          </div>
        </div>
      </div>

      </section>
          )}
          {mountedSections.has("bot") && (
          <section hidden={activeSection !== "bot"} className="space-y-4 pt-4">
      <div className="nm-card p-5">
        <h2 className="text-lg font-semibold text-[var(--t1)]">机器人设置</h2>

        {/* 内置机器人聊天开关 */}
        <div className="mt-4 flex items-center justify-between gap-4">
          <div className="min-w-0">
            <p className="text-sm font-medium text-[var(--t2)]">开启机器人聊天</p>
            <p className="mt-1 text-xs text-[var(--t5)]">
              在挂件下方显示聊天窗口，用大模型管理任务
            </p>
          </div>
          <button
            className={`shrink-0 min-w-[76px] px-4 py-1.5 text-sm text-[var(--t3)] ${
              botEnabled ? "nm-inset" : "nm-outset"
            }`}
            onClick={toggleBot}
            disabled={botBusy}
          >
            {botBusy ? "…" : botEnabled ? "已开启" : "已关闭"}
          </button>
        </div>

        {botError && (
          <p className="mt-2 text-xs text-[var(--danger)]">{botError}</p>
        )}

        {/* Python 编程开关 */}
        <div className="mt-4 flex items-center justify-between gap-4">
          <div className="min-w-0">
            <p className="text-sm font-medium text-[var(--t2)]">允许机器人执行 Python</p>
            <p className="mt-1 text-xs text-[var(--t5)]">
              文档处理/编程工具的本机执行开关；沙箱：独立临时目录 + 60 秒超时 + 审计留痕，默认关闭
            </p>
          </div>
          <button
            className={`shrink-0 min-w-[76px] px-4 py-1.5 text-sm text-[var(--t3)] ${
              pyEnabled ? "nm-inset" : "nm-outset"
            }`}
            onClick={togglePy}
            disabled={pyBusy}
          >
            {pyBusy ? "…" : pyEnabled ? "已开启" : "已关闭"}
          </button>
        </div>

        {/* Python 环境状态 */}
        <div className="mt-3 flex items-center justify-between gap-4">
          <div className="min-w-0">
            <p className="text-xs font-medium text-[var(--t4)]">本机 Python 环境</p>
            <p className="mt-1 text-[11px] text-[var(--t5)]">
              {pyEnvErr
                ? `检查失败：${pyEnvErr}`
                : pyEnv
                  ? pyEnv.available
                    ? `${pyEnv.version} · ${pyEnv.libs.join(" · ")}`
                    : "未检测到 Python（macOS 装 Command Line Tools；Windows 到 python.org 安装）"
                  : "点击右侧检查"}
            </p>
          </div>
          <button
            className={`shrink-0 min-w-[76px] px-4 py-1.5 text-sm text-[var(--t3)] ${
              pyEnvBusy ? "nm-inset" : "nm-outset"
            }`}
            onClick={checkPyEnv}
            disabled={pyEnvBusy}
          >
            {pyEnvBusy ? "检查中…" : "检查环境"}
          </button>
        </div>

        {/* Python 默认超时：pandas 大计算 60s 偏紧；模型可用 timeoutSecs 参数临时调 */}
        <div className="mt-3 flex items-center justify-between gap-4">
          <div className="min-w-0">
            <p className="text-xs font-medium text-[var(--t4)]">Python 默认超时（秒）</p>
            <p className="mt-1 text-[11px] text-[var(--t5)]">
              留空 = 60 秒；大计算可调大，上限 300 秒（超出自动钳制）
            </p>
          </div>
          <input
            value={config.pythonTimeoutSecs}
            onChange={(e) => setConfig((c) => ({ ...c, pythonTimeoutSecs: e.target.value }))}
            placeholder="60"
            inputMode="numeric"
            className="nm-inset shrink-0 w-24 rounded-xl px-3 py-1.5 text-xs text-[var(--t3)] outline-none text-right"
          />
        </div>

        {/* 机器人审计日志查看 */}
        <div className="mt-3 flex items-center justify-between gap-4">
          <div className="min-w-0">
            <p className="text-xs font-medium text-[var(--t4)]">机器人审计日志</p>
            <p className="mt-1 text-[11px] text-[var(--t5)]">
              记录机器人的用户指令、工具调用、参数与结果（数据目录 bot.log）
            </p>
          </div>
          <button
            className="shrink-0 min-w-[76px] px-4 py-1.5 text-sm text-[var(--t3)] nm-outset"
            onClick={openLog}
            disabled={logBusy}
          >
            {logBusy ? "读取中…" : "查看日志"}
          </button>
        </div>
        {/* U8：Python 超时等 setConfig 字段靠本卡保存钮落盘（bot 关闭时 model 卡按钮不可达） */}
        {renderSaveButton("mt-3 flex justify-end")}
      </div>
      </section>
      )}
      {mountedSections.has("memory") && (
      <section hidden={activeSection !== "memory"} className="space-y-4 pt-4">
      <div className="nm-card p-5">
        {/* 定时记忆整理 */}
        <div className="space-y-2">
          <div className="flex items-center justify-between gap-4">
            <div className="min-w-0">
              <p className="text-xs font-medium text-[var(--t4)]">记忆整理</p>
              <p className="mt-1 text-[11px] text-[var(--t5)]">
                定期用大模型对近期记忆做一轮反思：合并相关条目、裁决矛盾、提炼规律（需要已配置大模型 API）
              </p>
            </div>
            <button
              className={`shrink-0 min-w-[76px] px-4 py-1.5 text-sm text-[var(--t3)] ${
                config.memoryConsolidation.enabled ? "nm-inset" : "nm-outset"
              }`}
              onClick={() =>
                setConsolidation({ enabled: !config.memoryConsolidation.enabled })
              }
              disabled={configBusy}
            >
              {config.memoryConsolidation.enabled ? "已开启" : "已关闭"}
            </button>
          </div>
          {config.memoryConsolidation.enabled && (
            <>
              <div className="flex gap-2">
                {(
                  [
                    ["off", "关闭"],
                    ["12h", "每 12 小时"],
                    ["daily", "每天"],
                    ["weekly", "每周"],
                  ] as const
                ).map(([iv, label]) => (
                  <button
                    key={iv}
                    className={`flex-1 px-2 py-1.5 text-xs text-[var(--t3)] ${
                      config.memoryConsolidation.interval === iv ? "nm-inset" : "nm-outset"
                    }`}
                    onClick={() => setConsolidation({ interval: iv })}
                    disabled={configBusy}
                  >
                    {label}
                  </button>
                ))}
              </div>
              <div className="flex items-center justify-between gap-4">
                <p className="text-[11px] text-[var(--t5)]">
                  上次整理：
                  {config.memoryConsolidation.lastRunAt
                    ? new Date(config.memoryConsolidation.lastRunAt).toLocaleString("zh-CN", {
                        month: "2-digit",
                        day: "2-digit",
                        hour: "2-digit",
                        minute: "2-digit",
                      })
                    : "从未"}
                </p>
                <button
                  className={`shrink-0 min-w-[76px] px-4 py-1.5 text-sm text-[var(--t3)] ${
                    consolidateBusy ? "nm-inset" : "nm-outset"
                  }`}
                  onClick={consolidateNow}
                  disabled={consolidateBusy}
                >
                  {consolidateBusy ? "整理中…" : "立即整理"}
                </button>
              </div>
              {consolidateMsg && (
                <p className="text-[11px] text-[var(--t4)]">{consolidateMsg}</p>
              )}
            </>
          )}
        </div>
      </div>
      </section>
      )}
      {mountedSections.has("model") && (
      <section hidden={activeSection !== "model"} className="space-y-4 pt-4">
      {/* U9 页首：说明 + 刷新 + 添加供应商 */}
      <div className="flex items-center justify-between gap-3">
        <p className="text-xs text-[var(--t5)]">
          管理模型供应商，配置后可在挂件聊天时选择使用。
        </p>
        <div className="flex shrink-0 items-center gap-2">
          <button
            type="button"
            className="nm-btn px-2.5 py-1.5 text-xs text-[var(--t3)]"
            title="重新读取配置（丢弃未保存的本地改动）"
            onClick={() => {
              // 显式刷新 = 以服务端配置为准（覆盖未保存改动是预期语义）
              invoke("bot_get_config")
                .then((c) => setConfig((prev) => ({ ...prev, ...(c ?? {}) })))
                .catch((e) =>
                  console.error("[settings] bot_get_config 刷新失败", e)
                );
            }}
          >
            刷新
          </button>
          <button
            type="button"
            className="nm-btn px-3 py-1.5 text-xs text-[var(--t2)]"
            onClick={() => setShowProviderPicker(true)}
          >
            ＋ 添加供应商
          </button>
        </div>
      </div>
      <div className="nm-card p-5">
        {/* 大模型 API 配置（开关开启后显示） */}
        {botEnabled && (
          <div className="space-y-3">
            {/* U10 厂商中心：左厂商列表 + 右厂商页（截图骨架） */}
            <div className="flex min-h-[320px] gap-4">
              {/* 左栏：厂商列表（绿点 = 该厂商含全局 active 模型） */}
              <div className="w-44 shrink-0 space-y-1 border-r border-[var(--edge)] pr-3">
                <p className="text-[10px] text-[var(--t5)]">厂商</p>
                {vendorGroups.map((v) => (
                  <button
                    key={v.name}
                    type="button"
                    aria-current={activeVendor === v.name || undefined}
                    onClick={() => setActiveVendor(v.name)}
                    className={`flex w-full items-center gap-2 rounded-[var(--r-sm)] px-2.5 py-2 text-left text-xs transition-colors duration-100 ${
                      activeVendor === v.name
                        ? "nm-inset text-[var(--t1)]"
                        : "text-[var(--t3)] hover:bg-[var(--hover-bg)]"
                    }`}
                  >
                    <span className="flex-1 truncate">{v.name}</span>
                    <span
                      aria-hidden
                      className={`h-1.5 w-1.5 shrink-0 rounded-full ${
                        v.hasActive ? "bg-[var(--success)]" : "bg-[var(--t6)]"
                      }`}
                    />
                  </button>
                ))}
                <button
                  type="button"
                  onClick={() => {
                    setActiveVendor(null);
                    setShowProviderPicker(true);
                  }}
                  className={`flex w-full items-center gap-2 rounded-[var(--r-sm)] px-2.5 py-2 text-left text-xs text-[var(--brand)] hover:bg-[var(--hover-bg)] ${
                    showProviderPicker && activeVendor === null ? "nm-inset" : ""
                  }`}
                >
                  <Plus size={12} aria-hidden />
                  添加厂商
                </button>              </div>
              {/* 右栏：厂商页 / 添加厂商网格 / 引导 */}
              <div className="min-w-0 flex-1 space-y-3">
            {showProviderPicker ? (
              <>
                <div className="flex items-center gap-2">
                  <button
                    type="button"
                    className="nm-btn px-2 py-1 text-xs text-[var(--t3)]"
                    onClick={() => setShowProviderPicker(false)}
                  >
                    ← 返回
                  </button>
                  <p className="text-sm font-medium text-[var(--t1)]">添加厂商</p>
                </div>
                <p className="text-[10px] text-[var(--t5)]">
                  点选厂商即添加（同名厂商重新选择会覆盖其 Base URL 与模型列表）；添加后可改 Base URL、API 格式与模型列表。
                </p>
                <div className="grid grid-cols-2 gap-2">
                  {PROVIDER_PRESETS.map((p) => (
                    <button
                      key={p.name}
                      type="button"
                      onClick={() => applyProviderPreset(p)}
                      className="nm-btn flex items-center gap-2 px-3 py-2.5 text-left text-xs text-[var(--t2)]"
                    >
                      <span aria-hidden className="text-base">{p.icon}</span>
                      <span className="flex-1 truncate">{p.name}</span>
                      <span aria-hidden className="text-[10px] text-[var(--t5)]">
                        {p.protocol === "anthropic" ? "Anthropic 格式" : "OpenAI 格式"}
                      </span>
                    </button>
                  ))}
                </div>
              </>
            ) : !activeVendor ? (
              <p className="py-10 text-center text-xs text-[var(--t5)]">
                从左侧选择一个厂商，或点「添加厂商」接入新的模型供应商
              </p>
            ) : (
              <>
            {/* 厂商标题行：图标 + 名称 + 设为当前使用 */}
            <div className="flex items-center justify-between gap-3">
              <div className="flex min-w-0 items-center gap-2">
                <span aria-hidden className="text-lg">
                  {PROVIDER_PRESETS.find((p) => p.name === activeVendor)?.icon ?? "📦"}
                </span>
                <h3 className="truncate text-base font-semibold text-[var(--t1)]">
                  {activeVendor}
                </h3>
              </div>
              <button
                type="button"
                className="nm-btn shrink-0 px-3 py-1.5 text-xs text-[var(--t2)]"
                disabled={!activeVendorEntries.length}
                title="把该厂商的 active 模型设为聊天当前使用的模型"
                onClick={makeVendorActive}
              >
                {vendorOf(activeVendor)?.isActiveVendor ? "当前使用 ✓" : "设为当前使用"}
              </button>
              <button
                type="button"
                aria-label={`删除厂商 ${activeVendor}`}
                className="nm-icon-btn shrink-0 text-[var(--t5)] hover:text-[var(--danger)]"
                title="删除厂商（其全部模型条目一并移除）"
                onClick={() => {
                  if (
                    window.confirm(
                      `删除厂商「${activeVendor}」？其全部模型条目将一并移除。`,
                    )
                  ) {
                    deleteVendor(activeVendor);
                  }
                }}
              >
                <Trash2 size={13} aria-hidden />
              </button>
            </div>
            {/* Base URL：厂商条目共用一个（编辑同步全部条目） */}
            <div className="space-y-1">
              <p className="text-[10px] text-[var(--t5)]">Base URL</p>
              <input
                value={vendorBaseUrl}
                onChange={(e) => setVendorBaseUrl(e.target.value)}
                placeholder="https://api.example.com/v1"
                className="nm-inset w-full rounded-xl px-3 py-2 text-xs text-[var(--t3)] outline-none"
              />
            </div>
            {/* API 格式：该厂商条目所在的协议列表（切换 = 条目整体搬移） */}
            <div className="space-y-1">
              <p className="text-[10px] text-[var(--t5)]">API 格式</p>
              <div className="flex gap-2">
                {(
                  [
                    ["openai", "OpenAI Chat Completions"],
                    ["anthropic", "Anthropic Messages (/v1/messages)"],
                  ] as const
                ).map(([prov, label]) => (
                  <button
                    key={prov}
                    type="button"
                    className={`flex-1 px-2 py-1.5 text-xs ${
                      activeVendorProtocol === prov ? "nm-inset text-[var(--t1)]" : "nm-outset text-[var(--t3)]"
                    }`}
                    onClick={() => setVendorProtocol(prov)}
                    disabled={configBusy}
                  >
                    {label}
                  </button>
                ))}
              </div>
            </div>
            {/* API Key：全局一份（keyring 存储），所有厂商共用——挂账记录 */}
            <div className="space-y-1">
              <p className="text-[10px] text-[var(--t5)]">
                API Key（全局共用）{config.hasApiKey && <span className="text-[var(--success)]"> · 已存入系统凭据存储 ✓</span>}
              </p>
              <input
                type="password"
                value={keyInput}
                onChange={(e) => setKeyInput(e.target.value)}
                placeholder={config.hasApiKey ? "已保存（输入新 Key 可覆盖）" : "sk-…"}
                className="nm-inset w-full rounded-xl px-3 py-2 text-xs text-[var(--t3)] outline-none"
              />
              {config.hasApiKey && (
                <button
                  className="text-[10px] text-[var(--danger)] hover:underline"
                  onClick={clearApiKey}
                >
                  清除已保存的 Key
                </button>
              )}
            </div>
            {/* 模型列表：行 = radio（设为当前）+ 名称 + 上下文徽标 + 铅笔编辑 + 开关 */}
            <div className="space-y-1">
              <div className="flex items-center justify-between">
                <p className="text-[10px] text-[var(--t5)]">模型列表</p>
                <div className="flex items-center gap-1.5">
                  <button
                    type="button"
                    className="nm-btn px-2.5 py-1 text-[10px] text-[var(--t3)]"
                    title="从厂商的 /models 接口拉取模型列表；需该厂商已有模型条目（提供 Base URL），保存后新模型默认禁用"
                    onClick={fetchModelsForVendor}
                    disabled={fetchModelsBusy || !activeVendorEntries.length}
                  >
                    {fetchModelsBusy ? "获取中…" : "从厂商获取"}
                  </button>
                  <button
                    type="button"
                    className="nm-btn px-2.5 py-1 text-[10px] text-[var(--t3)]"
                    onClick={addModelToVendor}
                  >
                    ＋ 添加模型
                  </button>
                </div>
              </div>
              {fetchModelsMsg && (
                <p className="text-[10px] text-[var(--t5)]">{fetchModelsMsg}</p>
              )}
              {activeVendorEntries.length === 0 ? (
                <p className="py-1 text-[11px] text-[var(--t5)]">
                  暂无模型，点「＋ 添加模型」开始添加
                </p>
              ) : (
                <div className="space-y-1.5">
                  {activeVendorEntries.map((m) => (
                    <ModelRow
                      key={m.id}
                      model={m}
                      isActive={m.id === (config.activeModelId[activeVendorProtocol] ?? null)}
                      apiProvider={activeVendorProtocol}
                      onChange={(patch) => updateModel(m.id, patch)}
                      onSelect={() => selectModel(m.id)}
                      onDelete={() => deleteModel(m.id)}
                    />
                  ))}
                </div>
              )}
              <p className="text-[10px] text-[var(--t6)] leading-snug">
                圆点 = 当前使用的模型（机器人实际调用）；max_tokens 在 Anthropic 格式下按厂商设置。
              </p>
            </div>
            {activeVendorProtocol === "anthropic" && (
              <div className="space-y-1">
                <p className="text-[10px] text-[var(--t5)]">max_tokens</p>
                <input
                  value={config.maxTokens}
                  onChange={(e) => setConfig((c) => ({ ...c, maxTokens: e.target.value }))}
                  placeholder="8192"
                  inputMode="numeric"
                  className="nm-inset w-full rounded-xl px-3 py-2 text-xs text-[var(--t3)] outline-none"
                />
                <p className="text-[10px] text-[var(--t6)] leading-snug">
                  单次回复的最大 token 数（Anthropic 必填）。留空 = 8192；范围 256-200000，超出自动钳制。
                </p>
              </div>
            )}
            {renderSaveButton("mt-3 flex justify-end")}
              </>
            )}
              </div>
            </div>
          </div>
        )}
      </div>
      </section>
      )}
      {mountedSections.has("bot") && (
      <section hidden={activeSection !== "bot"} className="space-y-4 pt-4">
      <div className="nm-card p-5">
            {/* 授权模式（Kimi CLI 风格执行前授权） */}
            <div className="space-y-1">
              <p className="text-sm font-medium text-[var(--t2)]">授权模式</p>
              <div className="flex gap-2">
                {(
                  [
                    ["strict", "严格白名单"],
                    ["ask", "询问后放行"],
                    ["yolo", "yolo 全放行"],
                  ] as const
                ).map(([mode, label]) => (
                  <button
                    key={mode}
                    className={`flex-1 px-2 py-1.5 text-xs ${
                      config.permMode === mode ? "nm-inset" : "nm-outset"
                    } ${mode === "yolo" ? "text-[var(--danger)]" : "text-[var(--t3)]"}`}
                    onClick={() => setPermMode(mode)}
                    disabled={configBusy}
                  >
                    {label}
                  </button>
                ))}
              </div>
              <p className="text-[10px] text-[var(--t6)] leading-snug">
                {config.permMode === "strict" &&
                  "白名单外的文件访问一律拒绝。"}
                {config.permMode === "ask" &&
                  "白名单内的文件直接读；白名单外弹窗请你授权（允许一次 / 始终允许该目录 / 拒绝）。"}
                {config.permMode === "yolo" &&
                  "⚠️ 不弹任何授权：机器人可读本机任意文件，且 Python 编程免开关直接执行（以本机用户权限，可联网）。仅在你完全信任所用模型时开启。"}
              </p>
            </div>
            {/* 本地文件工具白名单（read_text_file/grep_files/list_files；
                追加语义：在内置默认之上追加放行） */}
            <div className="space-y-1">
              <p className="text-[10px] text-[var(--t5)]">文件工具白名单目录（一行一个绝对路径）</p>
              <textarea
                value={config.allowedDirs}
                onChange={(e) => setConfig((c) => ({ ...c, allowedDirs: e.target.value }))}
                placeholder={"在内置默认之上追加：~/Desktop、~/Downloads、~/Documents + 任务卡绑定文件夹始终放行"}
                rows={3}
                className="nm-inset w-full rounded-xl px-3 py-2 text-xs text-[var(--t3)] outline-none resize-y"
              />
              <p className="text-[10px] text-[var(--t6)] leading-snug">
                白名单内静默放行；白名单外按授权模式处理（见上）。授权弹窗点「始终允许该目录」会自动追加到这里。
              </p>
            </div>
        {/* U8：白名单 textarea 等 setConfig 字段靠本卡保存钮落盘 */}
        {renderSaveButton("mt-3 flex justify-end")}
      </div>
      </section>
      )}
      {mountedSections.has("mcp") && (
      <section hidden={activeSection !== "mcp"} className="space-y-4 pt-4">
      <div className="nm-card p-5">
            {/* Tavily 搜索 */}
            <div className="space-y-1">
              <div className="flex items-center justify-between gap-4">
                <div className="min-w-0">
                  <p className="text-sm font-medium text-[var(--t2)]">Tavily 搜索</p>
                  <p className="mt-1 text-xs text-[var(--t5)]">
                    开启后 web_search 走 Tavily API（需填 key）；关闭走 Bing+百度双引擎
                  </p>
                </div>
                <button
                  className={`shrink-0 min-w-[76px] px-4 py-1.5 text-sm text-[var(--t3)] ${
                    config.tavilyEnabled ? "nm-inset" : "nm-outset"
                  }`}
                  onClick={toggleTavily}
                  disabled={configBusy}
                >
                  {configBusy ? "…" : config.tavilyEnabled ? "已开启" : "已关闭"}
                </button>
              </div>
              <p className="text-[10px] text-[var(--t5)]">Tavily 搜索 API Key（可选）</p>
              <input
                type="password"
                value={tavilyKeyInput}
                onChange={(e) => setTavilyKeyInput(e.target.value)}
                placeholder={
                  config.hasTavilyKey
                    ? "已存入系统凭据存储 ✓（输入新 key 覆盖）"
                    : "tvly-…（开关关闭时不使用）"
                }
                className="nm-inset w-full rounded-xl px-3 py-2 text-xs text-[var(--t3)] outline-none"
              />
              {config.tavilyEnabled && !config.hasTavilyKey && !tavilyKeyInput.trim() ? (
                <p className="text-[10px] text-[var(--danger)] leading-snug">
                  已开启但未填 key：web_search 会报错提示，请填写 key 后点「保存配置」，或关闭开关。
                </p>
              ) : (
                <p className="text-[10px] text-[var(--t6)] leading-snug">
                  key 存系统凭据存储（与主 API key 同），不再明文写进配置文件；填 key 后点「保存配置」生效；
                  要停用请关闭开关（已存的 key 不提供清除入口）。开关开启但缺 key / Tavily 请求失败时会明确报错，不会静默走百度。
                </p>
              )}
            </div>
            {/* Brave 搜索（照搬 Tavily 模式；与 Tavily 互斥） */}
            <div className="space-y-1">
              <div className="flex items-center justify-between gap-4">
                <div className="min-w-0">
                  <p className="text-sm font-medium text-[var(--t2)]">Brave 搜索</p>
                  <p className="mt-1 text-xs text-[var(--t5)]">
                    开启后 web_search 走 Brave Search API（需填 key）；关闭走 Bing+百度双引擎
                  </p>
                </div>
                <button
                  className={`shrink-0 min-w-[76px] px-4 py-1.5 text-sm text-[var(--t3)] ${
                    config.braveEnabled ? "nm-inset" : "nm-outset"
                  }`}
                  onClick={toggleBrave}
                  disabled={configBusy}
                >
                  {configBusy ? "…" : config.braveEnabled ? "已开启" : "已关闭"}
                </button>
              </div>
              <p className="text-[10px] text-[var(--t5)]">Brave 搜索 API Key（可选）</p>
              <input
                type="password"
                value={braveKeyInput}
                onChange={(e) => setBraveKeyInput(e.target.value)}
                placeholder={
                  config.hasBraveKey
                    ? "已存入系统凭据存储 ✓（输入新 key 覆盖）"
                    : "BSA…（https://brave.com/search/api/ 免费获取；开关关闭时不使用）"
                }
                className="nm-inset w-full rounded-xl px-3 py-2 text-xs text-[var(--t3)] outline-none"
              />
              {config.braveEnabled && !config.hasBraveKey && !braveKeyInput.trim() ? (
                <p className="text-[10px] text-[var(--danger)] leading-snug">
                  已开启但未填 key：web_search 会报错提示，请填写 key 后点「保存配置」，或关闭开关。
                </p>
              ) : (
                <p className="text-[10px] text-[var(--t6)] leading-snug">
                  key 存系统凭据存储（与主 API key 同），不再明文写进配置文件；填 key 后点「保存配置」生效；
                  要停用请关闭开关（已存的 key 不提供清除入口）。与 Tavily 互斥，两个开关同时开启会报错。开关开启但缺 key / Brave 请求失败时会明确报错，不会静默走百度。
                </p>
              )}
            </div>
            {/* Tavily/Brave 双开冲突提示：后端同样明确报错，这里提前可见 */}
            {config.tavilyEnabled && config.braveEnabled && (
              <p className="text-[10px] text-[var(--danger)] leading-snug">
                ⚠️ Tavily 与 Brave 只能开启一个，请关闭其中一个。
              </p>
            )}
            {renderSaveButton("mt-3 flex justify-end")}
      </div>
      </section>
      )}
      {mountedSections.has("bot") && (
      <section hidden={activeSection !== "bot"} className="space-y-4 pt-4">
      <div className="nm-card p-5">
            {/* F-1 bypass_llm_on_pre_step_hit 开关：技能路由新链路 / 旧链路回退闸 */}
            <div className="space-y-1">
              <p className="text-[10px] text-[var(--t5)]">智能技能路由</p>
              <button
                className={`shrink-0 min-w-[160px] px-4 py-1.5 text-sm text-[var(--t3)] ${
                  config.bypassLlmOnPreStepHit ? "nm-inset" : "nm-outset"
                }`}
                onClick={() =>
                  setConfig((c) => ({
                    ...c,
                    bypassLlmOnPreStepHit: !c.bypassLlmOnPreStepHit,
                  }))
                }
                disabled={configBusy}
              >
                {config.bypassLlmOnPreStepHit
                  ? "已开启（推荐）"
                  : "已关闭（回退旧链路）"}
              </button>
              <p className="text-[10px] text-[var(--t6)] leading-snug">
                开启（推荐）：命中技能时 auto 模式走 DSL 调度器、interactive 模式由 LLM 驱动技能步骤。<br />
                关闭：退回旧链路，由主 LLM 自由选择技能——仅当新路由行为异常时紧急回退用。
              </p>
            </div>

            {/* 外部机器人 API 开关 */}
        <div className="mt-4 flex items-center justify-between gap-4">
          <div className="min-w-0">
            <p className="text-sm font-medium text-[var(--t2)]">
              开启外部机器人 API 接口
            </p>
            <p className="mt-1 text-xs text-[var(--t5)]">
              {status.enabled
                ? `运行中 · http://127.0.0.1:${status.port}`
                : "默认关闭，开启后仅监听本机 127.0.0.1"}
            </p>
          </div>
          <button
            className={`shrink-0 min-w-[76px] px-4 py-1.5 text-sm text-[var(--t3)] ${
              status.enabled ? "nm-inset" : "nm-outset"
            }`}
            onClick={toggle}
            disabled={busy}
          >
            {busy ? "…" : status.enabled ? "已开启" : "已关闭"}
          </button>
        </div>

        {error && (
          <p className="mt-2 text-xs text-[var(--danger)]">{error}</p>
        )}

        {/* token 展示 + 复制 */}
        {status.enabled && (
          <>
            <div className="mt-5">
              <p className="text-xs text-[var(--t4)]">访问令牌（Bearer Token）</p>
              <div className="mt-1.5 flex items-center gap-2">
                <input
                  readOnly
                  value={status.token}
                  className="nm-inset flex-1 min-w-0 rounded-xl px-3 py-2 text-xs text-[var(--t3)] outline-none"
                  onFocus={(e) => e.currentTarget.select()}
                />
                <button
                  className="nm-btn shrink-0 px-3 py-2 text-xs text-[var(--t3)]"
                  onClick={copyToken}
                >
                  {copied ? "已复制 ✓" : "复制"}
                </button>
                <button
                  className="nm-btn shrink-0 px-3 py-2 text-xs text-[var(--danger)]"
                  onClick={rotateToken}
                  disabled={busy}
                >
                  重新生成
                </button>
              </div>
            </div>

            {/* 接口说明 */}
            <div className="mt-5 space-y-1 text-xs text-[var(--t4)]">
              <p className="font-medium text-[var(--t3)]">
                接口（均需 Authorization: Bearer &lt;token&gt;）
              </p>
              <p>GET&nbsp;&nbsp;/api/health — 健康检查（免鉴权，仅服务状态）</p>
              <p>GET&nbsp;&nbsp;/api/tasks — 活跃任务（?status=todo|doing|done 筛选；?trash=1 回收站；?archived=1 归档；?all=1 全量）</p>
              <p>GET&nbsp;&nbsp;/api/tasks/:id — 单条任务</p>
              <p>POST&nbsp;/api/tasks — 新建任务（title 必填，note/status/filePath/fileIsDir/due/tags 可选）</p>
              <p>PUT&nbsp;&nbsp;/api/tasks/:id — 更新（title/note/status/filePath/fileIsDir/due/tags/archived/deleted）</p>
              <p>DELETE&nbsp;/api/tasks/:id — 软删进回收站（幂等）</p>
              <p>GET&nbsp;&nbsp;/api/events — SSE 实时推送（?since=事件id 断线重放）</p>
              <p className="mt-1 text-[var(--t5)]">开关状态自动记忆：退出时开启，下次启动自动恢复</p>
            </div>
          </>
        )}
        {/* U8：技能路由/外部 API 段的保存入口 */}
        {renderSaveButton("mt-4 flex justify-end")}
      </div>
      </section>
      )}
      {mountedSections.has("skills") && (
      <section hidden={activeSection !== "skills"} className="space-y-4 pt-4">
      <SkillsPanel />
      </section>
      )}
      {mountedSections.has("mcp") && (
      <section hidden={activeSection !== "mcp"} className="space-y-4 pt-4">
      <McpPanel />
      </section>
      )}
      {mountedSections.has("evolution") && (
      <section hidden={activeSection !== "evolution"} className="space-y-4 pt-4">
      <EvolutionPanel />
      </section>
      )}
      {mountedSections.has("desk") && (
      <section hidden={activeSection !== "desk"} className="space-y-4 pt-4">
      <MigrationPanel />
      </section>
      )}
      {mountedSections.has("tokens") && (
      <section hidden={activeSection !== "tokens"} className="space-y-4 pt-4">
      <div className="nm-card p-5">
        <EmptyState
          icon={<BarChart3 size={18} aria-hidden />}
          title="词元统计"
          description="大模型调用的词元用量统计规划中：将在机器人对话与任务执行里按会话/工具聚合展示"
        />
      </div>
      </section>
      )}
        </div>
      </div>

      {/* 机器人审计日志弹窗 */}
      {logOpen && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/40 p-6">
          <div className="nm-card w-full max-w-2xl h-[70vh] flex flex-col p-4">
            <div className="flex items-center justify-between mb-2 shrink-0">
              <p className="text-sm font-medium text-[var(--t2)]">
                机器人审计日志（最新在前）
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
