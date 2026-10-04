// 工作流任务可见性开关（W1-CANVAS，设计 §3.3/§10）：
// 看板/归档/回收站/挂件/命令面板是否混入 origin="workflow" 的卡，默认隐藏。
// 纯前端偏好，走 localStorage；跨组件/跨 webview（主窗/挂件/设置页）用 Tauri
// 全局事件即时同步（emit 广播到所有窗口的 listen——不能用 window CustomEvent，
// 它不跨 webview；也不能在 subscriber 用 DOM addEventListener，两个事件系统不连通）。
import { emit } from "@tauri-apps/api/event";

const SHOW_WORKFLOW_TASKS_KEY = "wm.showWorkflowTasks";
export const WORKFLOW_VISIBILITY_EVENT = "wm-workflow-visibility";

export function getShowWorkflowTasks(): boolean {
  try {
    return (
      typeof localStorage !== "undefined" &&
      localStorage.getItem(SHOW_WORKFLOW_TASKS_KEY) === "1"
    );
  } catch {
    return false;
  }
}

export function setShowWorkflowTasks(v: boolean): void {
  try {
    localStorage.setItem(SHOW_WORKFLOW_TASKS_KEY, v ? "1" : "0");
  } catch {
    // localStorage 不可用（隐私模式等）：不派发事件——订阅方以 localStorage
    // 现值为准，派发只会让它们重读到旧值（无意义且误导本地乐观态）
    return;
  }
  // fire-and-forget：事件丢失的退化路径 = 下次挂件重载时重读 localStorage（可自愈）
  emit(WORKFLOW_VISIBILITY_EVENT).catch(() => {});
}

/** 工作流卡判定（过滤选择器共用，勿散落内联判断） */
export const isWorkflowTask = (t: { origin?: "user" | "workflow" }): boolean =>
  t.origin === "workflow";
