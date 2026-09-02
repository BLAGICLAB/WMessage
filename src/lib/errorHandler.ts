// CommandError 统一处理入口（Phase 7 Q4 2026-08-18）
//
// Rust 侧 src-tauri/src/error.rs 把 Result<T, String> 升级为 CommandResult<T> =
// Result<T, CommandError>；序列化形如 `{ code, message, recoverable }`。
//
// 本模块：识别 → 按 code 给出对应提示 → 兜底展示 message。
//
// 设计取舍：
// - 不引入新依赖：项目无 toast 库，沿用项目现状（src/App.tsx 等）使用原生 alert()
// - silent 选项：调用方已有 inline 错误 UI（如 SettingsPage setError）时只 console
//   不弹 alert，避免双重提示
// - recoverable 驱动 UI（P0-6B 2026-08-18）：CommandError.recoverable === true 且调用方
//   传了 onRetry 时改用原生 confirm() 提供「重试」选择；recoverable === false 时只 alert
//   并引导反馈日志（hint 文案与后端 error.rs is_recoverable() 保持一致，不再误导"可重试"）
// - formatCommandError()：供调用方取出 user-friendly 文本（替代 `String(e)`，
//   后者对结构化对象只得到 "[object Object]"）
// - 空 message 兜底（P2-35 2026-08-19）：CommandError.message 为空时回退 code，
//   再空回退「未知错误」；非结构化空 msg 同样兜底——不弹空 alert、不静默跳过

/** Tauri 拒绝时拿到的反序列化 CommandError JSON 形状 */
export interface CommandErrorPayload {
  code: string;
  message: string;
  recoverable: boolean;
}

/** 运行时类型守卫：识别 Rust 端结构化 CommandError */
export function isCommandError(e: unknown): e is CommandErrorPayload {
  return (
    !!e &&
    typeof e === "object" &&
    typeof (e as { code?: unknown }).code === "string" &&
    typeof (e as { message?: unknown }).message === "string" &&
    typeof (e as { recoverable?: unknown }).recoverable === "boolean"
  );
}

/**
 * 按 code 给一个短提示（附加在 message 后），让用户知道下一步该做什么。
 * 没匹配到时返回 null，调用方只用 message。
 */
function hintForCode(code: string): string | null {
  switch (code) {
    case "BOT_DISABLED":
      return "请先到设置页「机器人设置」开启";
    case "API_KEY_MISSING":
      return "请先到设置页填写 API Key";
    case "KEYRING_ERROR":
      return "不可自动重试；请检查系统凭据存储权限或重启应用";
    case "HTTP_START_FAILED":
    case "PORT_IN_USE":
      return "请到设置页更换端口或关闭占用进程";
    case "AUTH_MISSING":
    case "AUTH_INVALID":
      return "请检查 Authorization Bearer token";
    case "PAYLOAD_TOO_LARGE":
      return "请求体过大（>1MB），请减小后重试";
    case "TASK_NOT_FOUND":
      return "该任务可能已被删除，请刷新列表";
    case "TASK_INVALID_STATE":
      return "任务当前状态不允许该操作（执行中/已完成/已归档），请调整后重试";
    case "INVALID_ARGUMENT":
      return "请检查输入参数";
    case "SKILL_LOAD_FAILED":
    case "SKILL_NOT_INSTALLED":
      return "请先到设置页导入对应技能";
    case "UNKNOWN_TOOL":
      return "不可自动重试；请反馈日志（模型行为异常）";
    case "ATOMIC_TOOL_BLOCKED":
      return "原子工具禁止直接调用，需通过 Skill 内部使用";
    case "LLM_REQUEST_FAILED":
    case "LLM_API_ERROR":
      return "请稍后重试或检查 API 配置";
    case "CONFIRM_TIMEOUT":
      return "请在 60 秒内确认操作";
    case "CONFIRM_REJECTED":
      return "已取消当前操作";
    case "DB_ERROR":
    case "IO_ERROR":
      return "不可自动重试；请反馈日志（含操作步骤）";
    case "INTERNAL":
      return "不可自动重试；请反馈日志（含复现步骤）";
    default:
      return null;
  }
}

/**
 * 把 invoke 抛出的 unknown 格式化成 user-friendly 文本（任意类型都能转）：
 * - CommandError → message
 * - Error → message
 * - 字符串 → 自身
 * - 其他对象 → JSON.stringify
 * - null / undefined → ""
 */
export function formatCommandError(e: unknown): string {
  if (e == null) return "";
  if (isCommandError(e)) return e.message;
  if (e instanceof Error) return e.message || e.name || "未知错误";
  if (typeof e === "string") return e;
  try {
    return JSON.stringify(e);
  } catch {
    return String(e);
  }
}

export interface HandleOptions {
  /**
   * 不弹 alert，仅写 console。
   * 用于已有 inline 错误 UI 的位置（如 SettingsPage 面板 setError），
   * 避免双重提示。
   * 默认 false（弹 alert 提示用户）。
   */
  silent?: boolean;
  /**
   * 重试回调：仅当错误 recoverable === true 时生效——
   * 弹 confirm 询问「是否重试」，用户确认后调用本回调重新执行失败的操作。
   * recoverable === false（或调用方未传）时走普通 alert，不调用本回调。
   */
  onRetry?: () => void;
}

/**
 * 统一错误处理入口。CommandError → console 打 code + 弹 alert；其他原样。
 *
 * @param e    invoke() 抛出的 unknown
 * @param ctx  调用上下文（command 名 / 函数名），用于 console 前缀
 * @param options.silent  已有 inline UI 时设 true（只 console，不 alert）
 * @param options.onRetry 可恢复错误的重试回调（recoverable=true 时弹 confirm 询问）
 */
export function handleCommandError(
  e: unknown,
  ctx?: string,
  options: HandleOptions = {}
): void {
  const prefix = ctx ? `[${ctx}]` : "[invoke]";
  if (isCommandError(e)) {
    console.error(
      `${prefix} CommandError code=${e.code} recoverable=${e.recoverable}`,
      e
    );
    if (options.silent) return;
    // P2-35：空 message 不弹空 alert——fallback 到 code，code 也空再兜底「未知错误」
    const msg = e.message.trim() ? e.message : e.code || "未知错误";
    const hint = hintForCode(e.code);
    const body = hint ? `${msg}\n\n💡 ${hint}` : msg;
    // recoverable 驱动 UI：可恢复 + 调用方给了重试回调 → confirm 提供「重试」选择；
    // 不可恢复（或无回调）→ alert + hint 引导（hint 已按 code 区分「去设置页」/「反馈日志」）
    if (e.recoverable && options.onRetry) {
      if (confirm(`❌ ${body}\n\n🔁 是否重试？`)) options.onRetry();
    } else {
      alert(`❌ ${body}`);
    }
    return;
  }
  // 非结构化错误（一般是同步抛出的 JS 异常，未被 Rust 端包装）
  const msg = formatCommandError(e);
  console.error(`${prefix} ${msg || "(empty)"}`, e);
  if (options.silent) return;
  // P2-35：msg 为空也兜底「未知错误」，不弹空 alert、也不静默跳过
  alert(`❌ ${msg || "未知错误"}`);
}