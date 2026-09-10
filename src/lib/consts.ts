/**
 * 前后端共享常量（Rust 为唯一真相，前端启动拉取缓存）。
 * 与 Rust 侧 src-tauri/src/consts.rs 的 AppConsts 对应，改字段需两侧同步。
 *
 * 拉取失败 / 纯前端测试环境（无 Tauri 运行时）回退到 FALLBACK_CONSTS 硬编码值。
 */
import { invoke } from "@tauri-apps/api/core";

/** Rust consts::AppConsts 的线缆格式（serde 默认 snake_case） */
export interface AppConsts {
  max_task_files: number;
  max_title: number;
  max_note: number;
  image_exts: string[];
}

/** 回退值：与 Rust 侧常量同值，改动需两侧同步 */
export const FALLBACK_CONSTS: AppConsts = {
  max_task_files: 10,
  max_title: 200,
  max_note: 5000,
  image_exts: ["png", "jpg", "jpeg", "webp", "gif", "bmp"],
};

let cache: AppConsts = FALLBACK_CONSTS;
let extSet: Set<string> = new Set(FALLBACK_CONSTS.image_exts);

/** 当前生效的共享常量（未拉取/拉取失败 = 回退值） */
export function appConsts(): AppConsts {
  return cache;
}

/** 图片扩展名 Set 视图（ChatPanel 图标识别用） */
export function imageExtSet(): Set<string> {
  return extSet;
}

/**
 * 绑定文件上限（活绑定：loadAppConsts 成功后同步更新）。
 * taskFiles.ts 以 MAX_TASK_FILES 原名 re-export，调用方零改动。
 */
export let maxTaskFiles: number = cache.max_task_files;

/** 启动时拉一次缓存为模块级单例；失败回退硬编码值。返回实际生效值（便于测试断言）。 */
export async function loadAppConsts(): Promise<AppConsts> {
  try {
    cache = await invoke<AppConsts>("app_consts");
  } catch (e) {
    console.warn("[consts] app_consts 拉取失败，回退硬编码值", e);
    cache = FALLBACK_CONSTS;
  }
  extSet = new Set(cache.image_exts);
  maxTaskFiles = cache.max_task_files;
  return cache;
}
