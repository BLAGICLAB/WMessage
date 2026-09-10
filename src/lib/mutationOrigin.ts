/**
 * 任务变更来源标记（字符串约定 → 类型守卫）。
 * 与 Rust 侧 src-tauri/src/mutation.rs 的 MutationOrigin 同值对应，改动需两侧同步。
 *
 * 语义（tasks-updated 事件）：
 * - bot / api / migration：后端线程已落盘，主窗口只合并 UI，不回写
 * - widget / undefined：挂件上报，主窗口统一落盘
 */
const MUTATION_ORIGINS = ["main", "widget", "bot", "api", "migration"] as const;

export type MutationOrigin = (typeof MUTATION_ORIGINS)[number];

export function isMutationOrigin(v: unknown): v is MutationOrigin {
  return typeof v === "string" && (MUTATION_ORIGINS as readonly string[]).includes(v);
}

/** 该来源是否已由后端落盘（主窗口收到后跳过回写，只合并 UI） */
export function isPersistedOrigin(v: unknown): boolean {
  return v === "bot" || v === "api" || v === "migration";
}
