import type { Task } from "../types";

/** 任务卡绑定文件条目（与 Rust db::TaskFile 对应） */
export interface TaskFile {
  path: string;
  isDir: boolean;
}

/** 绑定文件数量上限（2026-08-19 老板指令；Rust 侧 db::MAX_TASK_FILES 同值硬上限） */
export const MAX_TASK_FILES = 10;

/**
 * 有效绑定文件列表：files 非空优先；否则回退旧单绑定字段 filePath/fileIsDir
 * （迁移过渡兜底：启动迁移把老 filePath 写进 files，未迁移窗口内旧数据也能显示）
 */
export function taskFiles(t: Task): TaskFile[] {
  if (t.files && t.files.length > 0) return t.files;
  if (t.filePath) return [{ path: t.filePath, isDir: !!t.fileIsDir }];
  return [];
}

/**
 * 绑定变更的统一 patch：写 files 的同时双写旧 filePath/fileIsDir 首条
 * （过渡期旧版本/旧调用方仍读老字段）；空列表三字段全清。
 */
export function filesPatch(files: TaskFile[]): Partial<Task> {
  return {
    files,
    filePath: files[0]?.path,
    fileIsDir: files[0]?.isDir,
  };
}

/** 追加新绑定（去重保序），返回 null 表示无新增；超上限截断并标记 truncated */
export function mergeFiles(
  cur: TaskFile[],
  added: TaskFile[]
): { files: TaskFile[]; truncated: boolean } {
  const merged = [...cur];
  for (const f of added) {
    if (!merged.some((m) => m.path === f.path)) merged.push(f);
  }
  const truncated = merged.length > MAX_TASK_FILES;
  return { files: merged.slice(0, MAX_TASK_FILES), truncated };
}
