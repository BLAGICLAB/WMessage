// 唤起主窗口辅助（与 Rust 侧 src-tauri/src/lib.rs::bring_main_to_front 同源）
//
// 老板 2026-08-17 11:31/11:43 规则：所有唤起主窗口的路径（widget 双击标题、聊天区 📌
// 引用、全局快捷键、托盘点击/菜单）都必须把主窗口推到桌面屏幕最顶层才能被看见。
// JS 端只 invoke Rust 命令，不重复实现（单一真相在 Rust）。

import { invoke } from "@tauri-apps/api/core";
import { handleCommandError } from "./lib/errorHandler";

/**
 * 唤起主窗口并强制置顶（不长期驻顶）。
 *
 * Rust 侧 `bring_main_to_front` 实现：show (防隐藏) → unminimize (防最小化) →
 * setFocus (macOS/Linux 置顶) → setAlwaysOnTop(true) 80ms (Windows 强制置顶兜底) →
 * setAlwaysOnTop(false) (立即恢复，不长驻)。
 *
 * 用例：WidgetApp 双击任务标题、ChatPanel 点任务引用按钮。
 */
export async function focusMainWindow(): Promise<void> {
  try {
    await invoke("focus_main_window");
  } catch (e) {
    // best-effort：唤起失败只是看不到主窗口，不打扰用户
    handleCommandError(e, "focus_main_window", { silent: true });
  }
}