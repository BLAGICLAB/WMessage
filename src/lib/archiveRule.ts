// 任务卡归档规则（设置页「数据管理 → 任务卡归档时间」）：
// 任务完成满 N 天自动归档，从「完成」列隐藏，可在归档视图查看/恢复。
// 天数配置存 bot-config.json（archiveAfterDays）：后端 migration 兜底归档
// （主窗口关闭时照常到期）读同一份配置，两侧阈值恒一致——所以不能像
// workflowVisibility 那样只存 localStorage，后端线程读不到。
// 前端内存缓存 + bot-config-changed 事件同步（设置页保存后广播）。
import { invoke } from "@tauri-apps/api/core";
import type { Task } from "../types";

export const DEFAULT_ARCHIVE_DAYS = 7;
export const MIN_ARCHIVE_DAYS = 1;
export const MAX_ARCHIVE_DAYS = 365;

let archiveDays = DEFAULT_ARCHIVE_DAYS;

/** 钳制 + 取整（设置页输入与配置读取共用；非法值回退默认 7，与后端
 *  resolve_archive_after_days 同规则：1..=365） */
export function clampArchiveDays(n: number): number {
  if (!Number.isFinite(n)) return DEFAULT_ARCHIVE_DAYS;
  return Math.min(MAX_ARCHIVE_DAYS, Math.max(MIN_ARCHIVE_DAYS, Math.round(n)));
}

export function getArchiveAfterDays(): number {
  return archiveDays;
}

/** 写内存缓存（设置页保存广播 / 启动加载后调用；null = 后端未配置 → 默认 7） */
export function setArchiveAfterDays(days: number | null | undefined): void {
  archiveDays = days == null ? DEFAULT_ARCHIVE_DAYS : clampArchiveDays(days);
}

/** 从 bot-config.json 拉一次归档天数（失败保持现值——默认 7，不打扰）。
 *  App 启动时在首套归档规则前调用，避免竞态：首屏就按配置阈值归档。 */
export async function loadArchiveDaysFromConfig(): Promise<void> {
  try {
    const c = await invoke<{ archiveAfterDays?: number | null }>("bot_get_config");
    setArchiveAfterDays(c.archiveAfterDays);
  } catch (e) {
    console.error("[archiveRule] load failed, keep current value", e);
  }
}

/** 归档规则：完成超过归档天数的任务自动归档，从「完成」列隐藏 */
export function applyArchiveRule(tasks: Task[]): Task[] {
  const now = Date.now();
  const afterMs = archiveDays * 24 * 60 * 60 * 1000;
  return tasks.map((t) => {
    if (t.column !== "done" || t.archived || t.deletedAt) return t;
    const completedAt = t.completedAt ?? now; // 老数据补完成时间
    if (now - completedAt >= afterMs) return { ...t, completedAt, archived: true };
    return t.completedAt === completedAt ? t : { ...t, completedAt };
  });
}
