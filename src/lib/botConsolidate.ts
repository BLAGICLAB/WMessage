// 归档卡标签整理：4 个 tauri command 薄包装 + TS 类型。
// 后端 src-tauri/src/bot/tags_consolidate.rs + src-tauri/src/lib.rs
// 4 个 #[tauri::command]；本文件做 invoke 转发 + TS 端类型。
import { invoke } from "@tauri-apps/api/core";

/** 簇（后端 ClusterScanResult.clusters 元素） */
export interface TagCluster {
  members: string[];
  suggested_main: string;
  avg_similarity: number;
  affected_task_count: number;
}

/** scan 返回 */
export interface ClusterScanResult {
  scanned_tags: number;
  unique_tags: number;
  archived_tasks_scanned: number;
  clusters: TagCluster[];
}

/** merge 返回（RunRecord） */
export interface TagMergeRunRecord {
  run_id: string;
  merged_to: string;
  merged_from: string[];
  affected_task_ids: string[];
  previous_tags: Record<string, string[]>;
  timestamp_ms: number;
}

/** rollback 返回（RollbackResult） */
export interface TagRollbackResult {
  run_id: string;
  merged_to: string;
  restored_task_count: number;
  original_timestamp_ms: number;
}

/** list_runs 返回的每条 run 摘要（设置页操作历史用） */
export interface TagMergeRunSummary {
  run_id: string;
  merged_to: string;
  merged_from: string;
  affected_task_count: number;
  created_at: number;
  rolled_back_at: number | null;
}

export async function tagsConsolidateScan(): Promise<ClusterScanResult> {
  return invoke<ClusterScanResult>("tags_consolidate_scan");
}

export async function tagsConsolidateMerge(
  main: string,
  members: string[],
): Promise<TagMergeRunRecord> {
  return invoke<TagMergeRunRecord>("tags_consolidate_merge", { main, members });
}

export async function tagsConsolidateRollback(
  runId: string,
): Promise<TagRollbackResult> {
  return invoke<TagRollbackResult>("tags_consolidate_rollback", {
    runId,
  });
}

export async function tagsConsolidateListRuns(): Promise<TagMergeRunSummary[]> {
  return invoke<TagMergeRunSummary[]>("tags_consolidate_list_runs");
}
