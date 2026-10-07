// SettingsPage 主组件（orchestrator）。
// 1846 → 1340 行：types / constants / SkillsPanel / ApiProviderSelect / ProfileRow / ModelRow
// 拆到同目录子文件，本文件保留所有 useState / handlers / JSX render。
//
// 公开 import 路径保持稳定：外部仍 `import { SettingsPage } from "./components/SettingsPage"`，
// Vite 解析到 `./SettingsPage/index.tsx` → 透传 `./SettingsPage`。

import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { emit, listen } from "@tauri-apps/api/event";
import { openUrl } from "@tauri-apps/plugin-opener";
import {
  ArrowLeft,
  BarChart3,
  Workflow,
  Bot,
  Brain,
  Check,
  Cpu,
  DatabaseBackup,
  Download,
  Eye,
  EyeOff,
  FolderOutput,
  GitBranch,
  Info,
  Monitor,
  Moon,
  MoreHorizontal,
  Plug,
  Plus,
  Settings2,
  Sparkles,
  Sun,
  TriangleAlert,
  Upload,
  Waypoints,
  type LucideIcon,
} from "lucide-react";

import {
  getShowWorkflowTasks,
  setShowWorkflowTasks,
  WORKFLOW_VISIBILITY_EVENT,
} from "../../lib/workflowVisibility";
import {
  getDecomposeGuidance,
  MAX_GUIDANCE_CHARS,
  resetDecomposeGuidance,
  setDecomposeGuidance,
} from "../../lib/workflowPrompt";
import {
  clearAllWorkflowAudit,
  getWorkflowSettings,
  setWorkflowSettings,
} from "../../lib/workflowAudit";

import { handleCommandError, formatCommandError } from "../../lib/errorHandler";
import { DEFAULT_ARCHIVE_DAYS, MAX_ARCHIVE_DAYS } from "../../lib/archiveRule";
import {
  fetchProviders,
  fetchModelsByProvider,
  syncModelsDev,
  type MetaProvider,
  type MetaModel,
} from "../../lib/modelMeta";
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
import { BubbleStyleCard } from "./BubbleStyleCard";
import { normalizeVendorName } from "./providerLogoMap";
import { McpPanel } from "./McpPanel";
import { MemoryPanel } from "./MemoryPanel";
import GraphSettingsPanel from "./GraphSettingsPanel";
import { EvolutionPanel } from "../EvolutionPanel";
import { ProfileRow } from "./ProfileRow";
import { ModelRow } from "./ModelRow";
import { UsageStatsCard } from "./UsageStatsCard";
import { Toggle } from "../Toggle/Toggle";
import { ProviderLogo } from "./ProviderLogo";
import { ApiProviderSelect } from "./ApiProviderSelect";

/** 供应商预设（U9 添加供应商网格）：点击 = 切协议 + 预填 baseUrl/模型列表，
 *  完全落在现有双协议数据模型内（同一协议仅一份配置，预设会覆盖该协议当前
 *  Base URL/模型列表——网格页注明）。模型名用户可改。
 *  keyUrl = 厂商控制台的 API Key 获取页（详情页「获取 API Key」外链）。 */
type PresetModel = { id: string; contextK?: number; capabilities?: string[] };
type ProviderPreset = {
  name: string;
  protocol: "openai" | "anthropic";
  baseUrl: string;
  keyUrl: string;
  models: PresetModel[];
};
const PROVIDER_PRESETS: ProviderPreset[] = [
  { name: "DeepSeek", protocol: "openai", baseUrl: "https://api.deepseek.com", keyUrl: "https://platform.deepseek.com/api_keys", models: [{ id: "deepseek-chat", contextK: 128 }, { id: "deepseek-reasoner", contextK: 128 }] },
  { name: "Kimi", protocol: "openai", baseUrl: "https://api.moonshot.cn/v1", keyUrl: "https://platform.moonshot.cn/console/api-keys", models: [{ id: "kimi-k2-0711-preview", contextK: 256 }, { id: "moonshot-v1-8k", contextK: 8 }] },
  { name: "MiniMax", protocol: "anthropic", baseUrl: "https://api.minimaxi.com/anthropic", keyUrl: "https://platform.minimaxi.com/user-center/basic-information/interface-key", models: [{ id: "MiniMax-M3", contextK: 1000, capabilities: ["视觉"] }, { id: "MiniMax-M2", contextK: 204.8 }] },
  { name: "OpenRouter", protocol: "openai", baseUrl: "https://openrouter.ai/api/v1", keyUrl: "https://openrouter.ai/keys", models: [{ id: "openrouter/auto" }] },
  { name: "阿里云百炼", protocol: "openai", baseUrl: "https://dashscope.aliyuncs.com/compatible-mode/v1", keyUrl: "https://bailian.console.aliyun.com/?apiKey=1#/api-key", models: [{ id: "qwen-plus", contextK: 1024 }, { id: "qwen-max", contextK: 32 }] },
  { name: "OpenAI", protocol: "openai", baseUrl: "https://api.openai.com/v1", keyUrl: "https://platform.openai.com/api-keys", models: [{ id: "gpt-4o", contextK: 128, capabilities: ["视觉"] }, { id: "gpt-4o-mini", contextK: 128, capabilities: ["视觉"] }] },
  { name: "Anthropic", protocol: "anthropic", baseUrl: "https://api.anthropic.com", keyUrl: "https://console.anthropic.com/settings/keys", models: [{ id: "claude-sonnet-4-20250514", contextK: 200, capabilities: ["视觉"] }] },
  { name: "xAI", protocol: "openai", baseUrl: "https://api.x.ai/v1", keyUrl: "https://console.x.ai/", models: [{ id: "grok-4", contextK: 256, capabilities: ["视觉"] }] },
];

/** 厂商详情页 API 格式下拉选项（ApiProviderSelect 默认是协议短名，这里用全名） */
const API_FORMAT_OPTIONS = [
  { value: "openai", label: "OpenAI Chat Completions" },
  { value: "anthropic", label: "Anthropic Messages" },
] as const;

/** U16 自动记忆抽取三档（档位唯一事实源：state 默认/校验/UI 映射都从这里派生） */
const AUTO_EXTRACT_MODES = [
  { value: "off", label: "关闭", aria: "自动记忆抽取：关闭" },
  { value: "auto", label: "自动入库", aria: "自动记忆抽取：自动入库" },
  { value: "confirm", label: "需确认", aria: "自动记忆抽取：需确认" },
] as const;
type AutoExtractMode = (typeof AUTO_EXTRACT_MODES)[number]["value"];
const isAutoExtractMode = (v: unknown): v is AutoExtractMode =>
  AUTO_EXTRACT_MODES.some((m) => m.value === v);
/** 已知厂商的双协议 Base URL（key = 归一化厂商名，见 normalizeVendorName）：
 *  API 格式切换时 Base URL 自动跟随的数据源之一（预设/模型库只带单一协议端点，
 *  这里补「另一协议」的已知端点——目前只有 MiniMax 双协议都有官方端点） */
const KNOWN_PROTOCOL_URLS: Record<string, { openai?: string; anthropic?: string }> = {
  minimax: {
    openai: "https://api.minimaxi.com/v1",
    anthropic: "https://api.minimaxi.com/anthropic",
  },
};

/** 添加厂商网格白名单（models.dev 200+ 家收敛）：中国厂商尽量保留，
 *  外国厂商只留头部（OpenAI/Anthropic/Google/xAI）+ OpenRouter */
const CURATED_PROVIDER_KEYS = new Set([
  // 中国厂商
  "302ai", "aihubmix",
  "alibaba", "alibaba-cn", "alibaba-coding-plan", "alibaba-coding-plan-cn",
  "alibaba-token-plan", "alibaba-token-plan-cn",
  "bailing", "deepseek", "iflowcn", "kimi-code-plan-cn", "longcat",
  "minimax", "minimax-cn", "minimax-cn-coding-plan", "minimax-coding-plan",
  "modelscope", "moonshotai", "moonshotai-cn",
  "qiniu-ai", "sensenova", "siliconflow", "siliconflow-cn",
  "stepfun", "stepfun-ai", "stepfun-ai-step-plan", "stepfun-step-plan",
  "tencent-coding-plan", "tencent-token-plan", "tencent-tokenhub",
  "volcengine", "volcengine-coding-plan",
  "xiaomi", "xiaomi-token-plan-cn",
  "zai", "zai-coding-plan", "zhipuai", "zhipuai-coding-plan",
  // 头部外国厂商 + OpenRouter
  "openai", "anthropic", "google", "xai", "openrouter",
]);

/** 模型库模型 → ModelEntry：model 去掉 "provider/" 前缀（API 调用 id 取后段）、
 *  contextK = context_length/1000 四舍五入一位小数；推理参数仅非空才带（不写 undefined 字段） */
function metaModelToEntry(m: MetaModel, vendor: string, baseUrl: string): ModelEntry {
  const slash = m.model_key.indexOf("/");
  return {
    id: genModelId(),
    label: m.display_name || m.model_key,
    model: slash >= 0 ? m.model_key.slice(slash + 1) : m.model_key,
    baseUrl,
    vendor,
    ...(m.context_length ? { contextK: Math.round(m.context_length / 100) / 10 } : {}),
    ...(m.temperature != null ? { temperature: m.temperature } : {}),
    ...(m.top_p != null ? { topP: m.top_p } : {}),
    ...(m.max_tokens != null ? { maxTokens: m.max_tokens } : {}),
    ...(m.default_system_prompt ? { systemPrompt: m.default_system_prompt } : {}),
  };
}

/** 模型库网格右侧标注：默认 base URL 的域名（无法解析 → 空串不显示） */
function hostOf(url: string | null): string {
  if (!url) return "";
  try {
    return new URL(url).host;
  } catch {
    return "";
  }
}

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
  | "workflow"
  | "graph"
  | "tokens";
const SECTIONS: { key: SectionKey; label: string; icon: LucideIcon }[] = [
  { key: "general", label: "基础设置", icon: Settings2 },
  { key: "data", label: "数据管理", icon: DatabaseBackup },
  { key: "bot", label: "机器人", icon: Bot },
  { key: "model", label: "模型设置", icon: Cpu },
  { key: "memory", label: "记忆", icon: Brain },
  { key: "skills", label: "技能", icon: Sparkles },
  { key: "workflow", label: "工作流", icon: Workflow },
  { key: "graph", label: "任务图谱", icon: Waypoints },
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
  /** 模型库（内置 Rust meta 模块）：null = 模块异常/不可用（回退内置预设网格）。
   *  model 分类首次挂载时拉取一次（fetchProviders 内部 invoke，异常 → null） */
  const [metaProviders, setMetaProviders] = useState<MetaProvider[] | null>(null);
  /** 模型库网格：搜索过滤词 / 同步 busy / 结果与错误文案 / 点选服务商 busy */
  const [metaSearch, setMetaSearch] = useState("");
  const [metaSyncBusy, setMetaSyncBusy] = useState(false);
  const [metaSyncMsg, setMetaSyncMsg] = useState("");
  const [metaApplyBusy, setMetaApplyBusy] = useState(false);
  /** 预设/模型库共用的写入逻辑：同协议下替换同名厂商旧条目、首条启用+active、
   *  其余 enabled:false；覆盖后立即落盘（skipReload 同 toggleTavily 模式）。
   *  「同名」判定走 normalizeVendorName（U13 收尾）：预设名与模型库 provider_name
   *  的大小写/命名差异不再造成同厂商两条目并存；合并保留**既有厂商名**——
   *  keyring vendor:{名} 条目与 verified_vendors 都按既有名登记，换成新名即丢 key。 */
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
        (m) => normalizeVendorName(m.vendor ?? `__legacy__${protocol}`) === norm,
      )?.vendor ?? name;
    const flagged = models.map((m, i) => ({
      ...m,
      vendor: existingName,
      enabled: i === 0,
    }));
    const next = {
      ...config,
      apiProvider: protocol,
      modelsByProvider: {
        ...config.modelsByProvider,
        [protocol]: [...kept, ...flagged],
      },
      activeModelId: {
        ...config.activeModelId,
        [protocol]: flagged[0]?.id ?? null,
      },
    };
    setConfig(next);
    // skipReload：本地 state 已是合并后最新（loadConfig 会拿旧 mock/磁盘值覆盖刚点字段）
    await saveConfig(next, { skipReload: true });
    setShowProviderPicker(false);
    setActiveVendor(existingName);
  };
  /** 应用供应商预设（U9）：预填 baseUrl/模型列表，写入走共用 applyVendorModels */
  const applyProviderPreset = async (p: (typeof PROVIDER_PRESETS)[number]) => {
    const models = p.models.map((m) => ({
      id: genModelId(),
      label: m.id,
      model: m.id,
      baseUrl: p.baseUrl,
      vendor: p.name,
      ...(m.contextK != null ? { contextK: m.contextK } : {}),
      ...(m.capabilities ? { capabilities: m.capabilities } : {}),
    }));
    await applyVendorModels(p.name, p.protocol, models);
  };
  /** 点选模型库服务商：拉该服务商模型列表 → 字段映射 → 共用写入。
   *  协议启发：provider_key === "anthropic" 或 base_url 含 "anthropic" → anthropic，否则 openai；
   *  default_base_url 为 null 时用空串（用户手填）。 */
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
  /** 「更新模型库」：meta_sync_models_dev 回源 models.dev，完成后刷新服务商列表 */
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
        setMetaSyncMsg(r ? `更新失败：${r.error ?? "未知错误"}` : "更新失败：模型库服务不可用");
      }
    } finally {
      setMetaSyncBusy(false);
    }
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
    // 被禁用的厂商名列表：禁用后该厂商模型行变淡、开关禁用，聊天选模型入口过滤
    disabledVendors: [] as string[],
    // 连接测试通过的厂商名列表（可用性依据：左栏绿点 + 聊天下拉过滤；
    // key/URL/格式变更由后端置失效）
    verifiedVendors: [] as string[],
    // 界面字体大小：small/standard/large/xlarge 四档
    // 默认 small（老板拍板「目前字号为小」）；后端 None 也回退到 small
    uiFontSize: "small" as UiFontSize,
    hasApiKey: false,
    // 已存 API Key 的厂商名列表（后端按配置里的厂商名逐个探测 keyring）：
    // 厂商页按它显示「已存入 ✓」，key 按厂商名分条目存储
    vendorKeys: [] as string[],
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
    // Function 调用熔断上限（空 = 100 默认；W5-FUSE 全域，含工作流节点执行）
    maxFunctionCalls: "",
    // 任务卡归档天数（数据管理卡）：完成满 N 天自动归档；后端 migration 兜底归档同源
    // 显示为字符串输入；"7" = 默认（后端 None 亦回 7）
    archiveAfterDays: String(DEFAULT_ARCHIVE_DAYS),
    // 授权模式：strict=白名单外硬拒 / ask=白名单外弹授权（默认）/ yolo=全放行
    permMode: "ask" as "strict" | "ask" | "auto" | "yolo",
    // P3-c per-tool 权限规则表（deny/ask/allow 首中即停；空表 = 无规则）
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
    // U15 记忆可控开关：注入总闸 + 模型主动记忆门禁（后端缺字段 = 全开）；
    // U16 自动记忆抽取档位（后端缺字段/非法值 = off）
    memoryControl: {
      injectionEnabled: true,
      autoWriteEnabled: true,
      autoExtract: "off" as AutoExtractMode,
    },
    // U17 记忆参数（手改 bot-config.json 生效，本页无 UI）：
    // 原样回传保存，防止设置页整体替换写把手改值冲掉
    memoryTuning: null as Record<string, number> | null,
    // ── P3-a Agent 运行参数（空串 = 后端内置默认；显示与保存都是字符串输入） ──
    maxRounds: "",
    historyBudgetChars: "",
    subagentMaxTurns: "",
    subagentMaxWallSecs: "",
    searchMaxResults: "",
    maxToolOutputChars: "",
  });
  // 「立即整理」按钮状态与结果提示（转圈 → 短暂 toast 式文案，同 configSaved 模式）
  const [consolidateBusy, setConsolidateBusy] = useState(false);
  const [consolidateMsg, setConsolidateMsg] = useState("");
  // P4：执行痕迹清理（数据管理卡）
  const [traceRetainDays, setTraceRetainDays] = useState("30");
  const [traceClearBusy, setTraceClearBusy] = useState(false);
  const [traceClearMsg, setTraceClearMsg] = useState("");
  const clearTraces = async () => {
    if (traceClearBusy) return;
    setTraceClearBusy(true);
    setTraceClearMsg("");
    try {
      const days = parseInt(traceRetainDays, 10);
      const removed = await invoke<number>("trace_clear_before", {
        days: Number.isFinite(days) && days > 0 ? days : 30,
      });
      setTraceClearMsg(removed > 0 ? `已清理 ${removed} 条执行痕迹` : "没有需要清理的痕迹");
    } catch (e) {
      handleCommandError(e, "清理执行痕迹", { silent: true });
      setTraceClearMsg(formatCommandError(e));
    } finally {
      setTraceClearBusy(false);
    }
  };
  // 删除非本人的任务卡（数据管理卡）：点删除直接硬删（这些卡是别人程序导出的
  // 统计用数据，老板拍板无二次确认不放回收站）；结果在按钮文字上短暂反馈——
  // 不新增提示行，避免布局抖动
  const [deleteNonSelfBusy, setDeleteNonSelfBusy] = useState(false);
  const [deleteNonSelfDone, setDeleteNonSelfDone] = useState<string | null>(null);
  const deleteNonSelfTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(
    () => () => {
      if (deleteNonSelfTimer.current) clearTimeout(deleteNonSelfTimer.current);
    },
    []
  );
  const runDeleteNonSelf = async () => {
    if (deleteNonSelfBusy) return;
    setDeleteNonSelfBusy(true);
    try {
      const n = await invoke<number>("tasks_delete_non_self");
      setDeleteNonSelfDone(n > 0 ? `已删 ${n} 张` : "无此类卡");
      if (deleteNonSelfTimer.current) clearTimeout(deleteNonSelfTimer.current);
      deleteNonSelfTimer.current = setTimeout(() => setDeleteNonSelfDone(null), 2500);
    } catch (e) {
      handleCommandError(e, "删除非本人任务卡");
      setDeleteNonSelfBusy(false);
      return;
    }
    setDeleteNonSelfBusy(false);
  };
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
        className={`shrink-0 min-w-[76px] px-4 py-1.5 text-sm text-[var(--t3)] inline-flex items-center justify-center gap-1.5 whitespace-nowrap ${
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
        maxFunctionCalls?: number | null;
        // 任务卡归档天数（None = 后端默认 7）
        archiveAfterDays?: number | null;
        permMode?: string | null;
        apiProvider?: string | null;
        maxTokens?: number | null;
        // 推理强度后台默认（RE-1）：off/low/medium/high；缺字段/非法值 → 前端按 medium 显示
        reasoningEffort?: string | null;
        // 每协议下的大模型列表 + active 模型 id
        // 老后端版本（无这俩字段）→ undefined → 前端按空列表处理（"不设置默认厂商"）
        modelsByProvider?: ModelsByProvider | null;
        activeModelId?: ActiveModelId | null;
        // 被禁用的厂商名列表（老后端版本没返 → 空）
        disabledVendors?: string[] | null;
        // 已存 API Key 的厂商名列表（老后端版本没返 → 空）
        vendorKeys?: string[] | null;
        // 连接测试通过的厂商名列表（老后端版本没返 → 空）
        verifiedVendors?: string[] | null;
        // 界面字体大小（small/standard/large/xlarge）
        // 老后端版本没返 → 前端按 small 回退（老板拍板默认）
        uiFontSize?: string | null;
        // 定时记忆整理配置（老后端没返 → 默认启用 + 每天）
        memoryConsolidation?: {
          enabled?: boolean;
          interval?: string;
          lastRunAt?: number | null;
        } | null;
        // U15 记忆可控开关（老后端没返 → 全开）；U16 抽取档位（缺字段/非法值 → off）
        memoryControl?: {
          injectionEnabled?: boolean;
          autoWriteEnabled?: boolean;
          autoExtract?: AutoExtractMode | string | null;
        } | null;
        // U17 记忆参数（老后端没返 → null；原样回传）
        memoryTuning?: Record<string, number> | null;
        // P3-a Agent 运行参数（老后端没返 → null = 内置默认）
        maxRounds?: number | null;
        historyBudgetChars?: number | null;
        subagentMaxTurns?: number | null;
        subagentMaxWallSecs?: number | null;
        searchMaxResults?: number | null;
        maxToolOutputChars?: number | null;
        // P3-c：规则表（老后端没返 → 空）
        toolRules?: Array<{ tool: string; action: string }> | null;
      }>(
        "bot_get_config"
      );
      setConfig({
        // 老配置缺字段/非法值 → openai（与后端 ApiProvider::from_cfg 回退一致）
        apiProvider: c.apiProvider === "anthropic" ? "anthropic" : "openai",
        // 老后端版本没返 modelsByProvider → 空列表（无默认厂商）
        modelsByProvider: c.modelsByProvider ?? { openai: [], anthropic: [] },
        activeModelId: c.activeModelId ?? { openai: null, anthropic: null },
        disabledVendors: c.disabledVendors ?? [],
        // 连接测试通过的厂商名单（老后端没返 → 空）
        verifiedVendors: c.verifiedVendors ?? [],
        // 字体大小：老后端 / None / 非法值都回退 small（"目前字号为小"）
        uiFontSize:
          c.uiFontSize === "standard" ||
          c.uiFontSize === "large" ||
          c.uiFontSize === "xlarge"
            ? c.uiFontSize
            : "small",
        hasApiKey: !!c.hasApiKey,
        // 已存 key 的厂商名列表（老后端没返 → 空）
        vendorKeys: c.vendorKeys ?? [],
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
        maxFunctionCalls: c.maxFunctionCalls != null ? String(c.maxFunctionCalls) : "",
        // 归档天数：后端 None / 老版本没返 → 显示默认 7
        archiveAfterDays:
          c.archiveAfterDays != null ? String(c.archiveAfterDays) : String(DEFAULT_ARCHIVE_DAYS),
        // 老配置缺字段/非法值 → ask（与后端 PermMode::from_cfg 回退一致）
        permMode:
          c.permMode === "strict" || c.permMode === "yolo" || c.permMode === "auto"
            ? c.permMode
            : "ask",
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
        // U15 记忆可控开关：缺字段/老后端 → 全开；U16 抽取档位缺字段/非法 → off
        memoryControl: {
          injectionEnabled: c.memoryControl?.injectionEnabled ?? true,
          autoWriteEnabled: c.memoryControl?.autoWriteEnabled ?? true,
          autoExtract: isAutoExtractMode(c.memoryControl?.autoExtract)
            ? c.memoryControl.autoExtract
            : "off",
        },
        // U17 记忆参数原样回传
        memoryTuning: c.memoryTuning ?? null,
        // P3-c：规则表透传（老后端没返 → 空；非法 action 由后端清洗兜底）
        toolRules: Array.isArray(c.toolRules)
          ? c.toolRules.filter(
              (r): r is { tool: string; action: "allow" | "ask" | "deny" } =>
                typeof r?.tool === "string" &&
                (r.action === "allow" || r.action === "ask" || r.action === "deny")
            )
          : [],
        // P3-a Agent 运行参数：后端 None/老版本没返 → 空串（= 内置默认）
        maxRounds: c.maxRounds != null ? String(c.maxRounds) : "",
        historyBudgetChars: c.historyBudgetChars != null ? String(c.historyBudgetChars) : "",
        subagentMaxTurns: c.subagentMaxTurns != null ? String(c.subagentMaxTurns) : "",
        subagentMaxWallSecs: c.subagentMaxWallSecs != null ? String(c.subagentMaxWallSecs) : "",
        searchMaxResults: c.searchMaxResults != null ? String(c.searchMaxResults) : "",
        maxToolOutputChars: c.maxToolOutputChars != null ? String(c.maxToolOutputChars) : "",
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
          // Function 调用熔断上限：空 = 100 默认；非法输入按未配置处理
          maxFunctionCalls: (() => {
            const n = parseInt(c.maxFunctionCalls.trim(), 10);
            return Number.isFinite(n) && n > 0 ? n : null;
          })(),
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
          // max_tokens：空 = 8192 默认；非法输入按未配置处理（后端钳 256..=200000）
          maxTokens: (() => {
            const n = parseInt(c.maxTokens.trim(), 10);
            return Number.isFinite(n) && n > 0 ? n : null;
          })(),
          // 推理强度后台默认（RE-1）：四档抽象档位原样落盘
          reasoningEffort: c.reasoningEffort,
          // ── P3-a Agent 运行参数：空/非法 = null（后端内置默认）；数值合法即落盘，
          //    越界值由后端 bot_set_config 钳制（与 archiveAfterDays 同款）──
          maxRounds: (() => {
            const n = parseInt(c.maxRounds.trim(), 10);
            return Number.isFinite(n) && n > 0 ? n : null;
          })(),
          historyBudgetChars: (() => {
            const n = parseInt(c.historyBudgetChars.trim(), 10);
            return Number.isFinite(n) && n > 0 ? n : null;
          })(),
          subagentMaxTurns: (() => {
            const n = parseInt(c.subagentMaxTurns.trim(), 10);
            return Number.isFinite(n) && n > 0 ? n : null;
          })(),
          subagentMaxWallSecs: (() => {
            const n = parseInt(c.subagentMaxWallSecs.trim(), 10);
            return Number.isFinite(n) && n > 0 ? n : null;
          })(),
          searchMaxResults: (() => {
            const n = parseInt(c.searchMaxResults.trim(), 10);
            return Number.isFinite(n) && n > 0 ? n : null;
          })(),
          maxToolOutputChars: (() => {
            const n = parseInt(c.maxToolOutputChars.trim(), 10);
            return Number.isFinite(n) && n > 0 ? n : null;
          })(),
          // P3-c：规则表透传（空 tool 名/非法 action 由后端 sanitize 清洗）
          toolRules: c.toolRules.length > 0 ? c.toolRules : null,
          // 定时记忆整理：原样透传（后端 serde default 兜底缺字段）
          memoryConsolidation: c.memoryConsolidation,
          // U15 记忆可控开关：原样透传
          memoryControl: c.memoryControl,
          // U17 记忆参数：原样透传（手改 bot-config.json 的值不被整体替换写冲掉）
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
      }
      // 广播给挂件聊天区：头部 🧠 模型标签同步刷新
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

  /** 厂商页保存 + 自动连接测试联动：保存成功后，厂商下有开关为开的模型
   *  才跑一次厂商级探测（同厂商模型共享 Base URL/key，探一次即代表全部）；
   *  结果落盘 verified_vendors（后端），这里 reload 刷绿点并广播给聊天下拉 */
  const [vendorTestBusy, setVendorTestBusy] = useState(false);
  /** null = 未测；true/false = 最近一次自动测试结果 */
  const [vendorTestOk, setVendorTestOk] = useState<boolean | null>(null);
  const [vendorTestMsg, setVendorTestMsg] = useState("");
  const saveVendorPage = async () => {
    if (!(await saveConfig())) return;
    if (!activeVendor || !activeVendorEntries.some((m) => m.enabled !== false)) return;
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

  // ───────── 双协议下大模型列表 handlers ─────────
  // 厂商中心（U10）按厂商分组渲染；老配置可能缺 anthropic/openai 字段，
  // 读取处一律 ?? [] 兜底

  /** 更新大模型条目（label / baseUrl / model 任一字段变化）。
   *  按条目实际所在的协议列表定位——不能用 config.apiProvider（厂商页可停留在
   *  另一协议的厂商上，跨协议编辑会漂移到错的列表/写不进去）。 */
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

  /** 删除大模型条目：确认后移除并立即落盘（同 deleteVendor 语义——
   *  只改内存刷新会复活）；按条目实际所在协议定位（厂商页可停留在另一协议）；
   *  若删的是该协议 active → 列表第一个顶替（无则 null） */
  const deleteModel = async (id: string) => {
    const prov = (["openai", "anthropic"] as const).find((p) =>
      (config.modelsByProvider[p] ?? []).some((m) => m.id === id),
    );
    if (!prov) return;
    if (!window.confirm("删除该模型？删除后立即生效。")) return;
    const list = (config.modelsByProvider[prov] ?? []).filter((m) => m.id !== id);
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

  // ───────── 厂商中心（U10）：按厂商分组渲染，条目仍存双协议列表 ─────────
  // 全部 handler 走 setConfig 函数式（updater 内读最新 c.modelsByProvider）——
  // 连续操作（改 URL→切格式→加模型）不得用外层 render 快照互相覆盖（ocr HIGH）。
  /** 选中厂商页（左栏）；null = 未选（显示引导）。值 = 厂商名 */
  const [activeVendor, setActiveVendor] = useState<string | null>(null);
  /** API Key 显隐（本地态，不落盘） */
  const [showKey, setShowKey] = useState(false);
  /** 厂商头「⋯」菜单开合（浮层/点外部/Esc 收起，同 ApiProviderSelect 模式） */
  const [vendorMenuOpen, setVendorMenuOpen] = useState(false);
  const vendorMenuRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!vendorMenuOpen) return;
    const onDown = (e: MouseEvent) => {
      if (vendorMenuRef.current && !vendorMenuRef.current.contains(e.target as Node))
        setVendorMenuOpen(false);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setVendorMenuOpen(false);
    };
    document.addEventListener("mousedown", onDown);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDown);
      document.removeEventListener("keydown", onKey);
    };
  }, [vendorMenuOpen]);

  /** 条目归属厂商名：有 vendor 用 vendor，老条目按所在列表协议名兜底。
   *  prov 必须传「条目所在列表」的协议——不能用活的 apiProvider（会漂）。
   *  已知边界：兜底名与预设厂商名撞名时二者合并展示（概率低，登记）。 */
  const vendorNameOf = (m: ModelEntry, prov: "openai" | "anthropic") =>
    m.vendor ?? (prov === "anthropic" ? "Anthropic 兼容" : "OpenAI 兼容");

  /** 厂商分组：跨双协议列表聚合；isActiveVendor = 该厂商含全局 active 模型（⋯菜单「当前使用」）。
   *  普通派生而非 useMemo（数据量小；compiler 对含 config 依赖的 memo 化报 warn） */
  const vendorGroups = (() => {
    const map = new Map<string, { name: string; isActiveVendor: boolean }>();
    (["openai", "anthropic"] as const).forEach((prov) => {
      const list = config.modelsByProvider[prov] ?? [];
      const activeId = config.activeModelId[prov] ?? null;
      list.forEach((m) => {
        const name = vendorNameOf(m, prov);
        const g = map.get(name) ?? { name, isActiveVendor: false };
        if (m.id === activeId && config.apiProvider === prov) g.isActiveVendor = true;
        map.set(name, g);
      });
    });
    return [...map.values()].sort((a, b) => a.name.localeCompare(b.name));
  })();

  // U13 收尾引导：配置里有带 vendor 的条目，但 verifiedVendors 为空——
  // 可用性改造（U12）后无自动迁移，需逐厂商点一次插头（连接测试）恢复聊天下拉可见
  const needVendorReVerify =
    ((config.modelsByProvider.openai ?? []).some((m) => m.vendor) ||
      (config.modelsByProvider.anthropic ?? []).some((m) => m.vendor)) &&
    config.verifiedVendors.length === 0;

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

  /** 厂商级 Base URL 记忆（API 格式切换用）：{ 厂商名: { 协议: 上次用的 URL } }，
   *  切换时先把现值记到旧协议名下，再从目标协议恢复（无记忆走已知端点/预设/模型库） */
  const [vendorUrlMem, setVendorUrlMem] = useState<
    Record<string, Partial<Record<"openai" | "anthropic", string>>>
  >({});
  /** 目标协议的种子 URL：已知双协议表 > 预设（协议匹配时）> 模型库默认（端点风格匹配时） */
  const seedVendorUrl = (vendor: string, prov: "openai" | "anthropic"): string | null => {
    const norm = normalizeVendorName(vendor);
    const known = KNOWN_PROTOCOL_URLS[norm]?.[prov];
    if (known) return known;
    const preset = PROVIDER_PRESETS.find((p) => normalizeVendorName(p.name) === norm);
    if (preset?.protocol === prov) return preset.baseUrl;
    const meta = metaProviders?.find(
      (p) => p.provider_key === norm || normalizeVendorName(p.provider_name) === norm,
    );
    if (meta?.default_base_url) {
      const metaIsAnthropic =
        meta.provider_key === "anthropic" || meta.default_base_url.includes("anthropic");
      if (metaIsAnthropic === (prov === "anthropic")) return meta.default_base_url;
    }
    return null;
  };

  /** 厂商 API 格式切换：整厂商条目搬到另一协议列表（active 指针随条目走）；
   *  Base URL 自动跟随目标协议（记忆 > 已知端点 > 保持现值） */
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
    const targetUrl = vendorUrlMem[vendor]?.[prov] || seedVendorUrl(vendor, prov);
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

  /** 删除厂商：移除其全部条目（active 指针兜底）并立即落盘——
   *  确认框语义 = 最终操作，不能只改内存 state（否则刷新/重启后厂商复活） */
  const deleteVendor = async (name: string) => {
    const drop = (list: ModelEntry[], prov: "openai" | "anthropic") =>
      list.filter((m) => vendorNameOf(m, prov) !== name);
    const openai = drop(config.modelsByProvider.openai ?? [], "openai");
    const anthropic = drop(config.modelsByProvider.anthropic ?? [], "anthropic");
    const fix = (prov: "openai" | "anthropic", list: ModelEntry[]) => {
      const cur = config.activeModelId[prov] ?? null;
      if (cur === null) return null;
      return list.some((m) => m.id === cur) ? cur : list[0]?.id ?? null;
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

  /** 厂商总开关：checked = 不在 disabledVendors；切换即落盘（skipReload 同预设应用模式）。
   *  只影响 UI 过滤与展示，不动 bot_set_active_model 的既有语义。 */
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
  /** 当前厂商命中预设时携带的「获取 API Key」控制台地址 */
  const activeVendorKeyUrl =
    PROVIDER_PRESETS.find((p) => p.name === activeVendor)?.keyUrl ?? null;
  /** 当前厂商是否已存 key（key 按厂商名分条目存 keyring） */
  const activeVendorHasKey =
    activeVendor !== null && config.vendorKeys.includes(activeVendor);

  // 切厂商时清空 key 输入框与显隐（key 按厂商分存，输入框不带跨厂商残留）+ 自动测试结果
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

  // ───────── 模型库（内置 Rust meta 模块）对接 ─────────
  /** 模型库服务商列表收敛到白名单（中国厂商 + 头部外国 + OpenRouter） */
  const curateProviders = (ps: MetaProvider[]) =>
    ps.filter((p) => CURATED_PROVIDER_KEYS.has(p.provider_key));

  /** model 分类首次挂载后拉取一次服务商列表（异步回调 setState，非同步路径） */
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

  /** 厂商名 → 模型库记录：先精确匹配（provider_name / provider_key），
   *  再归一化匹配——两段式防兄弟厂商误抢（minimax / minimax-cn 归一化后同名）。
   *  左栏列表与厂商头共用——左栏也要靠它拿 providerKey 解析 logo，
   *  纯名字解析顶不住 models.dev 长显示名（"MiniMax Token Plan (minimax.cn)"） */
  const vendorMetaOf = (name: string): MetaProvider | null => {
    if (!metaProviders) return null;
    const norm = normalizeVendorName(name);
    return (
      metaProviders.find(
        (p) => p.provider_name === name || p.provider_key === name,
      ) ??
      metaProviders.find((p) => normalizeVendorName(p.provider_name) === norm) ??
      null
    );
  };

  /** 当前厂商在模型库里的记录（用于厂商头图标 +「从模型库添加」入口）；服务不可用/无匹配 → null */
  const activeVendorMeta =
    activeVendor !== null ? vendorMetaOf(activeVendor) : null;

  /** 「从模型库添加」自绘下拉（同 ApiProviderSelect 模式：浮层/点外部/Esc 收起） */
  const [metaPickerOpen, setMetaPickerOpen] = useState(false);
  /** 当前厂商在模型库中的模型列表（打开下拉时拉取）；null = 拉取失败 */
  const [metaPickerModels, setMetaPickerModels] = useState<MetaModel[] | null>(null);
  const metaPickerRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!metaPickerOpen) return;
    const onDown = (e: MouseEvent) => {
      if (metaPickerRef.current && !metaPickerRef.current.contains(e.target as Node))
        setMetaPickerOpen(false);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setMetaPickerOpen(false);
    };
    document.addEventListener("mousedown", onDown);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDown);
      document.removeEventListener("keydown", onKey);
    };
  }, [metaPickerOpen]);

  /** 开/收「从模型库添加」下拉；打开时拉该厂商的模型库模型 */
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

  /** 从模型库添加单个模型：追加一条 enabled:false 的 ModelEntry（字段映射同网格点选），立即落盘 */
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
      modelsByProvider: { ...config.modelsByProvider, [proto]: [...list, entry] },
    };
    setConfig(next);
    setMetaPickerOpen(false);
    await saveConfig(next, { skipReload: true });
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
  const setPermMode = async (mode: "strict" | "ask" | "auto" | "yolo") => {
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

  /** U15/U16 记忆可控开关（含抽取档位）：点档即时落盘（同 setConsolidation 模式） */
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

  /** 记忆权限开关行（U15）：role=switch + aria-checked（与厂商总开关同一可访问语义） */
  const renderMemoryToggle = (
    label: string,
    description: string,
    field: "injectionEnabled" | "autoWriteEnabled",
  ) => {
    const on = config.memoryControl[field];
    return (
      <div className="flex items-center justify-between gap-4">
        <div className="min-w-0">
          <p className="text-[11px] text-[var(--t3)]">{label}</p>
          <p className="mt-0.5 text-[11px] text-[var(--t5)]">{description}</p>
        </div>
        <button
          role="switch"
          aria-checked={on}
          aria-label={label}
          className={`shrink-0 min-w-[76px] px-4 py-1.5 text-sm text-[var(--t3)] ${
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

  const clearVendorKey = async () => {
    if (configBusy || !activeVendor) return;
    if (!window.confirm(`清除厂商「${activeVendor}」已保存的 API Key？清除后该厂商的机器人调用将失败。`)) return;
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

      {/* 基础设置：外观 + 开机自启动 wmessage */}
      <div className="nm-card p-5">
        <h2 className="text-lg font-semibold text-[var(--t1)]">基础设置</h2>
        <div className="mt-4 space-y-3">
          {/* 外观（原「深浅色模式」改名：主题是外观的一部分，名字更准确） */}
          <div>
            <p className="text-sm font-medium text-[var(--t2)]">外观</p>
            <div className="mt-3 flex gap-1">
              {(
                [
                  { value: "light", label: "浅色", icon: Sun },
                  { value: "dark", label: "深色", icon: Moon },
                  { value: "system", label: "跟随系统", icon: Monitor },
                ] as { value: ThemeSetting; label: string; icon: LucideIcon }[]
              ).map(({ value, label, icon: Icon }) => (
                <button
                  key={value}
                  className={`px-4 py-1.5 text-sm text-[var(--t3)] inline-flex items-center justify-center gap-1.5 whitespace-nowrap ${
                    theme === value ? "nm-inset" : "nm-outset"
                  }`}
                  onClick={() => onThemeChange(value)}
                >
                  <Icon size={13} aria-hidden className="inline-block align-[-2px]" /> {label}
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

      {/* 聊天气泡外观：用户气泡颜色 + 机器人气泡材质（纯前端偏好，即点即生效） */}
      <BubbleStyleCard />

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
              className={`shrink-0 min-w-[76px] px-4 py-1.5 text-sm text-[var(--t3)] inline-flex items-center justify-center gap-1.5 whitespace-nowrap ${
                exporting ? "nm-inset" : "nm-outset"
              }`}
              onClick={runExport}
              disabled={exporting}
            >
              {exporting ? "导出中…" : (<><Download size={12} aria-hidden /> 导出</>)}
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
              className={`shrink-0 min-w-[76px] px-4 py-1.5 text-sm text-[var(--t3)] inline-flex items-center justify-center gap-1.5 whitespace-nowrap ${
                importing ? "nm-inset" : "nm-outset"
              }`}
              onClick={runImport}
              disabled={importing}
            >
              {importing ? "导入中…" : (<><Upload size={12} aria-hidden /> 导入</>)}
            </button>
          </div>
          {/* 删除非本人的任务卡（2026-10-06 老板需求）：导入带来的归属人为他人的统计用卡，
              一键直接硬删（含回收站中的；本人 = 个人资料 personId，owner_id 为空的卡视为本人不动）。
              后端广播 tasks-updated 三端同步；结果在按钮文字上短暂反馈（不新增行防抖动） */}
          <div className="pt-3 border-t border-[var(--edge)] flex items-center justify-between gap-4">
            <div className="min-w-0">
              <p className="text-sm font-medium text-[var(--t2)]">删除非本人的任务卡</p>
              <p className="mt-1 text-xs text-[var(--t5)]">
                立即删除归属人为他人的全部任务卡（含回收站，不可恢复）；本人卡与未设归属人的卡不受影响
              </p>
            </div>
            <button
              className={`shrink-0 min-w-[110px] px-4 py-1.5 text-sm inline-flex items-center justify-center gap-1.5 whitespace-nowrap ${
                deleteNonSelfBusy ? "nm-inset" : "nm-outset"
              } ${deleteNonSelfDone ? "text-[var(--t4)]" : "text-[var(--danger)]"}`}
              onClick={() => void runDeleteNonSelf()}
              disabled={deleteNonSelfBusy}
            >
              {deleteNonSelfBusy
                ? "删除中…"
                : deleteNonSelfDone
                  ? deleteNonSelfDone
                  : "直接删除"}
            </button>
          </div>
          {/* 任务卡归档时间：完成满 N 天自动归档。前端看板规则与后端 migration
              兜底归档（主窗口关闭时照常到期）同读 bot-config.json，改完阈值两侧一致 */}
          <div className="pt-3 border-t border-[var(--edge)] flex items-center justify-between gap-4">
            <div className="min-w-0">
              <p className="text-sm font-medium text-[var(--t2)]">任务卡归档时间</p>
              <p className="mt-1 text-xs text-[var(--t5)]">
                任务完成超过该天数后自动归档，从「完成」列隐藏；默认 {DEFAULT_ARCHIVE_DAYS} 天，上限 {MAX_ARCHIVE_DAYS} 天
              </p>
            </div>
            <div className="shrink-0 flex items-center gap-2">
              <input
                value={config.archiveAfterDays}
                onChange={(e) => setConfig((c) => ({ ...c, archiveAfterDays: e.target.value }))}
                placeholder={String(DEFAULT_ARCHIVE_DAYS)}
                inputMode="numeric"
                aria-label="任务卡归档天数"
                className="nm-inset w-20 rounded-xl px-3 py-1.5 text-xs text-[var(--t3)] outline-none text-right"
              />
              <span className="text-xs text-[var(--t5)]">天</span>
            </div>
          </div>
        </div>
        {/* 归档天数等 setConfig 字段靠本卡保存钮落盘（同机器人卡模式） */}
        {renderSaveButton("mt-3 flex justify-end")}
      </div>

      {/* P4：执行痕迹清理（trace 三表 + 回滚快照保留期；trace_clear_before 命令） */}
      <div className="nm-card p-5">
        <h2 className="text-lg font-semibold text-[var(--t1)]">执行痕迹</h2>
        <p className="mt-1 text-xs text-[var(--t5)]">
          任务卡/定时/工作流执行的痕迹（工具调用时间线、文件 diff、回滚快照）落本地库，默认保留 30 天
        </p>
        <div className="mt-4 flex items-center justify-between gap-4">
          <div className="min-w-0">
            <p className="text-sm font-medium text-[var(--t2)]">清理执行痕迹</p>
            <p className="mt-1 text-xs text-[var(--t5)]">
              删除「已收尾超过指定天数」与「挂起超过指定天数」的痕迹（含子表与过期回滚快照引用）；不影响任务数据
            </p>
          </div>
          <div className="flex shrink-0 items-center gap-1.5">
            <input
              aria-label="保留天数"
              value={traceRetainDays}
              onChange={(e) => setTraceRetainDays(e.target.value.replace(/\D/g, ""))}
              inputMode="numeric"
              className="w-16 rounded-[var(--r-sm)] nm-inset px-2 py-1 text-right font-mono text-xs text-[var(--t2)] outline-none"
              title="保留天数（1–365，默认 30）"
            />
            <span className="text-[11px] text-[var(--t5)]">天前</span>
            <button
              className={`shrink-0 min-w-[76px] px-4 py-1.5 text-sm text-[var(--t3)] inline-flex items-center justify-center gap-1.5 whitespace-nowrap ${
                traceClearBusy ? "nm-inset" : "nm-outset"
              }`}
              onClick={clearTraces}
              disabled={traceClearBusy}
            >
              {traceClearBusy ? "清理中…" : "清理"}
            </button>
          </div>
        </div>
        {traceClearMsg && <p className="mt-2 text-xs text-[var(--t4)]">{traceClearMsg}</p>}
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
              className={`shrink-0 min-w-[76px] px-4 py-1.5 text-sm text-[var(--t3)] inline-flex items-center justify-center gap-1.5 whitespace-nowrap ${
                exportingWs ? "nm-inset" : "nm-outset"
              }`}
              onClick={runExportWs}
              disabled={exportingWs}
            >
              {exportingWs ? "导出中…" : (<><Download size={12} aria-hidden /> 导出</>)}
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
              className={`shrink-0 min-w-[76px] px-4 py-1.5 text-sm text-[var(--t3)] inline-flex items-center justify-center gap-1.5 whitespace-nowrap ${
                importingWs ? "nm-inset" : "nm-outset"
              }`}
              onClick={runImportWs}
              disabled={importingWs}
            >
              {importingWs ? "导入中…" : (<><Upload size={12} aria-hidden /> 导入</>)}
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

      {/* P3-a/b：Agent 运行参数卡（Agent 透明化设计 §3.3）——
          可调项逐行输入（空 = 内置默认，title 带调参代价说明）；
          系统固定参数折叠区来自 bot_effective_params（硬编码只读 + 当前默认） */}
      <div className="nm-card p-5">
        <h2 className="text-lg font-semibold text-[var(--t1)]">Agent 运行参数</h2>
        <p className="mt-1 text-xs text-[var(--t5)]">
          控制 agent 循环深度、子 agent 预算与搜索行为；留空 = 内置默认。调大 = 更强能力但更多 token 消耗
        </p>
        {/* P4 Profiles 三预设：一键整组切换（写 config 字段 + 落盘；Python 开关是
            独立 flag 文件不在预设内，保守预设的 py 语义 = 严格白名单下弹窗把关） */}
        <div className="mt-3 flex gap-2">
          {(
            [
              [
                "保守",
                "严格白名单 + 默认轮数，慎用自动化",
                {
                  permMode: "strict" as const,
                  maxRounds: "50",
                  maxFunctionCalls: "100",
                  pythonTimeoutSecs: "60",
                  toolRules: [] as Array<{ tool: string; action: "allow" | "ask" | "deny" }>,
                },
              ],
              [
                "标准",
                "默认参数（推荐日常档）",
                {
                  permMode: "ask" as const,
                  maxRounds: "",
                  maxFunctionCalls: "",
                  pythonTimeoutSecs: "",
                  historyBudgetChars: "",
                  subagentMaxTurns: "",
                  subagentMaxWallSecs: "",
                  searchMaxResults: "",
                  maxToolOutputChars: "",
                  toolRules: [] as Array<{ tool: string; action: "allow" | "ask" | "deny" }>,
                },
              ],
              [
                "放开",
                "白名单内全自动 + 高轮数长预算（适合无人值守批量）",
                {
                  permMode: "auto" as const,
                  maxRounds: "200",
                  maxFunctionCalls: "300",
                  pythonTimeoutSecs: "300",
                  toolRules: [] as Array<{ tool: string; action: "allow" | "ask" | "deny" }>,
                },
              ],
            ] as const
          ).map(([label, hint, patch]) => (
            <button
              key={label}
              className="flex-1 rounded-[var(--r-sm)] px-2 py-1.5 text-xs text-[var(--t3)] nm-outset hover:text-[var(--t1)] disabled:opacity-50"
              title={hint}
              disabled={configBusy}
              onClick={() => void saveConfig((c) => ({ ...c, ...patch }))}
            >
              {label}
            </button>
          ))}
        </div>
        {(
          [
            ["maxRounds", "模型循环最大轮数", "默认 50（钳 5–200）；Skill 自报轮数优先", "50"],
            ["maxFunctionCalls", "调用工具上限（全域熔断）", "默认 100（钳 1–500）；单次执行累计调用达到即熔断（软警告在 70% 处）；子 agent 工具调用预算同源此值", "100"],
            ["historyBudgetChars", "会话历史字符预算", "默认 100000（钳 20000–500000）；超出后最旧消息先丢并生成摘要", "100000"],
            ["subagentMaxTurns", "子 agent 轮数预算", "默认 30（硬顶 50）；LLM 派发子 agent 未给预算时用", "30"],
            ["subagentMaxWallSecs", "子 agent 墙钟预算（秒）", "默认 600（钳 30–3600）；从起跑计时，排队不算", "600"],
            ["searchMaxResults", "联网搜索默认条数", "默认 8（钳 1–10）；模型未指定 count 时用", "8"],
            ["maxToolOutputChars", "工具结果截断字符数", "默认不截断（填 0 或留空 = 不截断）；超大输出会挤占上下文，截断行为 P4 接线生效", "不截断"],
          ] as const
        ).map(([field, label, hint, def]) => (
          <div key={field} className="mt-3 flex items-center justify-between gap-4">
            <div className="min-w-0">
              <p className="text-sm font-medium text-[var(--t2)]">{label}</p>
              <p className="mt-0.5 text-[11px] text-[var(--t5)]">{hint}</p>
            </div>
            <input
              aria-label={label}
              title={hint}
              value={config[field]}
              onChange={(e) => setConfig((prev) => ({ ...prev, [field]: e.target.value }))}
              placeholder={def}
              inputMode="numeric"
              className="w-28 shrink-0 rounded-[var(--r-sm)] nm-inset px-2 py-1 text-right font-mono text-xs text-[var(--t2)] outline-none focus:ring-1 focus:ring-[var(--brand)]"
            />
          </div>
        ))}
        {renderSaveButton("mt-4 flex justify-end")}
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
      {/* U15 记忆可控开关：注入总闸 + 模型主动记忆门禁（点档即时落盘） */}
      <div className="nm-card p-5">
        <div className="space-y-3">
          <p className="text-xs font-medium text-[var(--t4)]">记忆权限</p>
          {renderMemoryToggle(
            "聊天注入记忆",
            "关闭后记忆仍会保留，但不再随对话发给模型（隐私总闸）",
            "injectionEnabled",
          )}
          {renderMemoryToggle(
            "模型自动记忆",
            "关闭后模型调用「记住 / 记教训」工具只会收到关闭提示，不写入",
            "autoWriteEnabled",
          )}
          {/* U16 自动记忆抽取三档：会话收尾低频触发；总闸（模型自动记忆）关闭时不跑，
              三档随总闸禁用（radiogroup/radio 与厂商开关的 switch 同一可访问标准） */}
          <div className="space-y-1.5">
            <p className="text-[11px] text-[var(--t3)]">自动记忆抽取</p>
            <p className="text-[11px] text-[var(--t5)]">
              每次聊天结束后低频（≥30 分钟一次）用大模型从对话里提取值得长期记住的偏好与事实；
              需「模型自动记忆」开启；「需确认」档会先存入下方待确认列表
            </p>
            <div className="flex gap-2" role="radiogroup" aria-label="自动记忆抽取">
              {AUTO_EXTRACT_MODES.map(({ value, label, aria }) => {
                const selected = config.memoryControl.autoExtract === value;
                const disabled = configBusy || !config.memoryControl.autoWriteEnabled;
                return (
                  <button
                    key={value}
                    type="button"
                    role="radio"
                    aria-checked={selected}
                    aria-label={aria}
                    className={`flex-1 px-2 py-1.5 text-xs text-[var(--t3)] ${
                      selected ? "nm-inset" : "nm-outset"
                    } ${disabled ? "opacity-50" : ""}`}
                    onClick={() => setMemoryControl({ autoExtract: value })}
                    disabled={disabled}
                  >
                    {label}
                  </button>
                );
              })}
            </div>
          </div>
        </div>
      </div>
      {/* 记忆库管理面板（U14）：列表/搜索/编辑/删除 + 统计与嵌入引擎状态 */}
      <MemoryPanel />
      </section>
      )}
      {mountedSections.has("model") && (
      <section hidden={activeSection !== "model"} className="space-y-4 pt-4">
      {/* U9 页首：说明（原「刷新」按钮删除——bot_get_config 只合顶层字段，
          厂商列表不跟随刷新，点了像没反应，属于误导性按钮） */}
      <p className="text-xs text-[var(--t5)]">
        管理模型供应商，配置后可在挂件聊天时选择使用。
      </p>
      {/* U13 收尾引导：有带 vendor 的条目但 verifiedVendors 为空（可用性状态无自动迁移） */}
      {needVendorReVerify && (
        <p
          className="flex items-center gap-1.5 text-xs text-[var(--t3)]"
          role="note"
          aria-label="厂商可用性引导"
        >
          <Info size={13} className="shrink-0 text-[var(--brand)]" aria-hidden />
          检测到已有厂商模型但均未通过连接测试：升级后需逐厂商点一次插头恢复可用。
        </p>
      )}
      <div className="nm-card p-5">
        {/* 大模型 API 配置（开关开启后显示） */}
        {botEnabled && (
          <div className="space-y-3">
            {/* U10 厂商中心：左厂商列表 + 右厂商页；高度跟随窗口（视口高 - 页头占用），两视图对齐 */}
            <div className="flex h-[calc(100vh-14rem)] min-h-[320px] gap-4">
              {/* 左栏：厂商列表（绿点 = 该厂商含全局 active 模型） */}
              <div className="w-44 shrink-0 space-y-1 overflow-y-auto border-r border-[var(--edge)] pr-3">
                <p className="text-[10px] text-[var(--t5)]">厂商</p>
                {vendorGroups.map((v) => {
                  const meta = vendorMetaOf(v.name);
                  return (
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
                    <ProviderLogo
                      name={v.name}
                      className="h-4 w-4"
                      providerKey={meta?.provider_key}
                      fallbackColor={meta?.fallback_color}
                      fallbackChar={meta?.fallback_char}
                    />
                    <span className="flex-1 truncate">{v.name}</span>
                    {/* 可用性提示点：绿 = 连接测试通过（key/URL/格式变了会自动失效需重测） */}
                    <span
                      className={`h-1.5 w-1.5 shrink-0 rounded-full ${
                        config.verifiedVendors.includes(v.name)
                          ? "bg-[var(--success)]"
                          : "bg-[var(--t6)]"
                      }`}
                      title={
                        config.verifiedVendors.includes(v.name)
                          ? "连接测试已通过，模型可在聊天窗口选用"
                          : "未通过连接测试：点模型行的插头图标测试，通过后模型才进聊天窗口"
                      }
                    />
                  </button>
                  );
                })}
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
              {/* 右栏：厂商页 / 添加厂商网格 / 引导（flex 列：列表区 flex-1 填满剩余高度） */}
              <div className="flex min-h-0 min-w-0 flex-1 flex-col gap-3">
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
                {metaProviders === null || metaProviders.length === 0 ? (
                  <>
                    <div className="flex items-center gap-2">
                      <p className="min-w-0 flex-1 text-[10px] text-[var(--t5)]">
                        模型库为空或同步中——启动时自动同步 models.dev，也可手动同步
                      </p>
                      <button
                        type="button"
                        className="nm-btn shrink-0 px-2.5 py-1.5 text-xs text-[var(--t3)]"
                        title="从 models.dev 回源同步最新厂商与模型数据"
                        onClick={syncMetaLibrary}
                        disabled={metaSyncBusy}
                      >
                        {metaSyncBusy ? "更新中…" : "更新模型库"}
                      </button>
                    </div>
                    {metaSyncMsg && (
                      <p className="text-[10px] text-[var(--t5)]">{metaSyncMsg}</p>
                    )}
                    <div className="grid grid-cols-2 gap-2">
                      {PROVIDER_PRESETS.map((p) => (
                        <button
                          key={p.name}
                          type="button"
                          onClick={() => applyProviderPreset(p)}
                          className="nm-btn flex items-center gap-2 px-3 py-2.5 text-left text-xs text-[var(--t2)]"
                        >
                          <ProviderLogo name={p.name} />
                          <span className="flex-1 truncate">{p.name}</span>
                          <span aria-hidden className="text-[10px] text-[var(--t5)]">
                            {p.protocol === "anthropic" ? "Anthropic 格式" : "OpenAI 格式"}
                          </span>
                        </button>
                      ))}
                    </div>
                  </>
                ) : (
                  <>
                    <div className="flex items-center gap-2">
                      <input
                        value={metaSearch}
                        onChange={(e) => setMetaSearch(e.target.value)}
                        placeholder="搜索厂商…"
                        aria-label="搜索厂商"
                        className="nm-inset min-w-0 flex-1 rounded-lg px-2.5 py-1.5 text-xs text-[var(--t3)] outline-none"
                      />
                      <button
                        type="button"
                        className="nm-btn shrink-0 px-2.5 py-1.5 text-xs text-[var(--t3)]"
                        title="从 models.dev 回源同步最新厂商与模型数据"
                        onClick={syncMetaLibrary}
                        disabled={metaSyncBusy}
                      >
                        {metaSyncBusy ? "更新中…" : "更新模型库"}
                      </button>
                    </div>
                    {metaSyncMsg && (
                      <p className="text-[10px] text-[var(--t5)]">{metaSyncMsg}</p>
                    )}
                    {/* 列数随宽度自适应；flex-1 填满右栏剩余高度，内部滚动保持搜索栏可见 */}
                    <div className="grid min-h-0 flex-1 grid-cols-[repeat(auto-fill,minmax(13rem,1fr))] content-start gap-2 overflow-y-auto">
                      {metaProviders
                        .filter((p) => {
                          const q = metaSearch.trim().toLowerCase();
                          if (!q) return true;
                          return (
                            p.provider_name.toLowerCase().includes(q) ||
                            p.provider_key.toLowerCase().includes(q)
                          );
                        })
                        .map((p) => (
                          <button
                            key={p.provider_key}
                            type="button"
                            onClick={() => applyMetaProvider(p)}
                            disabled={metaApplyBusy}
                            className="nm-btn flex items-center gap-2 px-3 py-2.5 text-left text-xs text-[var(--t2)]"
                          >
                            <ProviderLogo
                              name={p.provider_name}
                              providerKey={p.provider_key}
                              fallbackColor={p.fallback_color}
                              fallbackChar={p.fallback_char}
                              className="h-7 w-7"
                            />
                            <span className="flex-1 truncate">{p.provider_name}</span>
                            <span aria-hidden className="truncate text-[10px] text-[var(--t5)]">
                              {hostOf(p.default_base_url)}
                            </span>
                          </button>
                        ))}
                    </div>
                    {metaProviders.length > 0 &&
                      metaSearch.trim() &&
                      !metaProviders.some((p) => {
                        const q = metaSearch.trim().toLowerCase();
                        return (
                          p.provider_name.toLowerCase().includes(q) ||
                          p.provider_key.toLowerCase().includes(q)
                        );
                      }) && (
                        <p className="text-[10px] text-[var(--t5)]">
                          无匹配厂商，换个关键词试试
                        </p>
                      )}
                  </>
                )}
              </>
            ) : !activeVendor ? (
              <p className="m-auto text-center text-xs text-[var(--t5)]">
                从左侧选择一个厂商，或点「添加厂商」接入新的模型供应商
              </p>
            ) : (
              <>
            {/* 厂商标题行：logo + 名称 ｜ 右侧厂商总开关 +「⋯」菜单（设为当前使用/删除厂商） */}
            <div className="flex items-center justify-between gap-3">
              <div className="flex min-w-0 items-center gap-2">
                <ProviderLogo
                  name={activeVendor}
                  className="h-6 w-6"
                  providerKey={activeVendorMeta?.provider_key}
                  fallbackColor={activeVendorMeta?.fallback_color}
                  fallbackChar={activeVendorMeta?.fallback_char}
                />
                <h3 className="truncate text-base font-semibold text-[var(--t1)]">
                  {activeVendor}
                </h3>
              </div>
              <div className="flex shrink-0 items-center gap-2">
                <Toggle
                  checked={vendorEnabled}
                  onChange={toggleVendorEnabled}
                  ariaLabel={`启用厂商 ${activeVendor}`}
                  size="sm"
                />
                <div ref={vendorMenuRef} className="relative">
                  <button
                    type="button"
                    aria-label={`厂商 ${activeVendor} 更多操作`}
                    aria-haspopup="menu"
                    aria-expanded={vendorMenuOpen}
                    className="nm-icon-btn text-[var(--t5)] hover:text-[var(--t2)]"
                    onClick={() => setVendorMenuOpen((v) => !v)}
                  >
                    <MoreHorizontal size={15} aria-hidden />
                  </button>
                  {vendorMenuOpen && (
                    <div
                      role="menu"
                      className="nm-popover absolute right-0 z-50 mt-1 w-40 p-1 space-y-0.5"
                    >
                      <button
                        type="button"
                        role="menuitem"
                        className="w-full text-left px-3 py-1.5 text-xs rounded-lg text-[var(--t3)] hover:bg-[var(--hover-bg)]"
                        disabled={!activeVendorEntries.length}
                        title="把该厂商的 active 模型设为聊天当前使用的模型"
                        onClick={() => {
                          setVendorMenuOpen(false);
                          makeVendorActive();
                        }}
                      >
                        {vendorOf(activeVendor)?.isActiveVendor
                          ? "当前使用 ✓"
                          : "设为当前使用"}
                      </button>
                      <button
                        type="button"
                        role="menuitem"
                        className="w-full text-left px-3 py-1.5 text-xs rounded-lg text-[var(--danger)] hover:bg-[var(--hover-bg)]"
                        onClick={() => {
                          setVendorMenuOpen(false);
                          if (
                            window.confirm(
                              `删除厂商「${activeVendor}」？其全部模型条目将一并移除。`,
                            )
                          ) {
                            deleteVendor(activeVendor);
                          }
                        }}
                      >
                        删除厂商
                      </button>
                    </div>
                  )}
                </div>
              </div>
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
            {/* API 格式：该厂商条目所在的协议列表（切换 = 条目整体搬移）；自绘下拉同 ApiProviderSelect 模式 */}
            <div className="space-y-1">
              <p className="text-[10px] text-[var(--t5)]">API 格式</p>
              <ApiProviderSelect
                value={activeVendorProtocol}
                onChange={setVendorProtocol}
                options={API_FORMAT_OPTIONS}
              />
            </div>
            {/* API Key：按厂商分存（keyring 条目按厂商名），各厂商独立——挂账记录 */}
            <div className="space-y-1">
              <div className="flex items-center justify-between">
                <p className="text-[10px] text-[var(--t5)]">
                  API Key{activeVendorHasKey && <span className="text-[var(--success)]"> · 已存入系统凭据存储 ✓</span>}
                </p>
                {activeVendorKeyUrl && (
                  <button
                    type="button"
                    className="text-[10px] text-[var(--brand)] hover:underline"
                    onClick={() => {
                      openUrl(activeVendorKeyUrl).catch((e) =>
                        handleCommandError(e, "open url"),
                      );
                    }}
                  >
                    获取 API Key
                  </button>
                )}
              </div>
              <div className="relative">
                <input
                  type={showKey ? "text" : "password"}
                  value={keyInput}
                  onChange={(e) => setKeyInput(e.target.value)}
                  placeholder={activeVendorHasKey ? "已保存（输入新 Key 可覆盖）" : "sk-…"}
                  className="nm-inset w-full rounded-xl px-3 py-2 pr-9 text-xs text-[var(--t3)] outline-none"
                />
                <button
                  type="button"
                  aria-label={showKey ? "隐藏 API Key" : "显示 API Key"}
                  className="absolute right-2 top-1/2 -translate-y-1/2 text-[var(--t5)] hover:text-[var(--t2)]"
                  onClick={() => setShowKey((v) => !v)}
                >
                  {showKey ? <EyeOff size={13} aria-hidden /> : <Eye size={13} aria-hidden />}
                </button>
              </div>
              {activeVendorHasKey && (
                <button
                  className="text-[10px] text-[var(--danger)] hover:underline"
                  onClick={clearVendorKey}
                >
                  清除已保存的 Key
                </button>
              )}
            </div>
            {/* 模型列表：行 = 名称 + 上下文徽标 + 能力徽标 + 插头（连接测试）+ 铅笔编辑 + 开关；flex-1 与添加厂商网格同高对齐 */}
            <div className="flex min-h-0 flex-1 flex-col gap-1">
              <div className="flex items-center justify-between">
                <p className="text-[10px] text-[var(--t5)]">模型列表</p>
                <div className="flex items-center gap-1.5">
                  {activeVendorMeta && (
                    <div ref={metaPickerRef} className="relative">
                      <button
                        type="button"
                        aria-haspopup="listbox"
                        aria-expanded={metaPickerOpen}
                        className="nm-btn px-2.5 py-1 text-[10px] text-[var(--t3)]"
                        title="从模型库挑选该厂商的模型，自动填充上下文与推理参数；新模型默认禁用"
                        onClick={toggleMetaPicker}
                      >
                        从模型库添加 ▾
                      </button>
                      {metaPickerOpen && (
                        <div
                          role="listbox"
                          aria-label="从模型库添加"
                          className="nm-popover absolute right-0 z-50 mt-1 max-h-56 w-64 space-y-0.5 overflow-y-auto p-1"
                        >
                          {(() => {
                            const added = new Set(
                              activeVendorEntries.map((m) => m.model),
                            );
                            const options = (metaPickerModels ?? []).filter(
                              (m) =>
                                !added.has(
                                  m.model_key.includes("/")
                                    ? m.model_key.slice(m.model_key.indexOf("/") + 1)
                                    : m.model_key,
                                ),
                            );
                            if (metaPickerModels === null)
                              return (
                                <p className="px-3 py-1.5 text-[10px] text-[var(--t5)]">
                                  模型库服务不可用
                                </p>
                              );
                            if (options.length === 0)
                              return (
                                <p className="px-3 py-1.5 text-[10px] text-[var(--t5)]">
                                  模型库中的模型均已添加
                                </p>
                              );
                            return options.map((m) => (
                              <button
                                key={m.model_key}
                                type="button"
                                role="option"
                                aria-selected={false}
                                className="flex w-full items-center gap-2 rounded-lg px-3 py-1.5 text-left text-xs text-[var(--t3)] hover:bg-[var(--hover-bg)]"
                                onClick={() => addMetaModel(m)}
                              >
                                <span className="min-w-0 flex-1 truncate">
                                  {m.display_name || m.model_key}
                                </span>
                                {m.context_length ? (
                                  <span
                                    aria-hidden
                                    className="shrink-0 font-mono text-[10px] text-[var(--t5)]"
                                  >
                                    {Math.round(m.context_length / 100) / 10}K
                                  </span>
                                ) : null}
                              </button>
                            ));
                          })()}
                        </div>
                      )}
                    </div>
                  )}
                  <button
                    type="button"
                    className="nm-btn px-2.5 py-1 text-[10px] text-[var(--t3)]"
                    onClick={addModelToVendor}
                  >
                    ＋ 添加模型
                  </button>
                </div>
              </div>
              {activeVendorEntries.length === 0 ? (
                <p className="py-1 text-[11px] text-[var(--t5)]">
                  暂无模型，点「＋ 添加模型」开始添加
                </p>
              ) : (
                <div className="min-h-0 flex-1 space-y-1.5 overflow-y-auto">
                  {activeVendorEntries.map((m) => (
                    <ModelRow
                      key={m.id}
                      model={m}
                      apiProvider={activeVendorProtocol}
                      vendorDisabled={!vendorEnabled}
                      vendorVerified={config.verifiedVendors.includes(activeVendor ?? "")}
                      onChange={(patch) => updateModel(m.id, patch)}
                      onDelete={() => deleteModel(m.id)}
                      onTested={async () => {
                        // 可用性已落盘：await reload 拿到最新 verifiedVendors 再广播——
                        // 否则旧 state 的下一次整写会把刚落盘的验证状态覆盖掉
                        await loadConfig();
                        emit("bot-config-changed", null).catch(() => {});
                      }}
                    />
                  ))}
                </div>
              )}
              <p className="text-[10px] text-[var(--t6)] leading-snug">
                插头 = 连接测试：通过后厂商点亮绿点、开启的模型才进聊天窗口下拉（key/URL/格式变更后需重新测试）。当前使用的模型在聊天窗口 🧠 下拉切换；max_tokens 在各模型的编辑里设置（仅 Anthropic 格式生效）。
              </p>
            </div>
            {/* 全局 max_tokens 输入已删（每模型编辑里都有，页底重复）；BotConfig.maxTokens
                仍作条目留空时的兜底层透传保存（条目 > 全局 > 8192 默认），无 UI 入口 */}
            {/* 保存 + 自动连接测试：保存成功后厂商下有开启的模型即自动探测，
                结果决定左栏绿点与聊天下拉可见性 */}
            <div className="mt-3 flex items-center justify-end gap-3">
              {botError && (
                <p className="text-[10px] text-[var(--danger)]">{botError}</p>
              )}
              {vendorTestMsg && (
                <p
                  className={`text-[10px] ${
                    vendorTestOk ? "text-[var(--success)]" : "text-[var(--danger)]"
                  }`}
                >
                  {vendorTestMsg}
                </p>
              )}
              <button
                aria-label="保存配置"
                className={`shrink-0 min-w-[76px] px-4 py-1.5 text-sm text-[var(--t3)] inline-flex items-center justify-center gap-1.5 whitespace-nowrap ${
                  configBusy || vendorTestBusy ? "nm-inset" : "nm-outset"
                }`}
                onClick={saveVendorPage}
                disabled={configBusy || vendorTestBusy}
              >
                {configSaved
                  ? (<>
                      <Check size={12} aria-hidden /> 已保存
                    </>)
                  : configBusy
                    ? "保存中…"
                    : vendorTestBusy
                      ? "测试连接中…"
                      : "保存配置"}
              </button>
            </div>
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
                    ["auto", "白名单内自动"],
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
                {config.permMode === "auto" &&
                  "白名单内的文件操作全自动（含覆盖写免确认）——定时/工作流无人值守写文件不再失败；白名单外仍弹窗授权。"}
                {config.permMode === "yolo" &&
                  "⚠️ 不弹任何授权：机器人可读本机任意文件，且 Python 编程免开关直接执行（以本机用户权限，可联网）。仅在你完全信任所用模型时开启。"}
              </p>
            </div>
            {/* P3-c：per-tool 权限规则表（deny/ask/allow 首中即停；无命中走授权模式） */}
            <div className="space-y-1">
              <div className="flex items-center justify-between">
                <p className="text-[10px] text-[var(--t5)]">工具权限规则（按工具覆盖授权模式）</p>
                <button
                  className="nm-btn px-2 py-0.5 text-[10px] text-[var(--t3)]"
                  onClick={() =>
                    setConfig((c) => ({
                      ...c,
                      toolRules: [...c.toolRules, { tool: "", action: "deny" as const }],
                    }))
                  }
                >
                  + 添加规则
                </button>
              </div>
              {config.toolRules.length === 0 ? (
                <p className="text-[10px] text-[var(--t6)]">
                  例：deny run_python 禁用编程工具；ask edit_file 每次编辑都确认。规则优先于上方授权模式。
                </p>
              ) : (
                <div className="space-y-1">
                  {config.toolRules.map((r, i) => (
                    <div key={i} className="flex items-center gap-1.5">
                      {/* oxlint-disable-next-line react/no-array-index-key */}
                      <input
                        value={r.tool}
                        placeholder="工具名，如 run_python"
                        onChange={(e) =>
                          setConfig((c) => ({
                            ...c,
                            toolRules: c.toolRules.map((x, j) =>
                              j === i ? { ...x, tool: e.target.value } : x
                            ),
                          }))
                        }
                        className="min-w-0 flex-1 rounded-lg nm-inset px-2 py-1 font-mono text-[11px] text-[var(--t2)] outline-none"
                      />
                      <select
                        aria-label={`规则动作 ${r.tool || i + 1}`}
                        value={r.action}
                        onChange={(e) =>
                          setConfig((c) => ({
                            ...c,
                            toolRules: c.toolRules.map((x, j) =>
                              j === i
                                ? { ...x, action: e.target.value as "allow" | "ask" | "deny" }
                                : x
                            ),
                          }))
                        }
                        className="shrink-0 rounded-lg nm-inset px-1.5 py-1 text-[11px] text-[var(--t3)]"
                      >
                        <option value="allow">allow 放行</option>
                        <option value="ask">ask 确认</option>
                        <option value="deny">deny 禁用</option>
                      </select>
                      <button
                        aria-label={`删除规则 ${r.tool || i + 1}`}
                        className="shrink-0 px-1 text-sm text-[var(--t5)] hover:text-[var(--danger)]"
                        onClick={() =>
                          setConfig((c) => ({
                            ...c,
                            toolRules: c.toolRules.filter((_, j) => j !== i),
                          }))
                        }
                      >
                        ×
                      </button>
                    </div>
                  ))}
                </div>
              )}
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
                <TriangleAlert size={10} aria-hidden className="inline-block align-[-1px]" /> Tavily 与 Brave 只能开启一个，请关闭其中一个。
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
                  className="nm-btn shrink-0 px-3 py-2 text-xs text-[var(--t3)] inline-flex items-center gap-1 whitespace-nowrap"
                  onClick={copyToken}
                >
                  {copied ? (
                    <>
                      <Check size={12} aria-hidden /> 已复制
                    </>
                  ) : (
                    "复制"
                  )}
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
      {mountedSections.has("workflow") && (
      <section hidden={activeSection !== "workflow"} className="space-y-4 pt-4">
      {/* P3-b 迁移：熔断上限已入「Agent 运行参数」卡，本卡只剩可见开关与拆解提示词 */}
      <WorkflowSettingsCard />
      </section>
      )}
      {mountedSections.has("graph") && (
      <section hidden={activeSection !== "graph"} className="space-y-4 pt-4">
      <GraphSettingsPanel />
      </section>
      )}
      {mountedSections.has("desk") && (
      <section hidden={activeSection !== "desk"} className="space-y-4 pt-4">
      <MigrationPanel />
      </section>
      )}
      {mountedSections.has("tokens") && (
      <section hidden={activeSection !== "tokens"} className="space-y-4 pt-4">
      {/* P3-b：词元统计实装（原「规划中」占位）——exec_traces 按日聚合，纯本地 */}
      <UsageStatsCard />
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

/** 工作流设置卡（W1-CANVAS §10 + W2-DECOMPOSE §6.3）：可见性开关 + 拆解提示词。
 *  提示词两段式：这里编辑的是「指引段」；「输出契约段」在后端代码硬拼，
 *  用户改指引段破坏不了 JSON 契约。 */
function WorkflowSettingsCard() {
  const [showTasks, setShowTasks] = useState(getShowWorkflowTasks);
  const [guidance, setGuidance] = useState(getDecomposeGuidance);
  const [guidanceSaved, setGuidanceSaved] = useState(false);
  const savedTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const guidanceRef = useRef(guidance);
  // W10：验收与审计设置（后端 workflow_settings 表，运行期读取）
  const [nodeAcceptance, setNodeAcceptance] = useState<boolean | null>(null);
  const [acceptBusy, setAcceptBusy] = useState(false);
  const [retention, setRetention] = useState<number | null>(null);
  const [retentionDraft, setRetentionDraft] = useState("");
  const [auditMsg, setAuditMsg] = useState<string | null>(null);
  // W11：轻量评审模型（模型库条目 id；空 = 跟随全局）
  const [reviewModel, setReviewModel] = useState("");
  const [modelBusy, setModelBusy] = useState(false);
  const [modelOptions, setModelOptions] = useState<Array<{ id: string; label: string }>>([]);
  useEffect(() => {
    getWorkflowSettings()
      .then((s) => {
        setNodeAcceptance(s.nodeAcceptance);
        setRetention(s.auditRetentionRuns);
        setRetentionDraft(String(s.auditRetentionRuns));
        setReviewModel(s.reviewModel ?? "");
      })
      .catch((e) => setAuditMsg(String(e)));
    // 模型库下拉（与画布每卡模型同源：bot_get_config，停用条目不进列表）
    invoke<{
      modelsByProvider?: {
        openai?: Array<{ id: string; label: string; enabled?: boolean }>;
        anthropic?: Array<{ id: string; label: string; enabled?: boolean }>;
      } | null;
    }>("bot_get_config")
      .then((c) => {
        const m = c.modelsByProvider;
        setModelOptions(
          [...(m?.openai ?? []), ...(m?.anthropic ?? [])]
            .filter((e) => e.enabled !== false)
            .map((e) => ({ id: e.id, label: e.label }))
        );
      })
      .catch(() => {}); // 模型库读取失败：下拉只剩"跟随全局"，不挡设置页
  }, []);
  // 卸载时兜底持久化（OCR r1：焦点在 textarea 时切走/关窗，onBlur 可能不触发）
  useEffect(
    () => () => {
      setDecomposeGuidance(guidanceRef.current);
      if (savedTimerRef.current) clearTimeout(savedTimerRef.current);
    },
    []
  );
  useEffect(() => {
    guidanceRef.current = guidance;
  }, [guidance]);
  useEffect(() => {
    const un = listen(WORKFLOW_VISIBILITY_EVENT, () =>
      setShowTasks(getShowWorkflowTasks())
    );
    return () => {
      un.then((f) => f());
    };
  }, []);
  const persistGuidance = (v: string) => {
    const ok = setDecomposeGuidance(v);
    setGuidanceSaved(ok);
    if (ok) {
      // 已保存提示 2.5s 自动隐藏（OCR r1：不能一旦保存过就常亮）
      if (savedTimerRef.current) clearTimeout(savedTimerRef.current);
      savedTimerRef.current = setTimeout(() => setGuidanceSaved(false), 2500);
    }
  };
  return (
    <div className="space-y-4">
      <div className="nm-card p-5">
        <h2 className="text-lg font-semibold text-[var(--t1)]">工作流任务</h2>
        <p className="mt-1 text-xs text-[var(--t5)]">
          工作流画布的节点卡默认只在「工作流」栏目显示。开启后它们也会出现在首页看板、归档与挂件清单里。
        </p>
        <div className="mt-4 flex items-center justify-between gap-4">
          <div className="min-w-0">
            <p className="text-sm font-medium text-[var(--t2)]">在看板中显示工作流任务</p>
            <p className="mt-1 text-xs text-[var(--t5)]">
              关闭时工作流节点卡只在画布编辑、执行与查看结果；开关改完即时生效
            </p>
          </div>
          <button
            role="switch"
            aria-checked={showTasks}
            aria-label="在看板中显示工作流任务"
            className={`shrink-0 min-w-[76px] px-4 py-1.5 text-sm text-[var(--t3)] ${
              showTasks ? "nm-inset" : "nm-outset"
            }`}
            onClick={() => {
              const next = !showTasks;
              setShowTasks(next);
              setShowWorkflowTasks(next);
            }}
          >
            {showTasks ? "已开启" : "已关闭"}
          </button>
        </div>
      </div>
      {/* P3-b 迁移：调用工具上限（全域熔断）已并入「Agent 运行参数」卡（机器人 section）——
          它管的是全域所有 agent 执行（含普通聊天），挂在工作流 section 语义错位 */}
      <div className="nm-card p-5">
        <h2 className="text-lg font-semibold text-[var(--t1)]">验收与审计（W10）</h2>
        <p className="mt-1 text-xs text-[var(--t5)]">
          节点级验收：执行成功的卡对照验收标准做轻量核查，不达标带证据自动返工（每卡最多
          2 次，仍不达标标记失败）。审计：每次运行按 run 记录调度/验收/评审时间线，可在执行详情的「运行审计」页签查看。
        </p>
        <div className="mt-4 flex items-center justify-between gap-4">
          <div className="min-w-0">
            <p className="text-sm font-medium text-[var(--t2)]">节点级验收</p>
            <p className="mt-1 text-xs text-[var(--t5)]">
              关闭后节点执行成功即完成，不做质量核查（验收调用会消耗额外 token）
            </p>
          </div>
          <button
            role="switch"
            aria-checked={nodeAcceptance === true}
            aria-label="节点级验收"
            disabled={nodeAcceptance === null || acceptBusy}
            className={`shrink-0 min-w-[76px] px-4 py-1.5 text-sm text-[var(--t3)] disabled:opacity-50 ${
              nodeAcceptance ? "nm-inset" : "nm-outset"
            }`}
            onClick={async () => {
              if (acceptBusy) return; // in-flight 保护：连点不并发落盘（OCR r1 high）
              setAcceptBusy(true);
              const next = !(nodeAcceptance === true);
              try {
                const s = await setWorkflowSettings({ nodeAcceptance: next });
                setNodeAcceptance(s.nodeAcceptance);
              } catch (e) {
                setAuditMsg(String(e));
              } finally {
                setAcceptBusy(false);
              }
            }}
          >
            {nodeAcceptance ? "已开启" : "已关闭"}
          </button>
        </div>
        <div className="mt-4 flex items-center justify-between gap-4">
          <div className="min-w-0">
            <p className="text-sm font-medium text-[var(--t2)]">轻量评审模型</p>
            <p className="mt-1 text-xs text-[var(--t5)]">
              拆解前澄清与节点级验收核查用（这两步只是判断，不需要最强模型）；条目删除后自动回落全局模型
            </p>
          </div>
          <select
            aria-label="轻量评审模型"
            className="shrink-0 max-w-[180px] rounded-[var(--r-sm)] bg-transparent px-2 py-1 text-xs nm-inset text-[var(--t3)] disabled:opacity-50"
            disabled={modelBusy}
            value={reviewModel}
            onChange={async (e) => {
              if (modelBusy) return; // in-flight 守卫：连改下拉不并发落盘（OCR r1 high）
              setModelBusy(true);
              const v = e.target.value;
              try {
                // 以服务端返回值为准（不用乐观更新——落盘失败时 UI 不展示未持久化值）
                const st = await setWorkflowSettings({ reviewModel: v });
                setReviewModel(st.reviewModel ?? "");
              } catch (err) {
                setAuditMsg(String(err));
              } finally {
                setModelBusy(false);
              }
            }}
          >
            <option value="">⚙️ 跟随全局模型</option>
            {reviewModel && !modelOptions.some((m) => m.id === reviewModel) && (
              <option value={reviewModel}>{reviewModel}（条目已删除）</option>
            )}
            {modelOptions.map((m) => (
              <option key={m.id} value={m.id}>
                {m.label}
              </option>
            ))}
          </select>
        </div>
        <div className="mt-4 flex items-center justify-between gap-4">
          <div className="min-w-0">
            <p className="text-sm font-medium text-[var(--t2)]">审计保留次数</p>
            <p className="mt-1 text-xs text-[var(--t5)]">
              每个工作流保留最近 N 次运行的审计记录（5–100，超出自动清理）
            </p>
          </div>
          <div className="flex shrink-0 items-center gap-2">
            <input
              aria-label="审计保留次数"
              className="w-16 rounded-[var(--r-sm)] px-2 py-1 text-right text-sm nm-inset text-[var(--t1)] outline-none"
              inputMode="numeric"
              value={retentionDraft}
              onChange={(e) => setRetentionDraft(e.target.value.replace(/\D/g, ""))}
              onBlur={async () => {
                const v = parseInt(retentionDraft, 10);
                if (!Number.isFinite(v) || v === retention) return;
                try {
                  const s = await setWorkflowSettings({ auditRetentionRuns: v });
                  setRetention(s.auditRetentionRuns);
                  setRetentionDraft(String(s.auditRetentionRuns));
                } catch (e) {
                  setAuditMsg(String(e));
                }
              }}
            />
            <span className="text-xs text-[var(--t5)]">次 run</span>
          </div>
        </div>
        <div className="mt-4 flex items-center justify-between gap-4">
          <div className="min-w-0">
            <p className="text-sm font-medium text-[var(--t2)]">清空审计</p>
            <p className="mt-1 text-xs text-[var(--t5)]">
              删除所有工作流的审计记录（不影响任务卡与执行结果本身）
            </p>
          </div>
          <button
            className="shrink-0 nm-outset rounded-[var(--r-sm)] px-3 py-1.5 text-xs text-[var(--t3)]"
            onClick={async () => {
              if (!window.confirm("确定清空所有工作流的审计记录？此操作不可撤销。")) return;
              try {
                const n = await clearAllWorkflowAudit();
                setAuditMsg(`已清空 ${n} 条审计记录`);
              } catch (e) {
                setAuditMsg(String(e));
              }
            }}
          >
            清空全部
          </button>
        </div>
        {auditMsg && <p className="mt-2 text-xs text-[var(--t4)]">{auditMsg}</p>}
      </div>
      <div className="nm-card p-5">
        <h2 className="text-lg font-semibold text-[var(--t1)]">AI 拆解提示词</h2>
        <p className="mt-1 text-xs text-[var(--t5)]">
          「AI 生成」拆解目标时给模型的指引（拆解风格、粒度要求、领域偏好）。输出 JSON
          格式约束由系统强制追加，不在此处、也无需配置。
        </p>
        <textarea
          aria-label="AI 拆解提示词"
          className="mt-3 h-40 w-full resize-y rounded-[var(--r-sm)] nm-inset p-3 text-sm leading-6 text-[var(--t1)] outline-none"
          value={guidance}
          maxLength={MAX_GUIDANCE_CHARS}
          onChange={(e) => {
            setGuidance(e.target.value);
            setGuidanceSaved(false);
          }}
          onBlur={() => persistGuidance(guidance)}
        />
        <div className="mt-3 flex items-center justify-end gap-2">
          {guidanceSaved && (
            <span className="text-xs text-[var(--t5)]">已保存</span>
          )}
          <button
            className="nm-outset rounded-[var(--r-sm)] px-3 py-1 text-xs text-[var(--t3)]"
            onClick={() => {
              const d = resetDecomposeGuidance();
              setGuidance(d);
              persistGuidance(d);
            }}
          >
            恢复默认
          </button>
        </div>
      </div>
    </div>
  );
}
