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
export type SkillInfo = { name: string; description: string; lastOutcome?: SkillOutcome | null };
