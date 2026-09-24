/**
 * tasks-updated 事件的 source 协议（镜像 src-tauri/src/mutation.rs MutationOrigin：
 * main / widget / bot / api / migration，改动需两侧同步）。协议值保持字符串不变。
 *
 * 语义：Bot/Api/Migration 已由后端线程落盘，主窗口收到后只合并 UI 不回写；
 * Widget/未标记 = 挂件上报，主窗口统一落盘。initial-load 为历史值（无实际
 * emitter，旧版初始化曾用，保留兼容：收到同样跳过回写）。
 */
const BACKEND_PERSISTED_SOURCES: ReadonlySet<string> = new Set([
  "bot",
  "api",
  "migration",
  "initial-load",
]);

/** 全部已知 source（协议枚举 + 历史 initial-load）；集合外视为协议外字符串 */
export const KNOWN_SOURCES: ReadonlySet<string> = new Set([
  "main",
  "widget",
  "bot",
  "api",
  "migration",
  "initial-load",
]);

/** source 是否已由后端落盘（主窗口收到后跳过回写，只合并 UI） */
export function isBackendPersisted(source: string | undefined): boolean {
  return source !== undefined && BACKEND_PERSISTED_SOURCES.has(source);
}
