import { invoke } from "@tauri-apps/api/core";

/** 「需要人处理」聚合项：查询时聚合三源（工作项 / 子 agent / 定时作业） */
export interface NeedsAttentionItem {
  source: "work_item" | "subagent" | "scheduled_job";
  id: string;
  title: string;
  detail?: string;
  at: number;
}

const SOURCE_LABEL: Record<NeedsAttentionItem["source"], string> = {
  work_item: "工作项",
  subagent: "子 Agent",
  scheduled_job: "定时作业",
};

export function sourceLabel(source: NeedsAttentionItem["source"]): string {
  return SOURCE_LABEL[source];
}

export async function needsAttentionList(
  limit = 50,
): Promise<NeedsAttentionItem[]> {
  // 非数组回落空表：该面板是补充面，网关契约漂移不得炸掉通知主列表
  const rows = await invoke<NeedsAttentionItem[]>("needs_attention_list", {
    limit,
  });
  return Array.isArray(rows) ? rows : [];
}
