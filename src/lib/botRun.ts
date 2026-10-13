// 任务「交给机器人」通道（TodoCard / TaskDetailPanel / 挂件 TaskCard 共用）：
// execute-task 事件由挂件聊天区（ChatPanel）监听分发执行。
import { emit } from "@tauri-apps/api/event";
import { WebviewWindow } from "@tauri-apps/api/webviewWindow";

/** 广播 execute-task（载荷 { id, title }）。挂件内按钮已在现场，只发事件不唤窗。 */
export function emitExecuteTask(t: { id: string; title: string }): void {
  emit("execute-task", { id: t.id, title: t.title }).catch(() => {});
}

/** 主窗口入口：发事件 + 唤起挂件窗口并聚焦，让用户看到执行现场 */
export function runWithBot(t: { id: string; title: string }): void {
  emitExecuteTask(t);
  WebviewWindow.getByLabel("widget")
    .then((w) => {
      if (w) {
        w.show()
          .then(() => w.setFocus())
          .catch(() => {});
      }
    })
    .catch(() => {});
}
