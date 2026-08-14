// 旧版 localStorage 键：方案2 起任务数据落盘 data.json，此键仅用于首次迁移
export const STORAGE_KEY = "***";

// —— 方案2（2026-08-14）：任务数据落盘为应用数据目录下的 data.json ——
import { invoke } from "@tauri-apps/api/core";

/** 读取 data.json 原文；文件不存在或读取失败返回 null */
export async function loadTasksJson(): Promise<string | null> {
  try {
    const s = await invoke<string>("load_data");
    return s && s.length > 0 ? s : null;
  } catch (e) {
    console.error("load_data failed", e);
    return null;
  }
}

/** 写入 data.json（Rust 侧 temp + rename 原子写） */
export async function saveTasksJson(json: string): Promise<void> {
  try {
    await invoke("save_data", { json });
  } catch (e) {
    console.error("save_data failed", e);
  }
}
