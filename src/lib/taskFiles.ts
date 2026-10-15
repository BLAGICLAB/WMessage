import type { Task } from "../types";

/** 任务卡绑定文件条目（与 Rust db::TaskFile 对应） */
export interface TaskFile {
  path: string;
  isDir: boolean;
}

/** 绑定文件数量上限：与 Rust db::MAX_TASK_FILES 硬上限同值，改动需两侧同步。 */
export const MAX_TASK_FILES = 10;

/** 有效绑定文件列表（files 列，多文件；空 = 未绑定） */
export function taskFiles(t: Task): TaskFile[] {
  return t.files ?? [];
}

/**
 * 绑定变更的统一 patch。清空必须显式 null（undefined 键被 JSON 序列化丢弃，
 * 后端按「未提供」保留旧值，旧路径复活；后端契约：显式 null = 清空，缺键 = 保留）。
 */
export function filesPatch(files: TaskFile[]): Partial<Task> {
  return { files };
}

/** 追加新绑定（去重保序），返回 null 表示无新增；超上限截断并标记 truncated */
export function mergeFiles(
  cur: TaskFile[],
  added: TaskFile[],
): { files: TaskFile[]; truncated: boolean } {
  const merged = [...cur];
  for (const f of added) {
    if (!merged.some((m) => m.path === f.path)) merged.push(f);
  }
  const truncated = merged.length > MAX_TASK_FILES;
  return { files: merged.slice(0, MAX_TASK_FILES), truncated };
}
