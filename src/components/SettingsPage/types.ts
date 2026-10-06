// SettingsPage 子模块：纯类型定义 + 工具函数（无 React / 无 IO）。
// 由 SettingsPage 目录内其他子文件 import，外部不直接引用。

/** 单个大模型条目：label / baseUrl / model 三元组 + 稳定 id。
 *  id 是前端 crypto.randomUUID() 生成的字符串，仅用于 React key + 标识 active，
 *  不参与 API 调用。 */
export type ModelEntry = {
  id: string;
  label: string;
  baseUrl: string;
  model: string;
  /** 所属厂商名（U10 厂商中心）：老配置缺省 → 前端按协议名兜底分组 */
  vendor?: string;
  /** U11：false = 聊天 🧠 下拉不显示；老配置缺省 = 启用 */
  enabled?: boolean;
  /** U11：上下文窗口（千 token），徽标显示「204.8K」样式；缺省不显示 */
  contextK?: number;
  /** 能力徽标（如「视觉」）；缺省/空数组不渲染徽标 */
  capabilities?: string[];
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
  if (typeof crypto !== "undefined" && typeof crypto.randomUUID === "function") {
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

export type SkillOutcomeKind = "done" | "await_user" | "failed_recoverable" | "terminated";
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
  lastOutcome?: SkillOutcome | null;
  /** frontmatter version（N7-⑦，可选） */
  version?: string | null;
  /** 引用了未内置工具的清单（N7-①；空数组/缺省 = 全部兼容，设置页标红提示） */
  unknownTools?: string[];
};

// ───────────────────────── MCP（外部工具服务器） ─────────────────────────

/** 单个外部 MCP 服务器配置（与 Rust bot::mcp::config::McpServerConfig 同形，camelCase） */
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

/** mcp_status 返回：配置 × 连接槽合并视图 */
export type McpServerStatus = {
  id: string;
  name: string;
  transport: string;
  enabled: boolean;
  /** "connected" | "down" | "absent" */
  state: string;
  error?: string | null;
  toolCount: number;
};

/** 服务器已发现的工具（名称 + 描述，设置页展示用） */
export type McpToolBrief = { name: string; description?: string | null };

/** stdio 启动器白名单（与 Rust STDIO_LAUNCH_ALLOWLIST 同步——保存前后端双重把关） */
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
