import { invoke } from "@tauri-apps/api/core";

/** Agent 通知中心消息（与 Rust notifications::NotificationView 对应） */
export interface NotificationItem {
  id: string;
  kind: "memory_proposal" | "evolution_proposal" | "artifact_bind" | "workflow_question";
  title: string;
  body: string;
  payload: Record<string, unknown> | null;
  status: "pending" | "done" | "dismissed";
  createdAt: number;
  resolvedAt: number | null;
}

/** 通知变更广播（后端任何写入/解决后 emit，列表与导航角标据此刷新） */
export const NOTIFICATIONS_CHANGED_EVENT = "notifications-changed";

/** 列表（新→旧）；status 缺省全量 */
export function listNotifications(
  status?: NotificationItem["status"]
): Promise<NotificationItem[]> {
  return invoke<NotificationItem[]>("notifications_list", { status });
}

/** 待处理数（导航栏角标） */
export function pendingNotificationCount(): Promise<number> {
  return invoke<number>("notifications_pending_count");
}

/** 解决一条消息：忽略/跳过 → dismissed（收下/启用/绑定由对应业务命令回写 done） */
export function dismissNotification(id: string): Promise<void> {
  return invoke("notifications_resolve", { id, status: "dismissed" });
}

/** 清空已处理消息 */
export function clearDoneNotifications(): Promise<number> {
  return invoke<number>("notifications_clear_done");
}

// ── 各 kind 的动作命令（复用既有命令，后端尾部自动回写消息状态） ──

/** 记忆提案：全部收下 → mem_pending_approve(ids) */
export function approveMemoryProposals(ids: number[]): Promise<unknown> {
  return invoke("mem_pending_approve", { ids });
}

/** 记忆提案：忽略 → mem_pending_reject(ids) */
export function rejectMemoryProposals(ids: number[]): Promise<number> {
  return invoke("mem_pending_reject", { ids });
}

/** 自进化提案：启用 → evolution_toggle_proposal(proposalId, true) */
export function enableEvolutionProposal(proposalId: string): Promise<void> {
  return invoke("evolution_toggle_proposal", { proposalId, enabled: true });
}

/** 任务卡绑定文件：绑定选中 → confirm_artifact_batch(taskId, paths) */
export function confirmArtifactBatch(
  taskId: string,
  paths: string[]
): Promise<number> {
  return invoke("confirm_artifact_batch", { taskId, paths });
}
