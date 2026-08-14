// 旧版 localStorage 键：方案B 起任务数据存 SQLite（wmessage.db），此键仅用于首次迁移
export const STORAGE_KEY = "***";

// —— 方案B（2026-08-14）：任务数据存 SQLite，行级增量读写 ——
import { invoke } from "@tauri-apps/api/core";
import type { Task } from "./types";

/** 结构相等（用于 diff 行级变更） */
export const taskEq = (a: Task, b: Task) => JSON.stringify(a) === JSON.stringify(b);

/** 全量读取（Rust 侧自动迁移旧 data.json） */
export async function loadTasksFromDb(): Promise<Task[]> {
  try {
    return await invoke<Task[]>("db_load");
  } catch (e) {
    console.error("db_load failed", e);
    return [];
  }
}

/** 行级增量写入（INSERT OR REPLACE） */
export async function upsertTasks(tasks: Task[]): Promise<void> {
  if (!tasks.length) return;
  try {
    await invoke("db_upsert", { tasks });
  } catch (e) {
    console.error("db_upsert failed", e);
  }
}

/** 行级删除 */
export async function deleteTaskRows(ids: string[]): Promise<void> {
  if (!ids.length) return;
  try {
    await invoke("db_delete", { ids });
  } catch (e) {
    console.error("db_delete failed", e);
  }
}
