// SettingsPage 子模块：纯类型定义 + 工具函数（无 React / 无 IO）。
// 由 SettingsPage 目录内其他子文件 import，外部不直接引用；
// 例外：lib/botConfig.ts 的 BotConfigView（bot_get_config 线格式单源）
// 引用 ModelEntry / McpServerConfig——线格式本就与 wire 同形。

import type { ApiProvider, AutoExtractMode, UiFontSize } from "./constants";

import type { MetaModel } from "../../lib/modelMeta";

/** 单个大模型条目：label / baseUrl / model 三元组 + 稳定 id。
 *  id 是前端 crypto.randomUUID() 生成的字符串，仅用于 React key + 标识 active，
 *  不参与 API 调用。 */
export type ModelEntry = {
  id: string;
  label: string;
  baseUrl: string;
  model: string;
  /** 所属厂商名（ 厂商中心）：老配置缺省 → 前端按协议名兜底分组 */
  vendor?: string;
  /** false = 聊天 🧠 下拉不显示；老配置缺省 = 启用 */
  enabled?: boolean;
  /** 上下文窗口（千 token），徽标显示「204.8K」样式；缺省不显示 */
  contextK?: number;
  /** 每模型推理参数（模型库预填或手填）；缺省 = 跟随全局/默认。
   *  camelCase 与 Rust 端 serde 对齐（temperature/top_p/max_tokens/system_prompt） */
  temperature?: number;
  topP?: number;
  maxTokens?: number;
  systemPrompt?: string;
};

/** 双协议下各自的模型列表：设置页协议切换时整体切换显示；
 *  新增的 ModelEntry 落在当前 apiProvider 协议下。 */
export type ModelsByProvider = {
  openai: ModelEntry[];
  anthropic: ModelEntry[];
};

/** 双协议下各自的 active 模型 id：null = 该协议还没选 active。 */
export type ActiveModelId = {
  openai: string | null;
  anthropic: string | null;
};

/** crypto.randomUUID 的安全包装——老浏览器/Tauri webview 偶发缺 crypto 时回退
 *  到时间戳拼随机数（id 唯一性足够即可，碰撞概率 < 1e-10） */
export function genModelId(): string {
  if (
    typeof crypto !== "undefined" &&
    typeof crypto.randomUUID === "function"
  ) {
    return crypto.randomUUID();
  }
  return `m-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 8)}`;
}

export type ApiStatus = {
  enabled: boolean;
  port: number;
  /// disabled 状态下后端用 `skip_serializing_if` 剔除字段,TS 端拿 undefined
  /// (与 Rust 端 `Option<String>` + `skip_serializing_if = "Option::is_none"` 同步)
  token?: string;
};

export type SkillOutcomeKind =
  "done" | "await_user" | "failed_recoverable" | "terminated";
export type SkillOutcome = {
  skillName: string;
  kind: SkillOutcomeKind;
  reason?: string;
  completedSummary?: string;
  rollbackAttempted?: boolean;
  lastAtMs: number;
};
export type SkillInfo = {
  name: string;
  description: string;
  /** frontmatter enabled：关掉后技能不进清单、模型不可调用（缺省 = 启用） */
  enabled?: boolean;
  lastOutcome?: SkillOutcome | null;
  /** frontmatter version（可选） */
  version?: string | null;
  /** 引用了未内置工具的清单（空数组/缺省 = 全部兼容，设置页标红提示） */
  unknownTools?: string[];
};

// MCP（外部工具服务器）

/** 单个外部 MCP 服务器配置（与 Rust bot::mcp::config::McpServerConfig 同形，camelCase） */
/** bot_effective_params 返回的一行（与 Rust bot::params::ParamView 同形，camelCase）。
 *
 *  字段口径：
 *  - value = **当前生效值**（已被钳制过的那个数，不是用户填的原值）
 *  - default = 内置默认值（后端从 resolver 派生，与运行时同一份代码路径）
 *  - source = "config"（用户改过）/ "default"（还在用内置默认）/ "hardcoded"（只读常量）
 *  - min/max = 可填范围（与运行时钳制边界同一个数） */
export type ParamView = {
  key: string;
  label: string;
  category: string;
  hint: string;
  value: string;
  default: string;
  source: "config" | "default" | "hardcoded";
  editable: boolean;
  min?: number | null;
  max?: number | null;
};
export type McpServerConfig = {
  /** "" = 新建（后端生成 uuid） */
  id: string;
  name: string;
  /** "stdio" | "http" */
  transport: string;
  /** stdio：启动器（后端白名单校验） */
  command?: string | null;
  args?: string[];
  /** stdio：环境变量（明文存本机 bot-config.json） */
  env?: Record<string, string>;
  /** http：端点 URL */
  url?: string | null;
  /** http：随每个请求发送的自定义头（鉴权头等；明文存本机配置文件） */
  headers?: Record<string, string>;
  /** 单次工具调用超时秒数；null/undefined = 后端默认 60（钳 5..=600） */
  timeoutSecs?: number | null;
  enabled: boolean;
};

/** mcp_server_secrets_masked 返回：MCP 机密脱敏视图（键名 + 掩码值）。
 * 明文不出宿主；掩码值原样回传 = 「这一项保留原值」。 */
export type McpSecretsMasked = {
  /** 掩码占位符（提示文案用；值本身就是这个串 = 保留原值） */
  mask: string;
  env: Record<string, string>;
  headers: Record<string, string>;
};

/** mcp_status 返回：配置 × 连接槽合并视图 */
export type McpServerStatus = {
  id: string;
  name: string;
  transport: string;
  enabled: boolean;
  /** "connected"（调用中）| "idle"（静默待命）| "pending"（待发现工具）
   *  | "down"（失败）| "absent"（未启用 / 还没轮到） */
  state: string;
  error?: string | null;
  toolCount: number;
  /** 内置条目（Playwright）：不可删除，只能开关 */
  builtin?: boolean;
  /** 本地子进程 pid（stdio 连接才有；HTTP 连接没有子进程） */
  pid?: number | null;
  /** 常驻内存（MB，整棵进程树合计） */
  rssMb?: number | null;
  /** CPU 占用率（百分比）；第一次采样没有前值时为 null */
  cpuPct?: number | null;
  /** 累计 CPU 时间（秒） */
  cpuSeconds?: number | null;
  /** 子进程已运行秒数 */
  uptimeSecs?: number | null;
  /** 子进程树在监听的 socket（TCP LISTEN / UDP 已绑定；整棵树合计） */
  listening?: { protocol: string; address: string; port: number }[];
  /** 本平台能不能采到监听 socket；false = 面板说明「本平台不支持采集」
   * （采空不等于没人监听，那是**没判**） */
  listeningSupported?: boolean;
  /** 本平台能不能采到进程资源占用；false = 同上（内存 / CPU 恒空不是没占用） */
  resourceSupported?: boolean;
};

/** 服务器已发现的工具（名称 + 描述，设置页展示用） */
export type McpToolBrief = { name: string; description?: string | null };

/** stdio 启动器白名单（与 Rust STDIO_LAUNCH_ALLOWLIST 同步——保存前后端双重把关） */
/** 单个 per-onion 配置档（与 Rust bot_onions::OnionSummary 同形，camelCase） */
export type OnionSummary = {
  /** 配置档名（= onions/<name>.yaml，不含扩展名） */
  name: string;
  role: string;
  profile: string;
  /** yaml.allowed 条数（实际可用还要与 profile 白名单取交集） */
  allowedCount: number;
  maxTurn?: number | null;
  /** 该文件坏在哪（读不到 / YAML 语法错 / 字段不合法）；null = 可用 */
  broken?: string | null;
  /** 是否内置配置档（首次启动播种，启动时不会被覆盖） */
  builtin: boolean;
};

export const MCP_STDIO_LAUNCHERS = [
  "npx",
  "bunx",
  "uvx",
  "pipx",
  "node",
  "deno",
  "python",
  "python3",
  "docker",
  "podman",
] as const;


/** 供应商预设（添加供应商网格）：点击 = 预填 baseUrl/模型列表（全局无 active
 *  时连协议+active 一起接管，见 applyVendorModels），完全落在现有双协议数据
 *  模型内（同一协议仅一份配置，预设会覆盖该协议当前同厂商条目——网格页注明）。
 *  模型名用户可改。
 *  keyUrl = 厂商控制台的 API Key 获取页（详情页「获取 API Key」外链）。 */
type PresetModel = { id: string; contextK?: number };
export type ProviderPreset = {
  name: string;
  protocol: "openai" | "anthropic";
  baseUrl: string;
  keyUrl: string;
  models: PresetModel[];
};
/** 模型库模型 → ModelEntry：model 去掉 "provider/" 前缀（API 调用 id 取后段）、
 *  contextK = context_length/1000 四舍五入一位小数；推理参数仅非空才带（不写 undefined 字段） */
export function metaModelToEntry(
  m: MetaModel,
  vendor: string,
  baseUrl: string,
): ModelEntry {
  const slash = m.model_key.indexOf("/");
  return {
    id: genModelId(),
    label: m.display_name || m.model_key,
    model: slash >= 0 ? m.model_key.slice(slash + 1) : m.model_key,
    baseUrl,
    vendor,
    ...(m.context_length
      ? { contextK: Math.round(m.context_length / 100) / 10 }
      : {}),
    ...(m.temperature != null ? { temperature: m.temperature } : {}),
    ...(m.top_p != null ? { topP: m.top_p } : {}),
    ...(m.max_tokens != null ? { maxTokens: m.max_tokens } : {}),
    ...(m.default_system_prompt
      ? { systemPrompt: m.default_system_prompt }
      : {}),
  };
}
/** 模型库网格右侧标注：默认 base URL 的域名（无法解析 → 空串不显示） */
export function hostOf(url: string | null): string {
  if (!url) return "";
  try {
    return new URL(url).host;
  } catch {
    return "";
  }
}
/** 设置分类：key 顺序 = 侧栏顺序。
 *  「机器人」section 在源码中三段出现（同名同亮）：卡1 开关/Python、
 *  卡2 授权/白名单、卡3 技能路由/外部 API；「MCP 服务」两段：Tavily/Brave
 *  搜索引擎卡 + McpPanel。面板惰性挂载。
 */
export type SectionKey =
  | "general"
  | "data"
  | "bot"
  | "model"
  | "memory"
  | "skills"
  | "mcp"
  | "services"
  | "desk"
  | "graph"
  | "tokens"
  | "opsHistory"
  | "logs";

/** 设置页表单的 config 状态形状（bot_set_config 的入参）。
 *  各 section 组件都要读它——写成具名类型，省得每处抄一遍字段表。 */
export type SettingsConfig = {
  apiProvider: ApiProvider;
  modelsByProvider: ModelsByProvider;
  activeModelId: ActiveModelId;
  disabledVendors: string[];
  verifiedVendors: string[];
  uiFontSize: UiFontSize;
  hasApiKey: boolean;
  vendorKeys: string[];
  bypassLlmOnPreStepHit: boolean;
  allowedDirs: string;
  guardClaimKeywords: string;
  guardClaimKeywordsDefault: string[];
  hasTavilyKey: boolean;
  tavilyEnabled: boolean;
  hasBraveKey: boolean;
  braveEnabled: boolean;
  pythonTimeoutSecs: string;
  maxFunctionCalls: string;
  archiveAfterDays: string;
  permMode: "strict" | "ask" | "auto" | "yolo";
  toolRules: Array<{ tool: string; action: "allow" | "ask" | "deny" }>;
  maxTokens: string;
  reasoningEffort: "off" | "low" | "medium" | "high";
  memoryConsolidation: {
    enabled: boolean;
    interval: string;
    lastRunAt: number | null;
  };
  memoryControl: {
    injectionEnabled: boolean;
    autoWriteEnabled: boolean;
    autoExtract: AutoExtractMode;
  };
  memoryTuning: Record<string, number> | null;
  maxRounds: string;
  historyBudgetChars: string;
  historyTruncateTargetPct: string;
  subagentMaxTurns: string;
  subagentMaxWallSecs: string;
  searchMaxResults: string;
  maxToolOutputBytes: string;
  askBudget: string;
};
