// WidgetApp 乐观更新回滚 helper（前端 #4 收敛）
//
// 抽出 4 个同形 sites 共享的「snapshot prev → map 应用 patch → 写 ref + setTasks
// → invoke task_patch → on err handleCommandError + 还原 prev」模板：
// toggleCollapsed / commitTitle / toggleSubtask / removeFile。
//
// 字节对齐前提：
// - 后端 src-tauri/src/db/tasks.rs::task_patch 签名是 (id, patch)，线协议必须
//   嵌套 { id, patch } 而非铺平 { id, ...patch }（铺平会让 patch 字段被 serde
//   当成多余请求字段拒收，或退化成未知字段被忽略）。
// - toggleDone 不走本 helper：它走 task_set_column，与 task_patch 是两个独立
//   命令，前者在 Rust 侧做了 column 切换的 botAssigned / completedAt 清理，
//   后者只是字段白名单应用——不能等价替换。
import { invoke } from "@tauri-apps/api/core";
import type { Task } from "../../types";
import { handleCommandError } from "../../lib/errorHandler";

/**
 * 乐观更新回滚：定向命令失败时回收本地改动，不等下一次 tasks-changed/轮询
 * 静默翻回库值。仅当本地态仍是这次的乐观结果（未被收敛逻辑改写过）才回滚，
 * 避免拿失败前快照覆盖更新的库态。
 *
 * 与 WidgetApp 内同名闭包同形（共享 tasksRef + setTasks）；提到模块顶层仅为
 * 让 patchTaskOptimistic 直接复用——不在组件内仍要保留「tasksRef !== optimistic
 * 则放弃回滚」的守卫，否则会拿失败前快照盖掉后续 tasks-changed 已落定的更新。
 */
function rollbackOptimistic(
  tasksRef: React.MutableRefObject<Task[]>,
  setTasks: (t: Task[]) => void,
  prev: Task[],
  optimistic: Task[],
): void {
  if (tasksRef.current !== optimistic) return;
  tasksRef.current = prev;
  setTasks(prev);
}

/**
 * 乐观更新 task_patch 命令的样板：
 *   1. prev = tasksRef.current  // 失败回滚快照
 *   2. next = prev.map(...{...x, ...patch})  // 仅命中行合并 patch
 *   3. tasksRef.current = next; setTasks(next)  // 立刻反映在 UI
 *   4. invoke("task_patch", { id, patch })  // 同步线协议嵌套
 *   5. .catch: handleCommandError(e, errorLabel) + 回滚到 prev
 *
 * 调用方约定：errorLabel 是用户语义（例：「更新任务」），不是 Rust 命令名
 * ——errorHandler 把它当 ctx 串进 console 前缀 / 应用内弹窗前缀。
 */
export function patchTaskOptimistic(
  tasksRef: React.MutableRefObject<Task[]>,
  setTasks: (t: Task[]) => void,
  taskId: string,
  patch: Partial<Task>,
  errorLabel: string,
): void {
  const prev = tasksRef.current;
  const next = prev.map((x) => (x.id === taskId ? { ...x, ...patch } : x));
  tasksRef.current = next;
  setTasks(next);
  invoke<Task>("task_patch", { id: taskId, patch }).catch((e) => {
    handleCommandError(e, errorLabel);
    rollbackOptimistic(tasksRef, setTasks, prev, next);
  });
}