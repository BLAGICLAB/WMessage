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

/**
 * 追加绑定（统一绑定内核，与 Rust db::upsert_files_to_task 同语义）：
 * 查重主键 = 路径（小写归一，同名不同路径视为不同条目）；文件追加；
 * 文件夹槽替换——每卡最多一个文件夹，绑新文件夹占旧文件夹原位（文件条目与顺序不动），
 * 同路径重复绑无操作。追加永不清除既有绑定。
 * 返回 applied=实际生效条数（0 = 无变化，调用方免写库）；truncated=超上限截断标记。
 */
export function mergeFiles(
  cur: TaskFile[],
  added: TaskFile[],
): { files: TaskFile[]; truncated: boolean; applied: number } {
  const files = [...cur];
  let applied = 0;
  for (const f of added) {
    if (files.some((m) => m.path.toLowerCase() === f.path.toLowerCase()))
      continue;
    if (f.isDir) {
      const idx = files.findIndex((m) => m.isDir);
      if (idx >= 0) files[idx] = f;
      else files.push(f);
    } else {
      files.push(f);
    }
    applied++;
  }
  const truncated = files.length > MAX_TASK_FILES;
  if (truncated) files.length = MAX_TASK_FILES;
  return { files, truncated, applied };
}
