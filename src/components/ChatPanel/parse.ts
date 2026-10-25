// ChatPanel 子模块：解析工具（不依赖 React）。
// 从 ChatPanel 主组件搬出——主组件里只剩状态与副作用，纯解析留在这里单独可测。

import { TURN_STATUS_VALUES } from "./constants";
import type { TurnStatus } from "./types";

/** 子 agent 行里的契约/结果字段都是 JSON 字符串，详情视图要结构化渲染。
 *  解析失败或形状对不上（验收标准不是字符串数组、结果不是对象）都落回原文——
 *  原文本身可能就是排查线索，吞掉等于把「子 agent 到底写了什么」变成无法回答。 */
function parseJsonField(
  raw?: string | null,
): string | string[] | Record<string, unknown> | null {
  if (!raw) return null;
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return raw;
  }
  if (Array.isArray(parsed)) {
    return parsed.every((x) => typeof x === "string")
      ? (parsed as string[])
      : raw;
  }
  if (typeof parsed === "object" && parsed !== null) {
    return parsed as Record<string, unknown>;
  }
  return raw;
}

/** 回合状态取值域（与后端 TurnStatus::as_str 同源）。
 *  库里出现未知值按缺失处理：旧版本写入/库被手改都不能让整段会话加载炸掉 */
const parseTurnStatus = (raw?: string | null): TurnStatus | undefined =>
  raw && TURN_STATUS_VALUES.includes(raw) ? (raw as TurnStatus) : undefined;

export { parseJsonField, parseTurnStatus };
