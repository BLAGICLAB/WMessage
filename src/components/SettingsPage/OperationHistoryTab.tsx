// 设置页「操作历史」tab：列最近 50 条破坏性操作（标签整理 + 未来扩展），
// 每条带「撤销」按钮（按 run_id 调 rollback）。
// 数据源：tag_merge_history 表（src-tauri/src/db/mod.rs bootstrap），
// 后端 tags_consolidate_list_runs 命令 + tags_consolidate_rollback。
import { useEffect, useState } from "react";
import { History, Undo2, Loader2 } from "lucide-react";
import {
  tagsConsolidateListRuns,
  tagsConsolidateRollback,
  type TagMergeRunSummary,
} from "../../lib/botConsolidate";
import { handleCommandError } from "../../lib/errorHandler";
import { formatDateTime } from "../../format";

export function OperationHistoryTab() {
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [runs, setRuns] = useState<TagMergeRunSummary[]>([]);
  const [rolling, setRolling] = useState<string | null>(null);

  const reload = () => {
    setLoading(true);
    setError(null);
    tagsConsolidateListRuns()
      .then((r) => {
        setRuns(r);
        setLoading(false);
      })
      .catch((e) => {
        setError(String(e));
        setLoading(false);
      });
  };

  useEffect(reload, []);

  const onRollback = async (runId: string) => {
    if (!confirm(`确认撤销此标签合并？\nrun_id: ${runId}\n\n所有受影响任务的 tags 将恢复。`)) {
      return;
    }
    setRolling(runId);
    try {
      const r = await tagsConsolidateRollback(runId);
      // 简单提示
      alert(
        `已撤销「${r.merged_to}」的合并，恢复 ${r.restored_task_count} 个任务的 tags。`,
      );
      reload();
    } catch (e) {
      handleCommandError(e, "撤销标签合并");
    } finally {
      setRolling(null);
    }
  };

  return (
    <div className="nm-card p-5">
      <div className="flex items-center gap-2 mb-3">
        <History size={14} aria-hidden className="text-[var(--t3)]" />
        <span className="text-sm font-medium text-[var(--t2)]">操作历史</span>
        <span className="text-xs text-[var(--t5)] ml-2">
          标签合并等破坏性操作的最近 50 条
        </span>
        <button
          type="button"
          className="ml-auto nm-btn px-2.5 py-1 text-xs text-[var(--t3)]"
          onClick={reload}
        >
          刷新
        </button>
      </div>
      {loading && (
        <div className="flex items-center gap-2 text-xs text-[var(--t4)] py-3">
          <Loader2 size={11} aria-hidden className="animate-spin" />
          加载中…
        </div>
      )}
      {error && (
        <div className="text-xs text-[var(--danger)] py-2">{error}</div>
      )}
      {!loading && !error && runs.length === 0 && (
        <div className="text-xs text-[var(--t5)] py-3">暂无历史记录</div>
      )}
      {!loading && !error && runs.length > 0 && (
        <ul className="flex flex-col gap-2">
          {runs.map((r) => (
            <li
              key={r.run_id}
              className="flex items-center gap-3 px-3 py-2 rounded-lg bg-[var(--bg)] border border-[var(--bd)] text-xs"
            >
              <div className="flex-1 min-w-0">
                <div className="flex items-center gap-2">
                  <span className="text-[var(--t2)] font-medium">
                    合并 → #{r.merged_to}
                  </span>
                  <span className="text-[var(--t5)]">
                    · {r.affected_task_count} 任务
                  </span>
                  {r.rolled_back_at !== null && (
                    <span className="text-xs text-[var(--t5)] nm-inset px-1.5 py-0.5 rounded">
                      已撤销
                    </span>
                  )}
                </div>
                <div className="text-xs text-[var(--t5)] mt-0.5">
                  {formatDateTime(r.created_at)} · run_id: {r.run_id}
                </div>
                <div className="text-xs text-[var(--t5)] mt-0.5 truncate">
                  来源别名: {r.merged_from}
                </div>
              </div>
              {r.rolled_back_at === null && (
                <button
                  type="button"
                  className="nm-btn shrink-0 px-2.5 py-1 text-xs text-[var(--t3)] flex items-center gap-1 disabled:opacity-50"
                  onClick={() => onRollback(r.run_id)}
                  disabled={rolling !== null}
                >
                  {rolling === r.run_id ? (
                    <Loader2 size={11} aria-hidden className="animate-spin" />
                  ) : (
                    <Undo2 size={11} aria-hidden />
                  )}
                  撤销
                </button>
              )}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
