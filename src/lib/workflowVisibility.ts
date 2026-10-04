// 工作流任务可见性开关（W1-CANVAS，设计 §3.3/§10）：
// 看板/归档/回收站/挂件/命令面板是否混入 origin="workflow" 的卡，默认隐藏。
// 纯前端偏好，走 localStorage；跨组件（主窗/挂件/设置页）用 window 事件即时同步。

const SHOW_WORKFLOW_TASKS_KEY = "wm.showWorkflowTasks";
export const WORKFLOW_VISIBILITY_EVENT = "wm-workflow-visibility";

export function getShowWorkflowTasks(): boolean {
  try {
    return typeof localStorage !== "undefined" && localStorage.getItem(SHOW_WORKFLOW_TASKS_KEY) === "1";
  } catch {
    return false;
  }
}

export function setShowWorkflowTasks(v: boolean): void {
  try {
    localStorage.setItem(SHOW_WORKFLOW_TASKS_KEY, v ? "1" : "0");
  } catch {
    // localStorage 不可用（隐私模式等）：仅本次会话生效
  }
  window.dispatchEvent(new CustomEvent(WORKFLOW_VISIBILITY_EVENT));
}

/** 工作流卡判定（过滤选择器共用，勿散落内联判断） */
export const isWorkflowTask = (t: { origin?: string }): boolean =>
  t.origin === "workflow";
