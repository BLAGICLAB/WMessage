// 本会话文件改动（聊天侧栏「文件」工作面）的类型与命令封装。
// 类型与 Rust db/trace.rs 的 serde camelCase 输出一一对应；字段增删两端同批同步。

import { invoke } from "@tauri-apps/api/core";

/**
 * 累计口径能给 / 不能给：
 * - "session"：能给出「本会话第一次改之前 vs 现在」的累计 diff
 * - "perChange"：给不出累计（起始快照缺失 / 文件已不在 / 过大），只有逐次 diff
 * - "noLineDiff"：该文件没有行级 diff（二进制 / 无实际改动）
 */
type SessionFileChangeMode = "session" | "perChange" | "noLineDiff";

/** 清单项：某文件在本会话的改动概览 */
export type SessionFileChangeItem = {
  path: string;
  kind: string;
  /** 累计口径下的 + 行数（perChange 档是各次之和，见 note） */
  added: number;
  deleted: number;
  /** 本会话被改了几次 */
  changes: number;
  mode: SessionFileChangeMode;
  /** 文件在 AI 最后一次写盘后又被改过：累计 diff 含非 AI 的改动 */
  drifted: boolean;
  /** 降级/特殊情况的原因（人话） */
  note?: string | null;
};

/** 单次改动的摘要（逐次档里逐条列） */
type SessionFileChangeEntry = {
  id: number;
  kind: string;
  added: number;
  deleted: number;
  diff?: string | null;
  truncated: boolean;
};

/** 单个文件的改动详情 */
export type SessionFileDiff = {
  path: string;
  kind: string;
  mode: SessionFileChangeMode;
  added: number;
  deleted: number;
  changes: number;
  truncated: boolean;
  drifted: boolean;
  /** 累计 diff 文本（mode=session 时有值） */
  diff?: string | null;
  /** 逐次 diff：只有 mode=perChange 才带 */
  perChange: SessionFileChangeEntry[];
  note?: string | null;
};

export type SessionFileChanges = {
  items: SessionFileChangeItem[];
  detail?: SessionFileDiff | null;
};

/** 本会话改动文件清单（不含 diff 文本——列表层只要摘要） */
export function sessionFileChangeList(
  sessionId: string,
): Promise<SessionFileChanges> {
  return invoke<SessionFileChanges>("bot_session_file_changes", { sessionId });
}

/** 单个文件的改动详情（累计 diff 或降级后的逐次 diff） */
export function sessionFileChangeDetail(
  sessionId: string,
  path: string,
): Promise<SessionFileChanges> {
  return invoke<SessionFileChanges>("bot_session_file_changes", {
    sessionId,
    path,
  });
}
