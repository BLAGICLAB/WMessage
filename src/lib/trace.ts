// 执行痕迹（exec_trace）查询封装（Agent 透明化设计 §5.2）。
// 类型与 Rust db/trace.rs 的 serde camelCase 输出一一对应；字段增删两端同批同步
// （Rust 侧 DB 语义见 src-tauri/src/db/trace.rs 模块头）。

import { invoke } from "@tauri-apps/api/core";

/** 一次执行 = 一条 trace（run_task_in_chat / 定时 / 工作流 / 批量） */
export type TraceRow = {
  id: number;
  sessionId: string;
  taskId?: string | null;
  origin: string;
  title?: string | null;
  status: "running" | "done" | "failed" | "stopped" | "timeout";
  startedAt: number;
  finishedAt?: number | null;
  turnCount: number;
  toolCalls: number;
  filesChanged: number;
  promptTokens: number;
  completionTokens: number;
  error?: string | null;
};

/** 每次工具调用 = 一条 span（args/result 后端已按 16KB 钳制） */
export type SpanRow = {
  id: number;
  traceId: number;
  turn: number;
  toolCallId?: string | null;
  name: string;
  args?: string | null;
  result?: string | null;
  ok: boolean;
  errorClass?: string | null;
  durationMs?: number | null;
  createdAt: number;
};

/** 每次文件落盘修改 = 一条 change（diff 为 unified 文本，超 2000 行截断置位） */
export type FileChangeRow = {
  id: number;
  traceId: number;
  spanId?: number | null;
  path: string;
  kind: "create" | "modify" | "delete";
  added: number;
  deleted: number;
  diff?: string | null;
  truncated: boolean;
  beforeRef?: string | null;
  beforeSha?: string | null;
  afterSha?: string | null;
  createdAt: number;
};

export type TraceDetail = TraceRow & {
  spans: SpanRow[];
  fileChanges: FileChangeRow[];
};

/** 按任务卡查执行历史（startedAt 倒序） */
export function traceListByTask(taskId: string, limit = 20): Promise<TraceRow[]> {
  return invoke<TraceRow[]>("trace_list", { taskId, limit });
}

/** 单次执行完整痕迹（trace + spans + fileChanges）；无此 trace 返回 null */
export function traceDetail(traceId: number): Promise<TraceDetail | null> {
  return invoke<TraceDetail | null>("trace_detail", { traceId });
}

/** 文件级回滚：恢复到本次 AI 修改前（漂移闸：after_sha 不符拒绝） */
export function fileRollback(changeId: number): Promise<string> {
  return invoke<string>("file_rollback", { changeId });
}

/** ：导出单次执行痕迹为 JSONL（落 data_dir/exports/，返回绝对路径） */
export function traceExport(traceId: number): Promise<string> {
  return invoke<string>("trace_export", { traceId });
}
