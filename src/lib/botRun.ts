// 任务「交给机器人」通道（TodoCard / TaskDetailPanel 共用）：
// execute-task 事件由主窗口聊天页的 ChatPanel 监听分发执行。
import { emit } from "@tauri-apps/api/event";

/** 广播 execute-task（载荷 { id, title }），执行由 ChatPanel 接手 */
function emitExecuteTask(t: { id: string; title: string }): void {
  emit("execute-task", { id: t.id, title: t.title }).catch(() => {});
}

/** 主窗口入口：发事件 + 切到聊天页，让用户看到执行现场。   *  chat-focus-session 空载荷 = 只切视图不动当前会话（App 监听它 setView("chat")，
 *  ChatPanel 见无 sessionId 则不切会话）。 */
export function runWithBot(t: { id: string; title: string }): void {
  emitExecuteTask(t);
  emit("chat-focus-session", {}).catch(() => {});
}
